use std::env;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub const DATA_DIR_ENV: &str = "FRANK_SHERLOCK_DATA_DIR";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub base_dir: PathBuf,
    pub db_dir: PathBuf,
    pub db_file: PathBuf,
    pub cache_dir: PathBuf,
    pub classification_cache_dir: PathBuf,
    pub thumbnails_dir: PathBuf,
    pub scans_dir: PathBuf,
    pub surya_venv_dir: PathBuf,
    /// Dedicated venv for HEIC/NEF conversion (`pillow-heif` + `rawpy`).
    ///
    /// Deliberately separate from `surya_venv_dir`: wanting to convert raw photos must
    /// not require installing an OCR stack, and a broken `pip` in one venv must not take
    /// the other down with it.
    pub raw_venv_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub models_dir: PathBuf,
    pub face_crops_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPathsView {
    pub base_dir: String,
    pub db_file: String,
    pub cache_dir: String,
}

impl AppPaths {
    pub fn view(&self) -> AppPathsView {
        AppPathsView {
            base_dir: self.base_dir.display().to_string(),
            db_file: self.db_file.display().to_string(),
            cache_dir: self.cache_dir.display().to_string(),
        }
    }
}

pub fn resolve_paths() -> AppResult<AppPaths> {
    let base_dir = match env::var(DATA_DIR_ENV) {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => default_base_dir()?,
    };

    let db_dir = base_dir.join("db");
    let cache_dir = base_dir.join("cache");
    let classification_cache_dir = cache_dir.join("classifications");
    let thumbnails_dir = cache_dir.join("thumbnails");
    let scans_dir = cache_dir.join("scans");
    let surya_venv_dir = base_dir.join("surya_venv");
    let raw_venv_dir = base_dir.join("raw_venv");
    let tmp_dir = cache_dir.join("tmp");
    let db_file = db_dir.join("index.sqlite");
    let models_dir = base_dir.join("models");
    let face_crops_dir = cache_dir.join("face_crops");

    Ok(AppPaths {
        base_dir,
        db_dir,
        db_file,
        cache_dir,
        classification_cache_dir,
        thumbnails_dir,
        scans_dir,
        surya_venv_dir,
        raw_venv_dir,
        tmp_dir,
        models_dir,
        face_crops_dir,
    })
}

pub fn prepare_dirs(paths: &AppPaths) -> AppResult<()> {
    std::fs::create_dir_all(&paths.base_dir)?;
    std::fs::create_dir_all(&paths.db_dir)?;
    std::fs::create_dir_all(&paths.cache_dir)?;
    std::fs::create_dir_all(&paths.classification_cache_dir)?;
    std::fs::create_dir_all(&paths.thumbnails_dir)?;
    std::fs::create_dir_all(&paths.scans_dir)?;
    std::fs::create_dir_all(&paths.surya_venv_dir)?;
    std::fs::create_dir_all(&paths.raw_venv_dir)?;
    std::fs::create_dir_all(&paths.tmp_dir)?;
    std::fs::create_dir_all(&paths.models_dir)?;
    std::fs::create_dir_all(&paths.face_crops_dir)?;
    Ok(())
}

fn default_base_dir() -> AppResult<PathBuf> {
    if let Some(dir) = dirs::data_local_dir() {
        return Ok(dir.join("frank_sherlock"));
    }
    Err(AppError::Config(
        "could not resolve local data directory".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// UI config (~/.config/frank_sherlock/config.json)
// ---------------------------------------------------------------------------

fn user_config_path() -> AppResult<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        return Ok(config_dir.join("frank_sherlock").join("config.json"));
    }
    Err(AppError::Config(
        "could not resolve user config directory".to_string(),
    ))
}

pub fn load_user_config() -> AppResult<serde_json::Value> {
    let path = user_config_path()?;
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let data = std::fs::read_to_string(&path)?;
    serde_json::from_str(&data).map_err(|e| AppError::Config(format!("invalid config JSON: {e}")))
}

pub fn save_user_config(config: &serde_json::Value) -> AppResult<()> {
    let path = user_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| AppError::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(&path, data)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Runtime settings (~/.config/frank_sherlock/settings.toml)
// ---------------------------------------------------------------------------

const SETTINGS_TEMPLATE: &str = r#"# Frank Sherlock runtime settings.
#
# Changes are picked up on the next scan; no restart needed.
#
# Ollama vision model override.
# Leave empty to let the app choose from detected hardware.
#
# Recommended options:
# - qwen2.5vl:3b   Faster and safer for low VRAM, Windows NVIDIA <= 8GB, or CPU fallback.
# - qwen2.5vl:7b   Better quality when Ollama can load it fully on GPU.
# - qwen2.5vl:32b  Heavy; only realistic for large unified-memory systems.
#
# Experimental alternatives may work if installed in Ollama, but prompts are tuned for qwen2.5vl:
# - minicpm-v:8b
# - moondream
#
# When set, Frank Sherlock requires this exact model. If it is missing, first-run setup will
# offer to download it through Ollama before scanning.
model_override = ""

# Longest side (pixels) of the image sent to the vision model.
# 0 keeps the original file untouched.
# Lower is faster: 1024 keeps small subjects legible, 768 is ~40% faster but starts to lose
# small details. Ollama's image preprocessing (not the model itself) dominates the cost of
# large photos, so this is the single most effective speed knob.
image_max_side = 1024

# Description style for the primary classification pass.
# - concise   classifies + short description (max 12 words): ~30 output tokens
# - detailed  adds anime series/character candidates: ~78 output tokens (slower)
prompt_profile = "concise"

# Language of the free-text description (what the search box matches on).
# - pt_br  Brazilian Portuguese (default)
# - en     English
# Measured on qwen2.5vl:3b at 768 px: PT-BR costs the same as English (43 vs 46
# output tokens, 0.35 s vs 0.38 s of decode), so this is a reading choice, not a
# speed knob. JSON keys, enum values, OCR text and the `[Doc: …]` style markers
# stay English either way.
# Rows already in the index keep the description they have: use "Converter
# descrições existentes" in the settings screen to rewrite them (a text-only
# pass, no image re-encoding).
description_language = "pt_br"

# Resampling filter used to downscale the image sent to the vision model.
# - auto     area-average for photos/JPEG (fast), Lanczos3 for screenshots/documents
# - box      always area-average: the fastest option
# - lanczos  always Lanczos3: sharpest thin text, several times slower
image_filter = "auto"

# OCR engine used for screenshots/documents.
# - off    never run OCR
# - llm    reuse the vision model (fast, lower recall)
# - surya  dedicated OCR model in a Python venv (better recall, much slower)
ocr_engine = "llm"

# What to do when the app opens with a folder that is already indexed.
# - ask           ask every time (default)
# - classify_only resume pending classification without walking the tree again
# - full          run a complete scan (discovery + thumbnails + classification)
# - none          just open the existing index
startup_scan = "ask"

# Maximum Hamming distance between two dHash values (0-64 bits) for the images to
# be treated as the same picture. Near-duplicates inherit the classification of the
# first one instead of paying for another vision request.
# - 0   disables the dedupe (classify every file)
# - 6   default: identical-looking copies, including resized/re-encoded ones
# - 10+ starts folding bursts and near-identical shots into one description
dedupe_dhash_distance = 6

# Continue a full scan into the pipeline once discovery has written the rows.
# A `full` scan owns discovery and cleanup, and the pipeline is what produces the
# thumbnail and the classification in the same pass (it is also the only moment a
# fresh folder gives the producer real CPU work to hide from the GPU), which removes
# the mono-thread thumbnail phase from the scan.
# Set to false for the old shape: a sequential thumbnail pass followed by the
# sequential classification loop. "Refresh" is the thumbnail-only mode either way.
chain_backlog = true

# Read faces during the scan, on the image the pipeline has already decoded.
# The switch exists to be flipped: it is the only honest way to see what SCRFD costs
# on a real folder. Anything left pending is picked up by Detect Faces in the Faces
# view, which reads on its own and in parallel.
# - false  default: the scan only thumbnails and classifies
# - true   the scan also writes face_detections and starts grouping people
face_detection = false

# Ceiling on the threads the face engine may use. 0 = every core of the machine.
# It is a ceiling, not an allocation: while a pass is also classifying, faces get one
# thread and the rest belongs to the vision model; a pass that only has faces left runs
# GPU-idle and is allowed the whole budget. Lower it when the machine is busy with
# something else. Each worker carries its own copy of the models (~180 MB), so the
# engine starts at most 8 of them.
face_threads = 0
"#;

/// Default longest-side for images sent to the vision model.
pub const DEFAULT_IMAGE_MAX_SIDE: u32 = 1024;
/// Default Hamming distance used to fold near-duplicate photos (`0` = off).
pub const DEFAULT_DEDUPE_DHASH_DISTANCE: u32 = 6;
/// Upper bound accepted from `settings.toml`; beyond this bursts collapse.
pub const MAX_DEDUPE_DHASH_DISTANCE: u32 = 16;
/// Default resampler for the image handed to the model.
pub const DEFAULT_IMAGE_FILTER: ImageFilter = ImageFilter::Auto;
/// Default language for the free-text description. PT-BR is the project owner's
/// native language and reads better for the people who search the library.
pub const DEFAULT_DESCRIPTION_LANGUAGE: DescriptionLanguage = DescriptionLanguage::PtBr;
/// Smallest accepted longest-side. Below this the model starts losing small subjects.
pub const MIN_IMAGE_MAX_SIDE: u32 = 640;
/// Largest accepted longest-side. Above this Ollama caps image tokens anyway.
pub const MAX_IMAGE_MAX_SIDE: u32 = 2048;

/// Description style for the primary classification pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptProfile {
    /// `media_type` + `contains_text` + a description capped at 12 words.
    Concise,
    /// Adds `series_candidates` / `character_candidates` on the first pass.
    Detailed,
}

impl PromptProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            PromptProfile::Concise => "concise",
            PromptProfile::Detailed => "detailed",
        }
    }

    /// Accepts the values exposed in `settings.toml` and by the UI.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "concise" | "short" => Some(PromptProfile::Concise),
            "detailed" | "full" => Some(PromptProfile::Detailed),
            _ => None,
        }
    }
}

/// Language the free-text part of a description is written in.
///
/// Measured on `qwen2.5vl:3b` at 768 px with the concise prompt (`scripts/
/// bench-description-language.py`, 12 images from a real library): PT-BR cost
/// 43.2 output tokens / 0.35 s of decode against 46.2 / 0.38 s for English, so
/// the choice is about who reads the library, not about speed.
///
/// Only free text follows this setting. The JSON keys, the enum values
/// (`photo`, `receipt`, …), the OCR text and the machine markers
/// (`Password-protected PDF (skipped)`, `[Doc: …]`) stay English: the values are
/// matched in Rust and one marker is compared in SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptionLanguage {
    /// Brazilian Portuguese.
    PtBr,
    En,
}

impl DescriptionLanguage {
    pub fn as_str(&self) -> &'static str {
        match self {
            DescriptionLanguage::PtBr => "pt_br",
            DescriptionLanguage::En => "en",
        }
    }

    /// Accepts the values exposed in `settings.toml` and by the UI.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pt_br" | "pt-br" | "pt" | "ptbr" | "portuguese" | "portugues" | "português" => {
                Some(DescriptionLanguage::PtBr)
            }
            "en" | "en_us" | "en-us" | "english" | "ingles" | "inglês" => {
                Some(DescriptionLanguage::En)
            }
            _ => None,
        }
    }

    /// Whether a `files.description_lang` tag is already this language.
    ///
    /// `""` is the tag of a row written before the column existed: it is treated as
    /// "unknown", so a conversion pass picks those rows up instead of trusting the
    /// setting they were written under.
    pub fn owns_tag(&self, tag: &str) -> bool {
        !tag.is_empty() && DescriptionLanguage::parse(tag) == Some(*self)
    }
}

/// Resampling kernel used to downscale the image handed to the vision model.
///
/// Measured on a 174-file SMB library: `prepare` cost 606 ms/file with
/// Lanczos3, while the thumbnail (same resize + encode, box filter) cost 300 ms.
/// Lanczos is sharpest for thin text; the box filter is an area average, which
/// is what a 4× downscale of a photo actually needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFilter {
    /// Box for photos (JPEG/HEIC), Lanczos3 for screenshot/document formats.
    Auto,
    /// Always area-average (`imageops::thumbnail`).
    Box,
    /// Always Lanczos3.
    Lanczos,
}

impl ImageFilter {
    pub fn as_str(&self) -> &'static str {
        match self {
            ImageFilter::Auto => "auto",
            ImageFilter::Box => "box",
            ImageFilter::Lanczos => "lanczos",
        }
    }

    /// Accepts the values exposed in `settings.toml`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ImageFilter::Auto),
            "box" | "area" | "thumbnail" => Some(ImageFilter::Box),
            "lanczos" | "lanczos3" => Some(ImageFilter::Lanczos),
            _ => None,
        }
    }
}

/// Which engine runs OCR for screenshots and documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngine {
    Off,
    Llm,
    Surya,
}

impl OcrEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            OcrEngine::Off => "off",
            OcrEngine::Llm => "llm",
            OcrEngine::Surya => "surya",
        }
    }

    /// Accepts the values exposed in `settings.toml` and by the UI.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Some(OcrEngine::Off),
            "llm" | "vision" => Some(OcrEngine::Llm),
            "surya" => Some(OcrEngine::Surya),
            _ => None,
        }
    }
}

/// What to do when the app opens with a folder that is already indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupScan {
    /// Ask the user (default).
    Ask,
    /// Resume pending classification without re-discovering the tree.
    ClassifyOnly,
    /// Run a complete scan (discovery + thumbnails + classification).
    Full,
    /// Only open the existing index.
    OpenOnly,
}

impl StartupScan {
    pub fn as_str(&self) -> &'static str {
        match self {
            StartupScan::Ask => "ask",
            StartupScan::ClassifyOnly => "classify_only",
            StartupScan::Full => "full",
            StartupScan::OpenOnly => "none",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(StartupScan::Ask),
            "classify_only" | "classify-only" | "classify" => Some(StartupScan::ClassifyOnly),
            "full" | "scan" => Some(StartupScan::Full),
            "none" | "open" | "open_only" => Some(StartupScan::OpenOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    pub model_override: Option<String>,
    /// Longest side of the image handed to the model. `0` = original bytes.
    pub image_max_side: u32,
    pub prompt_profile: PromptProfile,
    /// Language of the free-text description (PT-BR by default).
    pub description_language: DescriptionLanguage,
    /// Resampler used when downscaling the image for the model.
    pub image_filter: ImageFilter,
    pub ocr_engine: OcrEngine,
    /// Behaviour when opening a folder that is already indexed.
    pub startup_scan: StartupScan,
    /// Hamming distance under which two photos are considered the same picture.
    /// `0` disables the near-duplicate dedupe.
    pub dedupe_dhash_distance: u32,
    /// Run the backlog pass when a `full` scan finishes.
    ///
    /// `full` is the only mode that discovers and cleans the index, and the backlog is
    /// what classifies with the producer/consumer overlap. Chaining them makes one
    /// action index and classify a folder, instead of asking for a second click.
    pub chain_backlog: bool,
    /// Run face detection inside the scan pipeline, on the image the producer has
    /// already decoded.
    ///
    /// Off by default: it is extra CPU inside the pass, the models are a 200 MB
    /// download, and the whole point of the switch is to be able to run the same
    /// folder with it on and off and compare. Anything left pending is picked up by
    /// the Detect Faces button, which reads on its own.
    pub face_detection: bool,
    /// Ceiling on the threads the face engine may use; `0` means every core.
    ///
    /// It is a ceiling, not an allocation: a pass that is also classifying gets one
    /// thread for faces and leaves the rest to Ollama, while a pass that only has
    /// faces left (GPU idle) is allowed the whole budget. Lower it when the machine is
    /// busy with something else.
    pub face_threads: u32,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            model_override: None,
            image_max_side: DEFAULT_IMAGE_MAX_SIDE,
            prompt_profile: PromptProfile::Concise,
            description_language: DEFAULT_DESCRIPTION_LANGUAGE,
            image_filter: DEFAULT_IMAGE_FILTER,
            ocr_engine: OcrEngine::Llm,
            startup_scan: StartupScan::Ask,
            dedupe_dhash_distance: DEFAULT_DEDUPE_DHASH_DISTANCE,
            chain_backlog: true,
            face_detection: false,
            face_threads: 0,
        }
    }
}

/// Everything the settings screen submits, in one struct.
///
/// Kept apart from [`RuntimeSettings`] because every field arrives as text from a form
/// and has to go through the same parsers `settings.toml` uses: one validation path for
/// a hand-edited file and for the UI, so the two cannot drift.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    /// `None` or empty restores automatic model selection from the hardware.
    pub model: Option<String>,
    pub image_max_side: u32,
    pub prompt_profile: String,
    /// `pt_br` | `en`.
    pub description_language: String,
    pub image_filter: String,
    pub ocr_engine: String,
    pub startup_scan: String,
    pub dedupe_dhash_distance: u32,
    pub chain_backlog: bool,
    pub face_detection: bool,
    pub face_threads: u32,
}

impl RuntimeSettings {
    /// Apply a whole update on top of the current settings, validating every value.
    ///
    /// Returns the offending field on the first error, without touching the rest: a
    /// half-applied update would leave `settings.toml` in a state neither the user nor
    /// the app asked for.
    pub fn apply_update(&mut self, update: &SettingsUpdate) -> Result<(), String> {
        let model_override = update
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string);
        if let Some(model) = model_override.as_deref() {
            validate_model_tag(model).map_err(|e| e.to_string())?;
        }
        let prompt_profile = PromptProfile::parse(&update.prompt_profile)
            .ok_or_else(|| format!("invalid prompt_profile '{}'", update.prompt_profile))?;
        let description_language = DescriptionLanguage::parse(&update.description_language)
            .ok_or_else(|| {
                format!(
                    "invalid description_language '{}'",
                    update.description_language
                )
            })?;
        let image_filter = ImageFilter::parse(&update.image_filter)
            .ok_or_else(|| format!("invalid image_filter '{}'", update.image_filter))?;
        let ocr_engine = OcrEngine::parse(&update.ocr_engine)
            .ok_or_else(|| format!("invalid ocr_engine '{}'", update.ocr_engine))?;
        let startup_scan = StartupScan::parse(&update.startup_scan)
            .ok_or_else(|| format!("invalid startup_scan '{}'", update.startup_scan))?;

        self.model_override = model_override;
        self.image_max_side = Self::normalize_image_max_side(update.image_max_side);
        self.prompt_profile = prompt_profile;
        self.description_language = description_language;
        self.image_filter = image_filter;
        self.ocr_engine = ocr_engine;
        self.startup_scan = startup_scan;
        self.dedupe_dhash_distance = Self::normalize_dedupe_distance(update.dedupe_dhash_distance);
        self.chain_backlog = update.chain_backlog;
        self.face_detection = update.face_detection;
        self.face_threads = Self::normalize_face_threads(update.face_threads);
        Ok(())
    }

    /// `0` keeps "all cores"; anything else is clamped to the supported ceiling.
    pub fn normalize_face_threads(value: u32) -> u32 {
        value.min(crate::face::MAX_FACE_THREADS)
    }

    /// `0` is preserved (meaning "original bytes"); anything else is clamped to the
    /// supported range so a hand-edited file cannot make classification absurdly slow
    /// or degrade quality below what the prompts expect.
    pub fn normalize_image_max_side(value: u32) -> u32 {
        if value == 0 {
            0
        } else {
            value.clamp(MIN_IMAGE_MAX_SIDE, MAX_IMAGE_MAX_SIDE)
        }
    }

    /// Clamp the dedupe threshold to a range where it still means "same picture".
    pub fn normalize_dedupe_distance(value: u32) -> u32 {
        value.min(MAX_DEDUPE_DHASH_DISTANCE)
    }
}

fn runtime_settings_path() -> AppResult<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        return Ok(config_dir.join("frank_sherlock").join("settings.toml"));
    }
    Err(AppError::Config(
        "could not resolve user config directory".to_string(),
    ))
}

pub fn ensure_runtime_settings_file() -> AppResult<PathBuf> {
    let path = runtime_settings_path()?;
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, SETTINGS_TEMPLATE)?;
    Ok(path)
}

pub fn load_runtime_settings() -> AppResult<RuntimeSettings> {
    let path = ensure_runtime_settings_file()?;
    let data = std::fs::read_to_string(&path)?;
    parse_runtime_settings(&data)
}

fn parse_runtime_settings(data: &str) -> AppResult<RuntimeSettings> {
    let mut settings = RuntimeSettings::default();

    for raw_line in data.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "model_override" => {
                let parsed = parse_settings_string(value.trim())?;
                let parsed = parsed.trim();
                if !parsed.is_empty() {
                    validate_model_tag(parsed)?;
                    settings.model_override = Some(parsed.to_string());
                }
            }
            "image_max_side" => {
                let parsed = parse_settings_string(value.trim())?;
                match parsed.trim().parse::<u32>() {
                    Ok(v) => settings.image_max_side = RuntimeSettings::normalize_image_max_side(v),
                    Err(_) => log::warn!(
                        "settings.toml: ignoring image_max_side '{parsed}' (expected: 0, or {MIN_IMAGE_MAX_SIDE}..={MAX_IMAGE_MAX_SIDE})"
                    ),
                }
            }
            "prompt_profile" => {
                let parsed = parse_settings_string(value.trim())?;
                match PromptProfile::parse(&parsed) {
                    Some(v) => settings.prompt_profile = v,
                    None => log::warn!(
                        "settings.toml: ignoring prompt_profile '{parsed}' (expected: concise | detailed)"
                    ),
                }
            }
            "description_language" => {
                let parsed = parse_settings_string(value.trim())?;
                match DescriptionLanguage::parse(&parsed) {
                    Some(v) => settings.description_language = v,
                    None => log::warn!(
                        "settings.toml: ignoring description_language '{parsed}' (expected: pt_br | en)"
                    ),
                }
            }
            "image_filter" => {
                let parsed = parse_settings_string(value.trim())?;
                match ImageFilter::parse(&parsed) {
                    Some(v) => settings.image_filter = v,
                    None => log::warn!(
                        "settings.toml: ignoring image_filter '{parsed}' (expected: auto | box | lanczos)"
                    ),
                }
            }
            "ocr_engine" => {
                let parsed = parse_settings_string(value.trim())?;
                match OcrEngine::parse(&parsed) {
                    Some(v) => settings.ocr_engine = v,
                    None => log::warn!(
                        "settings.toml: ignoring ocr_engine '{parsed}' (expected: off | llm | surya)"
                    ),
                }
            }
            "startup_scan" => {
                let parsed = parse_settings_string(value.trim())?;
                match StartupScan::parse(&parsed) {
                    Some(v) => settings.startup_scan = v,
                    None => log::warn!(
                        "settings.toml: ignoring startup_scan '{parsed}' (expected: ask | classify_only | full | none)"
                    ),
                }
            }
            "dedupe_dhash_distance" => {
                let parsed = parse_settings_string(value.trim())?;
                match parsed.trim().parse::<u32>() {
                    Ok(v) => {
                        settings.dedupe_dhash_distance =
                            RuntimeSettings::normalize_dedupe_distance(v)
                    }
                    Err(_) => log::warn!(
                        "settings.toml: ignoring dedupe_dhash_distance '{parsed}' (expected: 0..={MAX_DEDUPE_DHASH_DISTANCE})"
                    ),
                }
            }
            "chain_backlog" => {
                let parsed = parse_settings_string(value.trim())?;
                match parsed.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" | "on" => settings.chain_backlog = true,
                    "false" | "no" | "0" | "off" => settings.chain_backlog = false,
                    _ => log::warn!(
                        "settings.toml: ignoring chain_backlog '{parsed}' (expected: true | false)"
                    ),
                }
            }
            "face_detection" => {
                let parsed = parse_settings_string(value.trim())?;
                match parsed.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" | "on" => settings.face_detection = true,
                    "false" | "no" | "0" | "off" => settings.face_detection = false,
                    _ => log::warn!(
                        "settings.toml: ignoring face_detection '{parsed}' (expected: true | false)"
                    ),
                }
            }
            "face_threads" => {
                let parsed = parse_settings_string(value.trim())?;
                match parsed.trim().parse::<u32>() {
                    Ok(v) => settings.face_threads = RuntimeSettings::normalize_face_threads(v),
                    Err(_) => log::warn!(
                        "settings.toml: ignoring face_threads '{parsed}' (expected: 0..={})",
                        crate::face::MAX_FACE_THREADS
                    ),
                }
            }
            _ => {}
        }
    }

    Ok(settings)
}

/// Persist `settings` back to `settings.toml`, rewriting only the known keys so any
/// hand-written comments and unknown keys survive.
pub fn save_runtime_settings(settings: &RuntimeSettings) -> AppResult<()> {
    let path = runtime_settings_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| SETTINGS_TEMPLATE.to_string());
    let known = [
        "model_override",
        "image_max_side",
        "prompt_profile",
        "description_language",
        "image_filter",
        "ocr_engine",
        "startup_scan",
        "dedupe_dhash_distance",
        "chain_backlog",
        "face_detection",
        "face_threads",
    ];

    let model_line = match &settings.model_override {
        Some(model) => {
            validate_model_tag(model)?;
            format!("model_override = {}", serde_json::json!(model))
        }
        None => "model_override = \"\"".to_string(),
    };
    let mut replacements = std::collections::HashMap::new();
    replacements.insert("model_override", model_line);
    replacements.insert(
        "image_max_side",
        format!("image_max_side = {}", settings.image_max_side),
    );
    replacements.insert(
        "prompt_profile",
        format!(
            "prompt_profile = {}",
            serde_json::json!(settings.prompt_profile.as_str())
        ),
    );
    replacements.insert(
        "description_language",
        format!(
            "description_language = {}",
            serde_json::json!(settings.description_language.as_str())
        ),
    );
    replacements.insert(
        "image_filter",
        format!(
            "image_filter = {}",
            serde_json::json!(settings.image_filter.as_str())
        ),
    );
    replacements.insert(
        "ocr_engine",
        format!(
            "ocr_engine = {}",
            serde_json::json!(settings.ocr_engine.as_str())
        ),
    );
    replacements.insert(
        "startup_scan",
        format!(
            "startup_scan = {}",
            serde_json::json!(settings.startup_scan.as_str())
        ),
    );
    replacements.insert(
        "dedupe_dhash_distance",
        format!(
            "dedupe_dhash_distance = {}",
            RuntimeSettings::normalize_dedupe_distance(settings.dedupe_dhash_distance)
        ),
    );
    replacements.insert(
        "chain_backlog",
        format!("chain_backlog = {}", settings.chain_backlog),
    );
    replacements.insert(
        "face_detection",
        format!("face_detection = {}", settings.face_detection),
    );
    replacements.insert(
        "face_threads",
        format!(
            "face_threads = {}",
            RuntimeSettings::normalize_face_threads(settings.face_threads)
        ),
    );

    let mut out = String::new();
    let mut written: Vec<&str> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        let key = trimmed.split('=').next().unwrap_or("").trim();
        if let Some(replacement) = replacements.get(key) {
            if !written.contains(&key) {
                out.push_str(replacement);
                out.push('\n');
                written.push(key);
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    for key in known {
        if !written.contains(&key) {
            if let Some(replacement) = replacements.get(key) {
                out.push_str(replacement);
                out.push('\n');
            }
        }
    }

    std::fs::write(&path, out)?;
    Ok(())
}

fn parse_settings_string(value: &str) -> AppResult<String> {
    let without_comment = value.split('#').next().unwrap_or("").trim();
    if without_comment.starts_with('"') {
        let parsed: String = serde_json::from_str(without_comment)
            .map_err(|e| AppError::Config(format!("invalid quoted model_override value: {e}")))?;
        return Ok(parsed);
    }
    Ok(without_comment.to_string())
}

fn validate_model_tag(model: &str) -> AppResult<()> {
    let valid = !model.is_empty()
        && model.len() <= 128
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'));
    if valid {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "invalid model_override '{model}': use an Ollama model tag like qwen2.5vl:3b"
        )))
    }
}

/// Expand `~`, canonicalize, and validate that the result is a directory.
///
/// Handles:
/// - `~` and `~/...` (and `~\...` on Windows) expansion via `dirs::home_dir()`
/// - Trailing slashes, `.`, `..`, symlinks (via `canonicalize()`)
/// - Validates the resolved path is a directory
pub fn expand_and_canonicalize(raw: &str) -> AppResult<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidPath("empty path".into()));
    }

    let expanded = if trimmed == "~" {
        dirs::home_dir()
            .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?;
        home.join(rest)
    } else if cfg!(windows) {
        if let Some(rest) = trimmed.strip_prefix("~\\") {
            let home = dirs::home_dir()
                .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?;
            home.join(rest)
        } else {
            PathBuf::from(trimmed)
        }
    } else {
        PathBuf::from(trimmed)
    };

    let canonical = dunce::canonicalize(&expanded)
        .map_err(|e| AppError::InvalidPath(format!("cannot resolve path '{}': {}", raw, e)))?;

    if !canonical.is_dir() {
        return Err(AppError::InvalidPath(format!("not a directory: {}", raw)));
    }

    Ok(canonical)
}

pub fn canonical_root_path(path: &str) -> AppResult<PathBuf> {
    let root = Path::new(path);
    if !root.exists() {
        return Err(AppError::InvalidPath(format!(
            "path does not exist: {path}"
        )));
    }
    if !root.is_dir() {
        return Err(AppError::InvalidPath(format!(
            "path is not a directory: {path}"
        )));
    }
    Ok(dunce::canonicalize(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_from_env_override() {
        let _guard = ENV_LOCK.lock().expect("lock");
        env::remove_var(DATA_DIR_ENV);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("custom_data");
        env::set_var(DATA_DIR_ENV, path.as_os_str());
        let paths = resolve_paths().expect("paths");
        assert_eq!(paths.base_dir, path);
        env::remove_var(DATA_DIR_ENV);
    }

    #[test]
    fn expand_and_canonicalize_resolves_dot() {
        let cwd = dunce::canonicalize(".").expect("cwd");
        let result = expand_and_canonicalize(".").expect("dot");
        assert_eq!(result, cwd);
    }

    #[test]
    fn expand_and_canonicalize_strips_trailing_slash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let with_slash = format!("{}/", dir.path().display());
        let result = expand_and_canonicalize(&with_slash).expect("trailing slash");
        assert_eq!(result, dunce::canonicalize(dir.path()).expect("canon"));
    }

    /// GVFS mount names contain `:` and `,` and `=`, e.g.
    /// `smb-share:server=nas,share=fotos`. They must round-trip unchanged.
    #[test]
    fn expand_and_canonicalize_gvfs_style_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gvfs = dir.path().join("smb-share:server=nas,share=fotos");
        std::fs::create_dir_all(&gvfs).expect("create gvfs dir");

        let raw = gvfs.to_str().expect("utf8");
        let result = expand_and_canonicalize(raw).expect("gvfs path");
        assert_eq!(result, dunce::canonicalize(&gvfs).expect("canon"));
        assert!(result.to_string_lossy().contains("server=nas,share=fotos"));
    }

    #[test]
    fn expand_and_canonicalize_tilde() {
        if let Some(home) = dirs::home_dir() {
            let result = expand_and_canonicalize("~").expect("tilde");
            assert_eq!(result, dunce::canonicalize(&home).expect("canon"));
        }
    }

    #[test]
    fn expand_and_canonicalize_tilde_subdir() {
        // Only run if home dir exists and is a directory
        if let Some(home) = dirs::home_dir() {
            if home.is_dir() {
                // Just test that ~/. resolves to home
                let result = expand_and_canonicalize("~/.").expect("tilde dot");
                assert_eq!(result, dunce::canonicalize(&home).expect("canon"));
            }
        }
    }

    #[test]
    fn expand_and_canonicalize_nonexistent() {
        let err = expand_and_canonicalize("/nonexistent_path_that_should_not_exist_12345");
        assert!(err.is_err());
    }

    #[test]
    fn expand_and_canonicalize_empty() {
        let err = expand_and_canonicalize("");
        assert!(err.is_err());
    }

    #[test]
    fn expand_and_canonicalize_file_not_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("afile.txt");
        std::fs::write(&file, "hello").expect("write");
        let err = expand_and_canonicalize(file.to_str().unwrap());
        assert!(err.is_err());
    }

    #[test]
    fn prepare_dirs_creates_expected_structure() {
        let _guard = ENV_LOCK.lock().expect("lock");
        env::remove_var(DATA_DIR_ENV);
        let dir = tempfile::tempdir().expect("tempdir");
        env::set_var(DATA_DIR_ENV, dir.path().join("fs_data").as_os_str());
        let paths = resolve_paths().expect("paths");
        prepare_dirs(&paths).expect("prepare");

        assert!(paths.db_dir.exists());
        assert!(paths.thumbnails_dir.exists());
        assert!(paths.classification_cache_dir.exists());
        env::remove_var(DATA_DIR_ENV);
    }

    #[test]
    fn parse_runtime_settings_empty_override_uses_automatic() {
        let settings = parse_runtime_settings(r#"model_override = """#).expect("settings");
        assert_eq!(settings, RuntimeSettings::default());
    }

    #[test]
    fn parse_runtime_settings_defaults_when_file_is_empty() {
        let settings = parse_runtime_settings("").expect("settings");
        assert_eq!(settings.model_override, None);
        assert_eq!(settings.image_max_side, DEFAULT_IMAGE_MAX_SIDE);
        assert_eq!(settings.prompt_profile, PromptProfile::Concise);
        assert_eq!(
            settings.description_language,
            DescriptionLanguage::PtBr,
            "PT-BR is the default so a fresh install reads in the owner's language"
        );
        assert_eq!(settings.ocr_engine, OcrEngine::Llm);
    }

    #[test]
    fn parse_runtime_settings_description_language() {
        let en = parse_runtime_settings(r#"description_language = "en""#).expect("settings");
        assert_eq!(en.description_language, DescriptionLanguage::En);

        let pt = parse_runtime_settings(r#"description_language = "pt-br""#).expect("settings");
        assert_eq!(
            pt.description_language,
            DescriptionLanguage::PtBr,
            "the hyphenated spelling is what people type"
        );

        let invalid =
            parse_runtime_settings(r#"description_language = "klingon""#).expect("settings");
        assert_eq!(
            invalid.description_language, DEFAULT_DESCRIPTION_LANGUAGE,
            "an unknown language keeps the default instead of failing the whole file"
        );
    }

    #[test]
    fn description_language_owns_tag() {
        assert!(DescriptionLanguage::PtBr.owns_tag("pt_br"));
        assert!(DescriptionLanguage::En.owns_tag("en"));
        assert!(
            !DescriptionLanguage::PtBr.owns_tag(""),
            "a row written before the column existed must be converted"
        );
        assert!(!DescriptionLanguage::En.owns_tag("pt_br"));
    }

    #[test]
    fn parse_runtime_settings_vision_knobs() {
        let settings = parse_runtime_settings(
            r#"
            image_max_side = 768
            prompt_profile = "detailed"
            ocr_engine = "surya"
            "#,
        )
        .expect("settings");
        assert_eq!(settings.image_max_side, 768);
        assert_eq!(settings.prompt_profile, PromptProfile::Detailed);
        assert_eq!(settings.ocr_engine, OcrEngine::Surya);
    }

    #[test]
    fn parse_runtime_settings_accepts_original_side() {
        let settings = parse_runtime_settings("image_max_side = 0").expect("settings");
        assert_eq!(settings.image_max_side, 0, "0 means original bytes");
    }

    #[test]
    fn parse_runtime_settings_clamps_image_max_side() {
        // Too small would break quality; too large would just be wasteful.
        let small = parse_runtime_settings("image_max_side = 128").expect("settings");
        assert_eq!(small.image_max_side, MIN_IMAGE_MAX_SIDE);
        let large = parse_runtime_settings("image_max_side = 9999").expect("settings");
        assert_eq!(large.image_max_side, MAX_IMAGE_MAX_SIDE);
    }

    #[test]
    fn parse_runtime_settings_startup_scan() {
        let settings =
            parse_runtime_settings(r#"startup_scan = "classify_only""#).expect("settings");
        assert_eq!(settings.startup_scan, StartupScan::ClassifyOnly);

        let open_only = parse_runtime_settings(r#"startup_scan = "none""#).expect("settings");
        assert_eq!(open_only.startup_scan, StartupScan::OpenOnly);

        let invalid = parse_runtime_settings(r#"startup_scan = "sometimes""#).expect("settings");
        assert_eq!(
            invalid.startup_scan,
            StartupScan::Ask,
            "invalid falls back to ask"
        );
    }

    #[test]
    fn parse_runtime_settings_ignores_unknown_values() {
        let settings = parse_runtime_settings(
            r#"
            image_max_side = not-a-number
            prompt_profile = "verbose"
            ocr_engine = "tesseract"
            "#,
        )
        .expect("settings");
        assert_eq!(settings.image_max_side, DEFAULT_IMAGE_MAX_SIDE);
        assert_eq!(settings.prompt_profile, PromptProfile::Concise);
        assert_eq!(settings.ocr_engine, OcrEngine::Llm);
    }

    fn full_update() -> SettingsUpdate {
        SettingsUpdate {
            model: Some("qwen2.5vl:7b".to_string()),
            image_max_side: 768,
            prompt_profile: "detailed".to_string(),
            description_language: "en".to_string(),
            image_filter: "box".to_string(),
            ocr_engine: "surya".to_string(),
            startup_scan: "classify_only".to_string(),
            dedupe_dhash_distance: 4,
            chain_backlog: false,
            face_detection: true,
            face_threads: 6,
        }
    }

    #[test]
    fn apply_update_sets_every_field_and_normalizes() {
        let mut settings = RuntimeSettings::default();
        let mut update = full_update();
        // Out-of-range values are clamped instead of rejected: the form cannot send
        // them, but a hand-written JSON body can.
        update.image_max_side = 99_999;
        update.dedupe_dhash_distance = 99;

        settings.apply_update(&update).expect("apply");

        assert_eq!(settings.model_override.as_deref(), Some("qwen2.5vl:7b"));
        assert_eq!(settings.image_max_side, MAX_IMAGE_MAX_SIDE);
        assert_eq!(settings.prompt_profile, PromptProfile::Detailed);
        assert_eq!(settings.description_language, DescriptionLanguage::En);
        assert_eq!(settings.image_filter, ImageFilter::Box);
        assert_eq!(settings.ocr_engine, OcrEngine::Surya);
        assert_eq!(settings.startup_scan, StartupScan::ClassifyOnly);
        assert_eq!(settings.dedupe_dhash_distance, MAX_DEDUPE_DHASH_DISTANCE);
        assert!(!settings.chain_backlog);

        // An empty model restores the automatic choice from the hardware.
        let mut cleared = full_update();
        cleared.model = Some("   ".to_string());
        settings.apply_update(&cleared).expect("apply");
        assert!(settings.model_override.is_none());
    }

    #[test]
    fn apply_update_rejects_an_invalid_field_without_half_applying() {
        let mut settings = RuntimeSettings::default();
        let before = settings.clone();
        let mut update = full_update();
        update.prompt_profile = "verbose".to_string();

        let error = settings.apply_update(&update).expect_err("must reject");
        assert!(error.contains("prompt_profile"), "{error}");
        assert_eq!(settings, before, "a rejected update must change nothing");

        let mut update = full_update();
        update.description_language = "esperanto".to_string();
        let error = settings.apply_update(&update).expect_err("must reject");
        assert!(error.contains("description_language"), "{error}");
        assert_eq!(settings, before, "a rejected update must change nothing");
    }

    #[test]
    fn save_runtime_settings_round_trips_and_preserves_comments() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let dir = tempfile::tempdir().expect("tempdir");
        // Redirect the config dir so the real user file is never touched.
        env::set_var("XDG_CONFIG_HOME", dir.path());

        let settings = RuntimeSettings {
            model_override: Some("qwen2.5vl:3b".to_string()),
            image_max_side: 768,
            prompt_profile: PromptProfile::Detailed,
            description_language: DescriptionLanguage::En,
            image_filter: ImageFilter::Box,
            ocr_engine: OcrEngine::Off,
            startup_scan: StartupScan::ClassifyOnly,
            dedupe_dhash_distance: 4,
            chain_backlog: false,
            face_detection: true,
            face_threads: 6,
        };
        save_runtime_settings(&settings).expect("save");

        let path = runtime_settings_path().expect("path");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("# Frank Sherlock runtime settings."));
        assert!(text.contains("model_override = \"qwen2.5vl:3b\""));
        assert!(text.contains("image_max_side = 768"));
        assert!(text.contains("prompt_profile = \"detailed\""));
        assert!(text.contains("description_language = \"en\""));
        assert!(text.contains("image_filter = \"box\""));
        assert!(text.contains("ocr_engine = \"off\""));
        assert!(text.contains("startup_scan = \"classify_only\""));

        let loaded = parse_runtime_settings(&text).expect("parse");
        assert_eq!(loaded, settings);

        // Saving twice must not duplicate keys.
        save_runtime_settings(&settings).expect("save again");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text.matches("image_max_side =").count(), 1);

        env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn parse_runtime_settings_quoted_override() {
        let settings =
            parse_runtime_settings(r#"model_override = "qwen2.5vl:3b""#).expect("settings");
        assert_eq!(settings.model_override.as_deref(), Some("qwen2.5vl:3b"));
    }

    #[test]
    fn parse_runtime_settings_unquoted_override_with_comment() {
        let settings = parse_runtime_settings(
            r#"
            # local fallback
            model_override = qwen2.5vl:3b # exact Ollama tag
            "#,
        )
        .expect("settings");
        assert_eq!(settings.model_override.as_deref(), Some("qwen2.5vl:3b"));
    }

    #[test]
    fn parse_runtime_settings_rejects_invalid_model_tag() {
        let err = parse_runtime_settings(r#"model_override = "bad model""#);
        assert!(err.is_err());
    }

    #[test]
    fn runtime_settings_template_documents_model_override() {
        assert!(SETTINGS_TEMPLATE.contains("model_override"));
        assert!(SETTINGS_TEMPLATE.contains("qwen2.5vl:3b"));
        assert!(SETTINGS_TEMPLATE.contains("qwen2.5vl:7b"));
    }
}
