mod classify;
mod config;
mod db;
mod descriptions;
mod error;
mod exif;
mod face;
mod llm;
mod logs;
mod models;
mod pdf;
mod platform;
mod query_parser;
mod raw;
mod runtime;
mod scan;
mod similarity;
mod thumbnail;
mod video;
mod video_server;
mod vision_estimate;
mod vision_prep;

#[cfg(test)]
mod bench_cost;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use config::{prepare_dirs, resolve_paths, AppPaths};
use error::AppError;
use models::{
    Album, CompactResult, DeleteFilesResult, DuplicatesResponse, HealthCheckOutcome, HealthStatus,
    PdfPassword, ProtectedPdfInfo, PurgeResult, RenameFileResult, RetryProtectedPdfsResult,
    RootInfo, RuntimeStatus, ScanJobStatus, SearchRequest, SearchResponse, SetupDownloadStatus,
    SetupStatus, SmartFolder, SubdirEntry, VenvProvisionStatus,
};
use tauri::Manager;
use tauri::State;

/// Cosine distance used to fold faces into the same person.
///
/// Lives here until it becomes a setting: the value was hardcoded in two places (this
/// file and the tests) and the clustering pass now has to read the same one.
const FACE_CLUSTER_THRESHOLD: f32 = 0.30;

#[derive(Debug, Clone)]
struct ModelSelection {
    required_model: String,
    scan_model: String,
    tier: String,
    reason: String,
    exact_required: bool,
    settings_error: Option<String>,
}

/// The candidate list the modal already walked, kept so the job does not traverse the
/// share a second time. Short-lived on purpose: a window where a file added or removed in
/// between is handled by `needs_conversion` (per file) and by the sidecar ("source not
/// found"), so staleness costs at most one skipped file in the next run.
struct RawScanCache {
    root_id: i64,
    at_ms: i64,
    candidates: Vec<raw::RawCandidate>,
}

/// One running (or finished) HEIC/NEF conversion job.
#[derive(Default)]
struct RawJobSlot {
    /// Root the job belongs to: two jobs over the same root would convert the same
    /// files concurrently and could produce a half-written JPEG.
    root_id: i64,
    cancel: Arc<AtomicBool>,
    status: raw::RawJobStatus,
}

/// Id of the job already converting this root, if any.
fn running_raw_job_for_root(jobs: &HashMap<i64, RawJobSlot>, root_id: i64) -> Option<i64> {
    jobs.iter()
        .find(|(_, slot)| slot.root_id == root_id && slot.status.running)
        .map(|(id, _)| *id)
}

/// Update the phase of a running job so the modal can tell "walking" from "converting".
fn set_raw_phase(state: &AppState, job_id: i64, phase: &str) {
    if let Ok(mut jobs) = state.raw_jobs.lock() {
        if let Some(slot) = jobs.get_mut(&job_id) {
            slot.status.phase = phase.to_string();
        }
    }
}

#[derive(Clone)]
struct AppState {
    paths: AppPaths,
    read_only: bool,
    gpu_info: Arc<OnceLock<platform::gpu::GpuInfo>>,
    running_scan_jobs: Arc<Mutex<HashSet<i64>>>,
    setup_download: Arc<Mutex<llm::DownloadState>>,
    venv_provision: Arc<Mutex<platform::python::VenvProvisionState>>,
    cached_system_python: Arc<OnceLock<Option<std::path::PathBuf>>>,
    cancel_flags: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
    cli_folder_path: Option<String>,
    face_detect_progress: Arc<Mutex<Option<models::FaceDetectProgress>>>,
    face_detect_cancel: Arc<AtomicBool>,
    recluster_progress: Arc<Mutex<Option<models::ReclusterProgress>>>,
    thumb_repair_progress: Arc<Mutex<Option<models::ThumbRepairProgress>>>,
    thumb_repair_cancel: Arc<AtomicBool>,
    /// Live counters of the "convert existing descriptions" pass, polled by the
    /// settings screen.
    description_language_progress: Arc<Mutex<Option<models::DescriptionLanguageProgress>>>,
    description_language_cancel: Arc<AtomicBool>,
    /// HEIC/NEF conversion jobs by id, polled by the conversion modal.
    raw_jobs: Arc<Mutex<HashMap<i64, RawJobSlot>>>,
    /// Candidate list of the last scan, so the job does not walk the folder again.
    raw_scan_cache: Arc<Mutex<Option<RawScanCache>>>,
    /// Live counters of the producer/consumer pipeline (backlog, retry and the
    /// pipeline half of a `full`), polled by the sidebar.
    backlog_progress: Arc<Mutex<Option<models::BacklogProgress>>>,
}

impl AppState {
    /// Lazily detect GPU info (deferred so startup doesn't block on nvidia-smi).
    fn gpu_info(&self) -> &platform::gpu::GpuInfo {
        self.gpu_info.get_or_init(platform::gpu::detect_gpu_memory)
    }

    /// Lazily detect system Python (deferred so startup doesn't block on which/validate).
    fn system_python(&self) -> Option<&std::path::Path> {
        self.cached_system_python
            .get_or_init(platform::python::find_system_python)
            .as_deref()
    }
}

fn select_model_for_app(gpu: &platform::gpu::GpuInfo) -> ModelSelection {
    match config::load_runtime_settings() {
        Ok(settings) => {
            if let Some(model) = settings.model_override {
                return ModelSelection {
                    required_model: model.clone(),
                    scan_model: model.clone(),
                    tier: "manual".to_string(),
                    reason: format!("Manual model override from settings.toml: {model}"),
                    exact_required: true,
                    settings_error: None,
                };
            }
        }
        Err(e) => {
            let (recommended_tag, model_tier, model_reason) = llm::recommended_model(gpu);
            return ModelSelection {
                required_model: recommended_tag.to_string(),
                scan_model: llm::resolve_installed_model(recommended_tag),
                tier: model_tier.as_str().to_string(),
                reason: model_reason,
                exact_required: false,
                settings_error: Some(e.to_string()),
            };
        }
    }

    let (recommended_tag, model_tier, model_reason) = llm::recommended_model(gpu);
    ModelSelection {
        required_model: recommended_tag.to_string(),
        scan_model: llm::resolve_installed_model(recommended_tag),
        tier: model_tier.as_str().to_string(),
        reason: model_reason,
        exact_required: false,
        settings_error: None,
    }
}

#[tauri::command]
fn app_health(state: State<'_, AppState>) -> HealthStatus {
    HealthStatus {
        status: "ok".to_string(),
        mode: if state.read_only {
            "read-only".to_string()
        } else {
            "local-only".to_string()
        },
        read_only: state.read_only,
    }
}

fn require_writable(state: &AppState) -> Result<(), String> {
    if state.read_only {
        Err("Database is read-only".to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
fn get_app_paths(state: State<'_, AppState>) -> Result<config::AppPathsView, String> {
    Ok(state.paths.view())
}

#[tauri::command]
fn get_cli_folder_path(state: State<'_, AppState>) -> Option<String> {
    state.cli_folder_path.clone()
}

#[tauri::command]
fn ensure_database(state: State<'_, AppState>) -> Result<models::DbStats, String> {
    let mut stats = if state.read_only {
        db::database_stats(&state.paths.db_file).map_err(|e| e.to_string())?
    } else {
        db::init_database(&state.paths.db_file)
            .and_then(|_| db::database_stats(&state.paths.db_file))
            .map_err(|e| e.to_string())?
    };
    stats.db_size_bytes = file_size_bytes(&state.paths.db_file);
    stats.thumbs_size_bytes = dir_size_bytes(&state.paths.thumbnails_dir);
    Ok(stats)
}

fn file_size_bytes(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[tauri::command]
fn parse_query_nl(query: String) -> models::ParsedQuery {
    query_parser::parse_query(&query)
}

#[tauri::command]
async fn get_setup_status(state: State<'_, AppState>) -> Result<SetupStatus, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || compute_setup_status(&app_state))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn start_setup_download(state: State<'_, AppState>) -> Result<SetupDownloadStatus, String> {
    let setup = compute_setup_status(state.inner());
    if !setup.ollama_available {
        return Err("Ollama is not active. Start it first (`ollama serve`).".to_string());
    }
    if setup.missing_models.is_empty() {
        return Ok(setup.download);
    }

    {
        let current = state
            .setup_download
            .lock()
            .expect("setup download mutex poisoned");
        if current.status == "running" {
            return Ok(current.as_view());
        }
    }

    let model = setup
        .missing_models
        .first()
        .cloned()
        .ok_or_else(|| "No missing model to download".to_string())?;

    {
        let mut current = state
            .setup_download
            .lock()
            .expect("setup download mutex poisoned");
        current.status = "running".to_string();
        current.model = Some(model.clone());
        current.progress_pct = 0.0;
        current.message = format!("Starting download for {model}");
    }

    let setup_state = state.setup_download.clone();
    tauri::async_runtime::spawn(async move {
        llm::management::run_model_download(setup_state, model).await;
    });

    Ok(state
        .setup_download
        .lock()
        .expect("setup download mutex poisoned")
        .as_view())
}

#[tauri::command]
fn start_venv_provision(state: State<'_, AppState>) -> Result<VenvProvisionStatus, String> {
    // If already running, return current status
    {
        let current = state
            .venv_provision
            .lock()
            .expect("venv provision mutex poisoned");
        if current.status == "running" {
            return Ok(current.as_view());
        }
    }

    let system_python = state
        .system_python()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "No Python 3 found on this system. Install Python 3 first.".to_string())?;

    // Check if venv already works
    let python_status = platform::python::check_python_available(&state.paths.surya_venv_dir);
    if python_status.available {
        let venv_python = platform::python::python_venv_binary(&state.paths.surya_venv_dir);
        let surya_ok = platform::process::silent_command(&venv_python)
            .args(["-c", "import surya"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if surya_ok {
            let mut current = state
                .venv_provision
                .lock()
                .expect("venv provision mutex poisoned");
            current.status = "completed".to_string();
            current.progress_pct = 100.0;
            current.message = "Surya OCR is already installed.".to_string();
            return Ok(current.as_view());
        }
    }

    // Set running state
    {
        let mut current = state
            .venv_provision
            .lock()
            .expect("venv provision mutex poisoned");
        current.status = "running".to_string();
        current.step = "starting".to_string();
        current.progress_pct = 0.0;
        current.message = "Starting OCR setup...".to_string();
    }

    let provision_state = state.venv_provision.clone();
    let venv_dir = state.paths.surya_venv_dir.clone();
    tauri::async_runtime::spawn(async move {
        platform::python::run_venv_provision(provision_state, system_python, venv_dir).await;
    });

    Ok(state
        .venv_provision
        .lock()
        .expect("venv provision mutex poisoned")
        .as_view())
}

#[tauri::command]
async fn search_images(
    request: SearchRequest,
    state: State<'_, AppState>,
) -> Result<SearchResponse, String> {
    let db_path = state.paths.db_file.clone();
    tauri::async_runtime::spawn_blocking(move || db::search_images(&db_path, &request))
        .await
        .map_err(|e| AppError::Join(e.to_string()).to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn start_scan(
    root_path: String,
    mode: Option<String>,
    overrides: Option<models::ScanOverrides>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<ScanJobStatus, String> {
    let raw_mode = mode.unwrap_or_else(|| "full".to_string());
    let mode = scan::ScanMode::parse(&raw_mode)
        .ok_or_else(|| format!("invalid scan mode '{raw_mode}'"))?;
    if let Some(overrides) = overrides.as_ref() {
        validate_scan_overrides(overrides)?;
    }
    begin_scan(root_path, mode, overrides, state, app_handle)
}

/// Reject a typo before the job exists, so it cannot die on the first file with the
/// override silently ignored.
fn validate_scan_overrides(overrides: &models::ScanOverrides) -> Result<(), String> {
    if let Some(profile) = overrides.prompt_profile.as_deref() {
        config::PromptProfile::parse(profile)
            .ok_or_else(|| format!("invalid prompt_profile '{profile}'"))?;
    }
    if let Some(engine) = overrides.ocr_engine.as_deref() {
        config::OcrEngine::parse(engine).ok_or_else(|| format!("invalid ocr_engine '{engine}'"))?;
    }
    if let Some(max_side) = overrides.image_max_side {
        if max_side != 0
            && !(config::MIN_IMAGE_MAX_SIDE..=config::MAX_IMAGE_MAX_SIDE).contains(&max_side)
        {
            return Err(format!(
                "invalid image_max_side {max_side}: expected 0 or {}..={}",
                config::MIN_IMAGE_MAX_SIDE,
                config::MAX_IMAGE_MAX_SIDE
            ));
        }
    }
    Ok(())
}

/// Retry only the rows the previous attempts gave up on.
///
/// The default backlog pass skips marked failures (they would otherwise be sent to
/// the GPU on every pass forever), so this is the way back in — and the place where
/// trying another model or resolution makes sense, hence the overrides.
#[tauri::command]
fn retry_failed(
    root_path: String,
    overrides: Option<models::ScanOverrides>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<ScanJobStatus, String> {
    let overrides = overrides.unwrap_or_default();
    validate_scan_overrides(&overrides)?;
    begin_scan(
        root_path,
        scan::ScanMode::RetryFailed,
        Some(overrides),
        state,
        app_handle,
    )
}

/// How many rows of this root are waiting for a retry (`Retry failed`).
#[tauri::command]
fn failed_classifications(root_id: i64, state: State<'_, AppState>) -> Result<u64, String> {
    db::failed_classification_count(&state.paths.db_file, root_id).map_err(|e| e.to_string())
}

/// Body shared by every command that starts a scan job.
fn begin_scan(
    root_path: String,
    mode: scan::ScanMode,
    overrides: Option<models::ScanOverrides>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<ScanJobStatus, String> {
    require_writable(state.inner())?;
    log::info!("start_scan: {} in mode {}", root_path, mode.as_str());

    // Only require Ollama setup when the scan will actually classify files.
    if !mode.skips_classification() {
        let setup = compute_setup_status(state.inner());
        if !setup.is_ready {
            return Err(
                "Setup incomplete: ensure Ollama is running and required models are installed."
                    .to_string(),
            );
        }
    }

    // A backlog over a half-written index reports "completed" over a fraction of the
    // folder: discovery only fills the file list in memory and the rows are written by
    // the thumbnail phase. Measured on 2026-10-26 — a pass over 925 rows of a 50k
    // folder finished as "925/925 processed". Refuse and point at the resume instead
    // of producing a number that looks like success.
    if mode.uses_backlog_pipeline() {
        let canonical = config::canonical_root_path(&root_path)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| root_path.clone());
        if let Some((job_id, phase, processed, total)) =
            db::incomplete_scan_for_path(&state.paths.db_file, &canonical)
                .map_err(|e| e.to_string())?
        {
            return Err(format!(
                "The index of this folder is incomplete ({phase}: {processed} of {total} files written). \
                 Resume scan job {job_id} or run Refresh first — the backlog would only see part of the folder."
            ));
        }
    }

    start_scan_job(state.inner(), &app_handle, &root_path, mode, overrides)
}

/// Create the job and start its worker.
///
/// Split from [`begin_scan`] because the end of a scan starts the next pass itself: a
/// `full` scan chains into the backlog. The guards and the user-facing messages belong
/// to the command; this part is the same for both callers.
fn start_scan_job(
    app_state: &AppState,
    app_handle: &tauri::AppHandle,
    root_path: &str,
    mode: scan::ScanMode,
    overrides: Option<models::ScanOverrides>,
) -> Result<ScanJobStatus, String> {
    let job = scan::start_or_resume_scan_job(&app_state.paths.db_file, root_path)
        .map_err(|e| e.to_string())?;
    // If this root is a child of an existing parent root, adopt files from parent
    let _ = db::adopt_child_files(&app_state.paths.db_file, job.root_id, &job.root_path);

    // Any other action ends the backlog story: leaving a finished record behind kept
    // the sidebar panel (and its Stop button) on screen until the app restarted.
    if !mode.uses_backlog_pipeline() {
        let mut progress = app_state
            .backlog_progress
            .lock()
            .expect("backlog mutex poisoned");
        *progress = None;
    }

    // The backlog needs its own live record: the job row only carries one
    // counter, and the sidebar shows thumbnails and classifications side by side.
    if mode.uses_backlog_pipeline() {
        let mut progress = app_state
            .backlog_progress
            .lock()
            .expect("backlog mutex poisoned");
        *progress = Some(models::BacklogProgress {
            job_id: job.id,
            root_id: job.root_id,
            mode: mode.as_str().to_string(),
            total: 0,
            processed: 0,
            thumbnailed: 0,
            thumbs_total: 0,
            classified: 0,
            classify_total: 0,
            deduped: 0,
            faces: 0,
            faces_total: 0,
            failed: 0,
            started_at: unix_now_secs(),
            phase: "counting".to_string(),
            error: None,
        });
    }
    spawn_scan_worker_if_needed(
        app_state.clone(),
        app_handle,
        job.id,
        job.root_id,
        mode,
        overrides,
    );
    db::get_scan_job(&app_state.paths.db_file, job.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "scan job not found after start".to_string())
}

/// Indexed state of a root, used by the startup prompt.
///
/// Returns `None` when the path is not indexed yet (first run).
#[tauri::command]
fn pending_scan_summary(
    root_path: String,
    state: State<'_, AppState>,
) -> Result<Option<models::PendingScanInfo>, String> {
    let canonical = match config::canonical_root_path(&root_path) {
        Ok(path) => path.display().to_string(),
        // Unreachable mount: no index to reuse, and the prompt should not block.
        Err(_) => return Ok(None),
    };
    db::pending_scan_info(&state.paths.db_file, &canonical).map_err(|e| e.to_string())
}

/// Persist the startup behaviour (`ask`, `classify_only`, `full`, `none`).
#[tauri::command]
fn set_startup_scan(value: String) -> Result<config::RuntimeSettings, String> {
    let parsed = config::StartupScan::parse(&value)
        .ok_or_else(|| format!("invalid startup_scan '{value}'"))?;
    let mut settings = config::load_runtime_settings().unwrap_or_default();
    settings.startup_scan = parsed;
    config::save_runtime_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

#[tauri::command]
fn get_scan_job(job_id: i64, state: State<'_, AppState>) -> Result<Option<ScanJobStatus>, String> {
    db::get_scan_job(&state.paths.db_file, job_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_active_scans(state: State<'_, AppState>) -> Result<Vec<ScanJobStatus>, String> {
    db::list_resumable_scan_jobs(&state.paths.db_file).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime::gather_runtime_status(app_state.gpu_info())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_scan(job_id: i64, state: State<'_, AppState>) -> Result<bool, String> {
    require_writable(state.inner())?;
    let flags = state
        .cancel_flags
        .lock()
        .expect("cancel flags mutex poisoned");
    if let Some(flag) = flags.get(&job_id) {
        flag.store(true, Ordering::Relaxed);
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// HEIC/NEF conversion (opt-in)
//
// The only feature allowed to write inside a scanned directory, and only into
// `FrankSherlock-JPG/` next to the originals, with explicit consent in the modal.
// See `plans/heic-nef-conversion.md`.
// ---------------------------------------------------------------------------

static NEXT_RAW_JOB_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

fn next_raw_job_id() -> i64 {
    NEXT_RAW_JOB_ID.fetch_add(1, Ordering::Relaxed)
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Indexed path of a root, by id.
fn root_path_by_id(state: &AppState, root_id: i64) -> Result<String, String> {
    let roots = db::list_roots(&state.paths.db_file).map_err(|e| e.to_string())?;
    roots
        .into_iter()
        .find(|root| root.id == root_id)
        .map(|root| root.root_path)
        .ok_or_else(|| format!("root {root_id} not found"))
}

/// Counts of HEIC/NEF files per directory, for the modal that opens before anything is
/// written.
///
/// `async` on purpose: a synchronous Tauri command runs on the main thread, and walking a
/// large (or networked) root there freezes the window for as long as the walk takes. The
/// same reasoning applies to the readiness check: it is a `stat`, not a Python spawn, so
/// the modal paints immediately; the job does the real import test off-thread.
///
/// The walk itself is cached for the lifetime of the app — new HEIC/NEF files do not
/// appear often enough to justify re-traversing a share every time the modal opens. The
/// modal's "Rescan" button passes `force = true`, and the refreshed list also becomes the
/// one the job uses.
#[tauri::command]
async fn raw_scan_candidates(
    root_id: i64,
    force: Option<bool>,
    state: State<'_, AppState>,
) -> Result<raw::RawScanReport, String> {
    let db_path = state.paths.db_file.clone();
    let venv_dir = state.paths.raw_venv_dir.clone();
    let root_path = root_path_by_id(state.inner(), root_id)?;
    let cached = if force.unwrap_or(false) {
        None
    } else {
        state.raw_scan_cache.lock().ok().and_then(|cache| {
            cache
                .as_ref()
                .filter(|cache| cache.root_id == root_id)
                .map(|cache| cache.candidates.clone())
        })
    };

    let scanned = tauri::async_runtime::spawn_blocking(
        move || -> Result<(raw::RawScanReport, Vec<raw::RawCandidate>, bool), String> {
            let root = config::canonical_root_path(&root_path).map_err(|e| e.to_string())?;
            let (candidates, reused) = match cached {
                Some(candidates) => (candidates, true),
                None => (raw::list_candidates(&root, None), false),
            };
            log::info!(
                "raw scan: {} candidate(s) {} under {}",
                candidates.len(),
                if reused { "from cache" } else { "walked" },
                root.display()
            );
            let conversions =
                db::list_raw_conversions(&db_path, root_id).map_err(|e| e.to_string())?;
            let mut report = raw::build_report(&root, &candidates, &conversions);
            let present = raw::venv_present(&venv_dir);
            report.conversion_ready = present;
            report.conversion_message = if present {
                "conversion venv installed".to_string()
            } else {
                "conversion venv not installed".to_string()
            };
            Ok((report, candidates, reused))
        },
    )
    .await
    .map_err(|e| format!("could not scan the folder: {e}"))??;

    let (report, candidates, reused) = scanned;
    if !reused {
        // Hand the walk to the job: opening the modal and clicking Convert is the normal
        // flow, and doing the traversal twice is what kept the job invisible for minutes.
        if let Ok(mut cache) = state.raw_scan_cache.lock() {
            *cache = Some(RawScanCache {
                root_id,
                at_ms: unix_now_ms(),
                candidates,
            });
        }
    }
    Ok(report)
}

/// Whether the dedicated conversion venv can decode HEIC and NEF.
///
/// Spawns the interpreter, hence off the main thread.
#[tauri::command]
async fn raw_support_status(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<raw::RawSupport, String> {
    let venv_dir = state.paths.raw_venv_dir.clone();
    let script = resolve_script(&app_handle, "convert_raw_images.py");
    tauri::async_runtime::spawn_blocking(move || {
        raw::support(&platform::python::python_venv_binary(&venv_dir), &script)
    })
    .await
    .map_err(|e| format!("could not check the conversion support: {e}"))
}

/// A job report that carries one preparation failure, so the modal shows it like any
/// other conversion error instead of a promise that never resolves.
fn failed_raw_report(error: String) -> raw::RawJobReport {
    let mut report = raw::RawJobReport::default();
    report.failed = 1;
    report.failures.push(error);
    report
}

/// Install the conversion venv on demand (`Setup conversion support`).
///
/// Shares `venv_provision` with the Surya flow: it is the same "one setup runs at a
/// time" spinner, and two concurrent `pip install` runs would fight over the CPU.
#[tauri::command]
async fn provision_raw_support(state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    let system_python = state
        .system_python()
        .map(|path| path.to_path_buf())
        .ok_or_else(|| {
            "Python 3 not found. Install Python 3 and relaunch before converting HEIC/NEF."
                .to_string()
        })?;
    let provision_state = state.venv_provision.clone();
    let venv_dir = state.paths.raw_venv_dir.clone();
    tauri::async_runtime::spawn(async move {
        platform::python::run_raw_venv_provision(provision_state, system_python, venv_dir).await;
    });
    Ok(())
}

/// Start the conversion job and return its id; the modal polls `raw_conversion_status`.
///
/// `dirs` is the set of folders the user left checked (`None` = all of them). An explicit
/// empty list is rejected instead of being read as "everything": unchecking every box and
/// pressing Convert must not start a full conversion.
#[tauri::command]
fn start_raw_conversion(
    root_id: i64,
    options: Option<raw::ConversionOptions>,
    dirs: Option<Vec<String>>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    require_writable(state.inner())?;
    if dirs.as_ref().is_some_and(|selected| selected.is_empty()) {
        return Err("No folder selected: check at least one to convert.".to_string());
    }
    let selected_dirs = dirs;
    let root_path = root_path_by_id(state.inner(), root_id)?;
    let root = config::canonical_root_path(&root_path).map_err(|e| e.to_string())?;

    // Reserve the job *before* the expensive part (walk + report), under the same lock
    // that answers "is one already running". A double click, a second window or a CLI
    // invocation then cannot start two jobs over the same files: two `python` processes
    // writing the same output path is how a half-written JPEG happens. Idempotency at
    // the file level (`needs_conversion`) protects the second *run*, not two runs at
    // the same time.
    let job_id = next_raw_job_id();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.raw_jobs.lock().expect("raw jobs mutex poisoned");
        if let Some(running) = running_raw_job_for_root(&jobs, root_id) {
            return Err(format!(
                "A conversion is already running for this folder (job {running}). \
                 Wait for it to finish or cancel it first."
            ));
        }
        jobs.insert(
            job_id,
            RawJobSlot {
                root_id,
                cancel: cancel.clone(),
                status: raw::RawJobStatus {
                    running: true,
                    started_at_ms: unix_now_ms(),
                    ..raw::RawJobStatus::default()
                },
            },
        );
    }

    // Everything else — the Python support check, the folder walk, the conversion — runs
    // in the worker thread. A synchronous Tauri command executes on the main thread, and
    // doing any of that there froze the window: the walk alone touches every directory of
    // the root, and `support()` spawns an interpreter.
    let options = options.unwrap_or_default().normalized();
    let state_clone = state.inner().clone();
    std::thread::spawn(move || {
        let db_path = state_clone.paths.db_file.clone();
        let script = resolve_script(&app_handle, "convert_raw_images.py");
        let python = platform::python::python_venv_binary(&state_clone.paths.raw_venv_dir);
        set_raw_phase(&state_clone, job_id, raw::PHASE_SCANNING);

        // Reuse the walk the modal already did (cached for the app's lifetime). Without
        // this the job re-traversed the share and the modal sat at `0 / ?` for minutes.
        let prewalked = state_clone.raw_scan_cache.lock().ok().and_then(|cache| {
            cache
                .as_ref()
                .filter(|cached| cached.root_id == root_id)
                .map(|cached| {
                    log::info!(
                        "raw conversion: reusing the scan from {}s ago ({} candidates)",
                        (unix_now_ms() - cached.at_ms) / 1000,
                        cached.candidates.len()
                    );
                    cached.candidates.clone()
                })
        });

        let prepared = (|| -> Result<Vec<raw::RawCandidate>, String> {
            let support = raw::support(&python, &script);
            if !support.ready {
                return Err(format!(
                    "HEIC/NEF conversion is not available: {}",
                    support.message
                ));
            }
            let conversions =
                db::list_raw_conversions(&db_path, root_id).map_err(|e| e.to_string())?;
            let mut candidates = match &prewalked {
                Some(candidates) => candidates.clone(),
                None => raw::list_candidates(&root, Some(&cancel)),
            };
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled while scanning the folder".to_string());
            }
            // Only the folders the user left checked.
            if let Some(selected) = &selected_dirs {
                raw::filter_candidates_by_dirs(&mut candidates, selected);
            }
            let scan = raw::build_report(&root, &candidates, &conversions);
            if scan.pending == 0 {
                return Err(
                    "Nothing to convert in the selected folders: they already have their copies."
                        .to_string(),
                );
            }
            if let Ok(mut jobs) = state_clone.raw_jobs.lock() {
                if let Some(slot) = jobs.get_mut(&job_id) {
                    slot.status.total = scan.pending;
                    slot.status.phase = raw::PHASE_CONVERTING.to_string();
                }
            }
            Ok(candidates)
        })();

        let report = match prepared {
            Err(error) => {
                // A cancel during the walk is not a failure: the user asked for it.
                let mut report = failed_raw_report(error);
                if cancel.load(Ordering::Relaxed) {
                    report.failed = 0;
                    report.cancelled = true;
                    report.failures.clear();
                }
                report
            }
            Ok(candidates) => {
                // The filtered list is handed over so the job does not walk or filter
                // again; `total` comes from it directly.
                let result = raw::run_conversion(
                    &db_path,
                    root_id,
                    &root,
                    &python,
                    &script,
                    &options,
                    Some(&cancel),
                    Some(candidates),
                    |progress, done, total, current| {
                        if let Ok(mut jobs) = state_clone.raw_jobs.lock() {
                            if let Some(slot) = jobs.get_mut(&job_id) {
                                slot.status.done = done;
                                slot.status.total = total.max(slot.status.total);
                                slot.status.current = current.to_string();
                                slot.status.report = progress.clone();
                            }
                        }
                    },
                );
                result.unwrap_or_else(|error| failed_raw_report(error.to_string()))
            }
        };

        if let Ok(mut jobs) = state_clone.raw_jobs.lock() {
            if let Some(slot) = jobs.get_mut(&job_id) {
                slot.status.running = false;
                slot.status.phase = raw::PHASE_DONE.to_string();
                slot.status.report = report.clone();
            }
        }
        log::info!(
            "raw conversion job {job_id} finished: {} converted, {} skipped, {} failed{}",
            report.converted,
            report.skipped,
            report.failed,
            if report.cancelled { " (cancelled)" } else { "" }
        );
    });

    Ok(job_id)
}

#[tauri::command]
fn cancel_raw_conversion(job_id: i64, state: State<'_, AppState>) -> Result<bool, String> {
    require_writable(state.inner())?;
    let jobs = state.raw_jobs.lock().expect("raw jobs mutex poisoned");
    if let Some(slot) = jobs.get(&job_id) {
        slot.cancel.store(true, Ordering::Relaxed);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
fn raw_conversion_status(
    job_id: i64,
    state: State<'_, AppState>,
) -> Result<raw::RawJobStatus, String> {
    let jobs = state.raw_jobs.lock().expect("raw jobs mutex poisoned");
    Ok(jobs
        .get(&job_id)
        .map(|slot| slot.status.clone())
        .unwrap_or_default())
}

/// Id of the job already converting this root, so a reopened modal can reattach to it
/// instead of showing a fresh scan while a job is running in the background.
#[tauri::command]
fn raw_running_job(root_id: i64, state: State<'_, AppState>) -> Result<Option<i64>, String> {
    let jobs = state.raw_jobs.lock().expect("raw jobs mutex poisoned");
    Ok(running_raw_job_for_root(&jobs, root_id))
}

/// Provenance of the copies of a root, for the badge and the properties dialog.
#[tauri::command]
fn list_raw_conversions(
    root_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<models::RawConversion>, String> {
    db::list_raw_conversions(&state.paths.db_file, root_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_root(root_id: i64, state: State<'_, AppState>) -> Result<PurgeResult, String> {
    require_writable(state.inner())?;
    // If this root has a parent root, reassign files back instead of deleting
    if let Some(_parent_id) =
        db::reassign_to_parent_root(&state.paths.db_file, root_id).map_err(|e| e.to_string())?
    {
        // Files reassigned to parent, root + jobs already deleted by reassign_to_parent_root
        return Ok(PurgeResult {
            files_removed: 0,
            jobs_removed: 0,
            thumbs_cleaned: 0,
        });
    }
    db::purge_root(&state.paths.db_file, root_id).map_err(|e| e.to_string())
}

/// Blank the classification of every file of a root so the next pass redoes it.
///
/// The rows stay put: thumbnails, dHash, faces and album links are untouched, so
/// nothing is re-read from the share and no thumbnail is rebuilt. Reclassifying
/// through "remove + add" does not work for a folder inside another indexed root
/// (the rows are handed to the parent and adopted back), hence this command.
#[tauri::command]
fn reset_root_classifications(
    root_id: i64,
    drop_thumbnails: Option<bool>,
    state: State<'_, AppState>,
) -> Result<models::ReclassifyResult, String> {
    require_writable(state.inner())?;
    db::reset_root_classifications(
        &state.paths.db_file,
        root_id,
        drop_thumbnails.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_roots(state: State<'_, AppState>) -> Result<Vec<RootInfo>, String> {
    db::list_roots(&state.paths.db_file).map_err(|e| e.to_string())
}

/// Refresh `roots.availability` for every root.
///
/// Read-only mode only lists, because probing writes the availability column.
#[tauri::command]
fn probe_roots(state: State<'_, AppState>) -> Result<Vec<RootInfo>, String> {
    if state.read_only {
        return db::list_roots(&state.paths.db_file).map_err(|e| e.to_string());
    }
    db::probe_roots(&state.paths.db_file).map_err(|e| e.to_string())
}

/// Repoint a root at a new location, preserving its whole index.
#[tauri::command]
fn relink_root(
    root_id: i64,
    new_path: String,
    state: State<'_, AppState>,
) -> Result<Vec<RootInfo>, String> {
    require_writable(state.inner())?;
    let canonical = config::expand_and_canonicalize(&new_path).map_err(|e| e.to_string())?;
    db::relink_root(
        &state.paths.db_file,
        root_id,
        &canonical.display().to_string(),
    )
    .map_err(|e| e.to_string())?;
    db::probe_roots(&state.paths.db_file).map_err(|e| e.to_string())
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Regenerate every thumbnail whose cache file is missing, without walking the
/// share again (a full discovery is the slow part on SMB).
///
/// `root_id = None` repairs every root. Progress and cancellation follow the
/// face-detection pattern so the sidebar can show a live counter.
#[tauri::command]
async fn repair_thumbnails(
    root_id: Option<i64>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<bool, String> {
    require_writable(state.inner())?;
    if !state
        .running_scan_jobs
        .lock()
        .expect("scan job mutex poisoned")
        .is_empty()
    {
        return Err("Cannot repair thumbnails while a scan is running".to_string());
    }

    let app_state = state.inner().clone();
    {
        let mut progress = app_state
            .thumb_repair_progress
            .lock()
            .expect("thumb repair mutex poisoned");
        if progress
            .as_ref()
            .is_some_and(|p| p.phase == "scanning" || p.phase == "repairing")
        {
            return Err("Thumbnail repair is already running".to_string());
        }
        *progress = Some(models::ThumbRepairProgress {
            root_id,
            total: 0,
            processed: 0,
            repaired: 0,
            failed: 0,
            started_at: unix_now_secs(),
            phase: "scanning".to_string(),
            error: None,
        });
    }

    app_state
        .thumb_repair_cancel
        .store(false, Ordering::Relaxed);
    let ctx = build_scan_context(&app_state, &app_handle);
    let progress_arc = app_state.thumb_repair_progress.clone();
    let cancel = app_state.thumb_repair_cancel.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let outcome = scan::repair_missing_thumbnails(
            &ctx,
            root_id,
            &cancel,
            |processed, total, done, failed| {
                let mut guard = progress_arc.lock().expect("thumb repair mutex poisoned");
                if let Some(p) = guard.as_mut() {
                    p.phase = "repairing".to_string();
                    p.processed = processed;
                    p.total = total;
                    p.repaired = done;
                    p.failed = failed;
                }
            },
        );

        let mut guard = progress_arc.lock().expect("thumb repair mutex poisoned");
        match outcome {
            Ok(summary) => {
                if let Some(p) = guard.as_mut() {
                    p.processed = summary.scanned;
                    p.total = summary.scanned + summary.skipped;
                    p.repaired = summary.repaired;
                    p.failed = summary.failed;
                    p.phase = if cancel.load(Ordering::Relaxed) {
                        "cancelled".to_string()
                    } else {
                        "done".to_string()
                    };
                }
                log::info!(
                    "Thumbnail repair: {} repaired, {} failed, {} skipped in {} file(s)",
                    summary.repaired,
                    summary.failed,
                    summary.skipped,
                    summary.scanned
                );
            }
            Err(e) => {
                log::error!("Thumbnail repair failed: {e}");
                if let Some(p) = guard.as_mut() {
                    p.phase = "failed".to_string();
                    p.error = Some(e.to_string());
                }
            }
        }
    });

    Ok(true)
}

#[tauri::command]
fn thumb_repair_status(
    state: State<'_, AppState>,
) -> Result<Option<models::ThumbRepairProgress>, String> {
    Ok(state
        .thumb_repair_progress
        .lock()
        .expect("thumb repair mutex poisoned")
        .clone())
}

#[tauri::command]
fn cancel_thumb_repair(state: State<'_, AppState>) -> Result<bool, String> {
    state.thumb_repair_cancel.store(true, Ordering::Relaxed);
    Ok(true)
}

/// Rewrite the descriptions already in the index into the configured language.
///
/// `description_language` only governs what the classifier writes from now on, and an
/// existing index is mostly older than the setting. This pass translates the rows that
/// are already described — text in, text out, no image read — so the corpus becomes
/// consistent without paying for a full re-classification. Progress and cancellation
/// follow the thumbnail-repair pattern.
#[tauri::command]
async fn convert_existing_descriptions(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<bool, String> {
    require_writable(state.inner())?;
    if !state
        .running_scan_jobs
        .lock()
        .expect("scan job mutex poisoned")
        .is_empty()
    {
        return Err("Cannot rewrite descriptions while a scan is running".to_string());
    }

    let target = config::load_runtime_settings()
        .map_err(|e| e.to_string())?
        .description_language;

    let app_state = state.inner().clone();
    {
        let mut progress = app_state
            .description_language_progress
            .lock()
            .expect("description language mutex poisoned");
        if progress.as_ref().is_some_and(|p| p.phase == "converting") {
            return Err("Description conversion is already running".to_string());
        }
        *progress = Some(models::DescriptionLanguageProgress {
            language: target.as_str().to_string(),
            total: 0,
            processed: 0,
            converted: 0,
            skipped: 0,
            failed: 0,
            started_at: unix_now_secs(),
            phase: "converting".to_string(),
            error: None,
        });
    }

    app_state
        .description_language_cancel
        .store(false, Ordering::Relaxed);
    let ctx = build_scan_context(&app_state, &app_handle);
    let progress_arc = app_state.description_language_progress.clone();
    let cancel = app_state.description_language_cancel.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let outcome = descriptions::convert_existing_descriptions(
            &ctx.db_path,
            &ctx.model,
            target,
            &cancel,
            |summary| {
                let mut guard = progress_arc
                    .lock()
                    .expect("description language mutex poisoned");
                if let Some(p) = guard.as_mut() {
                    p.total = summary.total;
                    p.processed = summary.processed;
                    p.converted = summary.converted;
                    p.skipped = summary.skipped;
                    p.failed = summary.failed;
                }
            },
        );

        let mut guard = progress_arc
            .lock()
            .expect("description language mutex poisoned");
        match outcome {
            Ok(summary) => {
                if let Some(p) = guard.as_mut() {
                    p.total = summary.total;
                    p.processed = summary.processed;
                    p.converted = summary.converted;
                    p.skipped = summary.skipped;
                    p.failed = summary.failed;
                    p.phase = if cancel.load(Ordering::Relaxed) {
                        "cancelled".to_string()
                    } else {
                        "done".to_string()
                    };
                }
                log::info!(
                    "Description conversion into {}: {} converted, {} skipped, {} failed of {}",
                    target.as_str(),
                    summary.converted,
                    summary.skipped,
                    summary.failed,
                    summary.total
                );
            }
            Err(e) => {
                log::error!("Description conversion failed: {e}");
                if let Some(p) = guard.as_mut() {
                    p.phase = "failed".to_string();
                    p.error = Some(e.to_string());
                }
            }
        }
    });

    Ok(true)
}

#[tauri::command]
fn description_language_status(
    state: State<'_, AppState>,
) -> Result<Option<models::DescriptionLanguageProgress>, String> {
    Ok(state
        .description_language_progress
        .lock()
        .expect("description language mutex poisoned")
        .clone())
}

#[tauri::command]
fn cancel_description_conversion(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .description_language_cancel
        .store(true, Ordering::Relaxed);
    Ok(true)
}

/// Live counters of the backlog pass (`ScanMode::Backlog`), polled by the sidebar.
///
/// Cancelling uses the normal `cancel_scan(job_id)`; the record carries the job
/// id so the UI does not need to correlate it with the scan list.
#[tauri::command]
fn backlog_status(state: State<'_, AppState>) -> Result<Option<models::BacklogProgress>, String> {
    Ok(state
        .backlog_progress
        .lock()
        .expect("backlog mutex poisoned")
        .clone())
}

/// Reclaim disk space with `VACUUM` after deletions or a root purge.
#[tauri::command]
async fn compact_database(state: State<'_, AppState>) -> Result<CompactResult, String> {
    require_writable(state.inner())?;
    if !state
        .running_scan_jobs
        .lock()
        .expect("scan job mutex poisoned")
        .is_empty()
    {
        return Err("Cannot compact the database while a scan is running".to_string());
    }
    let db_path = state.paths.db_file.clone();
    tauri::async_runtime::spawn_blocking(move || db::compact_database(&db_path))
        .await
        .map_err(|e| AppError::Join(e.to_string()).to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_subdirectories(
    root_id: i64,
    parent_prefix: String,
    state: State<'_, AppState>,
) -> Result<Vec<SubdirEntry>, String> {
    db::list_subdirectories(&state.paths.db_file, root_id, &parent_prefix)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_files_to_clipboard(paths: Vec<String>) -> Result<(), String> {
    let text = paths.join("\n");
    platform::clipboard::copy_to_clipboard(&text)
}

#[tauri::command]
fn delete_files(
    file_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<DeleteFilesResult, String> {
    require_writable(state.inner())?;

    // Phase 1: collect file info from DB (read-only, no mutation)
    let file_infos =
        db::collect_files_for_delete(&state.paths.db_file, &file_ids).map_err(|e| e.to_string())?;

    // Phase 2: delete from filesystem first
    let mut fs_deleted_ids = Vec::new();
    let mut deleted_count = 0u64;
    let mut errors = Vec::new();

    for (fid, abs_path, thumb_path) in &file_infos {
        match std::fs::remove_file(abs_path) {
            Ok(()) => {
                fs_deleted_ids.push(*fid);
                deleted_count += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File already gone from disk — safe to remove DB record too
                fs_deleted_ids.push(*fid);
                deleted_count += 1;
            }
            Err(e) => {
                // Filesystem delete failed — do NOT remove from DB, keep state consistent
                errors.push(format!("{abs_path}: {e}"));
            }
        }
        if let Some(tp) = thumb_path {
            let _ = std::fs::remove_file(tp);
        }
    }

    // Phase 3: collect face crop paths before DB cascade deletes face_detections
    let face_crop_paths =
        db::collect_face_crop_paths_for_files(&state.paths.db_file, &fs_deleted_ids)
            .unwrap_or_default();

    // Phase 4: only remove DB records for files successfully deleted from disk
    if let Err(e) = db::delete_file_records(&state.paths.db_file, &fs_deleted_ids) {
        errors.push(format!("DB cleanup error: {e}"));
    }

    // Phase 5: best-effort face crop cleanup
    for cp in &face_crop_paths {
        let _ = std::fs::remove_file(cp);
    }

    Ok(DeleteFilesResult {
        deleted_count,
        errors,
    })
}

#[tauri::command]
fn rename_file(
    file_id: i64,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<RenameFileResult, String> {
    require_writable(state.inner())?;

    // Validate new_name
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err("Filename cannot be empty".to_string());
    }
    if new_name.contains('/') || new_name.contains('\\') {
        return Err("Filename cannot contain path separators".to_string());
    }

    let (old_abs, old_rel, _thumb) =
        db::get_file_path_info(&state.paths.db_file, file_id).map_err(|e| e.to_string())?;

    // Compute new paths by replacing the filename component
    let old_abs_path = std::path::Path::new(&old_abs);
    let new_abs_path = old_abs_path
        .parent()
        .map(|p| p.join(&new_name))
        .ok_or_else(|| "Cannot determine parent directory".to_string())?;
    let new_abs_str = new_abs_path.to_string_lossy().to_string();

    let new_rel = if let Some(parent) = platform::paths::rel_path_parent(&old_rel) {
        format!("{parent}/{new_name}")
    } else {
        new_name.clone()
    };

    // Phase 1: rename on disk first
    std::fs::rename(&old_abs, &new_abs_path).map_err(|e| format!("Rename failed: {e}"))?;

    // Phase 2: update DB — rollback filesystem rename on failure
    if let Err(e) = db::rename_file_record(
        &state.paths.db_file,
        file_id,
        &new_rel,
        &new_abs_str,
        &new_name,
    ) {
        // Attempt to restore the original filename on disk
        if let Err(rollback_err) = std::fs::rename(&new_abs_path, &old_abs) {
            log::error!(
                "DB update failed AND filesystem rollback failed: db_err={e}, rollback_err={rollback_err}, \
                 file is now at {new_abs_str} but DB still says {old_abs}"
            );
            return Err(format!(
                "Rename partially failed: DB error ({e}) and could not restore original filename ({rollback_err})"
            ));
        }
        return Err(format!(
            "Rename failed (DB update error, filesystem restored): {e}"
        ));
    }

    Ok(RenameFileResult {
        file_id,
        new_rel_path: new_rel,
        new_abs_path: new_abs_str,
        new_filename: new_name,
    })
}

#[tauri::command]
fn get_file_metadata(
    file_id: i64,
    state: State<'_, AppState>,
) -> Result<models::FileMetadata, String> {
    db::get_file_metadata(&state.paths.db_file, file_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_file_properties(
    file_id: i64,
    state: State<'_, AppState>,
) -> Result<models::FileProperties, String> {
    let mut props =
        db::get_file_properties(&state.paths.db_file, file_id).map_err(|e| e.to_string())?;

    // Enrich with EXIF data from the actual file (skip for videos)
    let path = std::path::Path::new(&props.abs_path);
    let is_vid = video::is_video_file(path);
    if path.exists() && !is_vid {
        props.exif = exif::extract_exif_details(path);

        // Get image dimensions (fast header-only read) if EXIF didn't have them
        if props.exif.image_width.is_none() || props.exif.image_height.is_none() {
            if let Ok(dim) = image::image_dimensions(path) {
                props.exif.image_width = Some(dim.0);
                props.exif.image_height = Some(dim.1);
            }
        }
    }

    Ok(props)
}

#[tauri::command]
fn update_file_metadata(
    file_id: i64,
    media_type: String,
    description: String,
    extracted_text: String,
    canonical_mentions: String,
    location_text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    require_writable(state.inner())?;
    db::update_file_metadata(
        &state.paths.db_file,
        file_id,
        &media_type,
        &description,
        &extracted_text,
        &canonical_mentions,
        &location_text,
    )
    .map_err(|e| e.to_string())
}

// ── Duplicates ──────────────────────────────────────────────────────

#[tauri::command]
async fn find_duplicates(
    root_scope: Vec<i64>,
    near_threshold: Option<f32>,
    state: State<'_, AppState>,
) -> Result<DuplicatesResponse, String> {
    let db_path = state.paths.db_file.clone();
    tauri::async_runtime::spawn_blocking(move || {
        db::find_duplicates(&db_path, &root_scope, near_threshold)
    })
    .await
    .map_err(|e| AppError::Join(e.to_string()).to_string())?
    .map_err(|e| e.to_string())
}

// ── Album commands ───────────────────────────────────────────────────

#[tauri::command]
fn create_album(name: String, state: State<'_, AppState>) -> Result<Album, String> {
    require_writable(state.inner())?;
    db::create_album(&state.paths.db_file, &name).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_album(album_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::delete_album(&state.paths.db_file, album_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_albums(state: State<'_, AppState>) -> Result<Vec<Album>, String> {
    db::list_albums(&state.paths.db_file).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_files_to_album(
    album_id: i64,
    file_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<u64, String> {
    require_writable(state.inner())?;
    db::add_files_to_album(&state.paths.db_file, album_id, &file_ids).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_files_from_album(
    album_id: i64,
    file_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<u64, String> {
    require_writable(state.inner())?;
    db::remove_files_from_album(&state.paths.db_file, album_id, &file_ids)
        .map_err(|e| e.to_string())
}

// ── Smart folder commands ───────────────────────────────────────────

#[tauri::command]
fn create_smart_folder(
    name: String,
    query: String,
    state: State<'_, AppState>,
) -> Result<SmartFolder, String> {
    require_writable(state.inner())?;
    db::create_smart_folder(&state.paths.db_file, &name, &query).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_smart_folder(folder_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::delete_smart_folder(&state.paths.db_file, folder_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_smart_folders(state: State<'_, AppState>) -> Result<Vec<SmartFolder>, String> {
    db::list_smart_folders(&state.paths.db_file).map_err(|e| e.to_string())
}

// ── Reorder commands ────────────────────────────────────────────────

#[tauri::command]
fn reorder_roots(ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::reorder_roots(&state.paths.db_file, &ids).map_err(|e| e.to_string())
}

#[tauri::command]
fn reorder_albums(ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::reorder_albums(&state.paths.db_file, &ids).map_err(|e| e.to_string())
}

#[tauri::command]
fn reorder_smart_folders(ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::reorder_smart_folders(&state.paths.db_file, &ids).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_user_config() -> Result<serde_json::Value, String> {
    config::load_user_config().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_user_config(config: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    config::save_user_config(&config).map_err(|e| e.to_string())
}

// ── PDF Password commands ────────────────────────────────────────────

#[tauri::command]
fn add_pdf_password(
    password: String,
    label: String,
    state: State<'_, AppState>,
) -> Result<PdfPassword, String> {
    require_writable(state.inner())?;
    db::add_pdf_password(&state.paths.db_file, &password, &label).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_pdf_password(password_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(state.inner())?;
    db::delete_pdf_password(&state.paths.db_file, password_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_pdf_passwords(state: State<'_, AppState>) -> Result<Vec<PdfPassword>, String> {
    db::list_pdf_passwords(&state.paths.db_file).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_protected_pdfs(state: State<'_, AppState>) -> Result<Vec<ProtectedPdfInfo>, String> {
    db::list_protected_pdfs(&state.paths.db_file).map_err(|e| e.to_string())
}

#[tauri::command]
async fn retry_protected_pdfs(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<RetryProtectedPdfsResult, String> {
    let db_path = state.paths.db_file.clone();
    let thumbnails_dir = state.paths.thumbnails_dir.clone();
    let scan_ctx = build_scan_context(state.inner(), &app_handle);

    tauri::async_runtime::spawn_blocking(move || {
        let protected = db::list_protected_pdfs(&db_path).map_err(|e| e.to_string())?;
        let passwords = db::get_all_pdf_password_strings(&db_path).map_err(|e| e.to_string())?;

        if passwords.is_empty() || protected.is_empty() {
            return Ok(RetryProtectedPdfsResult {
                total_attempted: protected.len() as u64,
                unlocked: 0,
                still_protected: protected.len() as u64,
            });
        }

        let total_attempted = protected.len() as u64;
        let mut unlocked = 0u64;

        for pdf in &protected {
            let abs = std::path::Path::new(&pdf.abs_path);
            if !abs.exists() {
                continue;
            }

            if let Some(pw) = crate::pdf::try_passwords(abs, &scan_ctx.pdfium_lib_path, &passwords)
            {
                // Re-classify with the working password
                let classification = classify::classify_pdf(
                    abs,
                    &scan_ctx.model,
                    &scan_ctx.tmp_dir,
                    &scan_ctx.surya_venv_dir,
                    &scan_ctx.surya_script,
                    &scan_ctx.pdfium_lib_path,
                    Some(&pw),
                    scan_ctx.vision_settings(),
                );

                // Generate thumbnail (root-scoped cache directory)
                let thumb_dir = thumbnails_dir.join(thumbnail::root_key(&pdf.root_path));
                let thumb_result = thumbnail::generate_pdf_thumbnail(
                    abs,
                    &thumb_dir,
                    &pdf.rel_path,
                    &scan_ctx.pdfium_lib_path,
                    Some(&pw),
                    true,
                );

                // Update the DB record with actual classification
                let _ = db::update_file_classification(
                    &db_path,
                    pdf.id,
                    &classification.media_type,
                    &classification.description,
                    &classification.extracted_text,
                    &classification.canonical_mentions,
                    classification.confidence,
                    &classification.lang_hint,
                    &classification.document_hint,
                    scan_ctx.description_language.as_str(),
                );

                if let Some(ref tr) = thumb_result {
                    let _ = db::update_file_thumb_path_by_id(&db_path, pdf.id, &tr.path);
                }

                unlocked += 1;
            }
        }

        Ok(RetryProtectedPdfsResult {
            total_attempted,
            unlocked,
            still_protected: total_attempted - unlocked,
        })
    })
    .await
    .map_err(|e| AppError::Join(e.to_string()).to_string())?
}

#[tauri::command]
async fn reclassify_pdf(
    file_id: i64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<bool, String> {
    let db_path = state.paths.db_file.clone();
    let thumbnails_dir = state.paths.thumbnails_dir.clone();
    let scan_ctx = build_scan_context(state.inner(), &app_handle);

    tauri::async_runtime::spawn_blocking(move || {
        let (abs_path, rel_path, _) =
            db::get_file_path_info(&db_path, file_id).map_err(|e| e.to_string())?;
        let abs = std::path::Path::new(&abs_path);

        if !abs.exists() {
            return Ok(false);
        }

        let passwords = db::get_all_pdf_password_strings(&db_path).map_err(|e| e.to_string())?;
        let working_pw = crate::pdf::try_passwords(abs, &scan_ctx.pdfium_lib_path, &passwords);

        if working_pw.is_none() && crate::pdf::is_password_protected(abs, &scan_ctx.pdfium_lib_path)
        {
            return Ok(false);
        }

        let classification = classify::classify_pdf(
            abs,
            &scan_ctx.model,
            &scan_ctx.tmp_dir,
            &scan_ctx.surya_venv_dir,
            &scan_ctx.surya_script,
            &scan_ctx.pdfium_lib_path,
            working_pw.as_deref(),
            scan_ctx.vision_settings(),
        );

        let thumb_dir = match db::get_root_path_for_file(&db_path, file_id) {
            Ok(Some(root_path)) => thumbnails_dir.join(thumbnail::root_key(&root_path)),
            _ => thumbnails_dir.clone(),
        };
        let thumb_result = thumbnail::generate_pdf_thumbnail(
            abs,
            &thumb_dir,
            &rel_path,
            &scan_ctx.pdfium_lib_path,
            working_pw.as_deref(),
            true,
        );

        let _ = db::update_file_classification(
            &db_path,
            file_id,
            &classification.media_type,
            &classification.description,
            &classification.extracted_text,
            &classification.canonical_mentions,
            classification.confidence,
            &classification.lang_hint,
            &classification.document_hint,
            scan_ctx.description_language.as_str(),
        );

        if let Some(ref tr) = thumb_result {
            let _ = db::update_file_thumb_path_by_id(&db_path, file_id, &tr.path);
        }

        Ok(true)
    })
    .await
    .map_err(|e| AppError::Join(e.to_string()).to_string())?
}

#[tauri::command]
fn get_video_stream_url(abs_path: String) -> String {
    video_server::stream_url(&abs_path)
}

// ── Face detection commands ─────────────────────────────────────────

#[tauri::command]
fn detect_faces(
    root_id: i64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<u64, String> {
    require_writable(state.inner())?;

    // How much there is to do, answered before the work starts: the toast is the only
    // place the user sees the queue, and "started" alone says nothing about whether
    // the scan left 3 or 30 000 images behind.
    let pending =
        db::pending_face_count(&state.paths.db_file, &[root_id]).map_err(|e| e.to_string())?;

    // Check if already running (a lingering "failed" state does not block a retry)
    {
        let progress = state
            .face_detect_progress
            .lock()
            .expect("face progress mutex poisoned");
        if let Some(p) = progress.as_ref() {
            if p.phase != "failed" {
                return Err("Face detection is already running".to_string());
            }
        }
    }

    // Reset cancel flag
    state.face_detect_cancel.store(false, Ordering::Relaxed);

    let db_path = state.paths.db_file.clone();
    let models_dir = state.paths.models_dir.clone();
    let face_crops_dir = state.paths.face_crops_dir.clone();
    let progress_arc = state.face_detect_progress.clone();
    let cancel_flag = state.face_detect_cancel.clone();
    let ort_lib_dir = resolve_ort_lib(&app_handle);

    tauri::async_runtime::spawn(async move {
        let progress_cleanup = progress_arc.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            // One engine for the whole app: a scan with `face_detection` on holds the
            // same guard, and two passes would load two copies of the models and write
            // face_detections for the same rows.
            let _guard = face::FaceEngineGuard::try_acquire()
                .ok_or_else(|| "Face detection is already running".to_string())?;
            run_face_detection(
                &db_path,
                &models_dir,
                &ort_lib_dir,
                &face_crops_dir,
                root_id,
                progress_arc,
                &cancel_flag,
            )
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::error!("Face detection failed: {e}"),
            Err(e) => {
                log::error!("Face detection task panicked: {e}");
                // Leave a failed state so the UI can report the crash
                let mut progress = progress_cleanup
                    .lock()
                    .expect("face progress mutex poisoned");
                *progress = Some(models::FaceDetectProgress {
                    root_id,
                    total: 0,
                    processed: 0,
                    faces_found: 0,
                    phase: "failed".into(),
                    error: Some(format!("Face detection crashed: {e}")),
                });
            }
        }
    });

    Ok(pending)
}

#[tauri::command]
fn get_face_detect_status(
    state: State<'_, AppState>,
) -> Result<Option<models::FaceDetectProgress>, String> {
    let progress = state
        .face_detect_progress
        .lock()
        .expect("face progress mutex poisoned");
    Ok(progress.clone())
}

#[tauri::command]
fn cancel_face_detect(state: State<'_, AppState>) -> Result<bool, String> {
    state.face_detect_cancel.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command]
fn list_files_with_faces(
    root_scope: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<models::SearchItem>, String> {
    db::list_files_with_faces(&state.paths.db_file, &root_scope).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_face_stats(
    root_scope: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<db::FaceStats, String> {
    db::get_face_stats(&state.paths.db_file, &root_scope).map_err(|e| e.to_string())
}

/// How many images of the current scope are still pending face detection.
///
/// This is what the Detect Faces button shows, and it is the same predicate the pass
/// selects on: a scan with `face_detection` off leaves everything here on purpose.
#[tauri::command]
fn get_pending_face_count(root_scope: Vec<i64>, state: State<'_, AppState>) -> Result<u64, String> {
    db::pending_face_count(&state.paths.db_file, &root_scope).map_err(|e| e.to_string())
}

#[tauri::command]
fn cluster_faces(state: State<'_, AppState>) -> Result<models::ClusterResult, String> {
    db::cluster_faces(&state.paths.db_file, FACE_CLUSTER_THRESHOLD).map_err(|e| e.to_string())
}

/// What `face_threads` resolves to on this machine.
///
/// The settings screen shows it so "0 = all cores" is not a guess: the number of cores,
/// how many sessions the budget buys and how many threads each one gets.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FaceThreadPlan {
    cores: usize,
    /// Threads the engine may use, after resolving the configured ceiling.
    budget: usize,
    /// Sessions the pool starts, each with its own copy of the models.
    workers: usize,
    intra_threads: usize,
}

fn face_thread_plan_for(configured: u32) -> FaceThreadPlan {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let budget = face::thread_budget(configured);
    let (workers, intra_threads) = face::plan_pool(budget);
    FaceThreadPlan {
        cores,
        budget,
        workers,
        intra_threads,
    }
}

#[tauri::command]
fn get_face_thread_plan() -> FaceThreadPlan {
    let configured = config::load_runtime_settings()
        .map(|settings| settings.face_threads)
        .unwrap_or(0);
    face_thread_plan_for(configured)
}

#[tauri::command]
fn recluster_faces(state: State<'_, AppState>) -> Result<(), String> {
    // Check if already running
    {
        let progress = state
            .recluster_progress
            .lock()
            .expect("recluster progress mutex poisoned");
        if progress.is_some() {
            return Err("Re-clustering is already running".to_string());
        }
    }

    let db_path = state.paths.db_file.clone();
    let face_crops_dir = state.paths.face_crops_dir.clone();
    let progress_arc = state.recluster_progress.clone();

    tauri::async_runtime::spawn(async move {
        let progress_cleanup = progress_arc.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            run_recluster(&db_path, &face_crops_dir, &progress_arc)
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::error!("Re-clustering failed: {e}"),
            Err(e) => log::error!("Re-clustering task panicked: {e}"),
        }
        // Clear progress after a short delay so UI can read the "done" state
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut progress = progress_cleanup
            .lock()
            .expect("recluster progress mutex poisoned");
        *progress = None;
    });

    Ok(())
}

fn run_recluster(
    db_path: &Path,
    face_crops_dir: &Path,
    progress_arc: &Arc<Mutex<Option<models::ReclusterProgress>>>,
) -> error::AppResult<()> {
    // Phase 1: Regenerate missing crops (batched — each image decoded only once)
    let jobs = db::faces_missing_crops(db_path).unwrap_or_default();
    let total_crops = jobs.len() as u64;

    if total_crops > 0 {
        log::info!("Regenerating {total_crops} missing face crops");
        {
            let mut progress = progress_arc.lock().expect("recluster progress mutex");
            *progress = Some(models::ReclusterProgress {
                phase: "crops".into(),
                total: total_crops,
                processed: 0,
                result: None,
            });
        }

        let progress_ref = progress_arc.clone();
        let results = face::generate_face_crops_batch(&jobs, face_crops_dir, |done| {
            let mut progress = progress_ref.lock().expect("recluster progress mutex");
            if let Some(ref mut p) = *progress {
                p.processed = done as u64;
            }
        });

        for r in results {
            match r.result {
                Ok(crop_path) => {
                    let _ =
                        db::update_face_crop_path(db_path, r.face_id, &crop_path.to_string_lossy());
                }
                Err(e) => {
                    log::warn!("Failed to regenerate crop for face {}: {e}", r.face_id);
                }
            }
        }
    }

    // Phase 2: Clustering
    {
        let mut progress = progress_arc.lock().expect("recluster progress mutex");
        *progress = Some(models::ReclusterProgress {
            phase: "clustering".into(),
            total: 0,
            processed: 0,
            result: None,
        });
    }

    let result = db::recluster_faces(db_path, 0.30)?;
    log::info!(
        "Re-clustering complete: {} persons, {} faces assigned",
        result.new_persons,
        result.assigned_faces
    );

    // Phase 3: Done
    {
        let mut progress = progress_arc.lock().expect("recluster progress mutex");
        *progress = Some(models::ReclusterProgress {
            phase: "done".into(),
            total: 0,
            processed: 0,
            result: Some(result),
        });
    }

    Ok(())
}

#[tauri::command]
fn get_recluster_status(
    state: State<'_, AppState>,
) -> Result<Option<models::ReclusterProgress>, String> {
    let progress = state
        .recluster_progress
        .lock()
        .expect("recluster progress mutex poisoned");
    Ok(progress.clone())
}

#[tauri::command]
fn person_similarity(
    person_a: i64,
    person_b: i64,
    state: State<'_, AppState>,
) -> Result<f32, String> {
    let (sim, pair) = db::person_similarity(&state.paths.db_file, person_a, person_b)
        .map_err(|e| e.to_string())?;
    if let Some((fa, fb)) = pair {
        log::info!("Person {person_a} vs {person_b}: best sim={sim:.4} (face {fa} vs {fb})");
    }
    Ok(sim)
}

#[tauri::command]
fn list_persons(
    root_scope: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<models::PersonInfo>, String> {
    db::list_persons(&state.paths.db_file, &root_scope).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_person(
    person_id: i64,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    db::rename_person(&state.paths.db_file, person_id, &new_name).map_err(|e| e.to_string())
}

#[tauri::command]
fn merge_persons(source_id: i64, target_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    db::merge_persons(&state.paths.db_file, source_id, target_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_faces_for_person(
    person_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<models::FaceInfo>, String> {
    db::list_faces_for_person(&state.paths.db_file, person_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn unassign_face_from_person(face_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    require_writable(&state)?;
    db::unassign_face_from_person(&state.paths.db_file, face_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn reassign_faces_to_person(
    face_ids: Vec<i64>,
    target_person_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    require_writable(&state)?;
    db::reassign_faces_to_person(&state.paths.db_file, &face_ids, target_person_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_representative_face(
    person_id: i64,
    face_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    require_writable(&state)?;
    db::set_representative_face(&state.paths.db_file, person_id, face_id).map_err(|e| e.to_string())
}

fn resolve_ort_lib(app_handle: &tauri::AppHandle) -> std::path::PathBuf {
    let lib_name = face::onnxruntime_lib_name();

    // macOS: check Frameworks dir
    if cfg!(target_os = "macos") {
        if let Ok(resource_dir) = app_handle.path().resource_dir() {
            let frameworks_dir = resource_dir.parent().map(|p| p.join("Frameworks"));
            if let Some(fw) = frameworks_dir {
                if fw.join(lib_name).exists() {
                    return fw;
                }
            }
        }
    }

    // Linux/Windows: check bundled lib/
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        let lib_dir = resource_dir.join("lib");
        if lib_dir.join(lib_name).exists() {
            return lib_dir;
        }
    }

    // Dev fallback: src-tauri/lib/
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib")
}

fn run_face_detection(
    db_path: &std::path::Path,
    models_dir: &std::path::Path,
    ort_lib_dir: &std::path::Path,
    face_crops_dir: &std::path::Path,
    root_id: i64,
    progress_arc: Arc<Mutex<Option<models::FaceDetectProgress>>>,
    cancel_flag: &AtomicBool,
) -> Result<(), String> {
    /// Helper to set progress phase (avoids repetition).
    fn set_phase(arc: &Arc<Mutex<Option<models::FaceDetectProgress>>>, root_id: i64, phase: &str) {
        let mut progress = arc.lock().expect("face progress mutex poisoned");
        *progress = Some(models::FaceDetectProgress {
            root_id,
            total: 0,
            processed: 0,
            faces_found: 0,
            phase: phase.into(),
            error: None,
        });
    }

    /// Record a failure in the progress state (kept until the next run starts)
    /// so the UI can report it, and pass the message through for logging.
    fn set_failed(
        arc: &Arc<Mutex<Option<models::FaceDetectProgress>>>,
        root_id: i64,
        error: String,
    ) -> String {
        let mut progress = arc.lock().expect("face progress mutex poisoned");
        *progress = Some(models::FaceDetectProgress {
            root_id,
            total: 0,
            processed: 0,
            faces_found: 0,
            phase: "failed".into(),
            error: Some(error.clone()),
        });
        error
    }

    // Phase 1: Download models if needed
    set_phase(&progress_arc, root_id, "downloading");
    let buffalo_dir = face::ensure_models(models_dir).map_err(|e| {
        set_failed(
            &progress_arc,
            root_id,
            format!("Failed to download face models: {e}"),
        )
    })?;

    // Phase 2: Initialize ONNX Runtime + load sessions
    set_phase(&progress_arc, root_id, "loading");
    match face::init_ort_from_dir(ort_lib_dir) {
        Ok(true) => log::info!("ONNX Runtime loaded from {}", ort_lib_dir.display()),
        Ok(false) => {
            return Err(set_failed(
                &progress_arc,
                root_id,
                format!(
                    "ONNX Runtime library not found in {}. Run scripts/download-onnxruntime.sh to install it.",
                    ort_lib_dir.display()
                ),
            ));
        }
        Err(e) => {
            return Err(set_failed(
                &progress_arc,
                root_id,
                format!("ONNX Runtime init failed: {e}"),
            ));
        }
    }
    // The sessions are built by each worker, once the work list says there is
    // something to do: loading 180 MB of models to find an empty queue is pure loss.

    // Get list of files to process
    let files = db::list_files_needing_face_scan(db_path, Some(root_id))
        .map_err(|e| set_failed(&progress_arc, root_id, format!("Failed to list files: {e}")))?;

    let total = files.len() as u64;

    // Set initial detecting progress
    {
        let mut progress = progress_arc.lock().expect("face progress mutex poisoned");
        *progress = Some(models::FaceDetectProgress {
            root_id,
            total,
            processed: 0,
            faces_found: 0,
            phase: "detecting".into(),
            error: None,
        });
    }

    if total == 0 {
        let mut progress = progress_arc.lock().expect("face progress mutex poisoned");
        *progress = None;
        log::info!("Face detection: nothing pending for root {root_id}");
        return Ok(());
    }

    // One detector per worker. `Session::run` takes `&mut self`, so a single shared
    // detector would serialize every inference and leave the workers waiting on a
    // lock; separate sessions cost their model load once and then run truly in
    // parallel. `plan_pool` keeps `workers * intra` inside the configured ceiling, and
    // this is exactly the place that may spend all of it: the button runs alone.
    let budget = face::thread_budget(
        config::load_runtime_settings()
            .map(|settings| settings.face_threads)
            .unwrap_or(0),
    );
    let (planned_workers, intra_threads) = face::plan_pool(budget);
    let workers = planned_workers.min(files.len()).max(1);

    log::info!(
        "Face detection: {total} file(s), {workers} worker(s), {intra_threads} thread(s) per session \
         (budget {budget})"
    );

    let processed = AtomicU64::new(0);
    let faces_found = AtomicU64::new(0);
    let next = AtomicUsize::new(0);

    // Progress is published by whichever worker finishes a file. The mutex is the one
    // the UI already polls, so the percent moves without a second channel.
    let publish = |processed: u64, found: u64| {
        let mut progress = progress_arc.lock().expect("face progress mutex poisoned");
        *progress = Some(models::FaceDetectProgress {
            root_id,
            total,
            processed,
            faces_found: found,
            phase: "detecting".into(),
            error: None,
        });
    };

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // `ort` and ONNX Runtime are native code: a missing library or a bad
                // driver panics instead of returning an error, and a panic here would
                // abort the whole scope — and the pass — through the implicit join.
                let loaded = std::panic::catch_unwind(|| {
                    face::FaceDetector::from_model_dir_with_threads(&buffalo_dir, intra_threads)
                });
                let mut detector = match loaded {
                    Ok(Ok(detector)) => detector,
                    Ok(Err(e)) => {
                        log::error!("Face detection worker could not load the models: {e}");
                        return;
                    }
                    Err(_) => {
                        log::error!("Face detection worker panicked while loading the models");
                        return;
                    }
                };

                loop {
                    if cancel_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(index) else {
                        break;
                    };

                    // One bad file (or one native panic) must not end the pass.
                    let survived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        detect_one_file(&mut detector, db_path, face_crops_dir, file, &faces_found);
                    }))
                    .is_ok();
                    if !survived {
                        log::error!(
                            "Face detection panicked on {}; the row stays pending",
                            file.rel_path
                        );
                    }

                    let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    publish(done, faces_found.load(Ordering::Relaxed));
                }
            });
        }
    });

    let processed = processed.load(Ordering::Relaxed);
    let faces_found = faces_found.load(Ordering::Relaxed);

    // Clear progress to signal completion
    {
        let mut progress = progress_arc.lock().expect("face progress mutex poisoned");
        *progress = None;
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("Face detection cancelled at {processed}/{total}");
    }
    log::info!("Face detection complete: {processed}/{total} images, {faces_found} faces found");

    // Auto-cluster faces into person groups
    if faces_found > 0 {
        match db::cluster_faces(db_path, FACE_CLUSTER_THRESHOLD) {
            Ok(result) => {
                log::info!(
                    "Face clustering: {} new persons, {} faces assigned",
                    result.new_persons,
                    result.assigned_faces
                );
            }
            Err(e) => {
                log::warn!("Face clustering failed: {e}");
            }
        }
    }

    Ok(())
}

/// Detect, crop and store the faces of one file.
///
/// Shared by the parallel manual pass. Everything happens on a single decode: the
/// crops come from the same buffer the detector saw, which is what removed the second
/// `image::open` that used to run for every photo with a face.
fn detect_one_file(
    detector: &mut face::FaceDetector,
    db_path: &std::path::Path,
    face_crops_dir: &std::path::Path,
    file: &models::UnclassifiedFile,
    faces_found: &AtomicU64,
) {
    let abs_path = std::path::Path::new(&file.abs_path);
    let decoded = match image::open(abs_path) {
        Ok(raw) => {
            let orientation = crate::exif::extract_orientation(abs_path);
            crate::exif::apply_orientation(raw, orientation)
        }
        Err(e) => {
            // A read failure is not an answer. The row stays pending so the next pass
            // retries it; the old code recorded it as "scanned, no faces" and the
            // photo never came back.
            log::warn!("Face detection could not read {}: {e}", file.rel_path);
            return;
        }
    };

    let (faces, crop_paths) =
        match face::detect_and_crop(detector, &decoded, face_crops_dir, file.id) {
            Ok(result) => result,
            Err(e) => {
                log::warn!("Face detection failed for {}: {e}", file.rel_path);
                return;
            }
        };

    if faces.is_empty() {
        if let Err(e) = db::mark_faces_scanned(db_path, file.id) {
            log::warn!("Could not mark {} as scanned for faces: {e}", file.rel_path);
        }
        return;
    }

    match db::record_faces(db_path, file.id, &faces, &crop_paths) {
        Ok(_) => {
            faces_found.fetch_add(faces.len() as u64, Ordering::Relaxed);
        }
        Err(e) => {
            log::warn!("Failed to store faces for {}: {e}", file.rel_path);
        }
    }
}

fn compute_setup_status(app_state: &AppState) -> SetupStatus {
    let gpu = app_state.gpu_info();
    let model_selection = select_model_for_app(gpu);
    let required_models = vec![model_selection.required_model.clone()];

    let installed = llm::list_installed_models();
    let ollama_available = installed.is_some();
    let missing_models = if let Some(models) = installed {
        required_models
            .iter()
            .filter(|required| {
                if model_selection.exact_required {
                    !models.iter().any(|m| m == *required)
                } else {
                    !models.iter().any(|m| llm::model_satisfies(m, required))
                }
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        required_models.clone()
    };

    let python_status = platform::python::check_python_available(&app_state.paths.surya_venv_dir);
    let os = platform::current_os();

    let mut instructions = if !ollama_available {
        let mut steps = vec!["Ollama is not detected.".to_string()];
        match os {
            platform::OsKind::Linux => {
                steps.push("Install: curl -fsSL https://ollama.com/install.sh | sh".to_string());
                steps.push("Start: ollama serve (or: systemctl start ollama)".to_string());
            }
            platform::OsKind::MacOS => {
                steps.push(
                    "Install: brew install ollama (or download from ollama.com/download)"
                        .to_string(),
                );
                steps.push(
                    "Start the Ollama app from Applications, or run: ollama serve".to_string(),
                );
            }
            platform::OsKind::Windows => {
                steps.push("Install: download from ollama.com/download".to_string());
                steps.push("Start Ollama from the Start Menu, or run: ollama serve".to_string());
            }
        }
        steps.push("Then click 'Recheck' above.".to_string());
        steps
    } else if !missing_models.is_empty() {
        vec![
            format!("Download required model(s): {}", missing_models.join(", ")),
            "Use the 'Download required model' button and wait for completion.".to_string(),
        ]
    } else {
        vec!["Setup complete.".to_string()]
    };

    if let Some(err) = &model_selection.settings_error {
        instructions.push(format!(
            "Settings warning: could not read settings.toml ({err}); using automatic model selection."
        ));
    }

    // Surya/Python is a soft requirement (OCR enrichment)
    let system_python_found = app_state.system_python().is_some();
    let surya_venv_ok = python_status.venv_exists && python_status.available;

    if !surya_venv_ok {
        if system_python_found {
            instructions
                .push("OCR (optional): Click 'Setup OCR' to auto-configure Surya OCR.".to_string());
        } else if !python_status.venv_exists || !python_status.available {
            match os {
                platform::OsKind::Linux => {
                    instructions.push(
                        "OCR (optional): Python 3 not found. Install Python 3 \
                         (e.g. sudo apt install python3 python3-venv) then relaunch."
                            .to_string(),
                    );
                }
                platform::OsKind::MacOS => {
                    instructions.push(
                        "OCR (optional): Python 3 not found. Install Python 3 \
                         (e.g. brew install python@3) then relaunch."
                            .to_string(),
                    );
                }
                platform::OsKind::Windows => {
                    instructions.push(
                        "OCR (optional): Python 3 not found. Install Python 3 \
                         from python.org/downloads, then relaunch."
                            .to_string(),
                    );
                }
            }
        }
    }

    let download = app_state
        .setup_download
        .lock()
        .expect("setup download mutex poisoned")
        .as_view();

    let venv_provision = app_state
        .venv_provision
        .lock()
        .expect("venv provision mutex poisoned")
        .as_view();

    let ffmpeg_available = video::is_ffmpeg_available();
    if !ffmpeg_available {
        instructions.push(
            "Video (optional): ffmpeg not found. Install ffmpeg for video thumbnails and classification.".to_string(),
        );
    }

    // HEIC/NEF conversion is opt-in and needs its own venv. Showing the hint only when
    // it is missing keeps the setup screen honest without blocking anything.
    let raw_support = raw::venv_ready(&app_state.paths.raw_venv_dir);
    if !raw_support.ready {
        instructions.push(
            "HEIC/NEF (optional): conversion support not installed. Right-click a folder and \
             choose \"Converter NEF/HEIC\" to install it on demand."
                .to_string(),
        );
    }

    SetupStatus {
        is_ready: ollama_available && missing_models.is_empty() && download.status != "running",
        ollama_available,
        required_models,
        missing_models,
        instructions,
        download,
        python_available: python_status.available,
        python_version: python_status.version,
        surya_venv_ok,
        recommended_model: model_selection.required_model,
        model_tier: model_selection.tier,
        model_selection_reason: model_selection.reason,
        system_python_found,
        venv_provision,
        ffmpeg_available,
        raw_conversion_ready: raw_support.ready,
        raw_conversion_message: raw_support.message,
    }
}

fn resolve_surya_script(app_handle: &tauri::AppHandle) -> std::path::PathBuf {
    resolve_script(app_handle, "surya_ocr.py")
}

/// Locate a bundled Python helper: resource dir first, checkout second.
fn resolve_script(app_handle: &tauri::AppHandle, file_name: &str) -> std::path::PathBuf {
    // In production builds, the helpers are bundled as Tauri resources.
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        let bundled = resource_dir.join("scripts").join(file_name);
        if bundled.exists() {
            return bundled;
        }
    }
    // Dev fallback: use CARGO_MANIFEST_DIR (set at compile time)
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join(file_name)
}

fn resolve_pdfium_lib(app_handle: &tauri::AppHandle) -> std::path::PathBuf {
    let lib_name = platform::pdfium_lib_name();

    // macOS: Tauri places frameworks in Contents/Frameworks/ (ad-hoc signed).
    if cfg!(target_os = "macos") {
        if let Ok(resource_dir) = app_handle.path().resource_dir() {
            // resource_dir points to Contents/Resources/; Frameworks is a sibling.
            let frameworks_dir = resource_dir.parent().map(|p| p.join("Frameworks"));
            if let Some(fw) = frameworks_dir {
                if fw.join(lib_name).exists() {
                    return fw;
                }
            }
        }
    }

    // Linux/Windows: PDFium is bundled as a Tauri resource under lib/.
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        let lib_dir = resource_dir.join("lib");
        if lib_dir.join(lib_name).exists() {
            return lib_dir;
        }
    }

    // Dev fallback: look in src-tauri/lib/
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib")
}

/// Vision knobs from `settings.toml`, with sane defaults if the file is broken.
fn vision_settings_from_config() -> models::VisionSettings {
    let settings = config::load_runtime_settings().unwrap_or_default();
    models::VisionSettings {
        image_max_side: settings.image_max_side,
        min_pixels: settings.min_pixels,
        library_kind: settings.library_kind,
        prompt_profile: settings.prompt_profile,
        description_language: settings.description_language,
        image_filter: settings.image_filter,
        ocr_engine: settings.ocr_engine,
    }
}

/// Identifies the hardware a calibration was measured on, so a GPU swap
/// invalidates cached measurements.
fn gpu_signature(gpu: &platform::gpu::GpuInfo) -> String {
    format!(
        "{:?}-{:?}-{}-{}",
        gpu.vendor, gpu.vram_total_mib, gpu.unified_memory, gpu.system_ram_mib
    )
}

fn build_scan_context(app_state: &AppState, app_handle: &tauri::AppHandle) -> models::ScanContext {
    let surya_script = resolve_surya_script(app_handle);
    let pdfium_lib_path = resolve_pdfium_lib(app_handle);
    let model_selection = select_model_for_app(app_state.gpu_info());
    let vision = vision_settings_from_config();
    let runtime = config::load_runtime_settings().unwrap_or_default();
    models::ScanContext {
        db_path: app_state.paths.db_file.clone(),
        thumbnails_dir: app_state.paths.thumbnails_dir.clone(),
        tmp_dir: app_state.paths.tmp_dir.clone(),
        surya_venv_dir: app_state.paths.surya_venv_dir.clone(),
        surya_script,
        model: model_selection.scan_model,
        image_max_side: vision.image_max_side,
        min_pixels: vision.min_pixels,
        library_kind: vision.library_kind,
        prompt_profile: vision.prompt_profile,
        description_language: vision.description_language,
        image_filter: vision.image_filter,
        ocr_engine: vision.ocr_engine,
        pdfium_lib_path,
        dedupe_dhash_distance: runtime.dedupe_dhash_distance,
        face_detection: runtime.face_detection,
        models_dir: app_state.paths.models_dir.clone(),
        face_crops_dir: app_state.paths.face_crops_dir.clone(),
        ort_lib_dir: resolve_ort_lib(app_handle),
        face_threads: runtime.face_threads,
    }
}

// ---------------------------------------------------------------------------
// Vision settings + pre-scan estimate
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_vision_settings() -> Result<config::RuntimeSettings, String> {
    config::load_runtime_settings().map_err(|e| e.to_string())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisionSettingsUpdate {
    /// `None` or empty restores automatic model selection from the hardware.
    model: Option<String>,
    image_max_side: u32,
    /// Total pixels below which a still image is not sent to the model (`0` = off).
    #[serde(default)]
    min_pixels: u32,
    /// `auto` | `photos` | `anime`; empty keeps whatever is stored.
    #[serde(default)]
    library_kind: String,
    prompt_profile: String,
    ocr_engine: String,
}

/// Save the whole settings surface from the Settings screen.
///
/// Takes the struct the form edits — all eight fields — instead of a per-field command,
/// so an update can never leave `settings.toml` half-applied, and so the UI and a
/// hand-edited file are validated by the same code
/// ([`config::RuntimeSettings::apply_update`]).
#[tauri::command]
fn update_settings(update: config::SettingsUpdate) -> Result<config::RuntimeSettings, String> {
    let mut settings = config::load_runtime_settings().unwrap_or_default();
    settings.apply_update(&update)?;
    config::save_runtime_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

#[tauri::command]
fn set_vision_settings(update: VisionSettingsUpdate) -> Result<config::RuntimeSettings, String> {
    let mut settings = config::load_runtime_settings().unwrap_or_default();
    settings.model_override = update
        .model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    settings.image_max_side =
        config::RuntimeSettings::normalize_image_max_side(update.image_max_side);
    settings.min_pixels = config::RuntimeSettings::normalize_min_pixels(update.min_pixels);
    settings.prompt_profile = config::PromptProfile::parse(&update.prompt_profile)
        .ok_or_else(|| format!("invalid prompt_profile '{}'", update.prompt_profile))?;
    settings.ocr_engine = config::OcrEngine::parse(&update.ocr_engine)
        .ok_or_else(|| format!("invalid ocr_engine '{}'", update.ocr_engine))?;
    if !update.library_kind.trim().is_empty() {
        settings.library_kind = config::LibraryKind::parse(&update.library_kind)
            .ok_or_else(|| format!("invalid library_kind '{}'", update.library_kind))?;
    }
    config::save_runtime_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

/// Models installed in Ollama, annotated with what this machine can hold.
#[tauri::command]
fn list_vision_models(
    state: State<'_, AppState>,
) -> Result<Vec<vision_estimate::VisionModelOption>, String> {
    let gpu = state.gpu_info();
    let selection = select_model_for_app(gpu);
    Ok(vision_estimate::list_models(
        gpu,
        &selection.required_model,
        &selection.scan_model,
    ))
}

/// Projection of the classification phase for `root_path` with the current settings.
///
/// The first call per (root, model, settings, GPU) calibrates by running real
/// requests on a few images; later calls are instant (cached).
#[tauri::command]
async fn estimate_vision_scan(
    root_path: String,
    state: State<'_, AppState>,
) -> Result<vision_estimate::VisionEstimate, String> {
    let root = config::expand_and_canonicalize(&root_path).map_err(|e| e.to_string())?;
    let selection = select_model_for_app(state.gpu_info());
    let vision = vision_settings_from_config();
    let tmp_dir = state.paths.tmp_dir.clone();
    let db_path = state.paths.db_file.clone();
    let signature = gpu_signature(state.gpu_info());

    tauri::async_runtime::spawn_blocking(move || {
        vision_estimate::estimate(
            &root,
            &selection.scan_model,
            vision,
            &tmp_dir,
            &signature,
            Some(&db_path),
        )
    })
    .await
    .map_err(|e| AppError::Join(e.to_string()).to_string())?
    .map_err(|e| e.to_string())
}

fn spawn_scan_worker_if_needed(
    app_state: AppState,
    app_handle: &tauri::AppHandle,
    job_id: i64,
    root_id: i64,
    mode: scan::ScanMode,
    overrides: Option<models::ScanOverrides>,
) {
    {
        let mut guard = app_state
            .running_scan_jobs
            .lock()
            .expect("scan job mutex poisoned");
        if guard.contains(&job_id) {
            return;
        }
        guard.insert(job_id);
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut flags = app_state
            .cancel_flags
            .lock()
            .expect("cancel flags mutex poisoned");
        flags.insert(job_id, cancel_flag.clone());
    }

    let mut scan_ctx = build_scan_context(&app_state, app_handle);
    if let Some(overrides) = overrides.as_ref() {
        overrides.apply(&mut scan_ctx);
        log::info!(
            "scan job {job_id}: running with overrides — model {}, {} px, profile {}, \
             description language {}, ocr {}",
            scan_ctx.model,
            scan_ctx.image_max_side,
            scan_ctx.prompt_profile.as_str(),
            scan_ctx.description_language.as_str(),
            scan_ctx.ocr_engine.as_str()
        );
    }
    let jobs = app_state.running_scan_jobs.clone();
    let cancel_flags = app_state.cancel_flags.clone();
    let app_state_for_task = app_state.clone();

    // Pipeline counters are reported live, once per file: the sidebar needs the
    // thumbnail and classification numbers separately, and writing them to
    // SQLite per file would add a write per vision request. Modes that do not
    // reach the pipeline (refresh, classify_only) never call the callback.
    //
    // The record is created on the first tick when it does not exist yet. A
    // `full` discovers the folder for minutes before the pipeline exists, and
    // showing "0 / 0" for the whole walk is worse than showing nothing; the
    // backlog tool still gets its "counting" record up front because listing the
    // rows can be slow.
    let progress_arc = app_state.backlog_progress.clone();
    let record_mode = mode.as_str().to_string();
    let on_tick = move |tick: scan::BacklogTick| {
        let mut guard = progress_arc.lock().expect("backlog mutex poisoned");
        if guard
            .as_ref()
            .is_some_and(|progress| progress.job_id != job_id)
        {
            // Another scan owns the panel; do not steal it.
            return;
        }
        if guard.is_none() {
            *guard = Some(models::BacklogProgress {
                job_id,
                root_id,
                mode: record_mode.clone(),
                total: 0,
                processed: 0,
                thumbnailed: 0,
                thumbs_total: 0,
                classified: 0,
                classify_total: 0,
                deduped: 0,
                faces: 0,
                faces_total: 0,
                failed: 0,
                started_at: unix_now_secs(),
                phase: "running".to_string(),
                error: None,
            });
        }
        if let Some(progress) = guard.as_mut() {
            progress.total = tick.total;
            progress.processed = tick.processed;
            progress.thumbnailed = tick.thumbnailed;
            progress.thumbs_total = tick.thumbs_total;
            progress.classified = tick.classified;
            progress.classify_total = tick.classify_total;
            progress.deduped = tick.deduped;
            progress.faces = tick.faces;
            progress.faces_total = tick.faces_total;
            progress.failed = tick.failed;
            progress.phase = "running".to_string();
        }
    };

    tauri::async_runtime::spawn(async move {
        let flag = cancel_flag;
        let cancel_watch = flag.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            scan::run_scan_job_with_progress(&scan_ctx, job_id, Some(&flag), mode, &on_tick)
        })
        .await
        .map_err(|e| AppError::Join(e.to_string()))
        .and_then(|v| v);

        if let Err(err) = &result {
            let _ = db::fail_scan_job(
                &app_state_for_task.paths.db_file,
                job_id,
                &format!("scan failed: {err}"),
            );
        }

        // Close the panel for whoever owns it, regardless of the mode: a `full`
        // runs the pipeline too, and leaving its record as "running" would keep the
        // counters (and the stop button) on screen until the app restarted.
        {
            let mut guard = app_state_for_task
                .backlog_progress
                .lock()
                .expect("backlog mutex poisoned");
            if let Some(progress) = guard.as_mut() {
                if progress.job_id == job_id {
                    progress.phase = if cancel_watch.load(Ordering::Relaxed) {
                        "cancelled".to_string()
                    } else if let Err(err) = &result {
                        progress.error = Some(err.to_string());
                        "failed".to_string()
                    } else {
                        "done".to_string()
                    };
                }
            }
        }

        // Remove from tracking first: the chain below starts another job, and that job
        // has to be registered before anything asks whether the app is idle.
        {
            let mut guard = jobs.lock().expect("scan job mutex poisoned");
            guard.remove(&job_id);
        }
        {
            let mut flags = cancel_flags.lock().expect("cancel flags mutex poisoned");
            flags.remove(&job_id);
        }

        if result.is_ok() {
            // WAL checkpoint + backup after each successful scan
            if let Err(e) = db::wal_checkpoint(&app_state_for_task.paths.db_file) {
                log::warn!("WAL checkpoint after scan failed: {e}");
            }
            let backup_path = app_state_for_task.paths.db_dir.join("index.sqlite.bak");
            if let Err(e) = db::backup_database(&app_state_for_task.paths.db_file, &backup_path) {
                log::warn!("Post-scan backup failed: {e}");
            }
        }

        // Only unload models once ALL scan jobs have finished
        let all_jobs_done = {
            let guard = jobs.lock().expect("scan job mutex poisoned");
            guard.is_empty()
        };
        if all_jobs_done {
            if let Err(e) = llm::cleanup_loaded_models() {
                log::warn!("Auto-cleanup after all scans finished: {e}");
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (paths, read_only) = resolve_paths()
        .and_then(|paths| {
            let dirs_ok = prepare_dirs(&paths).is_ok();
            let init_ok = dirs_ok && db::init_database(&paths.db_file).is_ok();

            if init_ok {
                // Full read-write mode
                db::recover_incomplete_scan_jobs(&paths.db_file).ok();

                // Move the thumbnail cache to `<thumbnails>/<root_key>/` once. The old
                // flat layout let one root's purge delete another root's thumbnails.
                match db::migrate_thumbnail_layout(&paths.db_file, &paths.thumbnails_dir) {
                    Ok(m) if m.moved > 0 || m.nulled > 0 => log::info!(
                        "Thumbnail cache migrated per root ({} moved, {} pointers nulled)",
                        m.moved,
                        m.nulled
                    ),
                    Ok(m) => {
                        if m.already_done {
                            log::debug!("Thumbnail cache layout already migrated");
                        }
                    }
                    Err(e) => log::warn!("Thumbnail cache migration skipped: {e}"),
                }

                match db::startup_health_check(&paths.db_file, &paths.db_dir) {
                    Ok(HealthCheckOutcome::RestoredFromBackup) => {
                        log::warn!("Database restored from backup");
                    }
                    Ok(HealthCheckOutcome::Recreated) => {
                        log::warn!("Database was recreated (previous was corrupt)");
                    }
                    Ok(HealthCheckOutcome::Healthy) => {}
                    Err(e) => {
                        log::error!("Startup health check failed: {e}");
                    }
                }

                // Probe roots instead of deleting them. A network share that is
                // unmounted at boot, after suspend, or after a logout used to be
                // purged here, destroying every indexed row and thumbnail.
                // Unreachable roots are now flagged `offline` and can be relinked.
                match db::probe_roots(&paths.db_file) {
                    Ok(roots) => {
                        for root in roots.iter().filter(|r| r.availability == "offline") {
                            log::warn!(
                                "Root offline at startup ({} files kept): {}",
                                root.file_count,
                                root.root_path
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("Root probe failed: {e}");
                    }
                }

                Ok((paths, false))
            } else if paths.db_file.exists() {
                // DB exists but filesystem is not writable — read-only mode
                log::warn!("Running in read-only mode (filesystem not writable)");
                Ok((paths, true))
            } else {
                Err(AppError::Config(
                    "Cannot create database and no existing database found".into(),
                ))
            }
        })
        .unwrap_or_else(|e| {
            log::error!("Failed to initialize application paths/database: {e}");
            eprintln!("Fatal: failed to initialize application paths/database: {e}");
            std::process::exit(1);
        });

    if let Err(e) = config::ensure_runtime_settings_file() {
        log::warn!("Failed to prepare runtime settings file: {e}");
    }

    let cli_folder_path: Option<String> =
        std::env::args()
            .nth(1)
            .and_then(|raw| match config::expand_and_canonicalize(&raw) {
                Ok(p) => Some(p.display().to_string()),
                Err(e) => {
                    eprintln!("Invalid folder argument '{}': {}", raw, e);
                    None
                }
            });

    let app_state = AppState {
        paths,
        read_only,
        gpu_info: Arc::new(OnceLock::new()),
        running_scan_jobs: Arc::new(Mutex::new(HashSet::new())),
        setup_download: Arc::new(Mutex::new(llm::DownloadState::idle())),
        venv_provision: Arc::new(Mutex::new(platform::python::VenvProvisionState::idle())),
        cached_system_python: Arc::new(OnceLock::new()),
        cancel_flags: Arc::new(Mutex::new(HashMap::new())),
        cli_folder_path,
        face_detect_progress: Arc::new(Mutex::new(None)),
        face_detect_cancel: Arc::new(AtomicBool::new(false)),
        recluster_progress: Arc::new(Mutex::new(None)),
        thumb_repair_progress: Arc::new(Mutex::new(None)),
        thumb_repair_cancel: Arc::new(AtomicBool::new(false)),
        description_language_progress: Arc::new(Mutex::new(None)),
        description_language_cancel: Arc::new(AtomicBool::new(false)),
        raw_jobs: Arc::new(Mutex::new(HashMap::new())),
        raw_scan_cache: Arc::new(Mutex::new(None)),
        backlog_progress: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                // The plugin's defaults are 40 KB and `KeepOne`: past 40 KB the file
                // is **deleted**, so a scan that logs one line per file erases its
                // own evidence while it runs (we kept reading tails of 20 lines out
                // of 191 and lost the warnings that explain the crashes). Keep a few
                // rotated files instead, which is what the diagnosis needs.
                .max_file_size(16_u128 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(app_state)
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let _ = llm::cleanup_loaded_models();
            }
        })
        // CLI folder handling is delegated to the frontend via get_cli_folder_path.
        .invoke_handler(tauri::generate_handler![
            app_health,
            logs::read_logs,
            get_app_paths,
            get_cli_folder_path,
            ensure_database,
            parse_query_nl,
            get_setup_status,
            start_setup_download,
            start_venv_provision,
            search_images,
            start_scan,
            pending_scan_summary,
            set_startup_scan,
            get_scan_job,
            list_active_scans,
            get_runtime_status,
            get_vision_settings,
            update_settings,
            set_vision_settings,
            list_vision_models,
            estimate_vision_scan,
            cancel_scan,
            raw_scan_candidates,
            raw_support_status,
            provision_raw_support,
            start_raw_conversion,
            cancel_raw_conversion,
            raw_conversion_status,
            raw_running_job,
            list_raw_conversions,
            remove_root,
            reset_root_classifications,
            list_roots,
            probe_roots,
            relink_root,
            compact_database,
            repair_thumbnails,
            thumb_repair_status,
            cancel_thumb_repair,
            convert_existing_descriptions,
            description_language_status,
            cancel_description_conversion,
            backlog_status,
            retry_failed,
            failed_classifications,
            list_subdirectories,
            load_user_config,
            save_user_config,
            copy_files_to_clipboard,
            delete_files,
            rename_file,
            get_file_metadata,
            get_file_properties,
            update_file_metadata,
            find_duplicates,
            create_album,
            delete_album,
            list_albums,
            add_files_to_album,
            remove_files_from_album,
            create_smart_folder,
            delete_smart_folder,
            list_smart_folders,
            reorder_roots,
            reorder_albums,
            reorder_smart_folders,
            add_pdf_password,
            delete_pdf_password,
            list_pdf_passwords,
            list_protected_pdfs,
            retry_protected_pdfs,
            reclassify_pdf,
            get_video_stream_url,
            detect_faces,
            get_face_detect_status,
            cancel_face_detect,
            get_face_stats,
            get_pending_face_count,
            get_face_thread_plan,
            list_files_with_faces,
            cluster_faces,
            recluster_faces,
            get_recluster_status,
            person_similarity,
            list_persons,
            rename_person,
            merge_persons,
            list_faces_for_person,
            unassign_face_from_person,
            reassign_faces_to_person,
            set_representative_face
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed face detection run must leave a "failed" progress state
    /// carrying the error so the UI can report it, instead of clearing the
    /// progress and failing silently (#12).
    #[test]
    fn face_detection_failure_leaves_failed_progress() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // A models dir nested under a regular file makes ensure_models fail
        // fast (create_dir_all -> NotADirectory) without touching the network.
        let blocker = tmp.path().join("blocker");
        std::fs::File::create(&blocker).expect("create blocker file");
        let models_dir = blocker.join("models");

        let progress_arc: Arc<Mutex<Option<models::FaceDetectProgress>>> =
            Arc::new(Mutex::new(None));
        let cancel_flag = AtomicBool::new(false);

        let result = run_face_detection(
            &tmp.path().join("index.sqlite"),
            &models_dir,
            &tmp.path().join("ort_lib"),
            &tmp.path().join("face_crops"),
            42,
            progress_arc.clone(),
            &cancel_flag,
        );

        assert!(
            result.is_err(),
            "run must fail when models cannot be stored"
        );
        let progress = progress_arc.lock().expect("progress mutex");
        let state = progress
            .as_ref()
            .expect("failed state must be kept, not cleared");
        assert_eq!(state.phase, "failed");
        assert_eq!(state.root_id, 42);
        assert!(
            state.error.as_deref().unwrap_or("").contains("face models"),
            "error should name the failing step, got: {:?}",
            state.error
        );
    }

    // ── HEIC/NEF conversion job registry ────────────────────────────
    //
    // A double click on "Convert" used to start two workers over the same files: both
    // computed the pending list before either wrote a `raw_conversions` row, so both
    // converted the same inputs and two `python` processes wrote the same output path.
    // The file-level idempotency protects the next *run*, not two runs at the same time.

    fn raw_slot(root_id: i64, running: bool) -> RawJobSlot {
        RawJobSlot {
            root_id,
            cancel: Arc::new(AtomicBool::new(false)),
            status: raw::RawJobStatus {
                running,
                ..raw::RawJobStatus::default()
            },
        }
    }

    #[test]
    fn a_running_job_blocks_a_second_one_for_the_same_root() {
        let mut jobs: HashMap<i64, RawJobSlot> = HashMap::new();
        jobs.insert(1, raw_slot(7, true));
        assert_eq!(running_raw_job_for_root(&jobs, 7), Some(1));
    }

    #[test]
    fn a_finished_job_does_not_block_a_retry() {
        let mut jobs: HashMap<i64, RawJobSlot> = HashMap::new();
        jobs.insert(1, raw_slot(7, false));
        assert_eq!(running_raw_job_for_root(&jobs, 7), None);
    }

    #[test]
    fn another_root_is_not_blocked() {
        let mut jobs: HashMap<i64, RawJobSlot> = HashMap::new();
        jobs.insert(1, raw_slot(7, true));
        assert_eq!(running_raw_job_for_root(&jobs, 8), None);
    }
}
