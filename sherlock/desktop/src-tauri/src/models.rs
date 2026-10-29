//! The desktop runtime's own structs, plus everything the web server shares.
//!
//! Row shapes live in `sherlock_core::models`; what remains here is what only makes
//! sense on the machine that runs the pipeline — the scan context, the vision
//! settings, the per-run overrides and the hardware status. They reference `config`
//! and `platform`, which is exactly why they cannot be shared.
//!
//! The glob re-export keeps every `crate::models::…` path in the rest of the crate
//! resolving, so this split did not require touching a single call site.

pub use sherlock_core::models::*;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub os: crate::platform::OsKind,
    pub current_model: Option<String>,
    pub loaded_models: Vec<String>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    pub gpu_vendor: crate::platform::gpu::GpuVendor,
    pub unified_memory: bool,
    pub system_ram_mib: u64,
    pub ollama_available: bool,
}

#[derive(Debug, Clone)]
pub struct ScanContext {
    pub db_path: std::path::PathBuf,
    pub thumbnails_dir: std::path::PathBuf,
    pub tmp_dir: std::path::PathBuf,
    pub surya_venv_dir: std::path::PathBuf,
    pub surya_script: std::path::PathBuf,
    pub model: String,
    /// Longest side of the image sent to the model (`0` = original bytes).
    pub image_max_side: u32,
    /// Total pixels below which a still image is not sent to the model (`0` = off).
    ///
    /// The thumbnail and the EXIF location are still produced, so the file stays in the
    /// grid; only the vision request is skipped. Kept as a pixel count so the backlog
    /// can filter it in SQL (`image_width * image_height`) and stay comparable.
    pub min_pixels: u32,
    /// What the library mostly holds, which decides the anime enrichment policy.
    pub library_kind: crate::config::LibraryKind,
    pub prompt_profile: crate::config::PromptProfile,
    /// Language the free-text description is written in.
    pub description_language: crate::config::DescriptionLanguage,
    /// Resampler used when downscaling the image for the model.
    pub image_filter: crate::config::ImageFilter,
    pub ocr_engine: crate::config::OcrEngine,
    pub pdfium_lib_path: std::path::PathBuf,
    /// Hamming distance under which two photos are treated as the same picture
    /// during classification. `0` disables the dedupe.
    pub dedupe_dhash_distance: u32,
    /// Run face detection inside the pipeline producer, on the image it has already
    /// decoded. `false` leaves every row pending for the Detect Faces button.
    pub face_detection: bool,
    /// Where the buffalo_l ONNX models live. Only inspected, never downloaded from a
    /// scan: without the files the pass skips faces and says so in the log.
    pub models_dir: std::path::PathBuf,
    /// Where the face crops are written.
    pub face_crops_dir: std::path::PathBuf,
    /// Directory holding `libonnxruntime.so`.
    ///
    /// Required, not decorative: `ort` with `load-dynamic` falls back to the loader
    /// search path when nobody calls `init_from`, and a library missing there is a
    /// `panic!` inside `ort::setup_api`, which killed the scan producer.
    pub ort_lib_dir: std::path::PathBuf,
    /// Ceiling on the threads the face engine may use; `0` means every core.
    pub face_threads: u32,
}

impl ScanContext {
    /// Root-scoped thumbnail directory: `<thumbnails>/<root_key>`.
    ///
    /// Keeping one directory per root is what stops two roots that cover the same
    /// tree from sharing — and deleting — each other's thumbnail files.
    pub fn thumb_dir_for(&self, root_key: &str) -> std::path::PathBuf {
        self.thumbnails_dir.join(root_key)
    }

    /// Parameters shared by every vision request issued by this scan.
    pub fn vision_settings(&self) -> VisionSettings {
        VisionSettings {
            image_max_side: self.image_max_side,
            min_pixels: self.min_pixels,
            library_kind: self.library_kind,
            prompt_profile: self.prompt_profile,
            description_language: self.description_language,
            image_filter: self.image_filter,
            ocr_engine: self.ocr_engine,
        }
    }
}

/// The subset of `RuntimeSettings` that changes how an image is described and how
/// much work the pipeline does after the first pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionSettings {
    pub image_max_side: u32,
    /// Total pixels below which a still image is not sent to the model (`0` = off).
    pub min_pixels: u32,
    /// What the library mostly holds; decides whether anime enrichment runs at all.
    pub library_kind: crate::config::LibraryKind,
    pub prompt_profile: crate::config::PromptProfile,
    pub description_language: crate::config::DescriptionLanguage,
    pub image_filter: crate::config::ImageFilter,
    pub ocr_engine: crate::config::OcrEngine,
}

impl Default for VisionSettings {
    fn default() -> Self {
        let defaults = crate::config::RuntimeSettings::default();
        Self {
            image_max_side: defaults.image_max_side,
            min_pixels: defaults.min_pixels,
            library_kind: defaults.library_kind,
            prompt_profile: defaults.prompt_profile,
            description_language: defaults.description_language,
            image_filter: defaults.image_filter,
            ocr_engine: defaults.ocr_engine,
        }
    }
}

/// Per-run overrides for a retry of failed classifications.
///
/// `None` means "use `settings.toml`". The point is to re-try a known-bad set with
/// another model or resolution without changing what the rest of the library uses.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOverrides {
    /// Empty or `None` keeps the configured/selected model.
    pub model: Option<String>,
    pub image_max_side: Option<u32>,
    /// `concise` | `detailed`.
    pub prompt_profile: Option<String>,
    /// `off` | `llm` | `surya`.
    pub ocr_engine: Option<String>,
    /// Read faces during this scan. `None` keeps what `settings.toml` says, which is
    /// what the context menu toggle overrides for a single run.
    pub face_detection: Option<bool>,
}

impl ScanOverrides {
    /// Apply onto a scan context. Unparsable values are ignored: the command that
    /// creates the overrides validates them first, and a typo must not turn a
    /// retry into a failure.
    pub fn apply(&self, ctx: &mut ScanContext) {
        if let Some(model) = self.model.as_deref().map(str::trim) {
            if !model.is_empty() {
                ctx.model = model.to_string();
            }
        }
        if let Some(max_side) = self.image_max_side {
            ctx.image_max_side = crate::config::RuntimeSettings::normalize_image_max_side(max_side);
        }
        if let Some(profile) = self
            .prompt_profile
            .as_deref()
            .and_then(crate::config::PromptProfile::parse)
        {
            ctx.prompt_profile = profile;
        }
        if let Some(engine) = self
            .ocr_engine
            .as_deref()
            .and_then(crate::config::OcrEngine::parse)
        {
            ctx.ocr_engine = engine;
        }
        if let Some(face_detection) = self.face_detection {
            ctx.face_detection = face_detection;
        }
    }
}
