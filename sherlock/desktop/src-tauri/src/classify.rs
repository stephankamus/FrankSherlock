use std::path::{Path, PathBuf};

use crate::platform::process::silent_command;

use regex::Regex;
use serde_json::Value;

use crate::config::{DescriptionLanguage, LibraryKind, OcrEngine, PromptProfile};
use crate::llm::{num_ctx_for, ollama_generate, parse_json_response, GenerateParams};
use crate::models::{ClassificationResult, VisionSettings};
use crate::vision_prep::{self, PreparedImage};

// ---------------------------------------------------------------------------
// Prompts (identical to Python prototype)
// ---------------------------------------------------------------------------

/// Detailed schema: adds anime candidates on the first pass (~78 output tokens).
const PRIMARY_PROMPT_DETAILED: &str = concat!(
    "Analyze this image and respond ONLY with valid JSON. Schema: ",
    r#"{"media_type":"screenshot|anime|manga|photo|document|artwork|other","#,
    r#""contains_text":true,"#,
    r#""is_anime_related":false,"#,
    r#""document_photo":"none|receipt|invoice|statement|other","#,
    r#""description":"short factual description","#,
    r#""series_candidates":["name"],"#,
    r#""character_candidates":["name"],"#,
    r#""confidence":0.0}"#,
    " Rules: ",
    "1) Use null-like empty arrays when unknown. ",
    "2) If image has visible text (UI, receipt, scan, subtitles), contains_text=true. ",
    "3) document_photo describes the picture ITSELF: a photographed receipt is \"receipt\", a scanned invoice is \"invoice\". A scene that only shows a sign, a logo or a price is \"none\" — never infer a document from brand names. ",
    "4) series_candidates and character_candidates must be unique and max 5 items each. ",
    "5) Keep description under 24 words. ",
    "6) Favor precision over guesswork.",
);

/// Concise schema used by default: everything needed to route enrichment and to
/// search, without the candidate arrays (those live in `ANIME_PROMPT`). Output
/// drops from ~78 to ~30 tokens, which is where most of the wall time went on
/// 7b models (`decode` dominates once the image is downscaled).
const PRIMARY_PROMPT_CONCISE: &str = concat!(
    "Analyze this image. Return ONLY compact JSON, no markdown. Schema: ",
    r#"{"media_type":"screenshot|anime|manga|photo|document|artwork|other","#,
    r#""contains_text":true,"#,
    r#""document_photo":"none|receipt|invoice|statement|other","#,
    r#""description":"max 12 words"}"#,
    " Rules: ",
    "1) contains_text=true only when text is clearly visible (UI, receipt, scan, subtitles). ",
    "2) document_photo=receipt|invoice|statement|other only when the picture IS that document; a scene showing signs, logos or prices is \"none\". ",
    "3) description must be factual and under 12 words. ",
    "4) Favor precision over guesswork.",
);

// PT-BR variants of the two primary prompts.
//
// The schema, the keys and every enum value stay **English**: `media_type`,
// `contains_text`, `document_photo` and their values are matched in Rust and the model
// is shown them as literals, so only the free-text field changes language.
//
// The word cap stays at 12. Measured on 12 real photos (`scripts/
// bench-description-language.py`), allowing 16 words made one image loop — `Pessoa de
// pé em pé de pé de pé…` — into the 180-token budget, which is the failure described
// below. PT-BR does not need extra room for time either: it produced *fewer* output
// tokens than English (43.2 vs 46.2) because it says the same thing in fewer words.
const PRIMARY_PROMPT_CONCISE_PT: &str = concat!(
    "Analise esta imagem. Responda SOMENTE com JSON compacto, sem markdown. Schema: ",
    r#"{"media_type":"screenshot|anime|manga|photo|document|artwork|other","#,
    r#""contains_text":true,"#,
    r#""document_photo":"none|receipt|invoice|statement|other","#,
    r#""description":"no máximo 12 palavras"}"#,
    " Regras: ",
    "1) contains_text=true apenas quando houver texto claramente visível (interface, recibo, digitalização, legendas). ",
    "2) document_photo=receipt|invoice|statement|other apenas quando a imagem FOR esse documento; uma cena com placas, logotipos ou preços é \"none\". ",
    "3) description deve ser factual, em português do Brasil, com no máximo 12 palavras. ",
    "4) Prefira precisão a adivinhação.",
);

const PRIMARY_PROMPT_DETAILED_PT: &str = concat!(
    "Analise esta imagem e responda SOMENTE com JSON válido. Schema: ",
    r#"{"media_type":"screenshot|anime|manga|photo|document|artwork|other","#,
    r#""contains_text":true,"#,
    r#""is_anime_related":false,"#,
    r#""document_photo":"none|receipt|invoice|statement|other","#,
    r#""description":"no máximo 24 palavras","#,
    r#""series_candidates":["nome"],"#,
    r#""character_candidates":["nome"],"#,
    r#""confidence":0.0}"#,
    " Regras: ",
    "1) Use arrays vazios quando não souber; nomes de séries e personagens ficam no original, sem traduzir. ",
    "2) contains_text=true apenas quando houver texto claramente visível (interface, recibo, digitalização, legendas). ",
    "3) document_photo descreve a FOTO em si: um recibo fotografado é \"receipt\", uma nota fiscal digitalizada é \"invoice\". Uma cena que apenas mostra uma placa, um logotipo ou um preço é \"none\" — nunca deduza um documento a partir de nomes de marcas. ",
    "4) series_candidates e character_candidates: no máximo 5 itens cada, sem repetir. ",
    "5) description em português do Brasil, com no máximo 24 palavras. ",
    "6) Prefira precisão a adivinhação.",
);

// The old `PRIMARY_PROMPT_FALLBACK` lived here: a second wording of the same schema
// used as the retry. It turned out to be the *cause* of the failure it was meant to
// recover from — on low-contrast photos (construction sites, concrete) that wording
// drives qwen2.5-VL into a repetition loop and Ollama 0.40 aborts the request with
// `prediction aborted, token repeat limit reached` (500). The same images answer fine
// with the normal prompt and no grammar, so the retry now keeps the prompt and only
// changes the sampling mode.

fn primary_prompt(profile: PromptProfile, language: DescriptionLanguage) -> &'static str {
    match (profile, language) {
        (PromptProfile::Concise, DescriptionLanguage::PtBr) => PRIMARY_PROMPT_CONCISE_PT,
        (PromptProfile::Detailed, DescriptionLanguage::PtBr) => PRIMARY_PROMPT_DETAILED_PT,
        (PromptProfile::Concise, DescriptionLanguage::En) => PRIMARY_PROMPT_CONCISE,
        (PromptProfile::Detailed, DescriptionLanguage::En) => PRIMARY_PROMPT_DETAILED,
    }
}

fn primary_num_predict(profile: PromptProfile) -> u32 {
    match profile {
        PromptProfile::Concise => 180,
        PromptProfile::Detailed => 500,
    }
}

/// Repetition guard for the recovery attempts of the first pass.
///
/// Ollama aborts a request with `500 prediction aborted, token repeat limit reached`
/// when the decoder repeats itself too many times, and a low-information photo (a
/// WhatsApp thumbnail, a flat wall, a concrete slab) is exactly that case. Measured
/// on job #35 (2026-10-28): 1,512 requests died this way — **with and without** the
/// JSON grammar — and the follow-up attempts looped again because nothing about the
/// sampling had changed. [`GenerateParams`] documents this very remedy, but the two
/// options were never set by the classifier: they were dead code.
///
/// 1.3 / 256 instead of Ollama's 1.1 / 64: the loop has to be broken inside a window
/// wide enough to contain it, and the JSON keys repeat within ~200 tokens.
const RECOVERY_REPEAT_PENALTY: f32 = 1.3;
const RECOVERY_REPEAT_LAST_N: u32 = 256;

/// Output budget for a recovery attempt.
///
/// A loop is cut by Ollama's own repeat limit, not by the budget, so a larger
/// `num_predict` only lets a loop run longer before dying — and growing it on the
/// third attempt is exactly what the old code did. The context window is unaffected
/// by this number on its own (it is derived from the image size, the prompt length
/// and the reserved output budget inside `num_ctx_for`), so shrinking it cannot make
/// Ollama rebuild the runner.
fn recovery_num_predict(primary: u32) -> u32 {
    primary.min(260)
}

const ANIME_PROMPT: &str = concat!(
    "This appears anime/manga-related. Return ONLY valid JSON with schema: ",
    r#"{"series":"name or null","#,
    r#""franchise":"name or null","#,
    r#""characters":[{"name":"full canonical name","series":"name or null","confidence":0.0}],"#,
    r#""canonical_mentions":["Name from Series"],"#,
    r#""scene_summary":"short","#,
    r#""confidence":0.0}"#,
    " Rules: prefer canonical full names when possible; if unknown set null/empty.",
);

/// `scene_summary` is free text, so it follows `description_language`; the series
/// and character names never do — they are what `canonical_mentions` (and therefore
/// search) is built from, and a translated name would stop matching.
const ANIME_PROMPT_PT: &str = concat!(
    "Esta imagem parece ser de anime/mangá. Responda SOMENTE com JSON válido, schema: ",
    r#"{"series":"nome ou null","#,
    r#""franchise":"nome ou null","#,
    r#""characters":[{"name":"nome canônico completo","series":"nome ou null","confidence":0.0}],"#,
    r#""canonical_mentions":["Nome da Série"],"#,
    r#""scene_summary":"curto","#,
    r#""confidence":0.0}"#,
    " Regras: prefira nomes canônicos completos quando possível; nomes de séries e personagens ficam no original, sem traduzir; scene_summary em português do Brasil; se não souber, use null ou vazio.",
);

fn anime_prompt(language: DescriptionLanguage) -> &'static str {
    match language {
        DescriptionLanguage::PtBr => ANIME_PROMPT_PT,
        DescriptionLanguage::En => ANIME_PROMPT,
    }
}

const OCR_PROMPT: &str =
    "Extract ALL visible text exactly as seen. Return ONLY raw text. Preserve line breaks.";

const RECEIPT_PROMPT: &str = concat!(
    "Given OCR text from a potential receipt/bank document, extract structured fields. ",
    "Return ONLY valid JSON schema: ",
    r#"{"document_kind":"receipt|invoice|bank_transfer|statement|other|none","#,
    r#""issuer":"string or null","#,
    r#""counterparty":"string or null","#,
    r#""date":"YYYY-MM-DD or null","#,
    r#""time":"HH:MM:SS or null","#,
    r#""amount":"string or null","#,
    r#""currency":"BRL|USD|EUR|other|null","#,
    r#""transaction_id":"string or null","#,
    r#""reference_numbers":["string"],"#,
    r#""important_fields":[{"key":"string","value":"string"}],"#,
    r#""confidence":0.0}"#,
    " Rules: use \"none\" when the text is not a receipt or a bank document — logos,",
    " signs, subtitles and captions are not; never infer a document from brand names,",
    " prices or dates on their own.",
    " OCR text:\n",
);

const VALID_MEDIA_TYPES: &[&str] = &[
    "screenshot",
    "anime",
    "manga",
    "photo",
    "document",
    "artwork",
    "other",
];

/// What the model is allowed to answer for `document_photo`.
///
/// Deliberately **not** the taxonomy of [`RECEIPT_PROMPT`] (`bank_transfer` and the
/// like): this field answers one question — is the picture itself that document? —
/// and it is what the OCR gate and the document bucket read.
pub const VALID_DOCUMENT_PHOTOS: &[&str] = &["none", "receipt", "invoice", "statement", "other"];

/// The document kind the primary pass named, when it named one of ours.
///
/// `none` is a real answer here (it means "this is a scene") and must survive
/// [`clean_nullable_str`], which drops the string "none" — hence the raw read.
fn document_photo_of(primary: &Value) -> Option<String> {
    let value = primary
        .get("document_photo")?
        .as_str()?
        .trim()
        .to_ascii_lowercase();
    VALID_DOCUMENT_PHOTOS
        .contains(&value.as_str())
        .then_some(value)
}

/// The `document_photo` value in a raw body whose JSON did not parse.
fn extract_document_photo(raw: &str) -> Option<String> {
    let re = Regex::new(r#"(?i)"document_photo"\s*:\s*"([^"]*)""#).ok()?;
    let value = re
        .captures(raw)?
        .get(1)?
        .as_str()
        .trim()
        .to_ascii_lowercase();
    VALID_DOCUMENT_PHOTOS
        .contains(&value.as_str())
        .then_some(value)
}

/// Regex-based salvage when all JSON attempts fail.
pub fn salvage_primary_from_raw(raw: &str) -> Option<Value> {
    if raw.is_empty() {
        return None;
    }
    let mt_re = Regex::new(r#"(?i)"media_type"\s*:\s*"([^"]+)""#).ok()?;
    let mt_match = mt_re.captures(raw)?;
    let media_type = mt_match.get(1)?.as_str().trim().to_lowercase();
    let media_type = if VALID_MEDIA_TYPES.contains(&media_type.as_str()) {
        media_type
    } else {
        "other".to_string()
    };

    fn extract_bool(raw: &str, key: &str, default: bool) -> bool {
        let pattern = format!(r#"(?i)"{key}"\s*:\s*(true|false)"#);
        Regex::new(&pattern)
            .ok()
            .and_then(|re| re.captures(raw))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().eq_ignore_ascii_case("true"))
            .unwrap_or(default)
    }

    let contains_text = extract_bool(raw, "contains_text", false);
    let is_anime = extract_bool(
        raw,
        "is_anime_related",
        matches!(media_type.as_str(), "anime" | "manga" | "artwork"),
    );
    let document_photo = extract_document_photo(raw).unwrap_or_else(|| {
        if matches!(media_type.as_str(), "document" | "screenshot") {
            "other".to_string()
        } else {
            "none".to_string()
        }
    });

    let desc_re = Regex::new(r#"(?i)"description"\s*:\s*"([^"]*)""#).ok()?;
    let desc = desc_re
        .captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default();

    let conf_re = Regex::new(r#"(?i)"confidence"\s*:\s*([0-9]*\.?[0-9]+)"#).ok()?;
    let confidence: f64 = conf_re
        .captures(raw)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0);

    let series = extract_quoted_items(raw, "series_candidates", 5);
    let chars = extract_quoted_items(raw, "character_candidates", 5);

    Some(serde_json::json!({
        "media_type": media_type,
        "contains_text": contains_text,
        "is_anime_related": is_anime,
        "document_photo": document_photo,
        "description": desc,
        "series_candidates": series,
        "character_candidates": chars,
        "confidence": confidence,
    }))
}

fn extract_quoted_items(raw: &str, key: &str, limit: usize) -> Vec<String> {
    let pattern = format!(r#"(?is)"{key}"\s*:\s*\[([^\]]*)"#);
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let fragment = match re.captures(raw).and_then(|c| c.get(1)) {
        Some(m) => m.as_str(),
        None => return Vec::new(),
    };
    let item_re = Regex::new(r#""([^"]+)""#).unwrap();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for cap in item_re.captures_iter(fragment) {
        let val = cap[1].trim().to_string();
        if val.is_empty() {
            continue;
        }
        let key = val.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        out.push(val);
        if out.len() >= limit {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Normalize / clean helpers
// ---------------------------------------------------------------------------

pub fn normalize_list(value: &Value) -> Vec<String> {
    let arr = match value.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| {
            !s.is_empty() && !matches!(s.to_lowercase().as_str(), "null" | "none" | "unknown")
        })
        .collect()
}

pub fn clean_nullable_str(value: &Value) -> Option<String> {
    let s = value.as_str()?.trim();
    if s.is_empty()
        || matches!(
            s.to_lowercase().as_str(),
            "null" | "none" | "unknown" | "n/a"
        )
    {
        return None;
    }
    Some(s.to_string())
}

// ---------------------------------------------------------------------------
// Enrichment routing
// ---------------------------------------------------------------------------

pub fn should_run_anime_enrichment(primary: &Value) -> bool {
    let media_type = primary
        .get("media_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_anime = primary
        .get("is_anime_related")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    matches!(media_type, "anime" | "manga" | "artwork") || is_anime
}

/// Whether OCR / document-field extraction should run for this classification.
///
/// `contains_text` alone must **not** trigger OCR: anime frames with burned-in
/// subtitles and fan art with text used to pay ~8 s of Surya per image for no
/// benefit. Surya spawns a fresh Python interpreter (torch + models) per file, so it
/// is only worth it for actual documents.
///
/// The routing signal is `document_photo`, the question the model is actually able to
/// answer — "is the picture itself this document?". The boolean it used to be
/// (`is_document_like`) was a guess: a beach-volleyball photo with the sponsor boards
/// of Banco do Brasil and GOL came back as `[Doc: bank_transfer] [Issuer: GOL]`
/// (2026-10-26) and paid the whole ~13 s OCR pass for a picture of a court. With the
/// explicit field a scene answers `none`, and a *photographed* receipt answers
/// `receipt` and does get its fields extracted — the recall the old rule traded away.
///
/// Anime, manga, artwork and video stay excluded no matter what the model says:
/// burned-in subtitles and logos are not documents, and those are the frames that
/// paid the most for nothing.
pub fn should_run_document_enrichment(primary: &Value, ocr_engine: OcrEngine) -> bool {
    if ocr_engine == OcrEngine::Off {
        return false;
    }
    let media_type = primary
        .get("media_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if matches!(media_type, "anime" | "manga" | "artwork" | "video") {
        return false;
    }
    // A scan or a screenshot of a document is a document even when the model calls the
    // picture a "photo": `document_photo` can add to this, never remove from it.
    if matches!(media_type, "document" | "screenshot") {
        return true;
    }
    matches!(
        document_photo_of(primary).as_deref(),
        Some("receipt" | "invoice" | "statement" | "other")
    )
}

/// Char-boundary-safe truncation for prompt payloads.
/// A result that carries nothing usable: the vision request failed.
///
/// [`classify_image_prepared`] cannot return an error (its callers work per file and
/// have nowhere to put one), so a failed request yields an empty description with
/// zero confidence. Writing that over a row that already had a good description is
/// data loss — measured once: 107 of 191 rows lost their description when Ollama
/// started answering 500.
pub fn is_empty_result(result: &ClassificationResult) -> bool {
    result.description.trim().is_empty()
        && result.extracted_text.trim().is_empty()
        && result.canonical_mentions.trim().is_empty()
        && result.confidence <= 0.0
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

// ---------------------------------------------------------------------------
// Receipt regex extraction
// ---------------------------------------------------------------------------

pub fn extract_receipt_regex(text: &str) -> Value {
    let date_patterns = [
        r"\b(\d{4}-\d{2}-\d{2})\b",
        r"\b(\d{2}/\d{2}/\d{4})\b",
        r"\b(\d{2}-\d{2}-\d{4})\b",
    ];
    let amount_patterns = [
        r"\b(?:R\$|\$|EUR|USD)\s?[0-9][0-9.,]*\b",
        r"\b[0-9]{1,3}(?:\.[0-9]{3})*,[0-9]{2}\b",
        r"\b[0-9]+(?:\.[0-9]{2})\b",
    ];
    let transaction_patterns = [
        r"(?i)\b(?:ID|Tx|Transaction|Protocolo|Comprovante|Reference)[:\s#-]*([A-Za-z0-9\-_/]{6,})\b",
    ];

    fn collect_matches(text: &str, patterns: &[&'static str]) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for pat in patterns {
            if let Ok(re) = Regex::new(pat) {
                for cap in re.captures_iter(text) {
                    let val = cap
                        .get(1)
                        .unwrap_or_else(|| cap.get(0).unwrap())
                        .as_str()
                        .to_string();
                    if !seen.contains(&val) {
                        seen.insert(val.clone());
                        out.push(val);
                    }
                }
            }
        }
        out
    }

    let dates: Vec<String> = collect_matches(text, &date_patterns)
        .into_iter()
        .take(6)
        .collect();
    let amounts: Vec<String> = collect_matches(text, &amount_patterns)
        .into_iter()
        .take(10)
        .collect();
    let refs: Vec<String> = collect_matches(text, &transaction_patterns)
        .into_iter()
        .take(10)
        .collect();

    let currency = if text.contains("R$") {
        Some("BRL")
    } else if text.contains("USD") || text.contains('$') {
        Some("USD")
    } else if text.contains("EUR") {
        Some("EUR")
    } else {
        None
    };

    serde_json::json!({
        "dates": dates,
        "amount_candidates": amounts,
        "reference_numbers": refs,
        "currency_guess": currency,
    })
}

// ---------------------------------------------------------------------------
// GIF first-frame extraction
// ---------------------------------------------------------------------------

fn first_frame_if_gif(image_path: &Path, tmp_dir: &Path) -> PathBuf {
    if image_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
        != Some("gif")
    {
        return image_path.to_path_buf();
    }
    let stem = image_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "frame".to_string());
    let out = tmp_dir.join(format!("{stem}_frame1.png"));
    if out.exists() {
        return out;
    }
    let _ = std::fs::create_dir_all(tmp_dir);
    match image::open(image_path) {
        Ok(img) => {
            if let Err(e) = img.to_rgb8().save(&out) {
                log::warn!("Failed to save GIF first frame: {e}");
                return image_path.to_path_buf();
            }
            out
        }
        Err(e) => {
            log::warn!("Failed to open GIF for first frame: {e}");
            image_path.to_path_buf()
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 1: Primary classification
// ---------------------------------------------------------------------------

/// Whether the original file still fits the window the prepared image resolved to.
///
/// Ollama counts the tokens of the file that is actually sent, and the original is
/// not the 768 px JPEG the window was computed for: a 12 MP photo needs several
/// times the 4096 tokens of the prepared one. Sending it anyway answered `400
/// request (4136 tokens) exceeds the available context size (4096 tokens)` **25
/// times on 2026-10-25**, always on the original-file attempt — a request spent and
/// the file stranded anyway.
///
/// Asking for a bigger window here would be worse than dropping the attempt: the
/// next file goes back to the prepared window, so every rescued file would cost two
/// model reloads (§5.5 of `plans/pipeline-cpu-gpu-overlap.md`). Those files belong
/// in a block of their own, with one window for the whole block.
///
/// `None` when the dimensions cannot be read (an exotic format): the attempt is
/// kept, because refusing to try would be a regression for small originals.
fn original_fits_window(
    path: &Path,
    num_ctx: u32,
    prompt_chars: usize,
    num_predict: u32,
) -> Option<bool> {
    let (width, height) = image::image_dimensions(path).ok()?;
    Some(num_ctx_for(width, height, prompt_chars, num_predict) <= num_ctx)
}

/// Prompt profile the first pass really uses.
///
/// `library_kind = anime` upgrades the first pass to the detailed schema, which already
/// asks for `series_candidates` and `character_candidates`. The value has to be the same
/// in every place that derives `num_ctx` from it — here, the backlog block planner and the
/// producer's deferral check — because a mismatch changes the context window and makes
/// Ollama rebuild the runner per file.
pub fn effective_profile(vision: &VisionSettings) -> PromptProfile {
    match vision.library_kind {
        LibraryKind::Anime => PromptProfile::Detailed,
        LibraryKind::Auto | LibraryKind::Photos => vision.prompt_profile,
    }
}

/// Context window a still image of `width × height` will need once prepared.
///
/// The backlog pipeline groups rows by this value before opening them, so one
/// runner identity serves a whole block instead of being rebuilt per file. The
/// classifier itself keeps deriving `num_ctx` from the prepared image it really
/// sends ([`num_ctx_for`]); [`vision_prep::prepared_size`] is what keeps the two
/// numbers identical.
pub fn primary_ctx_for(
    width: u32,
    height: u32,
    max_side: u32,
    profile: PromptProfile,
    language: DescriptionLanguage,
) -> u32 {
    let (prepared_w, prepared_h) = vision_prep::prepared_size(width, height, max_side);
    num_ctx_for(
        prepared_w,
        prepared_h,
        primary_prompt(profile, language).len(),
        primary_num_predict(profile),
    )
}

/// What the first pass produced, and why it produced nothing when it did not.
///
/// The value alone cannot tell a good classification from a default object built
/// after four failures, and the reason is exactly what a retry has to be aimed at —
/// so the first pass reports it instead of only logging it.
type PrimaryOutcome = (Value, Option<&'static str>, ParseKind);

/// How the first pass arrived at its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseKind {
    /// Valid JSON with a `media_type` on the first attempt.
    Strict,
    /// Recovered from the body text by [`salvage_primary_from_raw`].
    Salvaged,
    /// Nothing usable came back; the value is the safe default object.
    Default,
}

/// A confidence number the **pipeline** can justify.
///
/// The model's own `confidence` field was removed from the primary schema after job #35
/// (2026-10-28) measured **22,809 of 36,744** described rows at exactly 0.0: the schema
/// showed `"confidence":0.0` and the model copied that literal at temperature 0. A number
/// the model supplies cannot measure the model's own certainty, so the score is derived
/// from evidence that exists outside the answer:
///
/// - how the answer was obtained (strict JSON > salvage);
/// - whether `media_type` came back as one of the values we accept;
/// - whether the description has a usable length;
/// - whether a second signal corroborated it (OCR text, or a confirmed document kind).
///
/// `Default` returns exactly 0.0, which is what keeps [`is_empty_result`] honest: a row
/// with no description must stay pending, not be stored with a small score.
///
/// The ceiling is 0.95 on purpose — nothing the model says is certain enough for 1.0.
pub fn derived_confidence(
    parse: ParseKind,
    media_type: &str,
    description: &str,
    corroborated: bool,
) -> f32 {
    let mut score: f32 = match parse {
        ParseKind::Strict => 0.80,
        ParseKind::Salvaged => 0.55,
        ParseKind::Default => return 0.0,
    };
    if media_type == "other" || !VALID_MEDIA_TYPES.contains(&media_type) {
        score -= 0.15;
    }
    let words = description.split_whitespace().count();
    if !(3..=24).contains(&words) {
        score -= 0.10;
    }
    if corroborated {
        score += 0.10;
    }
    score.clamp(0.05, 0.95)
}

/// Why a request produced nothing, as a short taxonomy.
///
/// These strings are what `files.failure_kind` stores (migration 15) and what the
/// "Retry failed" grouping can key on. They are derived from the text Ollama answers
/// with, because that is all the reason we get: `400 … exceeds the available context
/// size`, `500 … token repeat limit reached`, the zero-value frame, a 200 whose body
/// does not parse, and the transport errors.
fn failure_kind_from(raw: &str) -> &'static str {
    let lower = raw.to_ascii_lowercase();
    // The zero-value frame wins over everything else: the runner answered without
    // doing any work, so a `token repeat limit` that follows it was produced by the
    // wedged model, not by the picture. Keeping this branch first is what stops a
    // runner wedge from being filed as a permanent `repeat_limit`.
    if lower.starts_with("empty_frame:")
        || lower.contains("\"model\":\"\"")
        || lower.contains("done=false")
    {
        // The Go zero-value frame: the runner never saw the request (§5.7.5).
        "empty_frame"
    } else if lower.contains("exceeds the available context size") {
        "ctx_overflow"
    } else if lower.contains("token repeat limit") {
        "repeat_limit"
    } else if lower.contains("connection refused") || lower.contains("io: ") {
        "io"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if raw.trim().is_empty() {
        "no_response"
    } else {
        "parse"
    }
}

/// Failure kind for a primary that produced nothing, given its answers.
///
/// When the **first** attempt never ran, the rest is noise from a wedged runner: the
/// client unloads and reloads the model, and the retries that follow come back as
/// `500 prediction aborted, token repeat limit reached` *because* the runner is wedged.
/// Job #35 (2026-10-28) filed 619 rows as `repeat_limit` — a permanent kind, hidden
/// from the backlog until a manual retry — purely because the later 500s won
/// [`failure_kind_from`] over the zero-value frame that started it all.
fn primary_failure_kind(first: &str, combined: &str) -> &'static str {
    let kind = failure_kind_from(first);
    if kind == "empty_frame" {
        return kind;
    }
    failure_kind_from(combined)
}

fn classify_primary(
    model: &str,
    prepared: &PreparedImage,
    profile: PromptProfile,
    language: DescriptionLanguage,
) -> PrimaryOutcome {
    let prompt = primary_prompt(profile, language);
    let num_predict = primary_num_predict(profile);
    let num_ctx = num_ctx_for(prepared.width, prepared.height, prompt.len(), num_predict);

    // Attempt 1: json mode (valid JSON in 100% of local runs, so a retry is rare).
    let params = GenerateParams::vision(num_predict, Some(num_ctx));
    let resp1 = ollama_generate(model, prompt, Some(&prepared.path), &params);
    log::debug!(
        "vision primary {}x{} bytes={} ctx={} prefill={:.2}s decode={:.2}s tokens={}/{}",
        prepared.width,
        prepared.height,
        prepared.prepared_bytes,
        num_ctx,
        resp1.timings.prefill_s,
        resp1.timings.decode_s,
        resp1.timings.prompt_tokens,
        resp1.timings.eval_tokens,
    );
    if resp1.ok {
        if let Some(v) = parse_json_response(&resp1.raw) {
            if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                return (v, None, ParseKind::Strict);
            }
        }
        // A 200 with an unusable body is a different failure from a 500, and it was
        // completely silent: the row ended up as `other` with no description and
        // nothing in the log explained why (107 of 191 files in one run).
        log::warn!(
            "primary (json) returned unparseable output for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp1.raw, 200)
        );
    } else {
        log::warn!(
            "primary (json) failed for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp1.raw, 300)
        );
    }

    // Attempt 2: the **same** prompt, just without the JSON grammar.
    //
    // Keeping the prompt matters. The old retry also swapped the wording, and that
    // wording is what makes qwen2.5-VL loop on flat images — Ollama answers 500
    // `prediction aborted, token repeat limit reached` (measured on 107 photos of a
    // construction site: 107 of 107 failed). With this prompt and no grammar the
    // same 107 answer correctly, so the retry keeps the prompt and changes only the
    // sampling: no grammar, the repetition guard, and a bounded budget.
    let params2 = GenerateParams {
        json_mode: false,
        repeat_penalty: Some(RECOVERY_REPEAT_PENALTY),
        repeat_last_n: Some(RECOVERY_REPEAT_LAST_N),
        ..GenerateParams::vision(recovery_num_predict(num_predict), Some(num_ctx))
    };
    let resp2 = ollama_generate(model, prompt, Some(&prepared.path), &params2);
    if resp2.ok {
        if let Some(v) = parse_json_response(&resp2.raw) {
            if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                return (v, None, ParseKind::Strict);
            }
        }
        log::warn!(
            "primary (fallback) returned unparseable output for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp2.raw, 200)
        );
    } else {
        log::warn!(
            "primary (fallback) failed for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp2.raw, 300)
        );
    }

    // Attempt 3: same prompt, no grammar, the repetition guard, and a little entropy.
    //
    // The repetition loop that kills attempts 1 and 2 is not deterministic — the same
    // image answered fine minutes later — so one more sample with temperature is the
    // cheap way out. The budget does **not** grow here any more: the abort comes from
    // Ollama's repeat limit, not from the budget, so a bigger `num_predict` only keeps
    // a loop running longer before it dies.
    let params3 = GenerateParams {
        json_mode: false,
        temperature: 0.3,
        repeat_penalty: Some(RECOVERY_REPEAT_PENALTY),
        repeat_last_n: Some(RECOVERY_REPEAT_LAST_N),
        ..GenerateParams::vision(recovery_num_predict(num_predict), Some(num_ctx))
    };
    let resp3 = ollama_generate(model, prompt, Some(&prepared.path), &params3);
    if resp3.ok {
        if let Some(v) = parse_json_response(&resp3.raw) {
            if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                log::info!(
                    "primary recovered on the third attempt for {}",
                    prepared.path.display()
                );
                return (v, None, ParseKind::Strict);
            }
        }
        log::warn!(
            "primary (third attempt) returned unparseable output for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp3.raw, 200)
        );
    } else {
        log::warn!(
            "primary (third attempt) failed for {}: {}",
            prepared.path.display(),
            truncate_chars(&resp3.raw, 300)
        );
    }

    // Attempt 4: the **original** file. Every attempt so far sent a re-encoded 768 px
    // JPEG; if all three loop on that, give the bytes the camera wrote a chance. Only
    // when the prepared image is a temp copy (a passthrough already *is* the original)
    // and only when those bytes fit the window already requested — see
    // [`original_fits_window`].
    // Two looping attempts in a row mean the problem is the picture, not the
    // sampling: the original bytes are the *same* picture at a higher resolution, so
    // the full-resolution request loops too — measured on job #35 (2026-10-28), where
    // the attempt-4 request answered with the very same `repeat limit` 500. It is
    // also the most expensive request of the file, so it is not worth sending.
    let looped_twice = failure_kind_from(&resp2.raw) == "repeat_limit"
        && failure_kind_from(&resp3.raw) == "repeat_limit";

    let mut resp4_raw = String::new();
    let original_fits = if looped_twice {
        log::info!(
            "primary: not sending the original file for {} — attempts 2 and 3 both hit \
             the token repeat limit",
            prepared.source.display()
        );
        None
    } else {
        (prepared.source != prepared.path)
            .then(|| original_fits_window(&prepared.source, num_ctx, prompt.len(), num_predict))
    };
    match original_fits {
        Some(Some(false)) => log::info!(
            "primary: not sending the original file for {} — it needs more than the \
             {num_ctx}-token window the prepared image uses",
            prepared.source.display()
        ),
        Some(_) => {
            let params4 = GenerateParams {
                json_mode: false,
                temperature: 0.3,
                repeat_penalty: Some(RECOVERY_REPEAT_PENALTY),
                repeat_last_n: Some(RECOVERY_REPEAT_LAST_N),
                ..GenerateParams::vision(recovery_num_predict(num_predict), Some(num_ctx))
            };
            let resp4 = ollama_generate(model, prompt, Some(&prepared.source), &params4);
            if resp4.ok {
                if let Some(v) = parse_json_response(&resp4.raw) {
                    if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                        log::info!(
                            "primary recovered from the original file for {}",
                            prepared.source.display()
                        );
                        return (v, None, ParseKind::Strict);
                    }
                }
                log::warn!(
                    "primary (original file) returned unparseable output for {}: {}",
                    prepared.source.display(),
                    truncate_chars(&resp4.raw, 200)
                );
            } else {
                log::warn!(
                    "primary (original file) failed for {}: {}",
                    prepared.source.display(),
                    truncate_chars(&resp4.raw, 300)
                );
            }
            resp4_raw = resp4.raw;
        }
        None => {}
    }

    let combined = format!("{}\n{}\n{}\n{}", resp1.raw, resp2.raw, resp3.raw, resp4_raw);
    if let Some(v) = salvage_primary_from_raw(&combined) {
        return (v, None, ParseKind::Salvaged);
    }
    // Nothing worked: keep the exact image we sent (it is deleted by `Drop`, and
    // without it there is no way to reproduce the failure outside the app).
    let keep = prepared.path.with_file_name(format!(
        "keep_{}",
        prepared
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image.jpg".to_string())
    ));
    // Both sizes go in the line. The source dimensions are what the resolution floor
    // compares (`min_pixels`), so a 4000x3000 photo that uploads as 768x576 and 70 KB
    // must not read like a file that slipped under a 1 MP floor.
    match std::fs::copy(&prepared.path, &keep) {
        Ok(bytes) => log::error!(
            "primary gave up on {} (source {}x{} at {} B, sent {}x{} at {} B, kept at {})",
            prepared.source.display(),
            prepared.source_width,
            prepared.source_height,
            prepared.original_bytes,
            prepared.width,
            prepared.height,
            bytes,
            keep.display()
        ),
        Err(err) => log::error!(
            "primary gave up on {} (source {}x{} at {} B, sent {}x{} at {} B, and the \
             prepared image could not be kept: {err})",
            prepared.source.display(),
            prepared.source_width,
            prepared.source_height,
            prepared.original_bytes,
            prepared.width,
            prepared.height,
            prepared.prepared_bytes
        ),
    }

    // Safe default, plus the reason the caller persists on the row.
    (
        serde_json::json!({
            "media_type": "other",
            "contains_text": false,
            "is_anime_related": false,
            "document_photo": "none",
            "description": "",
            "series_candidates": [],
            "character_candidates": [],
            "confidence": 0.0,
        }),
        Some(primary_failure_kind(&resp1.raw, &combined)),
        ParseKind::Default,
    )
}

/// Result of one real first-pass request, used to calibrate the pre-scan estimate.
#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub ok: bool,
    pub wall_s: f64,
    pub timings: crate::llm::OllamaTimings,
    /// Raw response body. Production only reads the timings; the cost bench keeps
    /// it to compare two resampling kernels on the same image.
    #[allow(dead_code)]
    pub raw: String,
}

/// Run the production first-pass request once and report how long it took.
///
/// This is what makes the scan projection hardware-accurate: instead of a
/// hardcoded throughput table, the estimator measures real images from the
/// user's library with the exact prompt, resolution and context that the scan
/// will use.
pub fn probe_primary(
    model: &str,
    prepared: &PreparedImage,
    profile: PromptProfile,
    language: DescriptionLanguage,
) -> ProbeResult {
    let prompt = primary_prompt(profile, language);
    let num_predict = primary_num_predict(profile);
    let num_ctx = num_ctx_for(prepared.width, prepared.height, prompt.len(), num_predict);

    let started = std::time::Instant::now();
    let resp = ollama_generate(
        model,
        prompt,
        Some(&prepared.path),
        &GenerateParams::vision(num_predict, Some(num_ctx)),
    );
    let wall_s = started.elapsed().as_secs_f64();
    log::debug!(
        "vision probe {}x{}: wall={:.2}s load={:.2}s total={:.2}s prefill={:.2}s decode={:.2}s",
        prepared.width,
        prepared.height,
        wall_s,
        resp.timings.load_s,
        resp.timings.total_s,
        resp.timings.prefill_s,
        resp.timings.decode_s,
    );
    ProbeResult {
        ok: resp.ok,
        wall_s,
        timings: resp.timings,
        raw: resp.raw,
    }
}

// ---------------------------------------------------------------------------
// Stage 2a: Anime enrichment
// ---------------------------------------------------------------------------

fn classify_anime_details(
    model: &str,
    prepared: &PreparedImage,
    language: DescriptionLanguage,
) -> Option<Value> {
    const NUM_PREDICT: u32 = 500;
    let prompt = anime_prompt(language);
    let num_ctx = num_ctx_for(prepared.width, prepared.height, prompt.len(), NUM_PREDICT);

    let params = GenerateParams::vision(NUM_PREDICT, Some(num_ctx));
    let resp = ollama_generate(model, prompt, Some(&prepared.path), &params);
    if resp.ok {
        if let Some(v) = parse_json_response(&resp.raw) {
            let conf = v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0);
            let has_data = v.get("series").and_then(clean_nullable_str).is_some()
                || v.get("characters")
                    .and_then(|a| a.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
            if has_data || conf >= 0.2 {
                return Some(v);
            }
        }
    }

    // Retry without json_mode
    let params2 = GenerateParams {
        json_mode: false,
        ..GenerateParams::vision(NUM_PREDICT, Some(num_ctx))
    };
    let resp2 = ollama_generate(model, prompt, Some(&prepared.path), &params2);
    if resp2.ok {
        if let Some(v) = parse_json_response(&resp2.raw) {
            return Some(v);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Stage 2b: OCR
// ---------------------------------------------------------------------------

fn run_surya_ocr(image_path: &Path, surya_venv: &Path, surya_script: &Path) -> OcrResult {
    let python_bin = crate::platform::python::python_venv_binary(surya_venv);
    if !python_bin.exists() || !surya_script.exists() {
        return OcrResult::failed("surya");
    }

    let result = silent_command(&python_bin)
        .arg(surya_script)
        .arg(image_path)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
                let ok = v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false);
                let text = v
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let line_count = v.get("line_count").and_then(|n| n.as_u64()).unwrap_or(0);
                return OcrResult {
                    ok,
                    engine: "surya".to_string(),
                    text,
                    line_count,
                };
            }
            OcrResult::failed("surya")
        }
        _ => OcrResult::failed("surya"),
    }
}

fn run_llm_ocr(model: &str, prepared: &PreparedImage) -> OcrResult {
    const NUM_PREDICT: u32 = 1500;
    let params = GenerateParams {
        json_mode: false,
        timeout_secs: 300,
        ..GenerateParams::vision(
            NUM_PREDICT,
            Some(num_ctx_for(
                prepared.width,
                prepared.height,
                OCR_PROMPT.len(),
                NUM_PREDICT,
            )),
        )
    };
    let resp = ollama_generate(model, OCR_PROMPT, Some(&prepared.path), &params);
    if !resp.ok {
        return OcrResult::failed("vision_llm");
    }
    let text = resp.raw.trim().to_string();
    let line_count = text.lines().count() as u64;
    OcrResult {
        ok: true,
        engine: "vision_llm".to_string(),
        text,
        line_count,
    }
}

struct OcrResult {
    ok: bool,
    #[allow(dead_code)]
    engine: String,
    text: String,
    #[allow(dead_code)]
    line_count: u64,
}

impl OcrResult {
    fn failed(engine: &str) -> Self {
        Self {
            ok: false,
            engine: engine.to_string(),
            text: String::new(),
            line_count: 0,
        }
    }
}

fn run_ocr(
    model: &str,
    prepared: &PreparedImage,
    ocr_engine: OcrEngine,
    surya_venv: &Path,
    surya_script: &Path,
) -> OcrResult {
    match ocr_engine {
        OcrEngine::Off => OcrResult::failed("disabled"),
        OcrEngine::Llm => run_llm_ocr(model, prepared),
        OcrEngine::Surya => {
            // Surya gets the original file: downscaling destroys the fine text it
            // exists to recover.
            let surya = run_surya_ocr(&prepared.source, surya_venv, surya_script);
            if surya.ok && !surya.text.is_empty() {
                surya
            } else {
                run_llm_ocr(model, prepared)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 2c: Document field extraction
// ---------------------------------------------------------------------------

/// The document type the model named, when it named a real one.
///
/// `"none"` is part of the schema on purpose: without it the model had to pick from
/// `receipt|invoice|bank_transfer|statement|other` even when the text was not a
/// document at all, and a beach-volleyball court came back as a `bank_transfer`
/// because the sponsor boards said "BANCO DO BRASIL" and "GOL" (2026-10-26).
fn document_kind_of(fields: &Value) -> Option<String> {
    let kind = fields.get("document_kind").and_then(clean_nullable_str)?;
    if matches!(kind.as_str(), "none" | "null") {
        return None;
    }
    Some(kind)
}

/// Whether the extracted fields describe a real document instead of text that merely
/// looks like one.
///
/// The model has to name a document **and** the OCR text has to carry a document
/// signal: an amount together with a date, or a currency. A court full of logos has
/// names and prices and is not a receipt. `extracted_text` keeps everything either
/// way — this only decides the `[Doc: …] [Issuer: …]` suffix on the description.
fn document_fields_are_real(fields: &Value, ocr_text: &str) -> bool {
    if document_kind_of(fields).is_none() {
        return false;
    }
    let signals = extract_receipt_regex(ocr_text);
    let has_items = |key: &str| {
        signals
            .get(key)
            .and_then(|value| value.as_array())
            .is_some_and(|items| !items.is_empty())
    };
    let currency = signals
        .get("currency_guess")
        .and_then(|value| value.as_str())
        .is_some_and(|code| !code.is_empty());
    (has_items("dates") && has_items("amount_candidates")) || currency
}

fn extract_document_fields(model: &str, ocr_text: &str) -> Value {
    let regex_fields = extract_receipt_regex(ocr_text);

    // Only the head of the OCR text matters for field extraction, and a long
    // receipt dump is pure prefill cost.
    let prompt = format!("{RECEIPT_PROMPT}{}", truncate_chars(ocr_text, 1200));
    const NUM_PREDICT: u32 = 400;
    let params = GenerateParams::vision(
        NUM_PREDICT,
        Some(num_ctx_for(0, 0, prompt.len(), NUM_PREDICT)),
    );
    let resp = ollama_generate(model, &prompt, None, &params);
    let llm_fields = if resp.ok {
        parse_json_response(&resp.raw)
    } else {
        None
    };

    let mut doc = serde_json::json!({});
    if let Some(ref llm) = llm_fields {
        for key in [
            "document_kind",
            "issuer",
            "counterparty",
            "date",
            "time",
            "amount",
            "currency",
            "transaction_id",
            "reference_numbers",
            "important_fields",
            "confidence",
        ] {
            if let Some(v) = llm.get(key) {
                doc[key] = v.clone();
            }
        }
    }

    // Fall back to regex for currency and reference_numbers
    if doc.get("currency").and_then(clean_nullable_str).is_none() {
        if let Some(c) = regex_fields.get("currency_guess").and_then(|v| v.as_str()) {
            doc["currency"] = serde_json::json!(c);
        }
    }
    if doc
        .get("reference_numbers")
        .and_then(|v| v.as_array())
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        doc["reference_numbers"] = regex_fields
            .get("reference_numbers")
            .cloned()
            .unwrap_or(serde_json::json!([]));
    }

    // Transaction ID cleanup
    let txid = doc
        .get("transaction_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    let mut final_txid = txid.clone();
    if let Some(ref t) = txid {
        let lower = t.to_lowercase();
        if lower == "ted realizado com sucesso." || lower == "ted realizado com sucesso" {
            final_txid = None;
        }
    }
    if final_txid.is_none() {
        if let Ok(re) = Regex::new(r"\b[A-Z0-9]{12,}\b") {
            let all_matches: Vec<String> = re
                .find_iter(ocr_text)
                .map(|m| m.as_str().to_string())
                .collect();
            if !all_matches.is_empty() {
                let has_digit = Regex::new(r"[0-9]").unwrap();
                let has_alpha = Regex::new(r"[A-Z]").unwrap();
                let mixed: Vec<&String> = all_matches
                    .iter()
                    .filter(|m| has_digit.is_match(m) && has_alpha.is_match(m))
                    .collect();
                let alpha: Vec<&String> = all_matches
                    .iter()
                    .filter(|m| has_alpha.is_match(m))
                    .collect();
                final_txid = Some(
                    mixed
                        .first()
                        .copied()
                        .or_else(|| alpha.first().copied())
                        .unwrap_or(&all_matches[0])
                        .clone(),
                );
            }
        }
    }
    if let Some(t) = final_txid {
        doc["transaction_id"] = serde_json::json!(t);
    } else {
        doc["transaction_id"] = Value::Null;
    }

    doc
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub fn classify_image(
    image_path: &Path,
    model: &str,
    tmp_dir: &Path,
    surya_venv: &Path,
    surya_script: &Path,
    vision: VisionSettings,
) -> ClassificationResult {
    // GIFs are classified from their first frame; everything else goes through
    // `vision_prep`, which downscales and applies EXIF orientation once so every
    // stage below reuses the same bytes instead of re-encoding per attempt.
    let frame_source = first_frame_if_gif(image_path, tmp_dir);
    let prepared = match vision_prep::prepare_with(
        &frame_source,
        vision.image_max_side,
        tmp_dir,
        vision.image_filter,
    ) {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "Cannot prepare {} for the vision model: {e}",
                image_path.display()
            );
            return ClassificationResult {
                media_type: "other".to_string(),
                description: String::new(),
                extracted_text: String::new(),
                canonical_mentions: String::new(),
                confidence: 0.0,
                lang_hint: String::new(),
                // The file was never read, so nothing was evaluated.
                document_hint: String::new(),
                failure_kind: "prepare".to_string(),
            };
        }
    };

    classify_image_prepared(
        image_path,
        &prepared,
        model,
        surya_venv,
        surya_script,
        vision,
    )
}

/// Classify an image whose model-ready JPEG already exists.
///
/// The pipeline producer pays for the decode and the downscale once, so the
/// consumer must not repeat either (`plans/pipeline-cpu-gpu-overlap.md`,
/// Phase 2). Everything below operates on `prepared` only — the original file is
/// never touched again except for OCR engines that need full resolution.
pub fn classify_image_prepared(
    image_path: &Path,
    prepared: &PreparedImage,
    model: &str,
    surya_venv: &Path,
    surya_script: &Path,
    vision: VisionSettings,
) -> ClassificationResult {
    log::info!("Classifying: {}", image_path.display());

    // `library_kind = anime` asks for series and characters in the *first* pass; see
    // [`effective_profile`].
    let (primary, primary_failure, parse_kind) = classify_primary(
        model,
        prepared,
        effective_profile(&vision),
        vision.description_language,
    );

    let media_type = primary
        .get("media_type")
        .and_then(|v| v.as_str())
        .unwrap_or("other")
        .to_string();
    let description = primary
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // `none` is the model saying "this is a scene". A document or a screenshot without
    // the field (old prompt, salvaged body) still belongs to the bucket, so it reads
    // `other` instead of `none`.
    let document_hint = document_photo_of(&primary).unwrap_or_else(|| {
        if matches!(media_type.as_str(), "document" | "screenshot") {
            "other".to_string()
        } else {
            "none".to_string()
        }
    });

    let mut full_description = description.clone();
    let mut extracted_text = String::new();
    let mut canonical_mentions = String::new();

    // Anime enrichment, under the policy declared for this library.
    //
    // This is the request that costs a second image pass: the same picture, sent again
    // with a bigger schema. `Photos` drops it even for the false positive path (a photo
    // the model calls `artwork`), and `Anime` drops it because the first pass already
    // asked. The mentions can still be filled later by the explicit "Enriquecer animes"
    // pass, which selects exactly the rows that have `media_type` anime-ish and no
    // `canonical_mentions`.
    let enrich_anime = match vision.library_kind {
        LibraryKind::Photos | LibraryKind::Anime => false,
        LibraryKind::Auto => should_run_anime_enrichment(&primary),
    };
    if enrich_anime {
        if let Some(anime) = classify_anime_details(model, prepared, vision.description_language) {
            if let Some(series) = anime.get("series").and_then(clean_nullable_str) {
                full_description = format!("{full_description} [Series: {series}]");
            }
            let mentions = normalize_list(anime.get("canonical_mentions").unwrap_or(&Value::Null));
            if !mentions.is_empty() {
                canonical_mentions = mentions.join(", ");
            }
            if let Some(chars) = anime.get("characters").and_then(|v| v.as_array()) {
                let char_names: Vec<String> = chars
                    .iter()
                    .filter_map(|c| {
                        c.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect();
                if !char_names.is_empty() && canonical_mentions.is_empty() {
                    canonical_mentions = char_names.join(", ");
                }
            }
        }
    }

    // Also add series/character candidates from primary to canonical_mentions
    let series_cands = normalize_list(primary.get("series_candidates").unwrap_or(&Value::Null));
    let char_cands = normalize_list(primary.get("character_candidates").unwrap_or(&Value::Null));
    let mut all_mentions: Vec<String> = Vec::new();
    if !canonical_mentions.is_empty() {
        all_mentions.extend(canonical_mentions.split(", ").map(|s| s.to_string()));
    }
    for c in series_cands.iter().chain(char_cands.iter()) {
        let lower = c.to_lowercase();
        if !all_mentions.iter().any(|m| m.to_lowercase() == lower) {
            all_mentions.push(c.clone());
        }
    }
    canonical_mentions = all_mentions.join(", ");

    // Document enrichment
    if should_run_document_enrichment(&primary, vision.ocr_engine) {
        let ocr = run_ocr(model, prepared, vision.ocr_engine, surya_venv, surya_script);
        if ocr.ok && !ocr.text.is_empty() {
            extracted_text = ocr.text.clone();

            // The suffix only goes in when the fields describe an actual document:
            // `document_kind` alone was not enough, because the model read the sponsor
            // boards of a volleyball court as a bank transfer.
            let doc_fields = extract_document_fields(model, &ocr.text);
            if document_fields_are_real(&doc_fields, &ocr.text) {
                if let Some(kind) = document_kind_of(&doc_fields) {
                    full_description = format!("{full_description} [Doc: {kind}]");
                }
                if let Some(issuer) = doc_fields.get("issuer").and_then(clean_nullable_str) {
                    full_description = format!("{full_description} [Issuer: {issuer}]");
                }
            }
        }
    }

    let lang_hint = detect_lang_hint(&extracted_text);

    // Derived from evidence, never from the model's own number: see
    // [`derived_confidence`]. `corroborated` means a second signal exists on the row — OCR
    // text, or a document kind the regex confirmed — which is what separates "the model
    // said something plausible" from "the pipeline can point at why".
    let confidence = derived_confidence(
        parse_kind,
        &media_type,
        &full_description,
        !extracted_text.trim().is_empty() || !matches!(document_hint.as_str(), "none" | ""),
    );

    ClassificationResult {
        media_type,
        description: full_description,
        extracted_text,
        canonical_mentions,
        confidence,
        lang_hint,
        document_hint,
        failure_kind: primary_failure.unwrap_or_default().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Video classification entry point
// ---------------------------------------------------------------------------

const VIDEO_PROMPT: &str = concat!(
    // Singular on purpose: one keyframe is sent, and a prompt that promised several made
    // the model talk about frames it could not see.
    "This is a keyframe from a video. Analyze it and respond ONLY with valid JSON. Schema: ",
    r#"{"media_type":"video","#,
    r#""video_category":"movie|tv_episode|personal|music_video|tutorial|other","#,
    r#""is_anime_related":false,"#,
    r#""description":"short factual description","#,
    r#""title_guess":"string or null","#,
    r#""series_candidates":["name"],"#,
    r#""character_candidates":["name"]}"#,
    " Rules: ",
    "1) Use null-like empty arrays when unknown. ",
    "2) video_category: movie=feature film, tv_episode=TV show episode, personal=home video/vlog, ",
    "music_video=music clip, tutorial=educational/how-to, other=everything else. ",
    "3) title_guess: best guess at the title or null if unknown. ",
    "4) series_candidates and character_candidates max 5 items each. ",
    "5) Keep description under 24 words. ",
    "6) Favor precision over guesswork.",
);

/// `title_guess` is deliberately **not** translated: it is the name of the work, and a
/// translated name would neither match the filename nor search. `description` and the
/// category explanation do follow `description_language`; the category values stay
/// English because `video_category` is matched in Rust.
const VIDEO_PROMPT_PT: &str = concat!(
    "Este é um quadro-chave de um vídeo. Analise e responda SOMENTE com JSON válido. Schema: ",
    r#"{"media_type":"video","#,
    r#""video_category":"movie|tv_episode|personal|music_video|tutorial|other","#,
    r#""is_anime_related":false,"#,
    r#""description":"descrição curta e factual","#,
    r#""title_guess":"título original ou null","#,
    r#""series_candidates":["nome"],"#,
    r#""character_candidates":["nome"]}"#,
    " Regras: ",
    "1) Use arrays vazios quando não souber. ",
    "2) video_category: movie=filme, tv_episode=episódio de série, personal=filme caseiro/vlog, ",
    "music_video=clipe musical, tutorial=educativo/passo a passo, other=todo o resto. ",
    "3) title_guess: o título original, sem traduzir; null se desconhecido. ",
    "4) series_candidates e character_candidates: no máximo 5 itens cada, nomes no original. ",
    "5) description em português do Brasil, com no máximo 24 palavras. ",
    "6) Prefira precisão a adivinhação.",
);

fn video_prompt(language: DescriptionLanguage) -> &'static str {
    match language {
        DescriptionLanguage::PtBr => VIDEO_PROMPT_PT,
        DescriptionLanguage::En => VIDEO_PROMPT,
    }
}

pub fn classify_video(
    video_path: &Path,
    model: &str,
    tmp_dir: &Path,
    _surya_venv: &Path,
    _surya_script: &Path,
    vision: VisionSettings,
) -> ClassificationResult {
    log::info!("Classifying video: {}", video_path.display());

    // Check ffmpeg availability — graceful degradation
    if !crate::video::is_ffmpeg_available() {
        log::info!(
            "ffmpeg not available, skipping video classification for {}",
            video_path.display()
        );
        let filename = video_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        return ClassificationResult {
            media_type: "video".to_string(),
            description: format!("Video file: {filename} (ffmpeg not available)"),
            extracted_text: String::new(),
            canonical_mentions: String::new(),
            confidence: 0.1,
            lang_hint: String::new(),
            // A video is a scene, never a document: no OCR pass for it.
            document_hint: "none".to_string(),
            // A degraded but usable answer, not a failure.
            failure_kind: String::new(),
        };
    }

    // Step 1: Extract metadata
    let meta = crate::video::extract_metadata(video_path);
    let duration_str = meta
        .as_ref()
        .and_then(|m| m.duration_secs)
        .map(crate::video::format_duration)
        .unwrap_or_else(|| "?".to_string());
    let resolution_str = meta
        .as_ref()
        .and_then(|m| match (m.width, m.height) {
            (Some(w), Some(h)) => Some(format!("{w}x{h}")),
            _ => None,
        })
        .unwrap_or_else(|| "?".to_string());

    // Step 2: Extract keyframes for vision classification
    let video_tmp = tmp_dir.join(format!(
        "_vidclass_{}",
        video_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "vid".to_string())
    ));
    let _ = std::fs::create_dir_all(&video_tmp);
    // Only the first frame is ever sent to the model (`prepared_frame` below), so asking
    // for five bought four extra `ffmpeg` seek+decodes per video, each of them a read of
    // the file over the network share. Measured on job #35 (2026-10-28): 2,175 videos at
    // ~5 s each is ~16 % of the whole pass. Two frames instead of one is deliberate — the
    // extractor drops all-black frames, so the second is the fallback when the middle of
    // the video is a cut to black. The thumbnail pass already extracts a single frame
    // with the same reasoning (`thumbnail::generate_video_thumbnail`).
    let keyframes = crate::video::extract_keyframes(video_path, &video_tmp, 2);

    // Step 3: Extract subtitle text
    let subtitle_text = crate::video::collect_subtitle_text(video_path, &video_tmp);

    // Step 4: Classify the best keyframe with Ollama.
    //
    // The keyframe is a video-sized PNG (ffmpeg output), so it gets the same
    // downscale treatment as photos: it reuses `prepared_frame` for the anime
    // enrichment pass below instead of re-reading the file.
    let prepared_frame = keyframes.first().and_then(|frame| {
        match vision_prep::prepare_with(
            frame,
            vision.image_max_side,
            &video_tmp,
            vision.image_filter,
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("cannot prepare keyframe {}: {e}", frame.display());
                None
            }
        }
    });

    let primary = if let Some(frame) = &prepared_frame {
        const NUM_PREDICT: u32 = 400;
        let prompt = video_prompt(vision.description_language);
        let num_ctx = num_ctx_for(frame.width, frame.height, prompt.len(), NUM_PREDICT);
        let resp = ollama_generate(
            model,
            prompt,
            Some(&frame.path),
            &GenerateParams::vision(NUM_PREDICT, Some(num_ctx)),
        );
        if resp.ok {
            parse_json_response(&resp.raw)
        } else {
            None
        }
    } else {
        None
    };

    // Build classification result
    let video_category = primary
        .as_ref()
        .and_then(|p| p.get("video_category").and_then(|v| v.as_str()))
        .unwrap_or("other")
        .to_string();

    let vision_description = primary
        .as_ref()
        .and_then(|p| p.get("description").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let title_guess = primary
        .as_ref()
        .and_then(|p| p.get("title_guess"))
        .and_then(clean_nullable_str);

    let confidence = primary
        .as_ref()
        .and_then(|p| p.get("confidence").and_then(|v| v.as_f64()))
        .unwrap_or(0.0) as f32;

    let is_anime = primary
        .as_ref()
        .map(should_run_anime_enrichment)
        .unwrap_or(false);

    // Build description with metadata prefix
    let mut description = format!("[{duration_str} {resolution_str}]");
    if let Some(ref title) = title_guess {
        description = format!("{description} {title}:");
    }
    if !vision_description.is_empty() {
        description = format!("{description} {vision_description}");
    }
    description = format!("{description} [{video_category}]");

    // Collect canonical mentions from primary
    let series_cands = primary
        .as_ref()
        .map(|p| normalize_list(p.get("series_candidates").unwrap_or(&Value::Null)))
        .unwrap_or_default();
    let char_cands = primary
        .as_ref()
        .map(|p| normalize_list(p.get("character_candidates").unwrap_or(&Value::Null)))
        .unwrap_or_default();

    let mut all_mentions: Vec<String> = Vec::new();

    // Anime enrichment on the best keyframe
    if is_anime {
        if let Some(best_frame) = &prepared_frame {
            if let Some(anime) =
                classify_anime_details(model, best_frame, vision.description_language)
            {
                if let Some(series) = anime.get("series").and_then(clean_nullable_str) {
                    description = format!("{description} [Series: {series}]");
                }
                let mentions =
                    normalize_list(anime.get("canonical_mentions").unwrap_or(&Value::Null));
                if !mentions.is_empty() {
                    all_mentions.extend(mentions);
                }
                if let Some(chars) = anime.get("characters").and_then(|v| v.as_array()) {
                    let char_names: Vec<String> = chars
                        .iter()
                        .filter_map(|c| {
                            c.get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    if !char_names.is_empty() && all_mentions.is_empty() {
                        all_mentions.extend(char_names);
                    }
                }
            }
        }
    }

    // Add series/character candidates
    for c in series_cands.iter().chain(char_cands.iter()) {
        let lower = c.to_lowercase();
        if !all_mentions.iter().any(|m| m.to_lowercase() == lower) {
            all_mentions.push(c.clone());
        }
    }
    let canonical_mentions = all_mentions.join(", ");

    // Cap subtitle text for extracted_text field (char-safe: subtitles are often
    // CJK, and byte slicing would panic mid-codepoint).
    let extracted_text = truncate_chars(&subtitle_text, 4000).to_string();

    let lang_hint = detect_lang_hint(&extracted_text);

    // Clean up temp directory
    let _ = std::fs::remove_dir_all(&video_tmp);

    // If no keyframes could be classified but we have subtitle text, still decent confidence
    let final_confidence = if confidence > 0.0 {
        confidence
    } else if !extracted_text.is_empty() {
        0.5
    } else {
        0.2
    };

    ClassificationResult {
        media_type: "video".to_string(),
        description,
        extracted_text,
        canonical_mentions,
        confidence: final_confidence,
        lang_hint,
        document_hint: "none".to_string(),
        failure_kind: String::new(),
    }
}

// ---------------------------------------------------------------------------
// PDF classification entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn classify_pdf(
    pdf_path: &Path,
    model: &str,
    tmp_dir: &Path,
    surya_venv: &Path,
    surya_script: &Path,
    pdfium_lib: &Path,
    password: Option<&str>,
    vision: VisionSettings,
) -> ClassificationResult {
    log::info!("Classifying PDF: {}", pdf_path.display());

    // Early exit for password-protected PDFs when no password is provided
    if crate::pdf::is_password_protected(pdf_path, pdfium_lib) && password.is_none() {
        log::info!("Skipping password-protected PDF: {}", pdf_path.display());
        return ClassificationResult {
            media_type: "document".to_string(),
            description: "Password-protected PDF (skipped)".to_string(),
            extracted_text: String::new(),
            canonical_mentions: String::new(),
            confidence: 0.0,
            lang_hint: String::new(),
            // A PDF is a document by construction, even when it cannot be opened.
            document_hint: "other".to_string(),
            failure_kind: String::new(),
        };
    }

    // Step 1: Extract text from PDF
    let (full_text, page_count) = match crate::pdf::extract_text(pdf_path, pdfium_lib, password) {
        Ok(result) => result,
        Err(e) => {
            log::warn!(
                "Failed to extract text from PDF {}: {e}",
                pdf_path.display()
            );
            (String::new(), 0)
        }
    };

    // Step 2: Render first content page for vision classification
    let content_pages = crate::pdf::find_content_pages(pdf_path, 1, pdfium_lib, password)
        .unwrap_or_else(|_| vec![0]);
    let page_idx = content_pages.first().copied().unwrap_or(0);

    let _ = std::fs::create_dir_all(tmp_dir);
    let tmp_img_path = tmp_dir.join("_pdf_page.png");

    // The rendered page is a PNG; `vision_prep` returns it untouched when it
    // already fits, so this only costs a re-encode for large pages.
    let prepared_page = match crate::pdf::render_page(pdf_path, page_idx, 200, pdfium_lib, password)
    {
        Ok(img) => {
            if let Err(e) = img.save(&tmp_img_path) {
                log::warn!("Failed to save PDF page for classification: {e}");
                None
            } else {
                match vision_prep::prepare_with(
                    &tmp_img_path,
                    vision.image_max_side,
                    tmp_dir,
                    vision.image_filter,
                ) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log::warn!("Failed to prepare PDF page for classification: {e}");
                        None
                    }
                }
            }
        }
        Err(e) => {
            log::warn!("Failed to render PDF page for classification: {e}");
            None
        }
    };

    let primary = prepared_page.as_ref().map(|prepared| {
        classify_primary(
            model,
            prepared,
            vision.prompt_profile,
            vision.description_language,
        )
        .0
    });

    // Use vision model results as the base, fall back to generic PDF description
    let media_type = primary
        .as_ref()
        .and_then(|p| p.get("media_type").and_then(|v| v.as_str()))
        .unwrap_or("document")
        .to_string();
    let vision_description = primary
        .as_ref()
        .and_then(|p| p.get("description").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let vision_confidence = primary
        .as_ref()
        .and_then(|p| p.get("confidence").and_then(|v| v.as_f64()))
        .unwrap_or(0.0) as f32;

    let mut description = if vision_description.is_empty() {
        format!("PDF document ({page_count} pages)")
    } else {
        format!("PDF ({page_count}p): {vision_description}")
    };

    // Collect canonical mentions from vision model (series/character candidates)
    let mut canonical_mentions = String::new();
    if let Some(ref p) = primary {
        // Anime enrichment on the rendered page (same logic as classify_image)
        if should_run_anime_enrichment(p) {
            if let Some(anime) = prepared_page.as_ref().and_then(|prepared| {
                classify_anime_details(model, prepared, vision.description_language)
            }) {
                if let Some(series) = anime.get("series").and_then(clean_nullable_str) {
                    description = format!("{description} [Series: {series}]");
                }
                let mentions =
                    normalize_list(anime.get("canonical_mentions").unwrap_or(&Value::Null));
                if !mentions.is_empty() {
                    canonical_mentions = mentions.join(", ");
                }
                if let Some(chars) = anime.get("characters").and_then(|v| v.as_array()) {
                    let char_names: Vec<String> = chars
                        .iter()
                        .filter_map(|c| {
                            c.get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    if !char_names.is_empty() && canonical_mentions.is_empty() {
                        canonical_mentions = char_names.join(", ");
                    }
                }
            }
        }

        let series_cands = normalize_list(p.get("series_candidates").unwrap_or(&Value::Null));
        let char_cands = normalize_list(p.get("character_candidates").unwrap_or(&Value::Null));
        let mut all_mentions: Vec<String> = Vec::new();
        if !canonical_mentions.is_empty() {
            all_mentions.extend(canonical_mentions.split(", ").map(|s| s.to_string()));
        }
        for c in series_cands.iter().chain(char_cands.iter()) {
            let lower = c.to_lowercase();
            if !all_mentions.iter().any(|m| m.to_lowercase() == lower) {
                all_mentions.push(c.clone());
            }
        }
        canonical_mentions = all_mentions.join(", ");
    }

    // Step 3: Text enrichment (document fields, OCR)
    let mut extracted_text;

    if !crate::pdf::is_scanned_pdf(&full_text, page_count) {
        // Text-rich PDF: use extracted text for LLM document field extraction
        extracted_text = full_text.clone();
        // Char-safe: PDF text can be CJK and byte slicing panics mid-codepoint.
        let context = truncate_chars(&full_text, 2000);
        let doc_fields = extract_document_fields(model, context);
        // A PDF is a document by construction, so only `none` has to be honoured here.
        if let Some(kind) = document_kind_of(&doc_fields) {
            description = format!("{description} [Doc: {kind}]");
        }
        if let Some(issuer) = doc_fields.get("issuer").and_then(clean_nullable_str) {
            description = format!("{description} [Issuer: {issuer}]");
        }
    } else {
        // Scanned PDF: run OCR on the already-rendered page
        extracted_text = String::new();
        if let Some(prepared) = prepared_page.as_ref() {
            let ocr = run_ocr(model, prepared, vision.ocr_engine, surya_venv, surya_script);
            if ocr.ok && !ocr.text.is_empty() {
                extracted_text = ocr.text.clone();

                let doc_fields = extract_document_fields(model, &ocr.text);
                if let Some(kind) = document_kind_of(&doc_fields) {
                    description = format!("{description} [Doc: {kind}]");
                }
                if let Some(issuer) = doc_fields.get("issuer").and_then(clean_nullable_str) {
                    description = format!("{description} [Issuer: {issuer}]");
                }
            }
        }
    }

    // Clean up temp image
    let _ = std::fs::remove_file(&tmp_img_path);

    let lang_hint = detect_lang_hint(&extracted_text);
    let confidence = if vision_confidence > 0.0 {
        vision_confidence
    } else if !extracted_text.is_empty() {
        0.7
    } else {
        0.3
    };

    ClassificationResult {
        media_type,
        description,
        extracted_text,
        canonical_mentions,
        confidence,
        lang_hint,
        // A PDF is a document by construction; the bucket is the point of this row.
        document_hint: "other".to_string(),
        failure_kind: String::new(),
    }
}

fn detect_lang_hint(text: &str) -> String {
    if text.is_empty() {
        return "unknown".to_string();
    }
    let lower = text.to_lowercase();
    // Simple heuristic based on common words
    let pt_words = [
        "comprovante",
        "transferência",
        "valor",
        "banco",
        "realizado",
        "pagamento",
    ];
    let ja_chars = text
        .chars()
        .any(|c| ('\u{3040}'..='\u{30FF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c));

    if ja_chars {
        return "ja".to_string();
    }
    if pt_words.iter().any(|w| lower.contains(w)) {
        return "pt".to_string();
    }
    "en".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The original-file attempt used to be sent with the window computed for the
    /// prepared image. On 2026-10-25 that answered `400 request (4136 tokens)
    /// exceeds the available context size (4096 tokens)` 25 times — always on this
    /// attempt, which existed to rescue the file and could never succeed.
    #[test]
    fn a_twelve_megapixel_original_is_not_sent_with_the_prepared_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.jpg");
        let img = image::RgbImage::from_fn(4000, 3000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        img.save(&path).expect("save");

        let prepared_ctx = num_ctx_for(768, 576, 1600, 180);
        assert_eq!(
            prepared_ctx, 4096,
            "a prepared 768 px image resolves to 4096"
        );
        assert_eq!(
            original_fits_window(&path, prepared_ctx, 1600, 180),
            Some(false),
            "a 4000x3000 original needs a bigger window than the prepared image"
        );
    }

    #[test]
    fn a_small_original_still_gets_its_attempt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("small.jpg");
        let img = image::RgbImage::from_fn(640, 480, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 120])
        });
        img.save(&path).expect("save");

        assert_eq!(original_fits_window(&path, 4096, 1600, 180), Some(true));
    }

    #[test]
    fn an_unreadable_header_keeps_the_attempt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.jpg");
        std::fs::write(&path, b"not an image").expect("write");
        assert_eq!(
            original_fits_window(&path, 4096, 1600, 180),
            None,
            "an unknown header must not silently drop the last attempt"
        );
    }

    /// A 768 px library resolves every still image to one window — which is why the
    /// journal shows a single `KvSize` — and a bigger `image_max_side` splits the
    /// library into two groups (square-ish images need the next window). That split
    /// is what the backlog pass turns into two blocks instead of per-file reloads.
    #[test]
    fn primary_ctx_splits_the_library_only_when_the_prep_size_grows() {
        for (w, h) in [(4000, 3000), (768, 1024), (640, 480), (12, 12)] {
            assert_eq!(
                primary_ctx_for(w, h, 768, PromptProfile::Concise, DescriptionLanguage::En),
                4096,
                "{w}x{h} at 768 px must stay in the single 4096 window"
            );
        }

        // At 1280 px the prepared sides are 1280x960 (4:3) and 1280x1280.
        assert_eq!(
            primary_ctx_for(
                4000,
                3000,
                1280,
                PromptProfile::Concise,
                DescriptionLanguage::En
            ),
            4096,
            "4:3 at 1280 px still fits 4096"
        );
        assert_eq!(
            primary_ctx_for(
                3000,
                3000,
                1280,
                PromptProfile::Concise,
                DescriptionLanguage::En
            ),
            8192,
            "a 1280x1280 square needs the next window"
        );
    }

    /// The taxonomy is what a retry is aimed at, so it has to be recognised from the
    /// answer Ollama actually gives — that is all the reason we get.
    /// The score has to separate evidence from guessing, and a row with no answer has to
    /// stay at 0.0 so `is_empty_result` keeps it pending.
    #[test]
    fn derived_confidence_separates_evidence_from_guessing() {
        let strict = derived_confidence(ParseKind::Strict, "photo", "a dog on the beach", false);
        let salvaged =
            derived_confidence(ParseKind::Salvaged, "photo", "a dog on the beach", false);
        let none = derived_confidence(ParseKind::Default, "other", "", false);

        assert!(strict > salvaged, "{strict} > {salvaged}");
        assert!(salvaged > none, "{salvaged} > {none}");
        assert_eq!(none, 0.0, "a row with no answer must stay pending");

        // An enum we do not accept and a description of the wrong length both cost.
        assert!(derived_confidence(ParseKind::Strict, "something", "a dog on the beach", false) < strict);
        assert!(derived_confidence(ParseKind::Strict, "photo", "dog", false) < strict);

        // A second signal helps, and nothing ever reaches certainty.
        let corroborated =
            derived_confidence(ParseKind::Strict, "document", "a receipt from the market", true);
        assert!(corroborated > strict, "{corroborated} > {strict}");
        assert!(corroborated <= 0.95);
    }

    #[test]
    fn failure_kinds_come_from_the_answer_ollama_gave() {
        assert_eq!(
            failure_kind_from("status 500: prediction aborted, token repeat limit reached"),
            "repeat_limit"
        );
        assert_eq!(
            failure_kind_from(
                "status 400: request (4136 tokens) exceeds the available context size (4096 tokens)"
            ),
            "ctx_overflow"
        );
        assert_eq!(
            failure_kind_from(
                r#"{"model":"","created_at":"0001-01-01T00:00:00Z","response":"","done":false}"#
            ),
            "empty_frame"
        );
        assert_eq!(
            failure_kind_from("http_error: io: Connection refused"),
            "io"
        );
        assert_eq!(failure_kind_from("   "), "no_response");
        assert_eq!(failure_kind_from("{\"media_type\":"), "parse");
        // The client's own message for the zero-value frame, which used to fall
        // through to "parse" (also a permanent kind) because it carries no body.
        assert_eq!(
            failure_kind_from("empty_frame: Ollama answered with its zero-value response"),
            "empty_frame"
        );
    }

    /// A wedged runner used to be filed as `repeat_limit`: permanent, and hidden from
    /// the backlog until a manual retry. That is what condemned 619 rows in job #35.
    #[test]
    fn a_wedged_runner_is_not_a_repeat_limit() {
        let wedged = "empty_frame: Ollama answered with its zero-value response";
        let looped = "http_error: status 500: prediction aborted, token repeat limit reached";
        assert_eq!(
            primary_failure_kind(wedged, &format!("{wedged}\n{looped}\n{looped}\n{looped}")),
            "empty_frame",
            "the frame came first: the 500s are the wedged runner, not the picture"
        );
        assert_eq!(
            primary_failure_kind(looped, &format!("{looped}\n{looped}\n{looped}")),
            "repeat_limit",
            "a picture that loops on a healthy runner stays a per-picture failure"
        );
    }

    #[test]
    fn salvage_primary_with_all_fields() {
        let raw = r#"blah "media_type": "anime", "contains_text": true, "is_anime_related": true, "document_photo": "none", "description": "A girl", "confidence": 0.85, "series_candidates": ["Ranma"], "character_candidates": ["Akane"] blah"#;
        let v = salvage_primary_from_raw(raw).unwrap();
        assert_eq!(v["media_type"], "anime");
        assert_eq!(v["contains_text"], true);
        assert_eq!(v["is_anime_related"], true);
        assert_eq!(v["document_photo"], "none");
        assert_eq!(v["description"], "A girl");
        assert!((v["confidence"].as_f64().unwrap() - 0.85).abs() < 0.01);
    }

    #[test]
    fn salvage_primary_missing_media_type() {
        let raw = r#""description": "something""#;
        assert!(salvage_primary_from_raw(raw).is_none());
    }

    #[test]
    fn salvage_primary_partial_json() {
        let raw = r#"{"media_type":"photo","confidence":0.3"#;
        let v = salvage_primary_from_raw(raw).unwrap();
        assert_eq!(v["media_type"], "photo");
    }

    /// A failed request must never reach the database: it would blank a description
    /// that is already there (measured: 107 of 191 rows lost theirs in one run).
    #[test]
    fn empty_results_are_detected_so_they_never_overwrite_a_row() {
        let failed = crate::models::ClassificationResult {
            media_type: "other".to_string(),
            description: String::new(),
            extracted_text: String::new(),
            canonical_mentions: String::new(),
            confidence: 0.0,
            lang_hint: String::new(),
            document_hint: String::new(),
            failure_kind: "repeat_limit".to_string(),
        };
        assert!(is_empty_result(&failed));

        let described = crate::models::ClassificationResult {
            description: "a cat on a sofa".to_string(),
            ..failed
        };
        assert!(
            !is_empty_result(&described),
            "a description with confidence 0 is still a result"
        );
    }

    #[test]
    fn normalize_list_filters_nulls() {
        let v = serde_json::json!(["Ranma", "null", "", "None", "Akane", "unknown"]);
        let result = normalize_list(&v);
        assert_eq!(result, vec!["Ranma", "Akane"]);
    }

    #[test]
    fn normalize_list_non_array() {
        let v = serde_json::json!("not an array");
        assert!(normalize_list(&v).is_empty());
    }

    #[test]
    fn clean_nullable_str_variants() {
        assert!(clean_nullable_str(&serde_json::json!("null")).is_none());
        assert!(clean_nullable_str(&serde_json::json!("none")).is_none());
        assert!(clean_nullable_str(&serde_json::json!("N/A")).is_none());
        assert!(clean_nullable_str(&serde_json::json!("")).is_none());
        assert_eq!(
            clean_nullable_str(&serde_json::json!("Ranma")),
            Some("Ranma".to_string())
        );
    }

    #[test]
    fn should_run_anime_for_anime_type() {
        let p = serde_json::json!({"media_type": "anime", "is_anime_related": false});
        assert!(should_run_anime_enrichment(&p));
    }

    #[test]
    fn should_run_anime_for_flag() {
        let p = serde_json::json!({"media_type": "photo", "is_anime_related": true});
        assert!(should_run_anime_enrichment(&p));
    }

    #[test]
    fn should_not_run_anime_for_photo() {
        let p = serde_json::json!({"media_type": "photo", "is_anime_related": false});
        assert!(!should_run_anime_enrichment(&p));
    }

    #[test]
    fn should_run_document_for_doc_type() {
        let p = serde_json::json!({"media_type": "document", "contains_text": false});
        assert!(should_run_document_enrichment(&p, OcrEngine::Llm));
    }

    /// The recall the old boolean could not give: a receipt that was *photographed* is
    /// still a `photo`, and the model now says so in `document_photo`.
    #[test]
    fn should_run_document_for_a_photographed_receipt() {
        let p = serde_json::json!({
            "media_type": "photo",
            "contains_text": true,
            "document_photo": "receipt",
        });
        assert!(should_run_document_enrichment(&p, OcrEngine::Llm));
    }

    /// The beach-volleyball case: a scene with logos answers `none`, so the ~13 s OCR
    /// pass is not paid for a picture of a court (2026-10-26).
    #[test]
    fn should_not_run_document_for_a_scene_with_logos() {
        for media_type in ["photo", "other"] {
            let p = serde_json::json!({
                "media_type": media_type,
                "contains_text": true,
                "document_photo": "none",
            });
            assert!(
                !should_run_document_enrichment(&p, OcrEngine::Llm),
                "{media_type} with document_photo=none must not pay for OCR"
            );
        }
    }

    /// Anime, manga, artwork and video never trigger OCR, whatever the model answers:
    /// burned-in subtitles used to cost a full Surya run (~8 s/image).
    #[test]
    fn document_photo_cannot_override_a_scene_media_type() {
        for media_type in ["anime", "manga", "artwork", "video"] {
            let p = serde_json::json!({
                "media_type": media_type,
                "contains_text": true,
                "document_photo": "receipt",
            });
            assert!(
                !should_run_document_enrichment(&p, OcrEngine::Surya),
                "{media_type} must never trigger OCR"
            );
        }
    }

    /// `document_photo` has to survive the regex salvage, including the "none" answer
    /// that [`clean_nullable_str`] would have dropped.
    #[test]
    fn salvage_keeps_document_photo() {
        let receipt =
            salvage_primary_from_raw(r#"{"media_type":"photo","document_photo":"receipt""#)
                .expect("salvage");
        assert_eq!(receipt["document_photo"], "receipt");

        let scene = salvage_primary_from_raw(r#"{"media_type":"photo","document_photo":"none""#)
            .expect("salvage");
        assert_eq!(scene["document_photo"], "none");
    }

    /// The suffix only goes in when the OCR text carries a document signal, so brand
    /// names alone can never turn a photo description into "[Doc: bank_transfer]".
    #[test]
    fn document_suffix_requires_a_document_signal() {
        let receipt = serde_json::json!({"document_kind": "receipt", "issuer": "Padaria"});
        assert!(document_fields_are_real(
            &receipt,
            "Padaria Central\nData: 2024-01-15\nTotal R$ 12,50"
        ));

        // The beach case: names and prices, no date+amount pair and no currency.
        assert!(!document_fields_are_real(
            &receipt,
            "BANCO DO BRASIL GOL ITAU BRADESCO"
        ));

        // The model answering "none" wins over any signal.
        let none = serde_json::json!({"document_kind": "none", "issuer": "GOL"});
        assert!(!document_fields_are_real(&none, "R$ 10,00 2024-01-15"));
    }

    #[test]
    fn should_not_run_document_when_ocr_is_off() {
        let p = serde_json::json!({"media_type": "document", "document_photo": "receipt"});
        assert!(!should_run_document_enrichment(&p, OcrEngine::Off));
    }

    #[test]
    fn should_not_run_document_for_plain_photo() {
        // A partial body without `document_photo` says nothing about being a document.
        let p = serde_json::json!({"media_type": "photo", "contains_text": false});
        assert!(!should_run_document_enrichment(&p, OcrEngine::Llm));

        let explicit = serde_json::json!({"media_type": "photo", "document_photo": "none"});
        assert!(!should_run_document_enrichment(&explicit, OcrEngine::Llm));
    }

    #[test]
    fn primary_prompt_matches_profile() {
        assert_eq!(
            primary_prompt(PromptProfile::Concise, DescriptionLanguage::En),
            PRIMARY_PROMPT_CONCISE
        );
        assert_eq!(
            primary_prompt(PromptProfile::Detailed, DescriptionLanguage::En),
            PRIMARY_PROMPT_DETAILED
        );
        assert_eq!(
            primary_prompt(PromptProfile::Concise, DescriptionLanguage::PtBr),
            PRIMARY_PROMPT_CONCISE_PT
        );
        assert_eq!(
            primary_prompt(PromptProfile::Detailed, DescriptionLanguage::PtBr),
            PRIMARY_PROMPT_DETAILED_PT
        );
        assert!(
            primary_num_predict(PromptProfile::Concise)
                < primary_num_predict(PromptProfile::Detailed)
        );
        // The concise schema must still carry the fields the routing relies on.
        let concise = primary_prompt(PromptProfile::Concise, DescriptionLanguage::En);
        assert!(concise.contains("media_type"));
        assert!(concise.contains("contains_text"));
        assert!(concise.contains("description"));
    }

    #[test]
    fn truncate_chars_is_codepoint_safe() {
        let text = "日本語のテキストです";
        let truncated = truncate_chars(text, 3);
        assert_eq!(truncated, "日本語");
        assert_eq!(truncate_chars(text, 100), text);
    }

    /// End-to-end smoke test of the real pipeline: prepare -> LLM -> parse.
    ///
    /// ```sh
    /// FRANK_VISION_SMOKE_DIR=~/Imagens/album FRANK_VISION_SMOKE_MODEL=qwen2.5vl:3b \
    ///   cargo test vision_pipeline_smoke -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "requires a running Ollama and a local image folder"]
    fn vision_pipeline_smoke() {
        let dir = std::env::var("FRANK_VISION_SMOKE_DIR")
            .expect("set FRANK_VISION_SMOKE_DIR to a folder with images");
        let model = std::env::var("FRANK_VISION_SMOKE_MODEL")
            .unwrap_or_else(|_| "qwen2.5vl:3b".to_string());

        let is_image = |p: &std::path::Path| {
            p.extension()
                .map(|e| {
                    let ext = e.to_string_lossy().to_ascii_lowercase();
                    matches!(ext.as_str(), "jpg" | "jpeg" | "png")
                })
                .unwrap_or(false)
        };
        let image = std::fs::read_dir(&dir)
            .expect("read FRANK_VISION_SMOKE_DIR")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| is_image(path))
            .expect("no image found in FRANK_VISION_SMOKE_DIR");

        let tmp = tempfile::tempdir().expect("tempdir");
        let vision = crate::models::VisionSettings {
            image_max_side: 1024,
            // The smoke test wants every image described, floor off.
            min_pixels: 0,
            library_kind: crate::config::LibraryKind::Auto,
            prompt_profile: PromptProfile::Concise,
            description_language: DescriptionLanguage::En,
            image_filter: crate::config::ImageFilter::Auto,
            // Surya needs a provisioned venv; keep the smoke test self-contained.
            ocr_engine: OcrEngine::Off,
        };

        let started = std::time::Instant::now();
        let result = classify_image(&image, &model, tmp.path(), tmp.path(), tmp.path(), vision);
        let elapsed = started.elapsed().as_secs_f64();

        println!(
            "{} -> media_type={:?} confidence={:.2} in {:.2}s\n  {}",
            image.display(),
            result.media_type,
            result.confidence,
            elapsed,
            result.description
        );
        assert!(!result.media_type.is_empty());
        assert!(
            !result.description.is_empty(),
            "concise profile must still produce a description"
        );
    }

    #[test]
    fn extract_receipt_regex_dates_amounts() {
        let text = "Date: 2024-01-15\nAmount: R$ 1.234,56\nRef: Protocolo: ABC123DEF";
        let result = extract_receipt_regex(text);
        let dates = result["dates"].as_array().unwrap();
        assert!(dates.iter().any(|d| d == "2024-01-15"));
        let amounts = result["amount_candidates"].as_array().unwrap();
        assert!(!amounts.is_empty());
        assert_eq!(result["currency_guess"], "BRL");
    }

    #[test]
    fn extract_receipt_regex_no_currency() {
        let text = "some random text without money";
        let result = extract_receipt_regex(text);
        assert!(result["currency_guess"].is_null());
    }

    #[test]
    fn detect_lang_hint_portuguese() {
        assert_eq!(detect_lang_hint("comprovante de pagamento"), "pt");
    }

    #[test]
    fn detect_lang_hint_japanese() {
        assert_eq!(detect_lang_hint("日本語のテキスト"), "ja");
    }

    #[test]
    fn detect_lang_hint_empty() {
        assert_eq!(detect_lang_hint(""), "unknown");
    }
}
