#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    pub status: String,
    pub mode: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub roots: u64,
    pub files: u64,
    pub db_size_bytes: u64,
    /// Bytes actually holding rows (`page_size * (page_count - freelist_count)`).
    /// Stays small after a large delete, unlike `db_size_bytes`.
    pub db_logical_bytes: u64,
    /// Bytes sitting in the freelist, recoverable with `VACUUM`.
    pub db_free_bytes: u64,
    pub thumbs_size_bytes: u64,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortField {
    Relevance,
    #[default]
    DateModified,
    Name,
    Type,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub root_scope: Vec<i64>,
    #[serde(default)]
    pub media_types: Vec<String>,
    #[serde(default)]
    pub min_confidence: Option<f32>,
    #[serde(default)]
    pub date_from: Option<String>,
    #[serde(default)]
    pub date_to: Option<String>,
    #[serde(default)]
    pub sort_by: SortField,
    #[serde(default)]
    pub sort_order: SortOrder,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
    pub items: Vec<SearchItem>,
    pub parsed_query: ParsedQuery,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub abs_path: String,
    pub media_type: String,
    pub description: String,
    pub confidence: f32,
    pub mtime_ns: i64,
    pub size_bytes: i64,
    pub thumbnail_path: Option<String>,
    pub face_count: Option<i64>,
    /// `heic` or `nef` when this row is a JPEG copy made by the conversion job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    /// Original file that produced the copy, relative to the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_rel_path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedQuery {
    pub raw_query: String,
    pub query_text: String,
    pub media_types: Vec<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub min_confidence: Option<f32>,
    pub parser_confidence: f32,
    #[serde(default)]
    pub album_name: Option<String>,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub person_id: Option<i64>,
    #[serde(default)]
    pub person_name: Option<String>,
}

impl ParsedQuery {
    pub fn passthrough(raw: &str) -> Self {
        Self {
            raw_query: raw.to_string(),
            query_text: raw.to_string(),
            media_types: Vec::new(),
            date_from: None,
            date_to: None,
            min_confidence: None,
            parser_confidence: 0.2,
            album_name: None,
            subdir: None,
            person_id: None,
            person_name: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub file_count: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartFolder {
    pub id: i64,
    pub name: String,
    pub query: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub root_id: i64,
    pub root_path: String,
    pub scanned: u64,
    pub added: u64,
    pub modified: u64,
    pub moved: u64,
    pub unchanged: u64,
    pub deleted: u64,
    pub elapsed_ms: u64,
    /// Unchanged files whose thumbnail was missing and got regenerated.
    pub healed: u64,
    /// Pending files that inherited the classification of a near-duplicate.
    pub deduped: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanJobStatus {
    pub id: i64,
    pub root_id: i64,
    pub root_path: String,
    pub status: String,
    pub scan_marker: i64,
    pub total_files: u64,
    pub processed_files: u64,
    pub progress_pct: f32,
    pub added: u64,
    pub modified: u64,
    pub moved: u64,
    pub unchanged: u64,
    pub deleted: u64,
    pub cursor_rel_path: Option<String>,
    pub error_text: Option<String>,
    pub updated_at: i64,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub phase: String,
    pub discovered_files: u64,
}

#[derive(Debug, Clone)]
pub struct ScanJobState {
    pub root_id: i64,
    pub root_path: String,
    pub scan_marker: i64,
    pub processed_files: u64,
    pub added: u64,
    pub modified: u64,
    pub moved: u64,
    pub unchanged: u64,
    pub cursor_rel_path: Option<String>,
    pub phase: String,
}

#[derive(Debug, Clone)]
pub struct UnclassifiedFile {
    pub id: i64,
    pub rel_path: String,
    pub abs_path: String,
    pub media_type: String,
    /// Perceptual hash of the thumbnail. `None` when it was never generated.
    pub dhash: Option<i64>,
}

/// A HEIC/NEF file that was converted into a JPEG copy.
///
/// The copy is an ordinary `files` row; this only records where it came from, so the
/// grid can badge it and the duplicate view can refuse to pick it as the keeper.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawConversion {
    pub id: i64,
    pub root_id: i64,
    pub source_rel_path: String,
    /// Lowercase, without the dot: `heic` or `nef`.
    pub source_ext: String,
    pub output_rel_path: String,
    pub source_size_bytes: i64,
    pub source_mtime_ns: i64,
    pub output_size_bytes: Option<i64>,
    pub max_side: u32,
    pub quality: u8,
    /// `heic`, `preview` or `demosaic`.
    pub mode: String,
    pub created_at: i64,
}

/// What the startup prompt needs to know about an already-indexed root.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingScanInfo {
    pub root_id: i64,
    /// Indexed files under this root with no description yet (not classified).
    ///
    /// Deliberately not `confidence = 0`: that is a value the model returns.
    pub unclassified: u64,
    /// Unix seconds of the last completed scan, if any.
    pub last_scan_at: Option<i64>,
}

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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupDownloadStatus {
    pub status: String,
    pub model: Option<String>,
    pub progress_pct: f32,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VenvProvisionStatus {
    pub status: String,
    pub step: String,
    pub progress_pct: f32,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub is_ready: bool,
    pub ollama_available: bool,
    pub required_models: Vec<String>,
    pub missing_models: Vec<String>,
    pub instructions: Vec<String>,
    pub download: SetupDownloadStatus,
    pub python_available: bool,
    pub python_version: Option<String>,
    pub surya_venv_ok: bool,
    pub recommended_model: String,
    pub model_tier: String,
    pub model_selection_reason: String,
    pub system_python_found: bool,
    pub venv_provision: VenvProvisionStatus,
    pub ffmpeg_available: bool,
    /// The HEIC/NEF conversion venv (`pillow-heif` + `rawpy`) is usable.
    pub raw_conversion_ready: bool,
    /// Why it is not usable, or a short confirmation when it is.
    pub raw_conversion_message: String,
}

#[derive(Debug, Clone)]
pub struct FileRecordUpsert {
    pub root_id: i64,
    pub rel_path: String,
    pub abs_path: String,
    pub filename: String,
    pub media_type: String,
    pub description: String,
    pub extracted_text: String,
    pub canonical_mentions: String,
    pub confidence: f32,
    pub lang_hint: String,
    pub mtime_ns: i64,
    pub size_bytes: i64,
    pub fingerprint: String,
    pub scan_marker: i64,
    pub location_text: String,
    pub dhash: Option<i64>,
    pub duration_secs: Option<f64>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub id: i64,
    pub media_type: String,
    pub description: String,
    pub extracted_text: String,
    pub canonical_mentions: String,
    pub location_text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProperties {
    pub id: i64,
    pub filename: String,
    pub abs_path: String,
    pub rel_path: String,
    pub root_path: String,
    pub media_type: String,
    pub description: String,
    pub extracted_text: String,
    pub canonical_mentions: String,
    pub location_text: String,
    pub confidence: f32,
    pub size_bytes: i64,
    pub mtime_ns: i64,
    pub fingerprint: String,
    pub duration_secs: Option<f64>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[serde(flatten)]
    pub exif: crate::exif::ExifDetails,
}

#[derive(Debug, Clone)]
pub struct ExistingFile {
    pub id: i64,
    pub rel_path: String,
    pub fingerprint: String,
    pub mtime_ns: i64,
    pub size_bytes: i64,
    #[allow(dead_code)] // Used for has_classification checks in future
    pub confidence: f32,
    /// Path recorded in the DB. May be stale: the scan verifies it on disk
    /// before trusting it (see the thumbnail self-heal pass).
    pub thumb_path: Option<String>,
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
            prompt_profile: defaults.prompt_profile,
            description_language: defaults.description_language,
            image_filter: defaults.image_filter,
            ocr_engine: defaults.ocr_engine,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub media_type: String,
    pub description: String,
    pub extracted_text: String,
    pub canonical_mentions: String,
    pub confidence: f32,
    pub lang_hint: String,
    /// What the picture itself is, when it is a document: `receipt`, `invoice`,
    /// `statement`, `other`, or `none` for everything else.
    ///
    /// Persisted as `files.document_hint`. It is the signal that decides whether the
    /// OCR pass runs ([`crate::classify::should_run_document_enrichment`]) and the key
    /// of the document bucket an A/B over documents iterates. Empty means "not
    /// evaluated" (a row written before the column existed), which is why the bucket
    /// filters `NOT IN ('', 'none')` instead of `<> 'none'`.
    pub document_hint: String,
    /// Why nothing usable came back, as a short taxonomy (`repeat_limit`,
    /// `ctx_overflow`, `empty_frame`, `parse`, `io`, `prepare`, ...). Empty on
    /// success. Persisted as `files.failure_kind` so a retry can be aimed at the
    /// files that failed for the same reason.
    pub failure_kind: String,
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub id: i64,
    pub root_path: String,
    pub root_name: String,
    pub created_at: i64,
    pub last_scan_at: Option<i64>,
    pub file_count: u64,
    /// `online`, `offline` or `unknown`. Roots are never deleted automatically;
    /// an unreachable root is kept and flagged so its index survives.
    pub availability: String,
    /// Unix seconds of the last successful reachability probe.
    pub last_seen_at: Option<i64>,
    /// Human readable reason for the last failed probe.
    pub last_error: Option<String>,
}

/// What a "Reclassify" pass actually cleared.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReclassifyResult {
    /// Rows whose vision result was blanked.
    pub files: u64,
    /// Thumbnail files deleted from the cache (0 unless asked for).
    pub thumbnails: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactResult {
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeResult {
    pub files_removed: u64,
    pub jobs_removed: u64,
    pub thumbs_cleaned: u64,
}

#[derive(Debug, Clone)]
pub enum HealthCheckOutcome {
    Healthy,
    RestoredFromBackup,
    Recreated,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFilesResult {
    pub deleted_count: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFileResult {
    pub file_id: i64,
    pub new_rel_path: String,
    pub new_abs_path: String,
    pub new_filename: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfPassword {
    pub id: i64,
    pub password: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedPdfInfo {
    pub id: i64,
    pub filename: String,
    pub rel_path: String,
    pub abs_path: String,
    pub root_path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryProtectedPdfsResult {
    pub total_attempted: u64,
    pub unlocked: u64,
    pub still_protected: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub abs_path: String,
    pub root_path: String,
    pub media_type: String,
    pub description: String,
    pub confidence: f32,
    pub mtime_ns: i64,
    pub size_bytes: i64,
    pub thumbnail_path: Option<String>,
    pub is_keeper: bool,
    pub similarity_score: Option<f32>,
    pub group_type: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub fingerprint: String,
    pub file_count: u64,
    pub total_size_bytes: i64,
    pub wasted_bytes: i64,
    pub files: Vec<DuplicateFile>,
    pub group_type: String,
    pub avg_similarity: Option<f32>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubdirEntry {
    pub rel_path: String,
    pub name: String,
    pub file_count: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatesResponse {
    pub total_groups: u64,
    pub total_duplicate_files: u64,
    pub total_wasted_bytes: i64,
    pub groups: Vec<DuplicateGroup>,
}

// ── Face detection ──────────────────────────────────────────────────

/// Progress of a thumbnail repair pass (`repair_thumbnails`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbRepairProgress {
    /// `None` while repairing every root.
    pub root_id: Option<i64>,
    pub total: u64,
    pub processed: u64,
    pub repaired: u64,
    pub failed: u64,
    /// Unix seconds when the run started, so the UI can show rate and ETA.
    pub started_at: i64,
    /// `"scanning"` while collecting dead pointers, `"repairing"` while
    /// regenerating, then `"done"`, `"cancelled"` or `"failed"`.
    pub phase: String,
    pub error: Option<String>,
}

/// Progress of the "convert existing descriptions" pass.
///
/// Text-only work: it rewrites the description of rows that are already classified
/// into `settings.description_language`, so it needs no image and no thumbnail. Kept
/// in memory and polled by the settings screen, like `ThumbRepairProgress`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescriptionLanguageProgress {
    /// Language the pass is writing (`pt_br` / `en`).
    pub language: String,
    pub total: u64,
    pub processed: u64,
    pub converted: u64,
    /// Rows left alone: machine markers, or a description that cannot be translated
    /// (empty, or the model answered garbage and the old text is kept).
    pub skipped: u64,
    pub failed: u64,
    pub started_at: i64,
    /// `"converting"`, then `"done"`, `"cancelled"` or `"failed"`.
    pub phase: String,
    pub error: Option<String>,
}

/// Live progress of the producer/consumer pipeline.
///
/// Kept in memory and polled by the sidebar, like `ThumbRepairProgress`: the scan
/// job row only carries `processed_files`, and writing two extra counters to
/// SQLite on every file would add a write per classification.
///
/// A `full` scan runs the same pipeline after discovery
/// (`plans/full-pipeline-cut.md`), so the record is not exclusive to the backlog
/// tool anymore: `mode` is what lets the sidebar label the panel and keep the
/// "Process Backlog" button from looking like it started the pass.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklogProgress {
    pub job_id: i64,
    pub root_id: i64,
    /// `full`, `backlog` or `retry_failed`.
    pub mode: String,
    /// Rows the pass has to work on. `0` while the index is still being read.
    pub total: u64,
    /// Rows consumed (thumbnail fixed and/or classification stored).
    pub processed: u64,
    /// Thumbnails written by the producer.
    pub thumbnailed: u64,
    /// Rows that actually need a thumbnail. Not the same as `total`: a row can be
    /// in the backlog only because it was never classified, and using `total` as
    /// the denominator made the thumbnail percentage look stuck near zero after a
    /// `Repair Thumbnails` run. Grows if the producer finds a dead pointer.
    pub thumbs_total: u64,
    /// Rows classified by the consumer, including those that inherited a
    /// classification from a near-duplicate.
    pub classified: u64,
    /// Rows that need a classification. Also narrower than `total`.
    pub classify_total: u64,
    /// Of those, how many were inherited instead of paying a vision request.
    pub deduped: u64,
    /// Faces written by the producer. Non-zero only when `face_detection` is on.
    pub faces: u64,
    /// Rows of this pass that still need face detection, so the counter has a
    /// denominator of its own instead of looking stuck at 0 %.
    pub faces_total: u64,
    pub failed: u64,
    /// Unix seconds when the pass started, for rate and ETA.
    pub started_at: i64,
    /// `counting` while listing the backlog, `running`, then `done`,
    /// `cancelled` or `failed`.
    pub phase: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceDetectProgress {
    pub root_id: i64,
    pub total: u64,
    pub processed: u64,
    pub faces_found: u64,
    /// `"downloading"` while fetching models, `"loading"` while initializing
    /// ONNX sessions, `"detecting"` while processing images, `"failed"` when
    /// the run aborted (kept until the next run starts so the UI can report it).
    pub phase: String,
    /// Human-readable failure reason, only set when `phase` is `"failed"`.
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonInfo {
    pub id: i64,
    pub name: String,
    pub face_count: u64,
    pub crop_path: Option<String>,
    pub thumbnail_path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResult {
    pub new_persons: u64,
    pub assigned_faces: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceInfo {
    pub id: i64,
    pub person_id: Option<i64>,
    pub file_id: i64,
    pub rel_path: String,
    pub filename: String,
    pub confidence: f32,
    pub crop_path: Option<String>,
}

/// A face that needs its crop regenerated.
#[derive(Debug)]
pub struct FaceCropJob {
    pub face_id: i64,
    pub bbox: [f32; 4], // [x1, y1, x2, y2]
    pub abs_path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReclusterProgress {
    /// `"crops"` while regenerating missing crops, `"clustering"` during assignment + merge,
    /// `"done"` when finished (briefly, before clearing).
    pub phase: String,
    pub total: u64,
    pub processed: u64,
    /// Populated once clustering finishes.
    pub result: Option<ClusterResult>,
}
