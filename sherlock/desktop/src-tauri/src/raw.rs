//! Opt-in conversion of HEIC/NEF photos into JPEG copies.
//!
//! The app deliberately does **not** teach the pipeline to decode either format: the
//! `image` crate supports neither, Ollama accepts neither, and no platform WebView
//! renders NEF. Instead the user converts them once, explicitly, and the copies land
//! in `FrankSherlock-JPG/` next to the originals — ordinary JPEGs that the existing
//! scan, thumbnail, dHash, vision, OCR and face code already handle
//! (`plans/heic-nef-conversion.md`).
//!
//! The decoding itself is delegated to `scripts/convert_raw_images.py`, which runs in
//! a dedicated venv (`pillow-heif` for HEIC, `rawpy` for NEF). One process converts a
//! whole batch: the interpreter startup is paid once, not once per file.
//!
//! Because the output lives **inside** the scanned root, nothing in `scan.rs` needs to
//! know that a copy is special: the next walk finds it, fingerprints it and generates
//! its thumbnail like any other JPEG.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{AppError, AppResult};
use crate::models::RawConversion;
use crate::platform::paths::normalize_rel_path;
use crate::platform::process::silent_command;

/// Folder the copies are written into, inside the directory of the original.
pub const OUTPUT_DIR_NAME: &str = "FrankSherlock-JPG";

/// Default JPEG quality of a copy. Matches `vision_prep::JPEG_QUALITY`, which is what
/// the vision pipeline would re-encode it to anyway.
pub const DEFAULT_QUALITY: u8 = 90;

/// Default NEF strategy: use the embedded JPEG preview when it covers the request.
pub const DEFAULT_NEF_MODE: &str = "auto";

/// Below this, a file is a stub or an icon, not a photo worth converting.
const MIN_RAW_BYTES: u64 = 10 * 1024;

/// Files handed to one sidecar process. Large enough to amortise the interpreter
/// startup, small enough that a cancel does not wait minutes.
///
/// The per-file cost constants used by the modal estimate live in
/// `sherlock/desktop/src/utils/rawEstimate.ts`: the estimate is presentation, it has to
/// react to the options without another walk of the folder, and keeping it there is what
/// lets the modal avoid re-scanning on every select change.
const BATCH_SIZE: usize = 25;

/// Extensions handled by the conversion job. `nrw` is Nikon's other raw container and
/// costs nothing extra: `rawpy` reads it with the same code path as NEF.
const RAW_EXTS: [&str; 4] = ["heic", "heif", "nef", "nrw"];

/// One convertible file found by the candidate walk (no I/O beyond the walk itself).
#[derive(Debug, Clone)]
pub struct RawCandidate {
    /// Path relative to the root, with `/` separators.
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    /// `heic` or `nef`.
    pub kind: &'static str,
}

/// Per-directory counters shown in the modal before anything is written.
///
/// Deliberately **no** write-permission field: probing writes a temporary file, and doing
/// that for every directory during a preview is both slow (SMB `create` on a dead mount
/// blocks for minutes) and a surprise. The job probes the directory it is about to write
/// into, inside its own worker thread, and reports the failures it finds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawDirCount {
    /// Directory relative to the root; empty for the root itself.
    pub rel_path: String,
    pub heic: u64,
    pub nef: u64,
    pub source_bytes: u64,
    pub already_converted: u64,
}

/// What the modal needs to explain the situation with real numbers.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawScanReport {
    pub dirs: Vec<RawDirCount>,
    pub total_files: u64,
    pub heic: u64,
    pub nef: u64,
    /// Files with no recorded conversion yet.
    pub pending: u64,
    pub source_bytes: u64,
    /// Whether the conversion venv exists. Cheap check only (`stat` on the interpreter):
    /// the real import test happens in the job, off the UI thread.
    pub conversion_ready: bool,
    pub conversion_message: String,
}

/// User choices for one conversion job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversionOptions {
    /// Longest side of the copy; `0` keeps the original resolution.
    pub max_side: u32,
    pub quality: u8,
    /// `auto`, `preview` or `demosaic`.
    pub nef_mode: String,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            max_side: 0,
            quality: DEFAULT_QUALITY,
            nef_mode: DEFAULT_NEF_MODE.to_string(),
        }
    }
}

impl ConversionOptions {
    /// Clamp user input to something the sidecar accepts.
    pub fn normalized(&self) -> Self {
        let nef_mode = match self.nef_mode.as_str() {
            "preview" | "demosaic" => self.nef_mode.clone(),
            _ => DEFAULT_NEF_MODE.to_string(),
        };
        Self {
            max_side: self.max_side.min(20_000),
            quality: self.quality.clamp(1, 100),
            nef_mode,
        }
    }
}

/// Progress and result of a conversion job.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawJobReport {
    pub converted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes_out: u64,
    pub cancelled: bool,
    /// One human-readable line per failure, already truncated for the UI.
    pub failures: Vec<String>,
}

/// Whether the conversion venv can actually do the work.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSupport {
    pub ready: bool,
    pub message: String,
}

/// `<ext>` of a convertible file, or `None`.
///
/// `heif` is treated as HEIC: it is the same container and `pillow-heif` reads both.
pub fn convertible_kind(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())?;
    if !RAW_EXTS.contains(&ext.as_str()) {
        return None;
    }
    if ext == "nef" || ext == "nrw" {
        Some("nef")
    } else {
        Some("heic")
    }
}

/// Path of the copy, relative to the root.
///
/// `<dir>/FrankSherlock-JPG/<stem>__<ext>.jpg` — the `__<ext>` suffix makes the name a
/// pure function of the source (so the job is idempotent), keeps it from ever colliding
/// with a native JPEG of the same stem, and makes the provenance visible when the user
/// browses the folder outside the app.
pub fn output_rel_path(source_rel_path: &str) -> String {
    let normalized = normalize_rel_path(source_rel_path);
    let path = Path::new(&normalized);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "raw".to_string());
    let name = format!("{stem}__{ext}.jpg");
    normalize_rel_path(&parent.join(OUTPUT_DIR_NAME).join(name).to_string_lossy())
}

/// Directory of a candidate, relative to the root; empty for the root itself.
///
/// This is the key the folder selection uses, so it has to be the same value the report
/// groups by.
pub fn candidate_dir_rel_path(source_rel_path: &str) -> String {
    let normalized = normalize_rel_path(source_rel_path);
    Path::new(&normalized)
        .parent()
        .map(|parent| normalize_rel_path(&parent.to_string_lossy()))
        .unwrap_or_default()
}

/// Keep only the candidates that live in one of `dirs`.
///
/// An empty selection means "everything": the modal sends the checked folders, and a
/// root with a single directory sends `[""]`, which is the root itself.
pub fn filter_candidates_by_dirs(candidates: &mut Vec<RawCandidate>, dirs: &[String]) {
    if dirs.is_empty() {
        return;
    }
    candidates.retain(|candidate| {
        let dir = candidate_dir_rel_path(&candidate.rel_path);
        dirs.iter().any(|selected| selected == &dir)
    });
}

/// Directory of a copy, relative to the root (used for the writability probe).
fn output_dir_rel_path(source_rel_path: &str) -> String {
    let normalized = normalize_rel_path(source_rel_path);
    let path = Path::new(&normalized);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    normalize_rel_path(&parent.join(OUTPUT_DIR_NAME).to_string_lossy())
}

/// True when this candidate still needs work: no recorded conversion, a different
/// source (size or mtime changed), a missing output, or different options.
pub fn needs_conversion(
    candidate: &RawCandidate,
    existing: Option<&RawConversion>,
    options: &ConversionOptions,
    output_abs: &Path,
) -> bool {
    let Some(conversion) = existing else {
        return true;
    };
    if !output_abs.exists() {
        return true;
    }
    if conversion.source_size_bytes != candidate.size_bytes as i64
        || conversion.source_mtime_ns != candidate.mtime_ns
    {
        return true;
    }
    conversion.max_side != options.max_side
        || conversion.quality != options.quality
        || conversion.mode != options.nef_mode
}

/// Walk the root and collect convertible files, skipping our own output folder.
///
/// Deliberately does no decoding: this is what makes opening the modal cheap even on a
/// 58k-file network share. The cancel flag is checked every 256 entries — a walk of a
/// large share takes minutes, and "Cancel" has to work *during* it, not after.
pub fn list_candidates(root: &Path, cancel: Option<&AtomicBool>) -> Vec<RawCandidate> {
    let mut candidates = Vec::new();
    let mut seen: u32 = 0;
    // A job cancelled before the walk starts must not touch the share at all.
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return candidates;
    }
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.file_type().is_dir() && entry.file_name() == OUTPUT_DIR_NAME)
        });

    for entry in walker.flatten() {
        seen = seen.wrapping_add(1);
        if seen % 256 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            log::info!("raw conversion: walk cancelled after {seen} entries");
            return candidates;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(kind) = convertible_kind(path) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let size_bytes = metadata.len();
        if size_bytes < MIN_RAW_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let rel_path = normalize_rel_path(&relative.to_string_lossy());
        candidates.push(RawCandidate {
            rel_path,
            abs_path: path.to_path_buf(),
            size_bytes,
            mtime_ns: modified_ns(&metadata),
            kind,
        });
    }

    candidates.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    candidates
}

fn modified_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Can we create files in this directory?
///
/// The probe writes and removes a temporary file **in the original directory** instead
/// of creating `FrankSherlock-JPG` up front: the user may look at the report and cancel,
/// and an empty folder per directory would be a side effect of merely asking.
pub fn probe_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(".frank-sherlock-write-probe");
    let written = std::fs::write(&probe, b"probe").is_ok();
    if written {
        let _ = std::fs::remove_file(&probe);
    }
    written
}

/// Build the report shown before the job starts.
///
/// Pure arithmetic over the walk result: no filesystem access beyond what the walk
/// already did, so opening the modal stays cheap on a large library.
pub fn build_report(
    _root: &Path,
    candidates: &[RawCandidate],
    conversions: &[RawConversion],
) -> RawScanReport {
    use std::collections::BTreeMap;

    let mut report = RawScanReport::default();
    let mut dirs: BTreeMap<String, RawDirCount> = BTreeMap::new();
    let conversions_by_source: BTreeMap<&str, &RawConversion> = conversions
        .iter()
        .map(|conversion| (conversion.source_rel_path.as_str(), conversion))
        .collect();

    for candidate in candidates {
        let dir_rel = candidate_dir_rel_path(&candidate.rel_path);
        let entry = dirs.entry(dir_rel.clone()).or_insert_with(|| RawDirCount {
            rel_path: dir_rel,
            heic: 0,
            nef: 0,
            source_bytes: 0,
            already_converted: 0,
        });

        match candidate.kind {
            "nef" => entry.nef += 1,
            _ => entry.heic += 1,
        }
        entry.source_bytes += candidate.size_bytes;
        report.total_files += 1;
        report.source_bytes += candidate.size_bytes;
        if candidate.kind == "nef" {
            report.nef += 1;
        } else {
            report.heic += 1;
        }

        if conversions_by_source.contains_key(candidate.rel_path.as_str()) {
            entry.already_converted += 1;
        } else {
            report.pending += 1;
        }
    }

    report.dirs = dirs.into_values().collect();
    report
}

/// Absolute path of a copy for a candidate, inside the root.
pub fn output_abs_path(root: &Path, source_rel_path: &str) -> PathBuf {
    root.join(output_rel_path(source_rel_path))
}

/// Absolute directory of a copy, for `create_dir_all`.
pub fn output_dir_abs_path(root: &Path, source_rel_path: &str) -> PathBuf {
    root.join(output_dir_rel_path(source_rel_path))
}

// ---------------------------------------------------------------------------
// Sidecar
// ---------------------------------------------------------------------------

/// One JSON line emitted by `convert_raw_images.py`.
#[derive(Debug, Deserialize)]
pub struct SidecarLine {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub fatal: Option<String>,
    #[serde(default)]
    pub skipped: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// Parse one sidecar line. A line that is not JSON is reported as a fatal error, because
/// that is what a traceback or a missing interpreter looks like.
pub fn parse_sidecar_line(line: &str) -> Result<Option<SidecarLine>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<SidecarLine>(trimmed) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(_) => Ok(Some(SidecarLine {
            source: None,
            output: None,
            ok: false,
            error: None,
            fatal: Some(trimmed.to_string()),
            skipped: None,
            mode: None,
            bytes: None,
            width: None,
            height: None,
        })),
    }
}

/// Check that the conversion venv exists and can import both decoders.
pub fn support(python: &Path, script: &Path) -> RawSupport {
    if !python.exists() {
        return RawSupport {
            ready: false,
            message: "conversion venv not installed".to_string(),
        };
    }
    if !script.exists() {
        return RawSupport {
            ready: false,
            message: format!("conversion script not found: {}", script.display()),
        };
    }
    let output = silent_command(python)
        .arg(script)
        .arg("--self-check")
        .output();
    let Ok(output) = output else {
        return RawSupport {
            ready: false,
            message: "could not run the conversion venv".to_string(),
        };
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or_default();
    match parse_sidecar_line(line) {
        Ok(Some(parsed)) if parsed.ok => RawSupport {
            ready: true,
            message: "pillow-heif and rawpy available".to_string(),
        },
        Ok(Some(parsed)) => RawSupport {
            ready: false,
            message: parsed
                .fatal
                .or(parsed.error)
                .unwrap_or_else(|| "conversion support unavailable".to_string()),
        },
        _ => RawSupport {
            ready: false,
            message: "unexpected output from the conversion self-check".to_string(),
        },
    }
}

/// Cheap readiness check for the modal: does the conversion interpreter exist?
///
/// A `stat`, not a spawn. Running the import test here put ~100 ms of process startup
/// between the click and the first paint (and hangs if the interpreter lives on a slow
/// mount), so the real check happens in the job, off the UI thread.
pub fn venv_present(venv_dir: &Path) -> bool {
    crate::platform::python::python_venv_binary(venv_dir).exists()
}

/// Ready check that does not need the script: used by the setup screen, which runs
/// before anyone opens the conversion modal.
///
/// Returns early when the venv does not exist, so the common case (feature unused) pays
/// a single `stat` instead of a Python spawn.
pub fn venv_ready(venv_dir: &Path) -> RawSupport {
    let python = crate::platform::python::python_venv_binary(venv_dir);
    if !python.exists() {
        return RawSupport {
            ready: false,
            message: "conversion venv not installed".to_string(),
        };
    }
    match silent_command(&python)
        .args(["-c", "import pillow_heif, rawpy"])
        .output()
    {
        Ok(output) if output.status.success() => RawSupport {
            ready: true,
            message: "pillow-heif and rawpy available".to_string(),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .last()
                .unwrap_or("import failed")
                .trim()
                .to_string();
            RawSupport {
                ready: false,
                message: format!("conversion venv incomplete: {detail}"),
            }
        }
        Err(e) => RawSupport {
            ready: false,
            message: format!("cannot run the conversion venv: {e}"),
        },
    }
}

/// Live state of a conversion job, polled by the modal.
///
/// Polling instead of Tauri events on purpose: every long job in this app (face
/// detection, recluster, thumbnail repair, description language) already exposes a
/// mutex-guarded progress struct that the frontend polls, and the scan events are the
/// only exception. One more polling source keeps the frontend hook model unchanged.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawJobStatus {
    pub running: bool,
    /// `scanning` while the folder walk runs, `converting` after that, `done` at the end.
    ///
    /// The walk has no total to show, and before this the modal displayed `0 / ?` for
    /// minutes with no sign of life.
    pub phase: String,
    pub done: u64,
    pub total: u64,
    /// File being reported, relative to the root; empty between batches.
    pub current: String,
    /// Unix milliseconds when the job was accepted, so the modal can show how long it
    /// has been running and extrapolate what is left.
    pub started_at_ms: i64,
    pub report: RawJobReport,
}

/// Phases reported by [`RawJobStatus::phase`].
pub const PHASE_SCANNING: &str = "scanning";
pub const PHASE_CONVERTING: &str = "converting";
pub const PHASE_DONE: &str = "done";

/// Run one batch through the sidecar, calling `on_line` for every file reported.
fn run_batch<F>(
    python: &Path,
    script: &Path,
    options: &ConversionOptions,
    pairs: &[(RawCandidate, PathBuf)],
    mut on_line: F,
) -> AppResult<()>
where
    F: FnMut(SidecarLine),
{
    let mut child = silent_command(python)
        .arg(script)
        .arg("--max-side")
        .arg(options.max_side.to_string())
        .arg("--quality")
        .arg(options.quality.to_string())
        .arg("--nef-mode")
        .arg(&options.nef_mode)
        .arg("--overwrite")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Config(format!("cannot start the conversion script: {e}")))?;

    if let Some(stdin) = child.stdin.as_mut() {
        for (candidate, output) in pairs {
            // The protocol is `<source>\t<output>`; a literal tab in a filename would
            // break it, so reject it loudly instead of converting the wrong file.
            if candidate.abs_path.to_string_lossy().contains('\t')
                || output.to_string_lossy().contains('\t')
            {
                return Err(AppError::Config(format!(
                    "path contains a tab character: {}",
                    candidate.rel_path
                )));
            }
            let line = format!(
                "{}\t{}\n",
                candidate.abs_path.to_string_lossy(),
                output.to_string_lossy()
            );
            stdin.write_all(line.as_bytes())?;
        }
    }
    drop(child.stdin.take());

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(Some(parsed)) = parse_sidecar_line(&line) {
                on_line(parsed);
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(AppError::Config(format!(
            "conversion script exited with {status}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Job
// ---------------------------------------------------------------------------

/// Convert every pending candidate of `root`.
///
/// The database is the source of truth for "already done": a candidate with a recorded
/// conversion, unchanged size and mtime, an existing output and the same options is
/// skipped without touching the file at all.
#[allow(clippy::too_many_arguments)]
pub fn run_conversion<F>(
    db_path: &Path,
    root_id: i64,
    root: &Path,
    python: &Path,
    script: &Path,
    options: &ConversionOptions,
    cancel: Option<&AtomicBool>,
    prewalked: Option<Vec<RawCandidate>>,
    mut on_progress: F,
) -> AppResult<RawJobReport>
where
    F: FnMut(&RawJobReport, u64, u64, &str),
{
    let options = options.normalized();
    // The modal already walked the folder to build its report; reusing that list avoids a
    // second traversal over the share. A stale entry is harmless: `needs_conversion`
    // re-checks every file against the database and the sidecar reports a vanished source.
    let candidates = match prewalked {
        Some(candidates) => {
            log::info!(
                "raw conversion: reusing {} candidate(s) for {}",
                candidates.len(),
                root.display()
            );
            candidates
        }
        None => list_candidates(root, cancel),
    };
    log::info!(
        "raw conversion: {} candidate(s) for {}",
        candidates.len(),
        root.display()
    );

    // A cancel during the walk must end the job here: converting a partial list silently
    // would be worse than reporting the cancellation.
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Ok(RawJobReport {
            cancelled: true,
            ..Default::default()
        });
    }
    let existing = crate::db::list_raw_conversions(db_path, root_id)?;
    let by_source: std::collections::HashMap<String, RawConversion> = existing
        .into_iter()
        .map(|conversion| (conversion.source_rel_path.clone(), conversion))
        .collect();

    let mut pending: Vec<(RawCandidate, PathBuf)> = Vec::new();
    let mut report = RawJobReport::default();

    for candidate in candidates {
        let output = output_abs_path(root, &candidate.rel_path);
        if needs_conversion(
            &candidate,
            by_source.get(&candidate.rel_path),
            &options,
            &output,
        ) {
            pending.push((candidate, output));
        } else {
            report.skipped += 1;
        }
    }

    let total = pending.len() as u64;
    let mut done: u64 = 0;
    // Writability is probed here, in the worker thread, once per directory: probing
    // writes a temporary file, and a dead SMB mount blocks that call for minutes — it
    // must never happen on the UI thread nor while merely previewing the folder.
    let mut writable: std::collections::HashMap<PathBuf, bool> = std::collections::HashMap::new();

    for chunk in pending.chunks(BATCH_SIZE) {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }

        let mut sendable: Vec<(RawCandidate, PathBuf)> = Vec::new();
        for (candidate, output) in chunk {
            let original_dir = output_dir_abs_path(root, &candidate.rel_path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf());
            let allowed = *writable
                .entry(original_dir.clone())
                .or_insert_with(|| probe_writable(&original_dir));
            if allowed {
                // The sidecar creates `FrankSherlock-JPG` itself; this only settles the
                // race with a concurrent prune.
                let _ = std::fs::create_dir_all(output_dir_abs_path(root, &candidate.rel_path));
                sendable.push((candidate.clone(), output.clone()));
            } else {
                done += 1;
                report.failed += 1;
                if report.failures.len() < 20 {
                    report.failures.push(format!(
                        "{}: the folder is not writable, so the copy cannot be written next to it",
                        short_path(&candidate.rel_path)
                    ));
                }
            }
        }
        if sendable.is_empty() {
            on_progress(&report, done, total, "");
            continue;
        }

        log::info!(
            "raw conversion: converting {} of {} ({} left in this batch's chunk)",
            sendable.len(),
            total,
            total.saturating_sub(done)
        );
        let chunk_started_done = done;
        let outcome = run_batch(python, script, &options, &sendable, |line| {
            let Some(source) = line.source.clone() else {
                if let Some(fatal) = line.fatal {
                    report
                        .failures
                        .push(format!("conversion support: {}", fatal.trim()));
                }
                return;
            };
            done += 1;
            if !line.ok {
                report.failed += 1;
                if report.failures.len() < 20 {
                    let reason = line.error.unwrap_or_else(|| "unknown error".to_string());
                    report
                        .failures
                        .push(format!("{}: {}", short_path(&source), reason.trim()));
                }
                return;
            }
            if line.skipped.is_some() {
                report.skipped += 1;
                return;
            }

            let rel_source = candidates_rel_path(root, &source);
            let Some(rel_source) = rel_source else {
                report.failed += 1;
                return;
            };
            let output_rel = output_rel_path(&rel_source);
            let bytes = line.bytes.unwrap_or(0);
            log::debug!(
                "raw conversion {}: mode={} {}x{} -> {} ({} bytes)",
                rel_source,
                line.mode.as_deref().unwrap_or("?"),
                line.width.unwrap_or(0),
                line.height.unwrap_or(0),
                line.output.as_deref().unwrap_or("?"),
                bytes
            );
            report.converted += 1;
            report.bytes_out += bytes;
            let mode = line.mode.unwrap_or_default();
            let record = crate::db::RawConversionRecord {
                root_id,
                source_rel_path: &rel_source,
                source_ext: if rel_source.to_ascii_lowercase().ends_with(".nef") {
                    "nef"
                } else {
                    "heic"
                },
                output_rel_path: &output_rel,
                source_size_bytes: file_len(&source),
                source_mtime_ns: file_mtime_ns(&source),
                output_size_bytes: Some(bytes as i64),
                max_side: options.max_side,
                quality: options.quality,
                mode: &mode,
            };
            if let Err(e) = crate::db::upsert_raw_conversion(db_path, &record) {
                log::warn!("raw conversion: could not record {}: {e}", rel_source);
            }
            on_progress(&report, done, total, &rel_source);
        });

        if let Err(e) = outcome {
            report.failed += sendable.len() as u64 - (done - chunk_started_done);
            if report.failures.len() < 20 {
                report.failures.push(format!("batch failed: {e}"));
            }
        }
        on_progress(&report, done, total, "");
    }

    Ok(report)
}

fn short_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn candidates_rel_path(root: &Path, absolute: &str) -> Option<String> {
    let path = Path::new(absolute);
    let relative = path.strip_prefix(root).ok()?;
    Some(normalize_rel_path(&relative.to_string_lossy()))
}

fn file_len(path: &str) -> i64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len() as i64)
        .unwrap_or(0)
}

fn file_mtime_ns(path: &str) -> i64 {
    std::fs::metadata(path)
        .map(|metadata| modified_ns(&metadata))
        .unwrap_or(0)
}

/// Remove an empty `FrankSherlock-JPG` folder left behind by a cancelled or failed job.
///
/// `remove_dir` fails on a non-empty directory, which is exactly the guard we want: it
/// can never delete a single converted copy.
#[allow(dead_code)]
pub fn prune_empty_output_dir(dir: &Path) {
    if dir
        .file_name()
        .map(|name| name == OUTPUT_DIR_NAME)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RawConversion;

    fn candidate(rel: &str, size: u64, mtime: i64) -> RawCandidate {
        RawCandidate {
            rel_path: rel.to_string(),
            abs_path: PathBuf::from("/root").join(rel),
            size_bytes: size,
            mtime_ns: mtime,
            kind: convertible_kind(Path::new(rel)).unwrap_or("heic"),
        }
    }

    fn conversion(rel: &str, size: i64, mtime: i64) -> RawConversion {
        RawConversion {
            id: 1,
            root_id: 1,
            source_rel_path: rel.to_string(),
            source_ext: "heic".to_string(),
            output_rel_path: output_rel_path(rel),
            source_size_bytes: size,
            source_mtime_ns: mtime,
            output_size_bytes: Some(100),
            max_side: 0,
            quality: DEFAULT_QUALITY,
            mode: DEFAULT_NEF_MODE.to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn convertible_kind_recognises_the_formats() {
        assert_eq!(convertible_kind(Path::new("a.heic")), Some("heic"));
        assert_eq!(convertible_kind(Path::new("a.HEIC")), Some("heic"));
        assert_eq!(convertible_kind(Path::new("a.heif")), Some("heic"));
        assert_eq!(convertible_kind(Path::new("a.NEF")), Some("nef"));
        assert_eq!(convertible_kind(Path::new("a.jpg")), None);
        assert_eq!(convertible_kind(Path::new("noext")), None);
    }

    #[test]
    fn output_rel_path_keeps_the_directory_and_marks_the_origin() {
        assert_eq!(
            output_rel_path("Trip/IMG_1234.HEIC"),
            "Trip/FrankSherlock-JPG/IMG_1234__heic.jpg"
        );
        assert_eq!(
            output_rel_path("DSC_0019.NEF"),
            "FrankSherlock-JPG/DSC_0019__nef.jpg"
        );
    }

    #[test]
    fn output_rel_path_never_collides_with_a_native_jpeg() {
        // Same stem, different extension: the copy cannot overwrite `IMG_1234.jpg`.
        assert_ne!(
            output_rel_path("IMG_1234.HEIC"),
            output_rel_path("IMG_1234.jpg")
        );
    }

    #[test]
    fn output_dir_is_the_folder_next_to_the_original() {
        assert_eq!(
            output_dir_rel_path("a/b/photo.nef"),
            "a/b/FrankSherlock-JPG"
        );
        assert_eq!(output_dir_rel_path("photo.nef"), "FrankSherlock-JPG");
    }

    #[test]
    fn needs_conversion_without_a_recorded_conversion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("missing.jpg");
        assert!(needs_conversion(
            &candidate("a.heic", 10, 20),
            None,
            &ConversionOptions::default(),
            &output
        ));
    }

    #[test]
    fn needs_conversion_when_the_source_changed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("a.jpg");
        std::fs::write(&output, b"x").expect("write");
        let existing = conversion("a.heic", 10, 20);
        assert!(!needs_conversion(
            &candidate("a.heic", 10, 20),
            Some(&existing),
            &ConversionOptions::default(),
            &output
        ));
        assert!(needs_conversion(
            &candidate("a.heic", 11, 20),
            Some(&existing),
            &ConversionOptions::default(),
            &output
        ));
        assert!(needs_conversion(
            &candidate("a.heic", 10, 21),
            Some(&existing),
            &ConversionOptions::default(),
            &output
        ));
    }

    #[test]
    fn needs_conversion_when_the_output_vanished() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("gone.jpg");
        let existing = conversion("a.heic", 10, 20);
        assert!(needs_conversion(
            &candidate("a.heic", 10, 20),
            Some(&existing),
            &ConversionOptions::default(),
            &output
        ));
    }

    #[test]
    fn needs_conversion_when_the_options_changed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("a.jpg");
        std::fs::write(&output, b"x").expect("write");
        let existing = conversion("a.heic", 10, 20);
        let bigger = ConversionOptions {
            max_side: 2048,
            ..ConversionOptions::default()
        };
        assert!(needs_conversion(
            &candidate("a.heic", 10, 20),
            Some(&existing),
            &bigger,
            &output
        ));
    }

    #[test]
    fn options_are_clamped() {
        let options = ConversionOptions {
            max_side: 999_999,
            quality: 0,
            nef_mode: "garbage".to_string(),
        }
        .normalized();
        assert_eq!(options.max_side, 20_000);
        assert_eq!(options.quality, 1);
        assert_eq!(options.nef_mode, DEFAULT_NEF_MODE);
    }

    #[test]
    fn sidecar_line_parses_success_and_failure() {
        let ok = parse_sidecar_line(
            r#"{"source":"/a.heic","output":"/b.jpg","ok":true,"mode":"heic","bytes":123}"#,
        )
        .expect("parse")
        .expect("line");
        assert!(ok.ok);
        assert_eq!(ok.bytes, Some(123));
        assert_eq!(ok.mode.as_deref(), Some("heic"));

        let fatal = parse_sidecar_line(r#"{"ok":false,"fatal":"rawpy not available"}"#)
            .expect("parse")
            .expect("line");
        assert!(!fatal.ok);
        assert_eq!(fatal.fatal.as_deref(), Some("rawpy not available"));
    }

    #[test]
    fn sidecar_line_treats_garbage_as_a_fatal_error() {
        let parsed = parse_sidecar_line("Traceback (most recent call last):")
            .expect("parse")
            .expect("line");
        assert!(!parsed.ok);
        assert!(parsed.fatal.is_some());
    }

    #[test]
    fn blank_lines_are_ignored() {
        assert!(parse_sidecar_line("   ").expect("parse").is_none());
    }

    #[test]
    fn filtering_by_directory_keeps_only_the_selected_folders() {
        fn three() -> Vec<RawCandidate> {
            vec![
                candidate("a.heic", 10, 1),
                candidate("Trip/b.heic", 10, 1),
                candidate("Trip/Sub/c.nef", 10, 1),
            ]
        }

        // Empty selection means "everything", which is what the modal sends when all the
        // boxes are checked.
        let mut all = three();
        filter_candidates_by_dirs(&mut all, &[]);
        assert_eq!(all.len(), 3);

        // Only the root itself.
        let mut root_only = three();
        filter_candidates_by_dirs(&mut root_only, &["".to_string()]);
        assert_eq!(root_only.len(), 1);
        assert_eq!(root_only[0].rel_path, "a.heic");

        // A directory is exact: its subdirectories are separate rows in the modal.
        let mut trip = three();
        filter_candidates_by_dirs(&mut trip, &["Trip".to_string()]);
        assert_eq!(trip.len(), 1);
        assert_eq!(trip[0].rel_path, "Trip/b.heic");

        let mut two = three();
        filter_candidates_by_dirs(&mut two, &["".to_string(), "Trip/Sub".to_string()]);
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn list_candidates_skips_the_output_folder_and_tiny_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/FrankSherlock-JPG")).expect("dirs");
        std::fs::write(
            root.join("a/photo.HEIC"),
            vec![0_u8; MIN_RAW_BYTES as usize],
        )
        .expect("write");
        std::fs::write(root.join("a/tiny.nef"), b"tiny").expect("write");
        std::fs::write(root.join("a/FrankSherlock-JPG/photo__heic.jpg"), b"copy").expect("write");
        std::fs::write(root.join("a/notes.jpg"), vec![0_u8; 4096]).expect("write");

        let found = list_candidates(root, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "a/photo.HEIC");
        assert_eq!(found[0].kind, "heic");

        // A flag already set must abort the walk instead of listing everything.
        let cancelled = AtomicBool::new(true);
        assert!(list_candidates(root, Some(&cancelled)).is_empty());
    }

    #[test]
    fn build_report_counts_directories_and_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("x")).expect("dirs");
        let candidates = vec![
            candidate("a.heic", 1000, 1),
            candidate("a.nef", 2000, 1),
            candidate("x/b.heic", 500, 1),
        ];
        // `a.heic` is already converted; the other two are pending.
        let conversions = vec![RawConversion {
            output_rel_path: output_rel_path("a.heic"),
            ..conversion("a.heic", 1000, 1)
        }];

        let report = build_report(root, &candidates, &conversions);
        assert_eq!(report.total_files, 3);
        assert_eq!(report.heic, 2);
        assert_eq!(report.nef, 1);
        assert_eq!(report.pending, 2);
        assert_eq!(report.source_bytes, 3500);
        assert_eq!(report.dirs.len(), 2);
        let root_dir = report
            .dirs
            .iter()
            .find(|d| d.rel_path.is_empty())
            .expect("root dir entry");
        assert_eq!(root_dir.already_converted, 1);
    }

    #[test]
    fn probe_writable_really_tests_the_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(probe_writable(dir.path()));
        assert!(!dir.path().join(".frank-sherlock-write-probe").exists());
        assert!(!probe_writable(&dir.path().join("nope")));
    }
}
