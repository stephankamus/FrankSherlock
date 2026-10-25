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

/// What the startup prompt needs to know about an already-indexed root.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingScanInfo {
    pub root_id: i64,
    /// Indexed files under this root that still have `confidence = 0`.
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
    pub ocr_engine: crate::config::OcrEngine,
    pub pdfium_lib_path: std::path::PathBuf,
    /// Hamming distance under which two photos are treated as the same picture
    /// during classification. `0` disables the dedupe.
    pub dedupe_dhash_distance: u32,
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
    pub ocr_engine: crate::config::OcrEngine,
}

impl Default for VisionSettings {
    fn default() -> Self {
        let defaults = crate::config::RuntimeSettings::default();
        Self {
            image_max_side: defaults.image_max_side,
            prompt_profile: defaults.prompt_profile,
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
