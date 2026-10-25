use std::path::{Path, PathBuf};

use crate::platform::process::silent_command;

use regex::Regex;
use serde_json::Value;

use crate::config::{OcrEngine, PromptProfile};
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
    r#""is_document_like":false,"#,
    r#""description":"short factual description","#,
    r#""series_candidates":["name"],"#,
    r#""character_candidates":["name"],"#,
    r#""confidence":0.0}"#,
    " Rules: ",
    "1) Use null-like empty arrays when unknown. ",
    "2) If image has visible text (UI, receipt, scan, subtitles), contains_text=true. ",
    "3) is_document_like=true for receipts/invoices/forms/scanned docs/screenshots of documents. ",
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
    r#""description":"max 12 words","#,
    r#""confidence":0.0}"#,
    " Rules: ",
    "1) contains_text=true only when text is clearly visible (UI, receipt, scan, subtitles). ",
    "2) description must be factual and under 12 words. ",
    "3) Favor precision over guesswork.",
);

// The old `PRIMARY_PROMPT_FALLBACK` lived here: a second wording of the same schema
// used as the retry. It turned out to be the *cause* of the failure it was meant to
// recover from — on low-contrast photos (construction sites, concrete) that wording
// drives qwen2.5-VL into a repetition loop and Ollama 0.40 aborts the request with
// `prediction aborted, token repeat limit reached` (500). The same images answer fine
// with the normal prompt and no grammar, so the retry now keeps the prompt and only
// changes the sampling mode.

fn primary_prompt(profile: PromptProfile) -> &'static str {
    match profile {
        PromptProfile::Concise => PRIMARY_PROMPT_CONCISE,
        PromptProfile::Detailed => PRIMARY_PROMPT_DETAILED,
    }
}

fn primary_num_predict(profile: PromptProfile) -> u32 {
    match profile {
        PromptProfile::Concise => 180,
        PromptProfile::Detailed => 500,
    }
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

const OCR_PROMPT: &str =
    "Extract ALL visible text exactly as seen. Return ONLY raw text. Preserve line breaks.";

const RECEIPT_PROMPT: &str = concat!(
    "Given OCR text from a potential receipt/bank document, extract structured fields. ",
    "Return ONLY valid JSON schema: ",
    r#"{"document_kind":"receipt|invoice|bank_transfer|statement|other","#,
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
    let is_doc = extract_bool(
        raw,
        "is_document_like",
        matches!(media_type.as_str(), "document" | "screenshot"),
    );

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
        "is_document_like": is_doc,
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
/// benefit. Surya spawns a fresh Python interpreter (torch + models) per file,
/// so it is only worth it for actual documents.
pub fn should_run_document_enrichment(primary: &Value, ocr_engine: OcrEngine) -> bool {
    if ocr_engine == OcrEngine::Off {
        return false;
    }
    let media_type = primary
        .get("media_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if matches!(media_type, "anime" | "manga" | "artwork") {
        return false;
    }
    let is_doc = primary
        .get("is_document_like")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    matches!(media_type, "document" | "screenshot") || is_doc
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

fn truncate_chars(text: &str, max_chars: usize) -> &str {
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

fn classify_primary(model: &str, prepared: &PreparedImage, profile: PromptProfile) -> Value {
    let prompt = primary_prompt(profile);
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
                return v;
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
    // same 107 answer correctly, so the retry only changes the sampling mode.
    let params2 = GenerateParams {
        json_mode: false,
        ..GenerateParams::vision(num_predict.min(260), Some(num_ctx))
    };
    let resp2 = ollama_generate(model, prompt, Some(&prepared.path), &params2);
    if resp2.ok {
        if let Some(v) = parse_json_response(&resp2.raw) {
            if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                return v;
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

    // Attempt 3: same prompt, no grammar, and a little entropy.
    //
    // The repetition loop that kills attempts 1 and 2 is not deterministic — the same
    // image answered fine minutes later — so one more sample with temperature is the
    // cheap way out. `num_predict` grows a little because a free-form answer is longer
    // than the JSON.
    let params3 = GenerateParams {
        json_mode: false,
        temperature: 0.3,
        ..GenerateParams::vision(num_predict.max(260), Some(num_ctx))
    };
    let resp3 = ollama_generate(model, prompt, Some(&prepared.path), &params3);
    if resp3.ok {
        if let Some(v) = parse_json_response(&resp3.raw) {
            if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                log::info!(
                    "primary recovered on the third attempt for {}",
                    prepared.path.display()
                );
                return v;
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
    // when the prepared image is a temp copy (a passthrough already *is* the original).
    let mut resp4_raw = String::new();
    if prepared.source != prepared.path {
        let params4 = GenerateParams {
            json_mode: false,
            temperature: 0.3,
            ..GenerateParams::vision(num_predict.max(260), Some(num_ctx))
        };
        let resp4 = ollama_generate(model, prompt, Some(&prepared.source), &params4);
        if resp4.ok {
            if let Some(v) = parse_json_response(&resp4.raw) {
                if v.get("media_type").and_then(|v| v.as_str()).is_some() {
                    log::info!(
                        "primary recovered from the original file for {}",
                        prepared.source.display()
                    );
                    return v;
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

    let combined = format!("{}\n{}\n{}\n{}", resp1.raw, resp2.raw, resp3.raw, resp4_raw);
    if let Some(v) = salvage_primary_from_raw(&combined) {
        return v;
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
    match std::fs::copy(&prepared.path, &keep) {
        Ok(bytes) => log::error!(
            "primary gave up on {} ({}x{}, {} B, kept at {})",
            prepared.source.display(),
            prepared.width,
            prepared.height,
            bytes,
            keep.display()
        ),
        Err(err) => log::error!(
            "primary gave up on {} ({}x{}, and the prepared image could not be kept: {err})",
            prepared.source.display(),
            prepared.width,
            prepared.height
        ),
    }

    // Safe default
    serde_json::json!({
        "media_type": "other",
        "contains_text": false,
        "is_anime_related": false,
        "is_document_like": false,
        "description": "",
        "series_candidates": [],
        "character_candidates": [],
        "confidence": 0.0,
    })
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
pub fn probe_primary(model: &str, prepared: &PreparedImage, profile: PromptProfile) -> ProbeResult {
    let prompt = primary_prompt(profile);
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

fn classify_anime_details(model: &str, prepared: &PreparedImage) -> Option<Value> {
    const NUM_PREDICT: u32 = 500;
    let num_ctx = num_ctx_for(
        prepared.width,
        prepared.height,
        ANIME_PROMPT.len(),
        NUM_PREDICT,
    );

    let params = GenerateParams::vision(NUM_PREDICT, Some(num_ctx));
    let resp = ollama_generate(model, ANIME_PROMPT, Some(&prepared.path), &params);
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
    let resp2 = ollama_generate(model, ANIME_PROMPT, Some(&prepared.path), &params2);
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

    let primary = classify_primary(model, prepared, vision.prompt_profile);

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
    let confidence = primary
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;

    let mut full_description = description.clone();
    let mut extracted_text = String::new();
    let mut canonical_mentions = String::new();

    // Anime enrichment
    if should_run_anime_enrichment(&primary) {
        if let Some(anime) = classify_anime_details(model, prepared) {
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

            let doc_fields = extract_document_fields(model, &ocr.text);
            if let Some(kind) = doc_fields.get("document_kind").and_then(clean_nullable_str) {
                full_description = format!("{full_description} [Doc: {kind}]");
            }
            if let Some(issuer) = doc_fields.get("issuer").and_then(clean_nullable_str) {
                full_description = format!("{full_description} [Issuer: {issuer}]");
            }
        }
    }

    let lang_hint = detect_lang_hint(&extracted_text);

    ClassificationResult {
        media_type,
        description: full_description,
        extracted_text,
        canonical_mentions,
        confidence,
        lang_hint,
    }
}

// ---------------------------------------------------------------------------
// Video classification entry point
// ---------------------------------------------------------------------------

const VIDEO_PROMPT: &str = concat!(
    "These are keyframes from a video. Analyze them and respond ONLY with valid JSON. Schema: ",
    r#"{"media_type":"video","#,
    r#""video_category":"movie|tv_episode|personal|music_video|tutorial|other","#,
    r#""is_anime_related":false,"#,
    r#""description":"short factual description","#,
    r#""title_guess":"string or null","#,
    r#""series_candidates":["name"],"#,
    r#""character_candidates":["name"],"#,
    r#""confidence":0.0}"#,
    " Rules: ",
    "1) Use null-like empty arrays when unknown. ",
    "2) video_category: movie=feature film, tv_episode=TV show episode, personal=home video/vlog, ",
    "music_video=music clip, tutorial=educational/how-to, other=everything else. ",
    "3) title_guess: best guess at the title or null if unknown. ",
    "4) series_candidates and character_candidates max 5 items each. ",
    "5) Keep description under 24 words. ",
    "6) Favor precision over guesswork.",
);

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
    let keyframes = crate::video::extract_keyframes(video_path, &video_tmp, 5);

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
        let num_ctx = num_ctx_for(frame.width, frame.height, VIDEO_PROMPT.len(), NUM_PREDICT);
        let resp = ollama_generate(
            model,
            VIDEO_PROMPT,
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
            if let Some(anime) = classify_anime_details(model, best_frame) {
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

    let primary = prepared_page
        .as_ref()
        .map(|prepared| classify_primary(model, prepared, vision.prompt_profile));

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
            if let Some(anime) = prepared_page
                .as_ref()
                .and_then(|prepared| classify_anime_details(model, prepared))
            {
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
        if let Some(kind) = doc_fields.get("document_kind").and_then(clean_nullable_str) {
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
                if let Some(kind) = doc_fields.get("document_kind").and_then(clean_nullable_str) {
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

    #[test]
    fn salvage_primary_with_all_fields() {
        let raw = r#"blah "media_type": "anime", "contains_text": true, "is_anime_related": true, "is_document_like": false, "description": "A girl", "confidence": 0.85, "series_candidates": ["Ranma"], "character_candidates": ["Akane"] blah"#;
        let v = salvage_primary_from_raw(raw).unwrap();
        assert_eq!(v["media_type"], "anime");
        assert_eq!(v["contains_text"], true);
        assert_eq!(v["is_anime_related"], true);
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
        let p = serde_json::json!({"media_type": "document", "contains_text": false, "is_document_like": false});
        assert!(should_run_document_enrichment(&p, OcrEngine::Llm));
    }

    #[test]
    fn should_run_document_for_document_like() {
        let p = serde_json::json!({"media_type": "photo", "contains_text": false, "is_document_like": true});
        assert!(should_run_document_enrichment(&p, OcrEngine::Llm));
    }

    /// Burned-in subtitles used to trigger a full Surya OCR run (~8 s/image).
    #[test]
    fn should_not_run_document_for_anime_with_text() {
        for media_type in ["anime", "manga", "artwork"] {
            let p = serde_json::json!({"media_type": media_type, "contains_text": true, "is_document_like": false});
            assert!(
                !should_run_document_enrichment(&p, OcrEngine::Surya),
                "{media_type} with text must not trigger OCR"
            );
        }
    }

    #[test]
    fn should_not_run_document_when_ocr_is_off() {
        let p = serde_json::json!({"media_type": "document", "contains_text": true, "is_document_like": true});
        assert!(!should_run_document_enrichment(&p, OcrEngine::Off));
    }

    #[test]
    fn should_not_run_document_for_plain_photo() {
        let p = serde_json::json!({"media_type": "photo", "contains_text": false, "is_document_like": false});
        assert!(!should_run_document_enrichment(&p, OcrEngine::Llm));
    }

    #[test]
    fn primary_prompt_matches_profile() {
        assert_eq!(
            primary_prompt(PromptProfile::Concise),
            PRIMARY_PROMPT_CONCISE
        );
        assert_eq!(
            primary_prompt(PromptProfile::Detailed),
            PRIMARY_PROMPT_DETAILED
        );
        assert!(
            primary_num_predict(PromptProfile::Concise)
                < primary_num_predict(PromptProfile::Detailed)
        );
        // The concise schema must still carry the fields the routing relies on.
        let concise = primary_prompt(PromptProfile::Concise);
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
            prompt_profile: PromptProfile::Concise,
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
