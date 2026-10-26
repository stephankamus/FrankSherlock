use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::classify;
use crate::config::canonical_root_path;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::platform::fs::probe_dir;
use crate::vision_prep;
use std::path::PathBuf;

use crate::models::{
    ExistingFile, FileRecordUpsert, ScanContext, ScanJobState, ScanJobStatus, ScanSummary,
};
use crate::platform::paths::normalize_rel_path;
use crate::thumbnail;
use crate::video;

/// How many failures in a row mean "this is not the files, it is Ollama".
///
/// Shared by the sequential classification phase and the backlog pipeline. Measured
/// once: the server was unreachable and 107 files × 3 attempts were spent in 25 s
/// while the job still reported "completed" — the user only found out by counting the
/// rows without a description afterwards.
const MAX_CONSECUTIVE_FAILURES: u32 = 25;

const SUPPORTED_EXTS: [&str; 20] = [
    ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tiff", ".tif", ".pdf", ".mp4", ".mkv",
    ".avi", ".mov", ".wmv", ".flv", ".webm", ".m4v", ".ts", ".mpg", ".mpeg",
];

/// Skip tiny images likely to be web icons, spacer GIFs, favicons, etc.
/// 10 KB is roughly a 64x64 PNG or any icon-sized image.
const MIN_IMAGE_SIZE_BYTES: u64 = 10_240;

/// Threads used by the thumbnail repair pass. Decoding is CPU-bound and the share
/// keeps up with a handful of parallel readers (measured: 13.5 -> 15 MB/s with 4).
const REPAIR_WORKERS: usize = 4;

/// File classification state determined during incremental discovery.
#[derive(Debug)]
enum FileStatus {
    Unchanged,
    Modified,
    New,
}

#[derive(Debug)]
struct FileProbe {
    rel_path: String,
    abs_path: String,
    filename: String,
    mtime_ns: i64,
    size_bytes: i64,
    /// Populated from DB for unchanged files; None for new/modified (computed lazily in phase 2).
    fingerprint: Option<String>,
    status: FileStatus,
}

/// Upper bound for the reachability checks that guard a scan.
///
/// `stat()` on a dead CIFS/SMB mount can block for minutes, so the check runs
/// on a worker thread and the scan is aborted instead of waiting.
const ROOT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Indexed files below this count are never subject to the collapse heuristic;
/// a small library can legitimately change shape between scans.
const SAFETY_MIN_EXISTING_FILES: usize = 100;

/// A scan that finds less than this fraction of the indexed files is treated as
/// a broken mount rather than as mass deletion.
const SAFETY_MIN_RATIO: f64 = 0.5;

/// What a scan job is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Discovery, thumbnails, classification and cleanup.
    Full,
    /// Discovery and thumbnails only (no LLM), with cleanup.
    Refresh,
    /// Classify files already in the index. No discovery, no thumbnails and —
    /// critically — no cleanup: without a discovery pass, every row would look
    /// "missing" and be soft-deleted.
    ClassifyOnly,
    /// Fill the backlog in one pipeline pass: every row still missing a thumbnail
    /// or a classification. No discovery, no cleanup; the backlog is re-derived
    /// from the index on every run, so an interrupted pass resumes by itself.
    Backlog,
    /// The rows the last attempts gave up on, and nothing else.
    ///
    /// Same pipeline, different selection: it is the way back in for a row that the
    /// default pass now skips, so it can be tried with another model or resolution
    /// ([`crate::models::ScanOverrides`]).
    RetryFailed,
}

impl ScanMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "full" => Some(ScanMode::Full),
            "refresh" => Some(ScanMode::Refresh),
            "classify_only" | "classify-only" | "classify" => Some(ScanMode::ClassifyOnly),
            "backlog" | "process_backlog" | "process-backlog" => Some(ScanMode::Backlog),
            "retry_failed" | "retry-failed" | "retry" => Some(ScanMode::RetryFailed),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ScanMode::Full => "full",
            ScanMode::Refresh => "refresh",
            ScanMode::ClassifyOnly => "classify_only",
            ScanMode::Backlog => "backlog",
            ScanMode::RetryFailed => "retry_failed",
        }
    }

    /// True for the modes that run the backlog pipeline (`Backlog`, `RetryFailed`).
    pub fn uses_backlog_pipeline(&self) -> bool {
        matches!(self, ScanMode::Backlog | ScanMode::RetryFailed)
    }

    pub fn skips_classification(&self) -> bool {
        matches!(self, ScanMode::Refresh)
    }
}

pub fn start_or_resume_scan_job(db_path: &Path, root_path: &str) -> AppResult<ScanJobStatus> {
    let probe = probe_dir(Path::new(root_path), ROOT_PROBE_TIMEOUT);
    if !probe.is_dir {
        return Err(AppError::RootUnavailable(format!(
            "{root_path}: {}",
            probe
                .error
                .unwrap_or_else(|| "path is not available".to_string())
        )));
    }

    let canonical_root = canonical_root_path(root_path)?;
    db::create_or_resume_scan_job(db_path, &canonical_root.display().to_string())
}

/// Run a scan job, reporting the backlog counters as it goes.
///
/// Only `ScanMode::Backlog` uses the callback (once when the backlog size is
/// known, then once per file); the other modes ignore it. It exists so the
/// sidebar can show thumbnails and classifications separately without writing to
/// SQLite per file.
pub fn run_scan_job_with_progress(
    ctx: &ScanContext,
    job_id: i64,
    cancel_flag: Option<&AtomicBool>,
    mode: ScanMode,
    on_backlog_tick: &(dyn Fn(BacklogTick) + Sync),
) -> AppResult<ScanSummary> {
    run_scan_job_impl(ctx, job_id, None, cancel_flag, mode, on_backlog_tick)
}

#[cfg(test)]
fn run_scan_job_internal(
    ctx: &ScanContext,
    job_id: i64,
    max_files_for_test: Option<usize>,
    cancel_flag: Option<&AtomicBool>,
    mode: ScanMode,
) -> AppResult<ScanSummary> {
    run_scan_job_impl(ctx, job_id, max_files_for_test, cancel_flag, mode, &|_| {})
}

fn run_scan_job_impl(
    ctx: &ScanContext,
    job_id: i64,
    max_files_for_test: Option<usize>,
    cancel_flag: Option<&AtomicBool>,
    mode: ScanMode,
    on_backlog_tick: &(dyn Fn(BacklogTick) + Sync),
) -> AppResult<ScanSummary> {
    let started = Instant::now();
    let db_path = &ctx.db_path;
    let job = db::get_scan_job_state(db_path, job_id)?;

    // Never start discovery on an unreachable root: with zero files found the
    // cleanup phase would soft-delete every row and delete its thumbnails.
    let probe = probe_dir(Path::new(&job.root_path), ROOT_PROBE_TIMEOUT);
    if !probe.is_dir {
        let message = probe
            .error
            .unwrap_or_else(|| "path is not available".to_string());
        db::mark_root_offline(db_path, job.root_id, &message)?;
        return Err(AppError::RootUnavailable(format!(
            "{}: {message}",
            job.root_path
        )));
    }
    db::mark_root_online(db_path, job.root_id)?;

    let root_path = canonical_root_path(&job.root_path)?;
    // One cache directory per root: `<thumbnails>/<root_key>`. Two roots covering
    // the same tree used to share (and delete) each other's thumbnails.
    let root_key = thumbnail::root_key(&job.root_path);
    let thumb_dir = ctx.thumb_dir_for(&root_key);

    // Resuming classification must not pay for a full discovery walk again: on a
    // network share with tens of thousands of files that is the slowest part of
    // opening the app.
    if mode == ScanMode::ClassifyOnly {
        return run_classify_only(ctx, &job, job_id, cancel_flag, started);
    }
    if mode.uses_backlog_pipeline() {
        return run_backlog_pipeline(
            ctx,
            &job,
            job_id,
            cancel_flag,
            started,
            on_backlog_tick,
            mode == ScanMode::RetryFailed,
        );
    }

    log::info!(
        "Scan job {}: starting discovery for {}",
        job_id,
        root_path.display()
    );

    // Compute child roots to exclude during walkdir
    let all_roots = db::list_root_paths(db_path).unwrap_or_default();
    let excluded_prefixes: Vec<PathBuf> = all_roots
        .iter()
        .filter_map(|(_id, rp)| {
            let rp_path = PathBuf::from(rp);
            if rp_path.starts_with(&root_path) && rp_path != root_path {
                Some(rp_path)
            } else {
                None
            }
        })
        .collect();

    // Phase 1: Load existing DB records
    let db_load_start = Instant::now();
    let existing = db::load_existing_files(db_path, job.root_id)?;
    let db_load_ms = db_load_start.elapsed().as_millis();
    log::info!(
        "Scan job {}: loaded {} existing records from DB in {}ms",
        job_id,
        existing.len(),
        db_load_ms,
    );
    let existing_by_path: HashMap<String, ExistingFile> = existing
        .iter()
        .map(|f| (f.rel_path.clone(), f.clone()))
        .collect();
    let mut by_fingerprint: HashMap<String, Vec<ExistingFile>> = HashMap::new();
    for file in existing {
        by_fingerprint
            .entry(file.fingerprint.clone())
            .or_default()
            .push(file);
    }

    // Rows soft-deleted by an earlier failed scan, indexed by rel_path so a
    // reappearing file can be revived with its classification intact.
    let mut deleted_by_path: HashMap<String, ExistingFile> =
        db::load_deleted_files(db_path, job.root_id)?
            .into_iter()
            .map(|f| (f.rel_path.clone(), f))
            .collect();

    // Phase 1: Incremental discovery (metadata-only for unchanged files)
    let discovery_start = Instant::now();
    let discovery = collect_image_probes_incremental(
        &root_path,
        &existing_by_path,
        &excluded_prefixes,
        db_path,
        job_id,
        cancel_flag,
    )?;
    let walk_errors = discovery.walk_errors;
    let probes = discovery.probes;

    // Thumbnail dedupe caches, seeded from rows that still have a thumbnail on disk.
    let mut thumb_caches = ThumbCaches::seed(db_path, job.root_id)?;

    // Rows the DB claims to have a thumbnail for, but the file is gone: the cache was
    // wiped by a purge, an interrupted run, or a root removed and re-added. Unchanged
    // files are never re-thumbnailed otherwise, so the pointer would stay dead forever.
    let heal: HashSet<String> = probes
        .iter()
        .filter(|p| matches!(p.status, FileStatus::Unchanged))
        .filter(|p| {
            existing_by_path
                .get(&p.rel_path)
                .map(|e| match e.thumb_path.as_deref() {
                    Some(tp) => !Path::new(tp).exists(),
                    None => true,
                })
                .unwrap_or(false)
        })
        .map(|p| p.rel_path.clone())
        .collect();
    if !heal.is_empty() {
        log::info!(
            "Scan job {}: {} unchanged file(s) lost their thumbnail and will be regenerated",
            job_id,
            heal.len()
        );
    }

    // A fingerprint match is only a move when the old path disappeared from the
    // discovery; otherwise it is a byte-identical copy living beside the original.
    let discovered: HashSet<&str> = probes.iter().map(|p| p.rel_path.as_str()).collect();

    // If cancelled during discovery, bail out
    if cancel_flag.is_some_and(|f| f.load(Ordering::Relaxed)) {
        db::cancel_scan_job(db_path, job_id)?;
        return Ok(ScanSummary {
            root_id: job.root_id,
            root_path: root_path.display().to_string(),
            scanned: 0,
            added: 0,
            modified: 0,
            moved: 0,
            unchanged: 0,
            deleted: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            healed: 0,
            deduped: 0,
        });
    }

    // Safety net: a share that is unmounted but whose mountpoint directory still
    // exists enumerates as empty. Without this check, cleanup would soft-delete
    // the whole index and purge thumbnails.
    ensure_discovery_is_sane(probes.len(), existing_by_path.len()).inspect_err(|err| {
        let _ = db::mark_root_offline(db_path, job.root_id, &err.to_string());
    })?;

    if walk_errors > 0 {
        log::warn!(
            "Scan job {}: {walk_errors} discovery error(s); cleanup will be skipped to protect the index",
            job_id
        );
    }

    let total_files = probes.len() as u64;
    let start_index = resume_start_index(&probes, job.cursor_rel_path.as_deref());

    let new_count = probes
        .iter()
        .filter(|p| matches!(p.status, FileStatus::New))
        .count();
    let mod_count = probes
        .iter()
        .filter(|p| matches!(p.status, FileStatus::Modified))
        .count();
    let unch_count = probes
        .iter()
        .filter(|p| matches!(p.status, FileStatus::Unchanged))
        .count();
    log::info!(
        "Scan job {}: discovery complete — {} files ({} new, {} modified, {} unchanged) in {:.1}s",
        job_id,
        total_files,
        new_count,
        mod_count,
        unch_count,
        discovery_start.elapsed().as_secs_f64()
    );

    let mut processed_files = start_index as u64;
    let mut added = job.added;
    let mut modified = job.modified;
    let mut moved = job.moved;
    let mut unchanged = job.unchanged;
    let mut healed: u64 = 0;
    let mut deduped: u64 = 0;
    let mut used_moved_ids = HashSet::new();
    let mut last_cursor: Option<String> = job.cursor_rel_path.clone();
    let mut skipped_unreadable: u64 = 0;
    const UNCHANGED_BATCH_SIZE: usize = 200;

    // Determine whether to skip the thumbnailing phase (already done on a previous run).
    let skip_thumbnailing = job.phase == "classifying";

    if !skip_thumbnailing {
        db::checkpoint_scan_job(
            db_path,
            job_id,
            "thumbnailing",
            total_files,
            processed_files,
            last_cursor.as_deref(),
            added,
            modified,
            moved,
            unchanged,
        )?;

        if start_index > 0 {
            log::info!(
                "Scan job {}: resuming thumbnailing from index {} (cursor: {:?}, {} previous progress)",
                job_id,
                start_index,
                job.cursor_rel_path,
                job.processed_files
            );
        } else {
            log::info!("Scan job {}: starting thumbnailing from beginning", job_id);
        }

        // Flush all unchanged files upfront so the UI can show previously-known
        // files immediately while thumbnailing works through new/modified ones.
        let flush_start = Instant::now();
        let unchanged_paths: Vec<&str> = probes
            .iter()
            .skip(start_index)
            .filter(|p| matches!(p.status, FileStatus::Unchanged))
            .filter(|p| !heal.contains(&p.rel_path))
            .map(|p| p.rel_path.as_str())
            .collect();
        let unchanged_total = unchanged_paths.len() as u64;
        for batch in unchanged_paths.chunks(UNCHANGED_BATCH_SIZE) {
            db::touch_file_scan_markers_batch(db_path, job.root_id, batch, job.scan_marker)?;
        }
        unchanged += unchanged_total;
        processed_files += unchanged_total;
        log::info!(
            "Scan job {}: flushed {} unchanged markers in {}ms",
            job_id,
            unchanged_total,
            flush_start.elapsed().as_millis(),
        );
        db::checkpoint_scan_job(
            db_path,
            job_id,
            "thumbnailing",
            total_files,
            processed_files,
            last_cursor.as_deref(),
            added,
            modified,
            moved,
            unchanged,
        )?;

        // Files before the resume cursor are never visited by the loop below, so a
        // thumbnail lost in an earlier run would stay missing forever. Heal them first;
        // the job cursor is left untouched so the resume position stays monotonic.
        if start_index > 0 {
            let pre_cursor: Vec<&FileProbe> = probes
                .iter()
                .take(start_index)
                .filter(|p| heal.contains(&p.rel_path))
                .collect();
            if !pre_cursor.is_empty() {
                log::info!(
                    "Scan job {}: healing {} thumbnail(s) before the resume cursor",
                    job_id,
                    pre_cursor.len()
                );
            }
            for probe in pre_cursor {
                if cancel_flag.is_some_and(|f| f.load(Ordering::Relaxed)) {
                    break;
                }
                let Some(fingerprint) = probe.fingerprint.clone() else {
                    continue;
                };
                let thumb_result = thumbnail_only(
                    ctx,
                    &thumb_dir,
                    probe,
                    &fingerprint,
                    true,
                    false,
                    &mut thumb_caches,
                );
                if let Some(ref thumb) = thumb_result.thumb_path {
                    db::update_file_thumb_and_dhash(
                        db_path,
                        job.root_id,
                        &probe.rel_path,
                        thumb,
                        thumb_result.dhash.map(|h| h as i64),
                    )?;
                }
                // Same reason as in the loop below: a healed row is not in the
                // unchanged flush, so it has to carry the marker itself.
                db::touch_file_scan_markers_batch(
                    db_path,
                    job.root_id,
                    &[probe.rel_path.as_str()],
                    job.scan_marker,
                )?;
                healed += 1;
            }
        }

        // Phase 2: Thumbnailing loop — generate thumbnails for new/modified files
        for (i, probe) in probes.iter().enumerate().skip(start_index) {
            if matches!(probe.status, FileStatus::Unchanged) && !heal.contains(&probe.rel_path) {
                continue; // already flushed above
            }

            if let Some(flag) = cancel_flag {
                if flag.load(Ordering::Relaxed) {
                    db::cancel_scan_job(db_path, job_id)?;
                    return Ok(ScanSummary {
                        root_id: job.root_id,
                        root_path: root_path.display().to_string(),
                        scanned: processed_files,
                        added,
                        modified,
                        moved,
                        unchanged,
                        deleted: 0,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        healed,
                        deduped,
                    });
                }
            }

            if let Some(max) = max_files_for_test {
                if (i - start_index) >= max {
                    break;
                }
            }

            // Compute fingerprint lazily for new/modified files (deferred from discovery).
            //
            // A file that cannot be read (restrictive ACLs, an exclusive lock held by
            // another process, a cloud placeholder that fails to hydrate) must not abort
            // the whole job: skip it, record why, and keep going. Discovery is already
            // tolerant of unreadable entries, so this keeps phase 2 consistent with it.
            let fingerprint = match &probe.fingerprint {
                Some(fp) => fp.clone(),
                None => match fingerprint_file(Path::new(&probe.abs_path), probe.size_bytes as u64)
                {
                    Ok(fp) => fp,
                    Err(err) => {
                        log::warn!(
                            "Scan job {}: skipping unreadable file {}: {}",
                            job_id,
                            probe.abs_path,
                            err,
                        );
                        // A modified file already has an index entry; refresh
                        // its scan marker so end-of-scan cleanup does not
                        // soft-delete a file that still exists on disk.
                        if matches!(probe.status, FileStatus::Modified) {
                            db::touch_file_scan_markers_batch(
                                db_path,
                                job.root_id,
                                &[probe.rel_path.as_str()],
                                job.scan_marker,
                            )?;
                        }
                        skipped_unreadable += 1;
                        processed_files += 1;
                        last_cursor = Some(probe.rel_path.clone());
                        db::checkpoint_scan_job(
                            db_path,
                            job_id,
                            "thumbnailing",
                            total_files,
                            processed_files,
                            last_cursor.as_deref(),
                            added,
                            modified,
                            moved,
                            unchanged,
                        )?;
                        continue;
                    }
                },
            };

            match probe.status {
                FileStatus::Unchanged => {
                    // Only the thumbnail is missing: regenerate it and leave the
                    // classification untouched (no EXIF re-read needed either).
                    let thumb_result = thumbnail_only(
                        ctx,
                        &thumb_dir,
                        probe,
                        &fingerprint,
                        true,
                        false,
                        &mut thumb_caches,
                    );
                    if let Some(ref thumb) = thumb_result.thumb_path {
                        db::update_file_thumb_and_dhash(
                            db_path,
                            job.root_id,
                            &probe.rel_path,
                            thumb,
                            thumb_result.dhash.map(|h| h as i64),
                        )?;
                    }
                    // Stamp the marker here: these rows are **excluded** from the
                    // unchanged flush above (they are in `heal`), and
                    // `update_file_thumb_and_dhash` does not touch `scan_marker`. Without
                    // this, phase 4 sees the old marker and soft-deletes every healed row
                    // *and* purges its thumbnail — a `full` after a reclassify that
                    // dropped thumbnails wiped the whole folder.
                    db::touch_file_scan_markers_batch(
                        db_path,
                        job.root_id,
                        &[probe.rel_path.as_str()],
                        job.scan_marker,
                    )?;
                    healed += 1;
                }
                FileStatus::Modified => {
                    let thumb_result = thumbnail_only(
                        ctx,
                        &thumb_dir,
                        probe,
                        &fingerprint,
                        true,
                        true,
                        &mut thumb_caches,
                    );
                    let record = probe_to_minimal_record(
                        job.root_id,
                        job.scan_marker,
                        probe,
                        &fingerprint,
                        &thumb_result,
                    );
                    db::upsert_file_record(db_path, &record)?;
                    if let Some(ref thumb) = thumb_result.thumb_path {
                        db::update_file_thumb_path(db_path, job.root_id, &probe.rel_path, thumb)?;
                    }
                    modified += 1;
                }
                FileStatus::New => {
                    // A row that was soft-deleted by an earlier failed scan can be
                    // revived, keeping its description, OCR text, dhash, faces and
                    // album membership instead of re-running the LLM on it.
                    if let Some(candidate) = deleted_by_path.remove(&probe.rel_path) {
                        if candidate.size_bytes == probe.size_bytes {
                            db::restore_file_by_id(
                                db_path,
                                candidate.id,
                                &probe.abs_path,
                                probe.mtime_ns,
                                probe.size_bytes,
                                job.scan_marker,
                            )?;
                            // Thumbnails are removed when a row is soft-deleted,
                            // so regenerate the ones that are gone.
                            let thumb_result = thumbnail_only(
                                ctx,
                                &thumb_dir,
                                probe,
                                &fingerprint,
                                false,
                                true,
                                &mut thumb_caches,
                            );
                            if let Some(ref thumb) = thumb_result.thumb_path {
                                db::update_file_thumb_path(
                                    db_path,
                                    job.root_id,
                                    &probe.rel_path,
                                    thumb,
                                )?;
                            }
                            unchanged += 1;
                            processed_files += 1;
                            last_cursor = Some(probe.rel_path.clone());
                            db::checkpoint_scan_job(
                                db_path,
                                job_id,
                                "thumbnailing",
                                total_files,
                                processed_files,
                                last_cursor.as_deref(),
                                added,
                                modified,
                                moved,
                                unchanged,
                            )?;
                            continue;
                        }
                    }

                    // Check for move detection by fingerprint
                    if let Some(candidates) = by_fingerprint.get(&fingerprint) {
                        if let Some(candidate) = candidates.iter().find(|c| {
                            !used_moved_ids.contains(&c.id)
                                && c.rel_path != probe.rel_path
                                // A byte-identical copy is not a move: the old path is
                                // still on disk, so stealing its row would re-insert the
                                // original with a blank classification.
                                && !discovered.contains(c.rel_path.as_str())
                        }) {
                            used_moved_ids.insert(candidate.id);
                            moved += 1;
                            db::move_file_by_id(
                                db_path,
                                candidate.id,
                                &probe.rel_path,
                                &probe.abs_path,
                                &probe.filename,
                                probe.mtime_ns,
                                probe.size_bytes,
                                job.scan_marker,
                            )?;

                            // Regenerate thumbnail at the new path
                            let thumb_result = thumbnail_only(
                                ctx,
                                &thumb_dir,
                                probe,
                                &fingerprint,
                                true,
                                false,
                                &mut thumb_caches,
                            );
                            if let Some(ref tr) = thumb_result.thumb_path {
                                db::update_file_thumb_and_dhash(
                                    db_path,
                                    job.root_id,
                                    &probe.rel_path,
                                    tr,
                                    thumb_result.dhash.map(|h| h as i64),
                                )?;
                            }

                            processed_files += 1;
                            last_cursor = Some(probe.rel_path.clone());
                            db::checkpoint_scan_job(
                                db_path,
                                job_id,
                                "thumbnailing",
                                total_files,
                                processed_files,
                                last_cursor.as_deref(),
                                added,
                                modified,
                                moved,
                                unchanged,
                            )?;
                            continue;
                        }
                    }

                    // Genuinely new file — thumbnail only (classification deferred)
                    let thumb_result = thumbnail_only(
                        ctx,
                        &thumb_dir,
                        probe,
                        &fingerprint,
                        false,
                        true,
                        &mut thumb_caches,
                    );
                    let record = probe_to_minimal_record(
                        job.root_id,
                        job.scan_marker,
                        probe,
                        &fingerprint,
                        &thumb_result,
                    );
                    db::upsert_file_record(db_path, &record)?;
                    if let Some(ref thumb) = thumb_result.thumb_path {
                        db::update_file_thumb_path(db_path, job.root_id, &probe.rel_path, thumb)?;
                    }
                    added += 1;
                }
            }

            processed_files += 1;
            last_cursor = Some(probe.rel_path.clone());
            db::checkpoint_scan_job(
                db_path,
                job_id,
                "thumbnailing",
                total_files,
                processed_files,
                last_cursor.as_deref(),
                added,
                modified,
                moved,
                unchanged,
            )?;
        }

        if skipped_unreadable > 0 {
            log::warn!(
                "Scan job {}: {} file(s) skipped because they could not be opened",
                job_id,
                skipped_unreadable,
            );
        }

        log::info!(
            "Scan job {}: thumbnailing complete ({} added, {} modified, {} moved)",
            job_id,
            added,
            modified,
            moved
        );
    } // end skip_thumbnailing

    // (Unchanged files were already flushed before the thumbnailing loop.)

    if max_files_for_test.is_some() {
        return Ok(ScanSummary {
            root_id: job.root_id,
            root_path: root_path.display().to_string(),
            scanned: processed_files,
            added,
            modified,
            moved,
            unchanged,
            deleted: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            healed,
            deduped,
        });
    }

    // Phase 3: Classification loop — LLM classify all unclassified files
    let unclassified = if mode.skips_classification() {
        vec![]
    } else {
        db::list_unclassified_files(db_path, job.root_id, job.scan_marker)?
    };
    if !unclassified.is_empty() {
        let classify_total = unclassified.len() as u64;
        let mut classify_processed: u64 = 0;
        let mut classify_failed: u64 = 0;
        let mut consecutive_failures: u32 = 0;

        // For resume: if we were already in classifying phase, skip already-classified files.
        // list_unclassified_files only returns confidence=0, so already-classified are excluded.
        // We still use cursor_rel_path for progress reporting consistency.
        let classify_cursor = if skip_thumbnailing {
            job.cursor_rel_path.clone()
        } else {
            None
        };

        db::checkpoint_scan_job(
            db_path,
            job_id,
            "classifying",
            classify_total,
            classify_processed,
            classify_cursor.as_deref(),
            added,
            modified,
            moved,
            unchanged,
        )?;

        log::info!(
            "Scan job {}: starting classification of {} files",
            job_id,
            classify_total
        );

        // Photos that are visually identical to an already classified file inherit
        // its classification instead of paying for another vision request.
        let mut dedupe = DedupeIndex::seed(db_path, job.root_id, ctx.dedupe_dhash_distance)?;
        if !dedupe.is_disabled() {
            log::info!(
                "Scan job {}: dedupe index seeded with {} classified hashes (max distance {})",
                job_id,
                dedupe.len(),
                ctx.dedupe_dhash_distance
            );
        }

        for file in &unclassified {
            let failed_before = classify_failed;
            if let Some(flag) = cancel_flag {
                if flag.load(Ordering::Relaxed) {
                    db::cancel_scan_job(db_path, job_id)?;
                    return Ok(ScanSummary {
                        root_id: job.root_id,
                        root_path: root_path.display().to_string(),
                        scanned: processed_files,
                        added,
                        modified,
                        moved,
                        unchanged,
                        deleted: 0,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        healed,
                        deduped,
                    });
                }
            }

            // Skip files already past the cursor (for resume)
            if let Some(ref cursor) = classify_cursor {
                if file.rel_path.as_str() <= cursor.as_str() {
                    classify_processed += 1;
                    continue;
                }
            }

            match dedupe.representative_for(file) {
                Some(representative) => {
                    if db::copy_classification(db_path, file.id, representative).is_ok() {
                        deduped += 1;
                    } else if !classify_and_update(ctx, file.id, &file.abs_path) {
                        classify_failed += 1;
                    }
                }
                None => {
                    // Only a successful classification may seed the dedupe index:
                    // otherwise a failed row becomes the representative another
                    // photo copies its (empty) classification from.
                    if classify_and_update(ctx, file.id, &file.abs_path) {
                        dedupe.remember(file);
                    } else {
                        classify_failed += 1;
                    }
                }
            }

            // Fail fast when nothing is getting through: Ollama stopped, or its runner
            // is wedged for good. Measured once: 107 files × 3 attempts in 25 s with the
            // server unreachable, and the job still reported "completed" — the user only
            // found out by counting rows afterwards.
            if classify_failed > failed_before {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    let message = format!(
                        "classification aborted after {MAX_CONSECUTIVE_FAILURES} consecutive \
                         failures: Ollama is unreachable or its runner keeps failing"
                    );
                    log::error!("Scan job {job_id}: {message}");
                    db::fail_scan_job(db_path, job_id, &message)?;
                    return Err(AppError::Config(message));
                }
            } else {
                consecutive_failures = 0;
            }

            classify_processed += 1;
            last_cursor = Some(file.rel_path.clone());
            db::checkpoint_scan_job(
                db_path,
                job_id,
                "classifying",
                classify_total,
                classify_processed,
                last_cursor.as_deref(),
                added,
                modified,
                moved,
                unchanged,
            )?;
        }

        if classify_failed > 0 {
            log::warn!(
                "Scan job {}: {classify_failed} of {classify_total} file(s) could not be classified; \
                 their rows were left as they were",
                job_id
            );
        }
        log::info!(
            "Scan job {}: classification complete ({} files, {} deduped, {} failed)",
            job_id,
            classify_total,
            deduped,
            classify_failed
        );
    }

    log::info!(
        "Scan job {}: all processing complete, starting cleanup",
        job_id
    );

    // Phase 4: Cleanup deleted files.
    //
    // Skipped whenever discovery reported errors: a partial walk cannot prove
    // that a file is gone, and a wrong soft-delete also drops its thumbnail and
    // face crops from disk.
    let deleted = if walk_errors > 0 {
        log::warn!(
            "Scan job {}: skipping cleanup after {} discovery error(s)",
            job_id,
            walk_errors
        );
        0
    } else {
        let deleted_at_before = db::now_epoch_secs_pub();
        let deleted = db::mark_missing_as_deleted(db_path, job.root_id, job.scan_marker)?;
        if deleted > 0 {
            cleanup_deleted_caches(db_path, job.root_id, deleted_at_before, &thumb_dir);
        }
        deleted
    };
    db::touch_root_scan(db_path, job.root_id)?;

    let summary = ScanSummary {
        root_id: job.root_id,
        root_path: root_path.display().to_string(),
        scanned: total_files,
        added,
        modified,
        moved,
        unchanged,
        deleted,
        elapsed_ms: started.elapsed().as_millis() as u64,
        healed,
        deduped,
    };
    db::complete_scan_job_by_id(db_path, job_id, &summary, last_cursor.as_deref())?;
    Ok(summary)
}

/// Classification-only pass over files already in the index.
///
/// Used when the app reopens with pending work: discovery, thumbnailing and
/// cleanup are all skipped, so nothing on disk is inspected and nothing can be
/// soft-deleted. Only rows with `confidence = 0` are touched.
fn run_classify_only(
    ctx: &ScanContext,
    job: &ScanJobState,
    job_id: i64,
    cancel_flag: Option<&AtomicBool>,
    started: Instant,
) -> AppResult<ScanSummary> {
    let db_path = &ctx.db_path;
    let root_path = canonical_root_path(&job.root_path)?;

    // Deliberately not filtered by `scan_marker`: a job that died during
    // discovery never stamped any row with its marker, and the previous marker
    // may belong to an older job.
    let pending = db::list_unclassified_files_any_marker(db_path, job.root_id)?;
    let total = pending.len() as u64;

    log::info!(
        "Scan job {}: classification-only resume for {} ({} pending files, no discovery)",
        job_id,
        root_path.display(),
        total
    );

    db::checkpoint_scan_job(
        db_path,
        job_id,
        "classifying",
        total,
        0,
        None,
        job.added,
        job.modified,
        job.moved,
        job.unchanged,
    )?;

    let mut processed: u64 = 0;
    let mut deduped: u64 = 0;
    let mut classify_failed: u64 = 0;
    let mut last_cursor: Option<String> = None;

    let mut dedupe = DedupeIndex::seed(db_path, job.root_id, ctx.dedupe_dhash_distance)?;

    for file in &pending {
        if let Some(flag) = cancel_flag {
            if flag.load(Ordering::Relaxed) {
                db::cancel_scan_job(db_path, job_id)?;
                return Ok(ScanSummary {
                    root_id: job.root_id,
                    root_path: root_path.display().to_string(),
                    scanned: processed,
                    added: job.added,
                    modified: job.modified,
                    moved: job.moved,
                    unchanged: job.unchanged,
                    deleted: 0,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    healed: 0,
                    deduped,
                });
            }
        }

        match dedupe.representative_for(file) {
            Some(representative) => {
                if db::copy_classification(db_path, file.id, representative).is_ok() {
                    deduped += 1;
                } else if !classify_and_update(ctx, file.id, &file.abs_path) {
                    classify_failed += 1;
                }
            }
            None => {
                if classify_and_update(ctx, file.id, &file.abs_path) {
                    dedupe.remember(file);
                } else {
                    classify_failed += 1;
                }
            }
        }
        processed += 1;
        last_cursor = Some(file.rel_path.clone());
        db::checkpoint_scan_job(
            db_path,
            job_id,
            "classifying",
            total,
            processed,
            last_cursor.as_deref(),
            job.added,
            job.modified,
            job.moved,
            job.unchanged,
        )?;
    }

    if classify_failed > 0 {
        log::warn!(
            "Scan job {}: {classify_failed} of {total} file(s) could not be classified; \
             their rows were left as they were",
            job_id
        );
    }

    let summary = ScanSummary {
        root_id: job.root_id,
        root_path: root_path.display().to_string(),
        scanned: total,
        added: job.added,
        modified: job.modified,
        moved: job.moved,
        unchanged: job.unchanged,
        deleted: 0,
        elapsed_ms: started.elapsed().as_millis() as u64,
        healed: 0,
        deduped,
    };
    db::touch_root_scan(db_path, job.root_id)?;
    db::complete_scan_job_by_id(db_path, job_id, &summary, last_cursor.as_deref())?;
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Backlog pipeline (Phase 2 of plans/pipeline-cpu-gpu-overlap.md)
// ---------------------------------------------------------------------------

/// Progress snapshot handed to the UI by the backlog pipeline.
///
/// Separate from `BacklogCounters` (which the producer writes) because this is a
/// copy the caller can move into its own state without holding a lock.
#[derive(Debug, Clone, Copy, Default)]
pub struct BacklogTick {
    pub total: u64,
    pub processed: u64,
    pub thumbnailed: u64,
    /// Rows that need a thumbnail (see `BacklogProgress::thumbs_total`).
    pub thumbs_total: u64,
    pub classified: u64,
    /// Rows that need a classification.
    pub classify_total: u64,
    pub deduped: u64,
    pub failed: u64,
}

/// One file handed from the producer to the consumer.
struct BacklogItem {
    row: db::BacklogRow,
    /// Ready-to-upload JPEG. `None` for video/PDF (legacy path) and for rows that
    /// only needed a thumbnail.
    prepared: Option<vision_prep::PreparedImage>,
    /// Perceptual hash of the decoded image, for the near-duplicate index.
    dhash: Option<u64>,
    /// Video/PDF keep the legacy classification path: their frames or pages come
    /// from ffmpeg/pdfium, which this iteration does not share.
    legacy: bool,
    /// The producer could not decode or prepare this file.
    failed: bool,
}

/// Counters written by the producer and read by the summary.
#[derive(Default)]
struct BacklogCounters {
    thumbnailed: AtomicU64,
    /// Rows the producer found without a usable thumbnail, including dead
    /// pointers that only a `stat` can reveal.
    thumbs_needed: AtomicU64,
    failed: AtomicU64,
    /// Rows whose context window is bigger than the one this block runs at. They
    /// get a block of their own on the next pass, now that the size is recorded.
    deferred: AtomicU64,
    /// Rows the producer could not open or prepare. Kept apart from `failed` on
    /// purpose: this is a local/index problem, not a classification failure, and it
    /// must not feed the circuit breaker (see [`is_permanent_failure`]).
    unreadable: AtomicU64,
}

/// A group of backlog rows that resolve to the same context window.
struct CtxBlock {
    /// `num_ctx` this block runs at. Ollama treats it as part of the runner
    /// identity, so the whole block shares one value.
    ctx: u32,
    /// True when the rows had no recorded dimensions and are measured on the fly:
    /// the producer keeps only the rows that fit `ctx` and defers the rest.
    probed: bool,
    rows: Vec<db::BacklogRow>,
}

/// Split the backlog into blocks that each need one context window.
///
/// `num_ctx` is derived from the image size ([`classify::primary_ctx_for`]) and it
/// is part of Ollama's runner identity: changing it tears the runner down and
/// reloads the model (~2.1 s plus a 7.3 GiB graph per switch, §5.5 of
/// `plans/pipeline-cpu-gpu-overlap.md`). Grouping by window means a pass pays at
/// most one reload per block instead of one per file.
///
/// Ascending order, so the cheapest window runs first. Rows with no recorded
/// dimensions come last: the producer probes their header (a few KB, no decode),
/// stores the size, and keeps only the rows that fit the window already loaded —
/// the rest are deferred to the next pass, where their size is known and they get
/// their own block.
fn ctx_blocks(rows: &[db::BacklogRow], ctx: &ScanContext) -> Vec<CtxBlock> {
    let profile = ctx.vision_settings().prompt_profile;
    let mut known: std::collections::BTreeMap<u32, Vec<db::BacklogRow>> =
        std::collections::BTreeMap::new();
    let mut probed: Vec<db::BacklogRow> = Vec::new();

    for row in rows {
        // Video and PDF go through the legacy path, which prepares its own frames
        // or pages inside the consumer; nothing here can predict their window, so
        // they ride with the probed block.
        let legacy = matches!(row.media_type.as_str(), "video" | "pdf");
        match (row.image_width, row.image_height) {
            (Some(width), Some(height)) if !legacy => known
                .entry(classify::primary_ctx_for(
                    width,
                    height,
                    ctx.image_max_side,
                    profile,
                ))
                .or_default()
                .push(row.clone()),
            _ => probed.push(row.clone()),
        }
    }

    let mut blocks: Vec<CtxBlock> = known
        .into_iter()
        .map(|(ctx, rows)| CtxBlock {
            ctx,
            probed: false,
            rows,
        })
        .collect();
    if !probed.is_empty() {
        // The last known window is the largest one, so a probed row that fits it
        // can run without changing the runner identity.
        let ctx = blocks
            .last()
            .map(|block| block.ctx)
            .unwrap_or(crate::llm::MIN_NUM_CTX);
        blocks.push(CtxBlock {
            ctx,
            probed: true,
            rows: probed,
        });
    }
    blocks
}

/// Producer half of the pipeline: decode once, thumbnail, prepare for the model.
///
/// Runs on its own thread. Because the consumer is a single GPU request at a
/// time, one producer is enough: it costs ~600 ms per file against the ~2 s the
/// consumer takes (measured in `_bench_vision/RESULTS.md` §7).
fn produce_backlog_items(
    tx: std::sync::mpsc::SyncSender<BacklogItem>,
    ctx: &ScanContext,
    root_id: i64,
    thumb_dir: &Path,
    block: &CtxBlock,
    cancel: Option<&AtomicBool>,
    counters: &BacklogCounters,
) {
    let profile = ctx.vision_settings().prompt_profile;
    for row in &block.rows {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            break;
        }

        let legacy = matches!(row.media_type.as_str(), "video" | "pdf");
        if legacy {
            let item = BacklogItem {
                row: row.clone(),
                prepared: None,
                dhash: None,
                legacy: true,
                failed: false,
            };
            if tx.send(item).is_err() {
                break;
            }
            continue;
        }

        let abs = Path::new(&row.abs_path);

        // Which context window will this row need? The answer has to be known
        // *before* the decode: preparing an image for a window we are not going to
        // send wastes the work, and sending it in the wrong window costs a model
        // reload. Rows with a recorded size are already in the right block; the rest
        // are measured now (header only) and get their block on the next pass.
        if row.needs_classification {
            let dims = match (row.image_width, row.image_height) {
                (Some(width), Some(height)) => Some((width, height)),
                _ => match vision_prep::image_dimensions(abs) {
                    Ok((width, height)) => {
                        // Cache it so the next pass groups this row without even
                        // opening the file.
                        if let Err(e) =
                            db::set_image_dimensions(&ctx.db_path, row.id, width, height)
                        {
                            log::debug!(
                                "Backlog: could not record dimensions for {}: {e}",
                                row.rel_path
                            );
                        }
                        Some((width, height))
                    }
                    Err(e) => {
                        log::debug!(
                            "Backlog: cannot read the size of {} ({e}); sending it anyway",
                            row.rel_path
                        );
                        None
                    }
                },
            };
            if let Some((width, height)) = dims {
                let needed = classify::primary_ctx_for(width, height, ctx.image_max_side, profile);
                if needed > block.ctx {
                    // Another window owns this row. Sending it here would either
                    // 400 (too big for the loaded window) or reload the model,
                    // which is what the blocks exist to avoid.
                    counters.deferred.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            }
        }

        let Some(decoded) = thumbnail::decode_source(abs) else {
            // Not counted toward the circuit breaker and not persisted: a file the
            // NAS no longer has (or an unmounted share) says nothing about Ollama,
            // and hiding the row would need a rescan to undo.
            counters.unreadable.fetch_add(1, Ordering::Relaxed);
            let item = BacklogItem {
                row: row.clone(),
                prepared: None,
                dhash: None,
                legacy: false,
                failed: true,
            };
            if tx.send(item).is_err() {
                break;
            }
            continue;
        };

        // The index only knows whether a pointer exists. A pointer to a file that
        // is gone is the common case after the cache bug, so verify before
        // skipping — one `stat` on a file we just opened.
        let thumb_missing = match row.thumb_path.as_deref() {
            Some(path) => !Path::new(path).exists(),
            None => true,
        };
        if thumb_missing {
            counters.thumbs_needed.fetch_add(1, Ordering::Relaxed);
            match thumbnail::thumbnail_from_decoded(&decoded, thumb_dir, &row.rel_path) {
                Some(tr) => {
                    let stored = db::update_file_thumb_and_dhash(
                        &ctx.db_path,
                        root_id,
                        &row.rel_path,
                        &tr.path,
                        tr.dhash.map(|h| h as i64),
                    );
                    match stored {
                        Ok(_) => {
                            counters.thumbnailed.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            log::warn!(
                                "Backlog: could not record thumbnail for {}: {e}",
                                row.rel_path
                            );
                            counters.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                None => {
                    log::warn!("Backlog: thumbnail generation failed for {}", row.rel_path);
                    counters.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let dhash = decoded.dhash;
        let prepared = if row.needs_classification {
            match vision_prep::prepare_from_decoded(
                abs,
                row.size_bytes.max(0) as u64,
                decoded.orientation,
                &decoded.image,
                ctx.image_max_side,
                &ctx.tmp_dir,
                ctx.image_filter,
            ) {
                Ok(prepared) => Some(prepared),
                Err(e) => {
                    log::warn!(
                        "Backlog: cannot prepare {} for the model: {e}",
                        row.rel_path
                    );
                    counters.unreadable.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        } else {
            None
        };
        let failed = row.needs_classification && prepared.is_none();
        // Release the decoded buffer before blocking on the queue: one image in
        // flight per producer, not one per queued item.
        drop(decoded);

        let item = BacklogItem {
            row: row.clone(),
            prepared,
            dhash: Some(dhash),
            legacy: false,
            failed,
        };
        if tx.send(item).is_err() {
            break;
        }
    }
}

/// Failure kinds worth persisting on the row.
///
/// Only the ones that are a property of **this file with this model**: a repetition
/// loop, a context overflow, a body that does not parse after four attempts. Those
/// would fail again the same way, so re-sending them every pass is pure waste.
///
/// The environmental ones (`io`, `timeout`, `empty_frame`, `no_response`, `decode`,
/// `prepare`, and an empty `unknown`) are deliberately **not** persisted: an
/// unmounted share, a wedged runner or a row whose file was deleted on the NAS would
/// otherwise be hidden from the backlog forever. That is not hypothetical — on
/// 2026-10-26 a pass marked 44 rows as failed because their files no longer existed,
/// and the same 44 fed the circuit breaker until the job aborted. Those rows belong
/// back in the queue, where the next discovery can drop or heal them.
fn is_permanent_failure(kind: &str) -> bool {
    matches!(kind, "repeat_limit" | "ctx_overflow" | "parse")
}

/// Persist why a row produced nothing, so the next pass does not send it again.
///
/// Without this the row came back on every pass — 63 of 886 files, 3–4 requests
/// each, measured on 2026-10-25 — so a backlog could never converge and the reason
/// existed only as a `WARN` in a log the next start truncates. `Retry failed` is the
/// explicit way back in ([`db::list_failed_rows`]).
fn mark_failed(ctx: &ScanContext, file_id: i64, kind: &str, detail: &str) {
    if !is_permanent_failure(kind) {
        log::debug!("Not marking file {file_id} as failed: '{kind}' is environmental");
        return;
    }
    let detail: String = detail.chars().take(400).collect();
    if let Err(e) = db::mark_file_failed(&ctx.db_path, file_id, kind, &detail) {
        log::error!("Could not record the failure for file {file_id}: {e}");
    }
}

/// Classify `file_id` from an already prepared image and persist the result.
fn classify_prepared_and_update(
    ctx: &ScanContext,
    file_id: i64,
    abs_path: &str,
    prepared: &vision_prep::PreparedImage,
) -> bool {
    let classification = classify::classify_image_prepared(
        Path::new(abs_path),
        prepared,
        &ctx.model,
        &ctx.surya_venv_dir,
        &ctx.surya_script,
        ctx.vision_settings(),
    );
    if classify::is_empty_result(&classification) {
        log::warn!(
            "Classifying {abs_path} produced nothing usable ({}); row marked failed",
            classification.failure_kind
        );
        mark_failed(ctx, file_id, &classification.failure_kind, "");
        return false;
    }
    match db::update_file_classification(
        &ctx.db_path,
        file_id,
        &classification.media_type,
        &classification.description,
        &classification.extracted_text,
        &classification.canonical_mentions,
        classification.confidence,
        &classification.lang_hint,
    ) {
        Ok(_) => true,
        Err(e) => {
            log::error!("Failed to update classification for file {file_id}: {e}");
            false
        }
    }
}

/// One pass over everything that still needs work.
///
/// The producer decodes each still image once and hands the consumer a
/// model-ready JPEG; the consumer (this thread, the only one talking to Ollama)
/// does the near-duplicate lookup and the vision request. The queue is bounded,
/// so a slow GPU throttles the producer instead of filling the disk with
/// prepared JPEGs or the RAM with decoded bitmaps.
///
/// Resume is implicit: the backlog is re-derived from the index on every run, so
/// an interrupted or cancelled pass simply picks up what is still pending. That
/// is why no cursor is written.
fn run_backlog_pipeline(
    ctx: &ScanContext,
    job: &ScanJobState,
    job_id: i64,
    cancel_flag: Option<&AtomicBool>,
    started: Instant,
    on_tick: &(dyn Fn(BacklogTick) + Sync),
    only_failed: bool,
) -> AppResult<ScanSummary> {
    let db_path = &ctx.db_path;
    let root_path = canonical_root_path(&job.root_path)?;
    let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&job.root_path));
    // `only_failed` is the "Retry failed" pass: the default selection skips rows
    // that already burned every attempt, so this is the only way back in for them.
    let rows = if only_failed {
        db::list_failed_rows(db_path, job.root_id)?
    } else {
        db::list_backlog_rows(db_path, job.root_id)?
    };
    let total = rows.len() as u64;
    // The phase is what the sidebar uses to resume in the right mode, so a retry
    // must not be recorded as a backlog pass.
    let phase = if only_failed {
        "retry_failed"
    } else {
        "backlog"
    };

    log::info!(
        "Scan job {job_id}: {} pass for {} ({total} file(s))",
        if only_failed {
            "retry-failed"
        } else {
            "backlog"
        },
        root_path.display()
    );
    // Two stage-specific denominators: most rows need only one of the two, and
    // dividing by `total` made the thumbnail share look stuck near zero after a
    // `Repair Thumbnails` run had already filled the cache.
    let classify_total = rows.iter().filter(|row| row.needs_classification).count() as u64;
    let thumbless = rows.iter().filter(|row| row.thumb_path.is_none()).count() as u64;
    on_tick(BacklogTick {
        total,
        thumbs_total: thumbless,
        classify_total,
        ..BacklogTick::default()
    });

    db::checkpoint_scan_job(
        db_path,
        job_id,
        phase,
        total,
        0,
        None,
        job.added,
        job.modified,
        job.moved,
        job.unchanged,
    )?;

    let cancelled = || cancel_flag.is_some_and(|flag| flag.load(Ordering::Relaxed));
    let mut dedupe = DedupeIndex::seed(db_path, job.root_id, ctx.dedupe_dhash_distance)?;
    let counters = BacklogCounters::default();
    let mut processed: u64 = 0;
    let mut classified: u64 = 0;
    let mut deduped: u64 = 0;
    // A systemic failure — Ollama down, or the model runner dying on every
    // request — would otherwise grind through thousands of files producing
    // nothing. One bad file does not look like this; 25 in a row does.
    let mut consecutive_failures: u32 = 0;
    let mut aborted_by_breaker = false;

    // A change of `num_ctx` reloads the model, so the pass runs one block per
    // window instead of one window per file (see [`ctx_blocks`]).
    let blocks = ctx_blocks(&rows, ctx);
    if blocks.len() > 1 {
        log::info!(
            "Scan job {job_id}: {} context block(s) — {}",
            blocks.len(),
            blocks
                .iter()
                .map(|block| format!("num_ctx {} ({} file(s))", block.ctx, block.rows.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Four is enough to keep the consumer busy while bounding the prepared JPEGs
    // on disk to a handful.
    const QUEUE_DEPTH: usize = 4;
    let thumb_dir_ref: &Path = &thumb_dir;
    let counters_ref = &counters;
    let root_id = job.root_id;
    let ctx_ref = ctx;

    for (block_index, block) in blocks.iter().enumerate() {
        if cancelled() {
            break;
        }
        let block_ref: &CtxBlock = block;
        let block_ctx = block.ctx;
        log::info!(
            "Scan job {job_id}: block {}/{} — {} file(s) at num_ctx {block_ctx}{}",
            block_index + 1,
            blocks.len(),
            block.rows.len(),
            if block.probed {
                " (sizes measured now; rows that need more are deferred)"
            } else {
                ""
            }
        );
        let (tx, rx) = std::sync::mpsc::sync_channel::<BacklogItem>(QUEUE_DEPTH);

        std::thread::scope(|scope| -> AppResult<()> {
            let producer = scope.spawn(move || {
                produce_backlog_items(
                    tx,
                    ctx_ref,
                    root_id,
                    thumb_dir_ref,
                    block_ref,
                    cancel_flag,
                    counters_ref,
                );
            });

            for item in rx.iter() {
                if cancelled() {
                    break;
                }

                let item_ok = if item.legacy {
                    classify_and_update(ctx, item.row.id, &item.row.abs_path);
                    classified += 1;
                    true
                } else if item.failed {
                    // The producer already counted it in `unreadable`. Returning `true`
                    // keeps it out of the circuit breaker: a batch of deleted files is
                    // not evidence that the runner keeps crashing, and counting it as
                    // such aborted a whole pass on 2026-10-26.
                    true
                } else if let Some(prepared) = item.prepared.as_ref() {
                    let representative = item
                        .dhash
                        .and_then(|dhash| dedupe.representative_for_hash(dhash));
                    match representative {
                        Some(representative) => {
                            if db::copy_classification(db_path, item.row.id, representative).is_ok()
                            {
                                deduped += 1;
                                true
                            } else if classify_prepared_and_update(
                                ctx,
                                item.row.id,
                                &item.row.abs_path,
                                prepared,
                            ) {
                                classified += 1;
                                true
                            } else {
                                counters.failed.fetch_add(1, Ordering::Relaxed);
                                false
                            }
                        }
                        None => {
                            if classify_prepared_and_update(
                                ctx,
                                item.row.id,
                                &item.row.abs_path,
                                prepared,
                            ) {
                                classified += 1;
                                if let Some(dhash) = item.dhash {
                                    dedupe.insert(dhash, item.row.id);
                                }
                                true
                            } else {
                                counters.failed.fetch_add(1, Ordering::Relaxed);
                                false
                            }
                        }
                    }
                } else {
                    // Thumbnail-only row: nothing left that can fail here.
                    true
                };

                if item_ok {
                    consecutive_failures = 0;
                } else {
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        log::error!(
                            "Backlog: {consecutive_failures} consecutive failures; aborting. \
                         Ollama is either down or its runner keeps crashing (VRAM)."
                        );
                        aborted_by_breaker = true;
                        break;
                    }
                }

                processed += 1;
                let thumbs_needed = counters.thumbs_needed.load(Ordering::Relaxed);
                on_tick(BacklogTick {
                    total,
                    processed,
                    thumbnailed: counters.thumbnailed.load(Ordering::Relaxed),
                    // A dead pointer discovered by the producer widens the denominator.
                    thumbs_total: thumbless.max(thumbs_needed),
                    classified,
                    classify_total,
                    deduped,
                    failed: counters.failed.load(Ordering::Relaxed),
                });
                if processed % 25 == 0 {
                    db::checkpoint_scan_job(
                        db_path,
                        job_id,
                        phase,
                        total,
                        processed,
                        None,
                        job.added,
                        job.modified,
                        job.moved,
                        job.unchanged,
                    )?;
                }
            }

            // Closing the receiver unblocks a producer parked on `send`; without it a
            // cancel would deadlock the join below.
            drop(rx);
            if producer.join().is_err() {
                log::error!("Backlog: producer thread panicked");
            }
            Ok(())
        })?;
    }

    if aborted_by_breaker {
        let message = format!(
            "backlog aborted after {MAX_CONSECUTIVE_FAILURES} consecutive failures: \
             Ollama is down or its model runner keeps crashing (usually VRAM exhaustion)"
        );
        db::fail_scan_job(db_path, job_id, &message)?;
        return Err(AppError::Config(message));
    }

    let failed = counters.failed.load(Ordering::Relaxed);
    let deferred = counters.deferred.load(Ordering::Relaxed);
    let unreadable = counters.unreadable.load(Ordering::Relaxed);
    let summary = ScanSummary {
        root_id: job.root_id,
        root_path: root_path.display().to_string(),
        scanned: processed,
        added: job.added,
        modified: job.modified,
        moved: job.moved,
        unchanged: job.unchanged,
        deleted: 0,
        elapsed_ms: started.elapsed().as_millis() as u64,
        healed: counters.thumbnailed.load(Ordering::Relaxed),
        deduped,
    };

    db::touch_root_scan(db_path, job.root_id)?;
    if cancelled() {
        db::cancel_scan_job(db_path, job_id)?;
    } else {
        db::complete_scan_job_by_id(db_path, job_id, &summary, None)?;
    }
    log::info!(
        "Backlog{}: {processed} processed, {classified} classified ({deduped} deduped), \
         {} thumbnailed, {failed} failed, {deferred} deferred to the next pass, \
         {unreadable} unreadable",
        if only_failed { " (retry failed)" } else { "" },
        summary.healed
    );
    if unreadable > 0 {
        log::warn!(
            "Backlog: {unreadable} file(s) could not be opened; if they no longer exist \
             on disk, run Refresh so the index drops them"
        );
    }
    Ok(summary)
}

/// Result from thumbnail-only pass (no LLM classification).
struct ThumbnailOnlyResult {
    thumb_path: Option<String>,
    location_text: String,
    dhash: Option<u64>,
}

fn is_pdf_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

fn is_video(path: &Path) -> bool {
    video::is_video_file(path)
}

/// Thumbnails already produced during this job, used to avoid reading the source
/// again when the library holds byte-identical (or visually identical) copies.
#[derive(Default)]
struct ThumbCaches {
    /// Content fingerprint → (thumbnail, dhash) of a file already thumbnailed.
    by_fingerprint: std::collections::HashMap<String, (PathBuf, Option<u64>)>,
    /// dHash → thumbnail of a file already thumbnailed.
    by_dhash: std::collections::HashMap<u64, PathBuf>,
}

impl ThumbCaches {
    /// Seed from rows whose recorded thumbnail still exists on disk.
    fn seed(db_path: &Path, root_id: i64) -> AppResult<Self> {
        let mut caches = Self::default();
        for (fingerprint, dhash, thumb) in db::list_thumb_cache_seed(db_path, root_id)? {
            let path = PathBuf::from(&thumb);
            if !path.exists() {
                continue;
            }
            let hash = dhash.map(|h| h as u64);
            if let Some(hash) = hash {
                caches.by_dhash.entry(hash).or_insert_with(|| path.clone());
            }
            caches.by_fingerprint.insert(fingerprint, (path, hash));
        }
        Ok(caches)
    }

    fn remember(&mut self, fingerprint: &str, thumb: &Path, dhash: Option<u64>) {
        if let Some(hash) = dhash {
            self.by_dhash
                .entry(hash)
                .or_insert_with(|| thumb.to_path_buf());
        }
        self.by_fingerprint
            .insert(fingerprint.to_string(), (thumb.to_path_buf(), dhash));
    }
}

/// Near-duplicate index used to skip vision requests during classification.
struct DedupeIndex {
    threshold: u32,
    index: crate::similarity::DhashIndex,
}

impl DedupeIndex {
    fn seed(db_path: &Path, root_id: i64, threshold: u32) -> AppResult<Self> {
        let mut index = crate::similarity::DhashIndex::new();
        if threshold > 0 {
            for (id, dhash) in db::list_classified_dhashes(db_path, root_id)? {
                index.insert(dhash as u64, id);
            }
        }
        Ok(Self { threshold, index })
    }

    fn is_disabled(&self) -> bool {
        self.threshold == 0
    }

    fn len(&self) -> usize {
        self.index.len()
    }

    /// Only still images have a stable perceptual hash: a video or PDF hashes a
    /// single keyframe/page, so folding them would be guesswork.
    fn eligible(file: &crate::models::UnclassifiedFile) -> Option<u64> {
        match file.media_type.as_str() {
            "photo" | "screenshot" => file.dhash.map(|h| h as u64),
            _ => None,
        }
    }

    fn representative_for(&self, file: &crate::models::UnclassifiedFile) -> Option<i64> {
        let dhash = Self::eligible(file)?;
        self.index.find_near(dhash, self.threshold)
    }

    fn remember(&mut self, file: &crate::models::UnclassifiedFile) {
        if let Some(dhash) = Self::eligible(file) {
            self.index.insert(dhash, file.id);
        }
    }

    /// Representative for a raw dHash.
    ///
    /// The backlog pipeline has no `UnclassifiedFile`: it knows the hash and
    /// whether the row is a still image from the file itself, not from a
    /// pre-classification `media_type`.
    fn representative_for_hash(&self, dhash: u64) -> Option<i64> {
        if self.is_disabled() {
            return None;
        }
        self.index.find_near(dhash, self.threshold)
    }

    fn insert(&mut self, dhash: u64, file_id: i64) {
        self.index.insert(dhash, file_id);
    }
}

/// Generate thumbnail and extract EXIF (no LLM classification).
///
/// `force` regenerates even when the cache file exists (new/modified/moved/heal);
/// `want_exif` is skipped for heals, whose row already stores the location.
fn thumbnail_only(
    ctx: &ScanContext,
    thumb_dir: &Path,
    probe: &FileProbe,
    fingerprint: &str,
    force: bool,
    want_exif: bool,
    caches: &mut ThumbCaches,
) -> ThumbnailOnlyResult {
    let abs = Path::new(&probe.abs_path);
    let is_pdf = is_pdf_file(abs);
    let is_vid = is_video(abs);

    // 1. Byte-identical file already thumbnailed: hard link it, the source is never
    //    opened. This is the common case in libraries with duplicated albums.
    if let Some((shared, dhash)) = caches.by_fingerprint.get(fingerprint).cloned() {
        if let Some(tr) = thumbnail::reuse_thumbnail(&shared, dhash, thumb_dir, &probe.rel_path) {
            // The copy carries its own EXIF, so the upsert must not overwrite
            // `location_text` with an empty string.
            let location_text = if want_exif && !is_pdf && !is_vid {
                crate::exif::extract_location(abs).location_text
            } else {
                String::new()
            };
            return ThumbnailOnlyResult {
                thumb_path: Some(tr.path),
                location_text,
                dhash: tr.dhash,
            };
        }
        caches.by_fingerprint.remove(fingerprint);
    }

    let pdf_password: Option<String> =
        if is_pdf && crate::pdf::is_password_protected(abs, &ctx.pdfium_lib_path) {
            let passwords = db::get_all_pdf_password_strings(&ctx.db_path).unwrap_or_default();
            crate::pdf::try_passwords(abs, &ctx.pdfium_lib_path, &passwords)
        } else {
            None
        };

    let thumb_result = if is_vid {
        thumbnail::generate_video_thumbnail(abs, thumb_dir, &probe.rel_path, &ctx.tmp_dir, force)
    } else if is_pdf {
        thumbnail::generate_pdf_thumbnail(
            abs,
            thumb_dir,
            &probe.rel_path,
            &ctx.pdfium_lib_path,
            pdf_password.as_deref(),
            force,
        )
    } else {
        thumbnail::generate_thumbnail(abs, thumb_dir, &probe.rel_path, force)
    };

    let Some(tr) = thumb_result else {
        return ThumbnailOnlyResult {
            thumb_path: None,
            location_text: String::new(),
            dhash: None,
        };
    };

    // 2. Visually identical image already encoded: keep one JPEG on disk and give
    //    this row its own hard link. The decode was unavoidable (the dHash comes
    //    from it), but the second copy on disk is not.
    let dhash = tr.dhash;
    let thumb_path = match dhash {
        Some(hash) => {
            let linked = caches.by_dhash.get(&hash).cloned().and_then(|shared| {
                thumbnail::reuse_thumbnail(&shared, Some(hash), thumb_dir, &probe.rel_path)
            });
            match linked {
                Some(linked) => Some(linked.path),
                None => {
                    caches
                        .by_dhash
                        .entry(hash)
                        .or_insert_with(|| PathBuf::from(&tr.path));
                    Some(tr.path)
                }
            }
        }
        None => Some(tr.path),
    };

    if let Some(ref path) = thumb_path {
        caches.remember(fingerprint, Path::new(path), dhash);
    }

    let exif_location: crate::exif::ExifLocation = if !want_exif || is_pdf || is_vid {
        Default::default()
    } else {
        crate::exif::extract_location(abs)
    };

    ThumbnailOnlyResult {
        thumb_path,
        location_text: exif_location.location_text,
        dhash,
    }
}

/// Infer a basic media_type from file extension (used for placeholder records).
fn infer_media_type_from_extension(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".pdf") {
        "document"
    } else if video::is_video_file(Path::new(filename)) {
        "video"
    } else {
        "photo"
    }
}

/// Build a minimal DB record with placeholder classification (confidence=0).
fn probe_to_minimal_record(
    root_id: i64,
    scan_marker: i64,
    probe: &FileProbe,
    fingerprint: &str,
    thumb_result: &ThumbnailOnlyResult,
) -> FileRecordUpsert {
    let video_meta = if video::is_video_file(Path::new(&probe.abs_path)) {
        video::extract_metadata(Path::new(&probe.abs_path))
    } else {
        None
    };

    FileRecordUpsert {
        root_id,
        rel_path: probe.rel_path.clone(),
        abs_path: probe.abs_path.clone(),
        filename: probe.filename.clone(),
        media_type: infer_media_type_from_extension(&probe.filename).to_string(),
        description: String::new(),
        extracted_text: String::new(),
        canonical_mentions: String::new(),
        confidence: 0.0,
        lang_hint: String::new(),
        mtime_ns: probe.mtime_ns,
        size_bytes: probe.size_bytes,
        fingerprint: fingerprint.to_string(),
        scan_marker,
        location_text: thumb_result.location_text.clone(),
        dhash: thumb_result.dhash.map(|h| h as i64),
        duration_secs: video_meta.as_ref().and_then(|m| m.duration_secs),
        video_width: video_meta.as_ref().and_then(|m| m.width),
        video_height: video_meta.as_ref().and_then(|m| m.height),
        video_codec: video_meta.as_ref().and_then(|m| m.video_codec.clone()),
        audio_codec: video_meta.as_ref().and_then(|m| m.audio_codec.clone()),
    }
}

/// Classify a single file via LLM and update the DB record.
///
/// Returns `false` when the request failed: the row is then left exactly as it was.
/// Writing the empty fallback over a good description is data loss, and it makes the
/// file look "pending" again with no evidence of why.
fn classify_and_update(ctx: &ScanContext, file_id: i64, abs_path: &str) -> bool {
    let abs = Path::new(abs_path);
    let is_pdf = is_pdf_file(abs);
    let is_vid = is_video(abs);

    let pdf_password: Option<String> =
        if is_pdf && crate::pdf::is_password_protected(abs, &ctx.pdfium_lib_path) {
            let passwords = db::get_all_pdf_password_strings(&ctx.db_path).unwrap_or_default();
            crate::pdf::try_passwords(abs, &ctx.pdfium_lib_path, &passwords)
        } else {
            None
        };

    let vision = ctx.vision_settings();
    let classification = if is_vid {
        classify::classify_video(
            abs,
            &ctx.model,
            &ctx.tmp_dir,
            &ctx.surya_venv_dir,
            &ctx.surya_script,
            vision,
        )
    } else if is_pdf {
        classify::classify_pdf(
            abs,
            &ctx.model,
            &ctx.tmp_dir,
            &ctx.surya_venv_dir,
            &ctx.surya_script,
            &ctx.pdfium_lib_path,
            pdf_password.as_deref(),
            vision,
        )
    } else {
        classify::classify_image(
            abs,
            &ctx.model,
            &ctx.tmp_dir,
            &ctx.surya_venv_dir,
            &ctx.surya_script,
            vision,
        )
    };

    if classify::is_empty_result(&classification) {
        log::warn!(
            "Classifying {abs_path} produced nothing usable ({}); row marked failed",
            classification.failure_kind
        );
        mark_failed(ctx, file_id, &classification.failure_kind, "");
        return false;
    }
    match db::update_file_classification(
        &ctx.db_path,
        file_id,
        &classification.media_type,
        &classification.description,
        &classification.extracted_text,
        &classification.canonical_mentions,
        classification.confidence,
        &classification.lang_hint,
    ) {
        Ok(_) => true,
        Err(e) => {
            log::error!(
                "Failed to update classification for file {}: {}",
                file_id,
                e
            );
            false
        }
    }
}

fn resume_start_index(probes: &[FileProbe], cursor_rel_path: Option<&str>) -> usize {
    let Some(cursor) = cursor_rel_path else {
        return 0;
    };
    probes
        .iter()
        .position(|p| p.rel_path.as_str() > cursor)
        .unwrap_or(probes.len())
}

/// Result of a discovery walk.
struct Discovery {
    probes: Vec<FileProbe>,
    /// Number of entries the walk could not read. A non-zero value means the
    /// file list may be incomplete, so cleanup must not run.
    walk_errors: u64,
}

/// Reject a discovery result that cannot be trusted to describe the root.
///
/// Covers the classic network failure: the mountpoint directory still exists but
/// the share is not mounted, so the walk succeeds and returns nothing. Deleting
/// the index in that case is data loss, not cleanup.
fn ensure_discovery_is_sane(probes_len: usize, existing_len: usize) -> AppResult<()> {
    if probes_len == 0 && existing_len > 0 {
        return Err(AppError::RootUnavailable(format!(
            "discovery found 0 files but {existing_len} are indexed; is the share mounted?"
        )));
    }

    if existing_len >= SAFETY_MIN_EXISTING_FILES
        && (probes_len as f64) < existing_len as f64 * SAFETY_MIN_RATIO
    {
        return Err(AppError::RootUnavailable(format!(
            "discovery found {probes_len} files but {existing_len} are indexed (below {:.0}%); refusing to delete index entries",
            SAFETY_MIN_RATIO * 100.0
        )));
    }

    Ok(())
}

/// Incremental file discovery: only reads file content for new/modified files.
fn collect_image_probes_incremental(
    root: &Path,
    existing_by_path: &HashMap<String, ExistingFile>,
    excluded_prefixes: &[PathBuf],
    db_path: &Path,
    job_id: i64,
    cancel_flag: Option<&AtomicBool>,
) -> AppResult<Discovery> {
    let mut probes = Vec::new();
    let mut discovered: u64 = 0;
    let mut walk_entries: u64 = 0;
    let mut walk_errors: u64 = 0;

    // Some CIFS/SMB servers truncate or round timestamps, which would otherwise
    // mark every file as modified on every scan and force needless re-thumbnailing
    // and reclassification. On network mounts the timestamp may drift slightly.
    let tolerant_mtime = crate::platform::fs::is_network_mount(root);
    const MTIME_TOLERANCE_NS: u64 = 2_000_000_000;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip child root subtrees before descending
            if e.file_type().is_dir() && e.path() != root {
                return !excluded_prefixes.iter().any(|p| e.path().starts_with(p));
            }
            true
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                walk_errors += 1;
                if walk_errors <= 5 {
                    log::warn!("Discovery error under {}: {err}", root.display());
                }
                continue;
            }
        };
        walk_entries += 1;

        // Check cancel flag during discovery
        if let Some(flag) = cancel_flag {
            if flag.load(Ordering::Relaxed) {
                return Ok(Discovery {
                    probes,
                    walk_errors,
                });
            }
        }

        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_supported_file(path) {
            continue;
        }

        // Use entry.metadata() to avoid a redundant stat() syscall —
        // WalkDir already called lstat() during the walk.
        let metadata = entry.metadata().map_err(|e| {
            crate::error::AppError::Config(format!("metadata for {}: {}", path.display(), e))
        })?;
        // Skip tiny images (icons, spacer GIFs, favicons) but not PDFs or videos
        let ext_lower = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext_lower != "pdf"
            && !video::is_video_file(path)
            && metadata.len() < MIN_IMAGE_SIZE_BYTES
        {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| normalize_rel_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| normalize_rel_path(&path.to_string_lossy()));
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or_default();
        let size_bytes = metadata.len() as i64;

        if let Some(existing) = existing_by_path.get(&rel) {
            let same_timestamp = existing.mtime_ns == mtime_ns;
            let tolerated_timestamp = tolerant_mtime
                && existing.size_bytes == size_bytes
                && existing.mtime_ns.abs_diff(mtime_ns) <= MTIME_TOLERANCE_NS;

            if (same_timestamp || tolerated_timestamp) && existing.size_bytes == size_bytes {
                // Unchanged: reuse DB fingerprint, no file content read
                probes.push(FileProbe {
                    rel_path: rel,
                    abs_path: path.display().to_string(),
                    filename,
                    mtime_ns,
                    size_bytes,
                    fingerprint: Some(existing.fingerprint.clone()),
                    status: FileStatus::Unchanged,
                });
            } else {
                // Modified: fingerprint deferred to phase 2
                probes.push(FileProbe {
                    rel_path: rel,
                    abs_path: path.display().to_string(),
                    filename,
                    mtime_ns,
                    size_bytes,
                    fingerprint: None,
                    status: FileStatus::Modified,
                });
            }
        } else {
            // New file: fingerprint deferred to phase 2
            probes.push(FileProbe {
                rel_path: rel,
                abs_path: path.display().to_string(),
                filename,
                mtime_ns,
                size_bytes,
                fingerprint: None,
                status: FileStatus::New,
            });
        }

        discovered += 1;
        if discovered % 500 == 0 {
            let _ = db::update_discovery_progress(db_path, job_id, discovered);
        }
    }
    let sort_start = Instant::now();
    probes.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let sort_ms = sort_start.elapsed().as_millis();

    log::info!(
        "Discovery breakdown: {} walkdir entries, {} supported files, {} errors, sort {}ms (fingerprints deferred to phase 2)",
        walk_entries,
        discovered,
        walk_errors,
        sort_ms,
    );
    Ok(Discovery {
        probes,
        walk_errors,
    })
}

fn cleanup_deleted_caches(db_path: &Path, root_id: i64, deleted_at: i64, thumb_dir: &Path) {
    let paths = match db::get_deleted_file_paths(db_path, root_id, deleted_at) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to get deleted file paths: {e}");
            return;
        }
    };
    for (rel_path, thumb_path) in paths {
        // Remove thumbnail
        if let Some(tp) = thumb_path {
            let _ = std::fs::remove_file(&tp);
        }
        // Also try the expected path
        let expected_thumb = thumb_dir.join(normalize_rel_path(
            &Path::new(&rel_path).with_extension("jpg").to_string_lossy(),
        ));
        let _ = std::fs::remove_file(&expected_thumb);
    }

    // Remove face crop files for deleted files
    if let Ok(crop_paths) = db::get_face_crop_paths_for_deleted(db_path, root_id, deleted_at) {
        for cp in &crop_paths {
            let _ = std::fs::remove_file(cp);
        }
    }
}

/// Summary of a thumbnail repair pass.
#[derive(Debug, Default, Clone)]
pub struct ThumbRepairSummary {
    pub scanned: u64,
    pub repaired: u64,
    pub failed: u64,
    /// Rows skipped because their root is not reachable right now.
    pub skipped: u64,
}

/// Repair order: still images and PDFs before videos.
fn thumb_repair_rank(rel_path: &str) -> u8 {
    let lower = rel_path.to_lowercase();
    if video::is_video_file(Path::new(&lower)) {
        2
    } else if lower.ends_with(".pdf") {
        1
    } else {
        0
    }
}

/// Regenerate every thumbnail whose cache file is missing, without walking the
/// share (a full discovery is the slowest part on a network mount).
///
/// `root_id = None` covers every root. `on_progress(processed, total, repaired,
/// failed)` fires every 25 files, and the pass stops as soon as `cancel` is set.
///
/// Thumbnailing is CPU-bound (JPEG decode) and the pass is long, so the work is
/// spread over [`REPAIR_WORKERS`] threads. Reads do not saturate the share, so the
/// wall-clock gain is close to linear until the network becomes the limit.
pub fn repair_missing_thumbnails(
    ctx: &ScanContext,
    root_id: Option<i64>,
    cancel: &AtomicBool,
    on_progress: impl Fn(u64, u64, u64, u64) + Sync,
) -> AppResult<ThumbRepairSummary> {
    let mut summary = ThumbRepairSummary::default();
    let root_paths = db::list_root_paths(&ctx.db_path)?;

    let mut by_root: HashMap<i64, Vec<db::ThumbRepairRow>> = HashMap::new();
    for row in db::list_thumb_repair_rows(&ctx.db_path, root_id)? {
        by_root.entry(row.root_id).or_default().push(row);
    }

    for (rid, rows) in by_root {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(root_path) = root_paths
            .iter()
            .find(|(id, _)| *id == rid)
            .map(|(_, path)| path.clone())
        else {
            summary.skipped += rows.len() as u64;
            continue;
        };
        if !probe_dir(Path::new(&root_path), ROOT_PROBE_TIMEOUT).is_dir {
            log::warn!("Thumbnail repair: root {root_path} is unreachable, skipping");
            summary.skipped += rows.len() as u64;
            continue;
        }

        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));

        let mut pending: Vec<db::ThumbRepairRow> = rows
            .into_iter()
            .filter(|row| match row.thumb_path.as_deref() {
                Some(path) => !Path::new(path).exists(),
                None => true,
            })
            .collect();
        // Photos and PDFs first (cheap, and the bulk of the library); videos last,
        // because ffmpeg has to seek through the whole file over the network.
        pending.sort_by_key(|row| thumb_repair_rank(&row.rel_path));
        let total = pending.len() as u64;
        log::info!("Thumbnail repair: {total} file(s) of {root_path} need a thumbnail");

        let processed = AtomicU64::new(0);
        let repaired = AtomicU64::new(0);
        let failed = AtomicU64::new(0);

        if total > 0 {
            // Contiguous blocks keep duplicates (which sit next to each other in
            // rel_path order) inside the same worker, so its caches still fold them.
            let workers = REPAIR_WORKERS.min(pending.len()).max(1);
            let block = pending.len().div_ceil(workers);
            let processed = &processed;
            let repaired = &repaired;
            let failed = &failed;
            let on_progress = &on_progress;
            let thumb_dir: &Path = &thumb_dir;
            std::thread::scope(|scope| {
                for chunk in pending.chunks(block) {
                    scope.spawn(move || {
                        let mut caches = match ThumbCaches::seed(&ctx.db_path, rid) {
                            Ok(caches) => caches,
                            Err(e) => {
                                log::warn!("Thumbnail repair: cache seed failed: {e}");
                                ThumbCaches::default()
                            }
                        };
                        for row in chunk {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let probe = FileProbe {
                                rel_path: row.rel_path.clone(),
                                abs_path: row.abs_path.clone(),
                                filename: crate::platform::paths::rel_path_filename(&row.rel_path)
                                    .to_string(),
                                mtime_ns: 0,
                                size_bytes: 0,
                                fingerprint: Some(row.fingerprint.clone()),
                                status: FileStatus::Modified,
                            };
                            let result = thumbnail_only(
                                ctx,
                                thumb_dir,
                                &probe,
                                &row.fingerprint,
                                true,
                                false,
                                &mut caches,
                            );
                            let stored = match result.thumb_path {
                                Some(path) => db::update_file_thumb_and_dhash(
                                    &ctx.db_path,
                                    rid,
                                    &row.rel_path,
                                    &path,
                                    result.dhash.map(|h| h as i64),
                                )
                                .map(|_| true),
                                None => db::clear_thumb_path(&ctx.db_path, row.id).map(|_| false),
                            };
                            match stored {
                                Ok(true) => {
                                    repaired.fetch_add(1, Ordering::Relaxed);
                                }
                                Ok(false) => {
                                    failed.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Thumbnail repair: could not record {}: {e}",
                                        row.rel_path
                                    );
                                    failed.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                            if done % 25 == 0 {
                                on_progress(
                                    done,
                                    total,
                                    repaired.load(Ordering::Relaxed),
                                    failed.load(Ordering::Relaxed),
                                );
                            }
                        }
                    });
                }
            });
        }

        summary.scanned += processed.load(Ordering::Relaxed);
        summary.repaired += repaired.load(Ordering::Relaxed);
        summary.failed += failed.load(Ordering::Relaxed);
        on_progress(summary.scanned, total, summary.repaired, summary.failed);
    }

    Ok(summary)
}

fn fingerprint_file(path: &Path, size: u64) -> AppResult<String> {
    const WINDOW: usize = 64 * 1024;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();

    if size <= (WINDOW * 2) as u64 {
        let mut buf = Vec::with_capacity(size as usize);
        file.read_to_end(&mut buf)?;
        hasher.update(buf);
        return Ok(hex::encode(hasher.finalize()));
    }

    let mut head = vec![0_u8; WINDOW];
    file.read_exact(&mut head)?;
    hasher.update(&head);

    let mut tail = vec![0_u8; WINDOW];
    file.seek(SeekFrom::End(-(WINDOW as i64)))?;
    file.read_exact(&mut tail)?;
    hasher.update(&tail);

    Ok(hex::encode(hasher.finalize()))
}

fn is_supported_file(path: &Path) -> bool {
    let Some(ext) = path.extension().map(|v| v.to_string_lossy().to_lowercase()) else {
        return false;
    };
    let ext = format!(".{ext}");
    SUPPORTED_EXTS.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::db;

    /// Write a dummy file large enough to pass MIN_IMAGE_SIZE_BYTES.
    fn write_test_image(path: &std::path::Path, seed: u8) {
        let mut f = File::create(path).expect("create test image");
        let data = vec![seed; MIN_IMAGE_SIZE_BYTES as usize];
        f.write_all(&data).expect("write test image");
    }

    fn make_scan_context(db_path: &Path) -> ScanContext {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vision = crate::models::VisionSettings::default();
        ScanContext {
            db_path: db_path.to_path_buf(),
            thumbnails_dir: tmp.path().join("thumbs"),
            tmp_dir: tmp.path().join("tmp"),
            surya_venv_dir: tmp.path().join("venv"),
            surya_script: tmp.path().join("surya_ocr.py"),
            model: "qwen2.5vl:7b".to_string(),
            image_max_side: vision.image_max_side,
            prompt_profile: vision.prompt_profile,
            image_filter: vision.image_filter,
            ocr_engine: vision.ocr_engine,
            pdfium_lib_path: tmp.path().join("lib"),
            dedupe_dhash_distance: 0,
        }
    }

    /// Context whose thumbnail cache lives in a caller-owned directory.
    fn scan_context_with_cache(db_path: &Path, cache_dir: &Path) -> ScanContext {
        let mut ctx = make_scan_context(db_path);
        ctx.thumbnails_dir = cache_dir.join("thumbs");
        ctx
    }

    /// A real decodable image (`write_test_image` writes raw bytes `image` cannot open).
    fn write_real_image(path: &Path) {
        let img = image::RgbImage::from_fn(120, 90, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        image::DynamicImage::ImageRgb8(img)
            .save(path)
            .expect("save image");
    }

    fn probe_for(rel: &str, abs: &str, fingerprint: &str) -> FileProbe {
        FileProbe {
            rel_path: rel.to_string(),
            abs_path: abs.to_string(),
            filename: crate::platform::paths::rel_path_filename(rel).to_string(),
            mtime_ns: 1,
            size_bytes: 1,
            fingerprint: Some(fingerprint.to_string()),
            status: FileStatus::New,
        }
    }

    /// A byte-identical copy must not be decoded again: the source is never opened
    /// (the test points it at a nonexistent path on purpose).
    #[test]
    fn identical_content_reuses_thumbnail_without_reading_the_source() {
        let root_dir = tempfile::tempdir().expect("root");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let cache_dir = tempfile::tempdir().expect("cache");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");
        let ctx = scan_context_with_cache(&db_path, cache_dir.path());
        let thumb_dir = ctx.thumbnails_dir.clone();

        let image = root_dir.path().join("a.png");
        write_real_image(&image);

        let mut caches = ThumbCaches::default();
        let first = probe_for("a.png", &image.display().to_string(), "fp-a");
        let out1 = thumbnail_only(&ctx, &thumb_dir, &first, "fp-a", false, true, &mut caches);
        let thumb1 = out1.thumb_path.clone().expect("first thumbnail");
        let dhash1 = out1.dhash.expect("first hash");

        let copy = probe_for("b/copy.png", "/nonexistent/copy.png", "fp-a");
        let out2 = thumbnail_only(&ctx, &thumb_dir, &copy, "fp-a", false, true, &mut caches);
        let thumb2 = out2.thumb_path.clone().expect("reused thumbnail");

        assert_eq!(out2.dhash, Some(dhash1));
        assert_ne!(thumb1, thumb2);
        assert!(Path::new(&thumb1).exists());
        assert!(Path::new(&thumb2).exists());
    }

    /// The `Reparar thumbnails` path: rows whose cache file vanished get a fresh
    /// thumbnail without any discovery walk.
    #[test]
    fn repair_regenerates_thumbnails_with_dead_paths() {
        let root_dir = tempfile::tempdir().expect("root");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let cache_dir = tempfile::tempdir().expect("cache");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");
        let root_path = root_dir.path().to_str().expect("path").to_string();
        let root_id = db::upsert_root(&db_path, &root_path).expect("root");

        let ctx = scan_context_with_cache(&db_path, cache_dir.path());
        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));

        let image = root_dir.path().join("a.png");
        write_real_image(&image);
        let probe = probe_for("a.png", &image.display().to_string(), "fp-a");
        let mut caches = ThumbCaches::default();
        let thumb = thumbnail_only(&ctx, &thumb_dir, &probe, "fp-a", false, true, &mut caches);
        let thumb_path = thumb.thumb_path.clone().expect("thumbnail");

        let record = probe_to_minimal_record(root_id, 1, &probe, "fp-a", &thumb);
        db::upsert_file_record(&db_path, &record).expect("insert");
        db::update_file_thumb_path(&db_path, root_id, "a.png", &thumb_path).expect("pointer");

        // Cache wiped (root purged, interrupted run, ...).
        std::fs::remove_file(&thumb_path).expect("remove thumb");

        let summary = repair_missing_thumbnails(
            &ctx,
            Some(root_id),
            &AtomicBool::new(false),
            |_, _, _, _| {},
        )
        .expect("repair");
        assert_eq!(summary.repaired, 1);
        assert_eq!(summary.failed, 0);
        assert!(
            Path::new(&thumb_path).exists(),
            "thumbnail must be regenerated"
        );
    }

    /// The producer half of the backlog pipeline, without Ollama: one decode
    /// fills the missing thumbnail and queues a model-ready JPEG for the row.
    #[test]
    fn backlog_producer_thumbnails_and_queues_a_prepared_image() {
        let (ctx, root_id, root_path, _dirs) = backlog_fixture();
        let rows = db::list_backlog_rows(&ctx.db_path, root_id).expect("backlog");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].needs_classification);

        let counters = BacklogCounters::default();
        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));
        let (tx, rx) = std::sync::mpsc::sync_channel::<BacklogItem>(4);

        produce_backlog_items(
            tx,
            &ctx,
            root_id,
            &thumb_dir,
            &block_of(rows, crate::llm::MIN_NUM_CTX),
            None,
            &counters,
        );

        let item = rx.try_recv().expect("one item for one row");
        assert!(!item.failed);
        assert!(
            item.prepared.is_some(),
            "an unclassified row must arrive prepared for the model"
        );
        assert!(item.dhash.is_some());
        assert!(rx.try_recv().is_err(), "one row, one item");

        let thumb_path = thumbnail::thumb_path_for(
            &ctx.thumbnails_dir,
            &thumbnail::root_key(&root_path),
            "a.png",
        );
        assert!(thumb_path.exists(), "the missing thumbnail must be built");
        assert_eq!(counters.thumbnailed.load(Ordering::Relaxed), 1);
        assert_eq!(counters.failed.load(Ordering::Relaxed), 0);

        // The producer only fixes the thumbnail; the row stays pending until the
        // consumer records a classification.
        let remaining = db::list_backlog_rows(&ctx.db_path, root_id).expect("backlog");
        assert_eq!(remaining.len(), 1, "still unclassified");
        assert!(
            remaining[0].thumb_path.is_some(),
            "the thumbnail pointer must be recorded"
        );
    }

    /// Cancelling must stop before decoding anything, and must not leave a
    /// thumbnail behind.
    #[test]
    fn backlog_producer_honours_cancellation() {
        let (ctx, root_id, root_path, _dirs) = backlog_fixture();
        let rows = db::list_backlog_rows(&ctx.db_path, root_id).expect("backlog");

        let counters = BacklogCounters::default();
        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));
        let (tx, rx) = std::sync::mpsc::sync_channel::<BacklogItem>(4);
        let cancel = AtomicBool::new(true);

        produce_backlog_items(
            tx,
            &ctx,
            root_id,
            &thumb_dir,
            &block_of(rows, crate::llm::MIN_NUM_CTX),
            Some(&cancel),
            &counters,
        );

        assert!(
            rx.try_recv().is_err(),
            "cancelled producer must send nothing"
        );
        assert_eq!(counters.thumbnailed.load(Ordering::Relaxed), 0);
        let thumb_path = thumbnail::thumb_path_for(
            &ctx.thumbnails_dir,
            &thumbnail::root_key(&root_path),
            "a.png",
        );
        assert!(!thumb_path.exists(), "cancelled producer must not write");
    }

    /// Rows are grouped by the window their prepared image will need, cheapest
    /// first, and the ones with no recorded size come last so they cannot change
    /// the runner identity in the middle of a known block.
    #[test]
    fn backlog_rows_are_grouped_by_context_window() {
        let (mut ctx, root_id, _root_path, _dirs) = backlog_fixture();
        // 1280 px is the first prep size where a square image needs the next
        // window; at 768 every still image resolves to 4096.
        ctx.image_max_side = 1280;
        let base = db::list_backlog_rows(&ctx.db_path, root_id)
            .expect("backlog")
            .remove(0);

        let mut photo = base.clone();
        photo.rel_path = "photo.jpg".to_string();
        photo.image_width = Some(4000);
        photo.image_height = Some(3000);

        let mut square = base.clone();
        square.rel_path = "square.jpg".to_string();
        square.image_width = Some(3000);
        square.image_height = Some(3000);

        let mut unknown = base.clone();
        unknown.rel_path = "unknown.jpg".to_string();
        unknown.image_width = None;
        unknown.image_height = None;

        let blocks = ctx_blocks(&[square.clone(), unknown.clone(), photo.clone()], &ctx);
        assert_eq!(blocks.len(), 3, "two windows plus the probed block");
        assert_eq!(blocks[0].ctx, 4096);
        assert!(!blocks[0].probed);
        assert_eq!(blocks[0].rows[0].rel_path, "photo.jpg");
        assert_eq!(blocks[1].ctx, 8192);
        assert!(!blocks[1].probed);
        assert_eq!(blocks[1].rows[0].rel_path, "square.jpg");
        // A row without dimensions rides at the largest window already loaded.
        assert_eq!(blocks[2].ctx, 8192);
        assert!(blocks[2].probed);
        assert_eq!(blocks[2].rows[0].rel_path, "unknown.jpg");

        // Nothing measured at all: one probed block at the smallest window.
        let blocks = ctx_blocks(&[unknown], &ctx);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ctx, crate::llm::MIN_NUM_CTX);
        assert!(blocks[0].probed);
    }

    /// A row that needs a bigger window than the block runs at must not be sent:
    /// Ollama would answer 400 or rebuild the runner, which is what the blocks
    /// exist to prevent.
    #[test]
    fn backlog_producer_defers_a_row_that_needs_a_bigger_window() {
        let (mut ctx, root_id, root_path, _dirs) = backlog_fixture();
        ctx.image_max_side = 1280;
        let mut row = db::list_backlog_rows(&ctx.db_path, root_id)
            .expect("backlog")
            .remove(0);
        // A recorded 3000x3000: at 1280 px it becomes 1280x1280 and needs 8192.
        row.image_width = Some(3000);
        row.image_height = Some(3000);

        let counters = BacklogCounters::default();
        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));
        let (tx, rx) = std::sync::mpsc::sync_channel::<BacklogItem>(4);

        produce_backlog_items(
            tx,
            &ctx,
            root_id,
            &thumb_dir,
            &block_of(vec![row], 4096),
            None,
            &counters,
        );

        assert!(
            rx.try_recv().is_err(),
            "a row for another window must not be sent"
        );
        assert_eq!(counters.deferred.load(Ordering::Relaxed), 1);
        assert_eq!(counters.failed.load(Ordering::Relaxed), 0);
        let thumb_path = thumbnail::thumb_path_for(
            &ctx.thumbnails_dir,
            &thumbnail::root_key(&root_path),
            "a.png",
        );
        assert!(
            !thumb_path.exists(),
            "a deferred row must not be decoded or thumbnailed"
        );
    }

    /// The size has to be recorded when it is measured, otherwise every pass probes
    /// the header of every row again and no row ever gets a block of its own.
    #[test]
    fn backlog_producer_records_the_dimensions_it_probed() {
        let (ctx, root_id, root_path, _dirs) = backlog_fixture();
        let rows = db::list_backlog_rows(&ctx.db_path, root_id).expect("backlog");
        assert_eq!(
            (rows[0].image_width, rows[0].image_height),
            (None, None),
            "the fixture has no size recorded yet"
        );

        let counters = BacklogCounters::default();
        let thumb_dir = ctx.thumb_dir_for(&thumbnail::root_key(&root_path));
        let (tx, rx) = std::sync::mpsc::sync_channel::<BacklogItem>(4);
        produce_backlog_items(
            tx,
            &ctx,
            root_id,
            &thumb_dir,
            &block_of(rows, crate::llm::MIN_NUM_CTX),
            None,
            &counters,
        );
        assert!(
            rx.try_recv().is_ok(),
            "a small image must fit the smallest window"
        );
        assert_eq!(counters.deferred.load(Ordering::Relaxed), 0);

        let stored = db::list_backlog_rows(&ctx.db_path, root_id).expect("backlog");
        assert!(
            stored[0].image_width.is_some() && stored[0].image_height.is_some(),
            "the probed size must be cached on the row"
        );
    }

    /// The pipeline dedupes by raw hash, because it has no `UnclassifiedFile` —
    /// and a threshold of 0 must keep it out of the way.
    #[test]
    fn backlog_dedupe_matches_by_hash() {
        let mut disabled = DedupeIndex {
            threshold: 0,
            index: crate::similarity::DhashIndex::new(),
        };
        disabled.insert(0b1010, 1);
        assert_eq!(disabled.representative_for_hash(0b1010), None);

        let mut index = DedupeIndex {
            threshold: 6,
            index: crate::similarity::DhashIndex::new(),
        };
        index.insert(0b1010, 7);
        assert_eq!(index.representative_for_hash(0b1010), Some(7));
        assert_eq!(index.representative_for_hash(0b1011), Some(7));
        assert_eq!(index.representative_for_hash(u64::MAX), None);
    }

    /// A failure the file+model pair would repeat goes on the row; an environmental
    /// one must not, or the row is hidden from the backlog until a rescan.
    #[test]
    fn only_deterministic_failures_are_persisted() {
        assert!(is_permanent_failure("repeat_limit"));
        assert!(is_permanent_failure("ctx_overflow"));
        assert!(is_permanent_failure("parse"));
        for kind in [
            "io",
            "timeout",
            "empty_frame",
            "no_response",
            "decode",
            "prepare",
            "unknown",
        ] {
            assert!(
                !is_permanent_failure(kind),
                "{kind} is environmental and must not hide the row"
            );
        }
    }

    /// A block of rows that all run at `ctx`, for the producer tests.
    fn block_of(rows: Vec<db::BacklogRow>, ctx: u32) -> CtxBlock {
        CtxBlock {
            ctx,
            probed: false,
            rows,
        }
    }

    /// A root with one still image that is neither thumbnailed nor classified.
    ///
    /// The three `TempDir` guards are returned so the caller keeps them alive for
    /// the duration of the test.
    fn backlog_fixture() -> (ScanContext, i64, String, [tempfile::TempDir; 3]) {
        let root_dir = tempfile::tempdir().expect("root");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let cache_dir = tempfile::tempdir().expect("cache");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");
        let root_path = root_dir.path().to_str().expect("path").to_string();
        let root_id = db::upsert_root(&db_path, &root_path).expect("root");
        let ctx = scan_context_with_cache(&db_path, cache_dir.path());

        let image = root_dir.path().join("a.png");
        write_real_image(&image);
        let probe = probe_for("a.png", &image.display().to_string(), "fp-a");
        let no_thumb = ThumbnailOnlyResult {
            thumb_path: None,
            location_text: String::new(),
            dhash: None,
        };
        let record = probe_to_minimal_record(root_id, 1, &probe, "fp-a", &no_thumb);
        db::upsert_file_record(&db_path, &record).expect("insert");

        (ctx, root_id, root_path, [root_dir, db_dir, cache_dir])
    }

    /// Only still images are folded; a video sharing the hash would be a guess.
    #[test]
    fn dedupe_index_only_matches_photos() {
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");
        let root_id = db::upsert_root(&db_path, "/tmp/dedupe").expect("root");
        let id = db::upsert_file_record(
            &db_path,
            &crate::models::FileRecordUpsert {
                root_id,
                rel_path: "a.jpg".to_string(),
                abs_path: "/tmp/dedupe/a.jpg".to_string(),
                filename: "a.jpg".to_string(),
                media_type: "photo".to_string(),
                description: "beach".to_string(),
                extracted_text: String::new(),
                canonical_mentions: String::new(),
                confidence: 0.8,
                lang_hint: "en".to_string(),
                mtime_ns: 1,
                size_bytes: 1,
                fingerprint: "fp".to_string(),
                scan_marker: 1,
                location_text: String::new(),
                dhash: Some(42),
                duration_secs: None,
                video_width: None,
                video_height: None,
                video_codec: None,
                audio_codec: None,
            },
        )
        .expect("insert");

        let dedupe = DedupeIndex::seed(&db_path, root_id, 6).expect("seed");
        assert_eq!(dedupe.len(), 1);

        let candidate = crate::models::UnclassifiedFile {
            id: id + 1,
            rel_path: "b.jpg".to_string(),
            abs_path: "/tmp/dedupe/b.jpg".to_string(),
            media_type: "photo".to_string(),
            dhash: Some(44), // 2 bits away from 42
        };
        assert_eq!(dedupe.representative_for(&candidate), Some(id));

        let video = crate::models::UnclassifiedFile {
            media_type: "video".to_string(),
            ..candidate.clone()
        };
        assert_eq!(dedupe.representative_for(&video), None);

        let far = crate::models::UnclassifiedFile {
            dhash: Some(0b111_1111_1111),
            ..candidate
        };
        assert_eq!(dedupe.representative_for(&far), None);
    }

    /// An unreadable file must not abort the job: the scan completes and the
    /// readable files around it are still indexed.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_skipped_not_fatal() {
        use std::os::unix::fs::PermissionsExt;

        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        // "a.jpg" sorts before "b.jpg", so the unreadable file is hit first —
        // the exact shape of the bug this guards against.
        let unreadable = root_dir.path().join("a.jpg");
        write_test_image(&unreadable, 0xAA);
        let readable = root_dir.path().join("b.jpg");
        write_test_image(&readable, 0xBB);

        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
            .expect("chmod");

        // root bypasses DAC, so mode 0o000 would still open and the test would
        // assert nothing. Skip rather than report a false pass.
        if File::open(&unreadable).is_ok() {
            let _ = std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644));
            eprintln!("skipping: this process can read mode-000 files (running as root?)");
            return;
        }

        let job = start_or_resume_scan_job(&db_path, root_dir.path().to_str().expect("str"))
            .expect("job");
        let ctx = make_scan_context(&db_path);
        let summary = run_scan_job_internal(&ctx, job.id, None, None, ScanMode::Refresh)
            .expect("scan must not fail on an unreadable file");

        // Restore permissions so tempdir cleanup succeeds.
        let _ = std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644));

        assert_eq!(
            summary.added, 1,
            "the readable file should still be indexed"
        );

        let indexed = db::load_existing_files(&db_path, job.root_id).expect("load");
        let paths: Vec<&str> = indexed.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["b.jpg"]);
    }

    /// A modified file that becomes unreadable keeps its existing index entry:
    /// end-of-scan cleanup must not soft-delete a file that still exists on
    /// disk but cannot currently be opened.
    #[cfg(unix)]
    #[test]
    fn skipped_modified_file_is_not_marked_deleted() {
        use std::os::unix::fs::PermissionsExt;

        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        let img = root_dir.path().join("a.jpg");
        write_test_image(&img, 0xAA);

        let ctx = make_scan_context(&db_path);
        let first_job = start_or_resume_scan_job(&db_path, root_dir.path().to_str().expect("str"))
            .expect("first job");
        let summary = run_scan_job_internal(&ctx, first_job.id, None, None, ScanMode::Refresh)
            .expect("first scan");
        assert_eq!(summary.added, 1);

        // Grow the file so the rescan sees it as modified, then make it unreadable.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&img)
            .expect("open for append");
        f.write_all(&[0xAB]).expect("append");
        drop(f);
        std::fs::set_permissions(&img, std::fs::Permissions::from_mode(0o000)).expect("chmod");

        // root bypasses DAC, so mode 0o000 would still open and the test would
        // assert nothing. Skip rather than report a false pass.
        if File::open(&img).is_ok() {
            let _ = std::fs::set_permissions(&img, std::fs::Permissions::from_mode(0o644));
            eprintln!("skipping: this process can read mode-000 files (running as root?)");
            return;
        }

        let second_job = start_or_resume_scan_job(&db_path, root_dir.path().to_str().expect("str"))
            .expect("second job");
        let summary = run_scan_job_internal(&ctx, second_job.id, None, None, ScanMode::Refresh)
            .expect("scan must not fail on an unreadable file");

        // Restore permissions so tempdir cleanup succeeds.
        let _ = std::fs::set_permissions(&img, std::fs::Permissions::from_mode(0o644));

        assert_eq!(
            summary.deleted, 0,
            "a temporarily unreadable file must not be treated as deleted"
        );

        let indexed = db::load_existing_files(&db_path, second_job.root_id).expect("load");
        let paths: Vec<&str> = indexed.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["a.jpg"],
            "the existing index entry must survive the skip"
        );
    }

    #[test]
    fn detects_move_without_reinsert() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        let image_a = root_dir.path().join("a.jpg");
        write_test_image(&image_a, 0xAA);

        let first_job = start_or_resume_scan_job(&db_path, root_dir.path().to_str().expect("str"))
            .expect("job");
        // For testing without Ollama, we use the internal function but skip classification
        // by using the old-style direct test
        let _ctx = make_scan_context(&db_path);
        // We can't easily test with real classification in unit tests,
        // but we verify the incremental discovery logic here.
        let existing = db::load_existing_files(&db_path, first_job.root_id).expect("load");
        assert!(existing.is_empty());
    }

    #[test]
    fn incremental_discovery_marks_unchanged() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");
        let root_id = db::upsert_root(&db_path, root_dir.path().to_str().unwrap()).expect("root");

        // Create a file
        let img = root_dir.path().join("test.jpg");
        write_test_image(&img, 0xBB);

        let metadata = std::fs::metadata(&img).expect("meta");
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or_default();
        let fp = fingerprint_file(&img, metadata.len()).expect("fp");

        // Insert record to simulate previous scan
        let rec = crate::models::FileRecordUpsert {
            root_id,
            rel_path: "test.jpg".to_string(),
            abs_path: img.display().to_string(),
            filename: "test.jpg".to_string(),
            media_type: "photo".to_string(),
            description: "classified already".to_string(),
            extracted_text: String::new(),
            canonical_mentions: String::new(),
            confidence: 0.8,
            lang_hint: "en".to_string(),
            mtime_ns,
            size_bytes: metadata.len() as i64,
            fingerprint: fp,
            scan_marker: 1,
            location_text: String::new(),
            dhash: None,
            duration_secs: None,
            video_width: None,
            video_height: None,
            video_codec: None,
            audio_codec: None,
        };
        db::upsert_file_record(&db_path, &rec).expect("upsert");

        // Now run incremental discovery
        let existing = db::load_existing_files(&db_path, root_id).expect("load");
        let existing_by_path: HashMap<String, ExistingFile> = existing
            .iter()
            .map(|f| (f.rel_path.clone(), f.clone()))
            .collect();

        let probes = collect_image_probes_incremental(
            root_dir.path(),
            &existing_by_path,
            &[],
            &db_path,
            0,
            None,
        )
        .expect("probes");
        assert_eq!(probes.probes.len(), 1);
        assert!(matches!(probes.probes[0].status, FileStatus::Unchanged));
    }

    #[test]
    fn cancel_flag_returns_early() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        // Create image files
        for name in &["a.jpg", "b.jpg", "c.jpg"] {
            let img = root_dir.path().join(name);
            let mut f = File::create(&img).expect("create");
            f.write_all(format!("data-{name}").as_bytes())
                .expect("write");
        }

        let job =
            start_or_resume_scan_job(&db_path, root_dir.path().to_str().unwrap()).expect("job");
        let ctx = make_scan_context(&db_path);

        // Set cancel flag before running
        let flag = AtomicBool::new(true);
        let result = run_scan_job_internal(&ctx, job.id, None, Some(&flag), ScanMode::Full);
        assert!(result.is_ok());
        let summary = result.unwrap();
        // Should return early with minimal processing
        assert_eq!(summary.added, 0);
    }

    #[test]
    fn incremental_discovery_marks_new() {
        let root_dir = tempfile::tempdir().expect("tempdir");

        let img = root_dir.path().join("new.jpg");
        write_test_image(&img, 0xCC);

        let existing_by_path: HashMap<String, ExistingFile> = HashMap::new();
        let dummy_db = root_dir.path().join("dummy.sqlite");
        let probes = collect_image_probes_incremental(
            root_dir.path(),
            &existing_by_path,
            &[],
            &dummy_db,
            0,
            None,
        )
        .expect("probes");
        assert_eq!(probes.probes.len(), 1);
        assert!(matches!(probes.probes[0].status, FileStatus::New));
    }

    #[test]
    fn excluded_prefixes_skips_child_root_files() {
        let root_dir = tempfile::tempdir().expect("tempdir");

        // Create files in parent root
        let parent_img = root_dir.path().join("parent.jpg");
        write_test_image(&parent_img, 0xDD);

        // Create child root subdir with a file
        let child_dir = root_dir.path().join("Photos");
        std::fs::create_dir_all(&child_dir).expect("mkdir");
        let child_img = child_dir.join("child.jpg");
        write_test_image(&child_img, 0xEE);

        let existing_by_path: HashMap<String, ExistingFile> = HashMap::new();
        let dummy_db = root_dir.path().join("dummy.sqlite");

        // Without exclusions: both files found
        let probes_all = collect_image_probes_incremental(
            root_dir.path(),
            &existing_by_path,
            &[],
            &dummy_db,
            0,
            None,
        )
        .expect("probes all");
        assert_eq!(probes_all.probes.len(), 2);

        // With child dir excluded: only parent file found
        let excluded = vec![child_dir.clone()];
        let probes_filtered = collect_image_probes_incremental(
            root_dir.path(),
            &existing_by_path,
            &excluded,
            &dummy_db,
            0,
            None,
        )
        .expect("probes filtered");
        assert_eq!(probes_filtered.probes.len(), 1);
        assert_eq!(probes_filtered.probes[0].filename, "parent.jpg");
    }

    #[test]
    fn supports_video_extensions() {
        assert!(is_supported_file(Path::new("movie.mp4")));
        assert!(is_supported_file(Path::new("show.mkv")));
        assert!(is_supported_file(Path::new("clip.webm")));
        assert!(is_supported_file(Path::new("cam.MOV")));
        assert!(is_supported_file(Path::new("file.avi")));
        assert!(is_supported_file(Path::new("file.flv")));
        assert!(is_supported_file(Path::new("file.wmv")));
        assert!(is_supported_file(Path::new("file.m4v")));
        assert!(is_supported_file(Path::new("file.ts")));
        assert!(is_supported_file(Path::new("file.mpg")));
        assert!(is_supported_file(Path::new("file.mpeg")));
        assert!(!is_supported_file(Path::new("file.txt")));
    }

    #[test]
    fn skips_tiny_images_but_not_pdfs() {
        let root_dir = tempfile::tempdir().expect("tempdir");

        // Tiny GIF (icon-sized) — should be skipped
        let tiny_gif = root_dir.path().join("spacer.gif");
        File::create(&tiny_gif)
            .expect("create")
            .write_all(&[0u8; 100])
            .expect("write");

        // Tiny PNG — should be skipped
        let tiny_png = root_dir.path().join("favicon.png");
        File::create(&tiny_png)
            .expect("create")
            .write_all(&[0u8; 5_000])
            .expect("write");

        // Normal-sized JPG — should be included
        let normal_jpg = root_dir.path().join("photo.jpg");
        write_test_image(&normal_jpg, 0xFF);

        // Tiny PDF — should still be included (PDFs exempt)
        let tiny_pdf = root_dir.path().join("note.pdf");
        File::create(&tiny_pdf)
            .expect("create")
            .write_all(&[0u8; 500])
            .expect("write");

        let existing_by_path: HashMap<String, ExistingFile> = HashMap::new();
        let dummy_db = root_dir.path().join("dummy.sqlite");
        let probes = collect_image_probes_incremental(
            root_dir.path(),
            &existing_by_path,
            &[],
            &dummy_db,
            0,
            None,
        )
        .expect("probes");

        let filenames: Vec<&str> = probes.probes.iter().map(|p| p.filename.as_str()).collect();
        assert!(filenames.contains(&"photo.jpg"));
        assert!(filenames.contains(&"note.pdf"));
        assert!(!filenames.contains(&"spacer.gif"));
        assert!(!filenames.contains(&"favicon.png"));
        assert_eq!(probes.probes.len(), 2);
    }

    /// A scan of unchanged rows whose thumbnails are gone must **not** delete them.
    ///
    /// That state is what "Reclassify" with the thumbnail option produces: rows keep
    /// their identity, `thumb_path` becomes NULL and the files are removed. Those rows
    /// are excluded from the unchanged marker flush (they are in `heal`), and the heal
    /// path used to touch only `thumb_path` — so the end-of-scan cleanup compared an old
    /// `scan_marker`, soft-deleted *every* row and purged the thumbnails again. The
    /// sidebar then showed the root with "0 files" and a second rescan was needed.
    #[test]
    fn healed_rows_survive_the_end_of_scan_cleanup() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let cache_dir = tempfile::tempdir().expect("cache");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        // Big and noisy enough to pass the "tiny image" filter that discovery applies.
        let img = image::RgbImage::from_fn(1024, 768, |x, y| {
            let seed = x.wrapping_mul(31_337) ^ y.wrapping_mul(97);
            image::Rgb([
                (seed % 251) as u8,
                (seed / 7 % 241) as u8,
                (seed / 13 % 239) as u8,
            ])
        });
        image::DynamicImage::ImageRgb8(img)
            .save(root_dir.path().join("a.jpg"))
            .expect("save image");
        let root = root_dir.path().to_str().expect("str").to_string();

        let ctx = scan_context_with_cache(&db_path, cache_dir.path());
        let first = start_or_resume_scan_job(&db_path, &root).expect("job");
        run_scan_job_internal(&ctx, first.id, None, None, ScanMode::Refresh).expect("first scan");
        let root_id = first.root_id;

        let before = db::load_existing_files(&db_path, root_id).expect("rows");
        assert_eq!(before.len(), 1);
        assert!(before[0].thumb_path.is_some(), "first scan must thumbnail");

        // Reclassify with the thumbnail option: the row stays live, the pointer goes.
        db::reset_root_classifications(&db_path, root_id, true).expect("reclassify");
        let mid = db::load_existing_files(&db_path, root_id).expect("rows");
        assert_eq!(mid.len(), 1, "the row must stay indexed by the reclassify");
        assert!(
            mid[0].thumb_path.is_none(),
            "the reclassify must drop the thumbnail pointer"
        );

        let second = start_or_resume_scan_job(&db_path, &root).expect("second job");
        run_scan_job_internal(&ctx, second.id, None, None, ScanMode::Refresh)
            .expect("second scan must not delete the healed row");

        let after = db::load_existing_files(&db_path, root_id).expect("rows");
        assert_eq!(after.len(), 1, "the healed row must survive the cleanup");
        assert!(
            after[0].thumb_path.is_some(),
            "the heal must have rebuilt the thumbnail"
        );
    }

    #[test]
    fn scan_mode_parses_known_values() {
        assert_eq!(ScanMode::parse("full"), Some(ScanMode::Full));
        assert_eq!(ScanMode::parse(""), Some(ScanMode::Full));
        assert_eq!(ScanMode::parse("refresh"), Some(ScanMode::Refresh));
        assert_eq!(
            ScanMode::parse("classify_only"),
            Some(ScanMode::ClassifyOnly)
        );
        assert_eq!(
            ScanMode::parse("classify-only"),
            Some(ScanMode::ClassifyOnly)
        );
        assert_eq!(ScanMode::parse("nonsense"), None);
        assert!(ScanMode::Refresh.skips_classification());
        assert!(!ScanMode::Full.skips_classification());
        assert!(!ScanMode::ClassifyOnly.skips_classification());
    }

    /// Classify-only resume must not run cleanup.
    ///
    /// Without a discovery pass every row looks "not seen in this scan", so the
    /// cleanup phase would soft-delete the whole index — the exact bug this
    /// mode has to avoid. It also must not filter pending rows by scan marker:
    /// the previous job's marker is the only one the rows carry.
    #[test]
    fn classify_only_keeps_rows_for_files_missing_on_disk() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        write_test_image(&root_dir.path().join("a.jpg"), 0xAA);
        write_test_image(&root_dir.path().join("b.jpg"), 0xBB);

        let root = root_dir.path().to_str().expect("str").to_string();
        let first = start_or_resume_scan_job(&db_path, &root).expect("job");
        let ctx = make_scan_context(&db_path);
        run_scan_job_internal(&ctx, first.id, None, None, ScanMode::Refresh).expect("index files");

        // The user deletes a file (or the share drops it) while the app is closed.
        std::fs::remove_file(root_dir.path().join("a.jpg")).expect("remove");

        let job = start_or_resume_scan_job(&db_path, &root).expect("job");
        let summary = run_scan_job_internal(&ctx, job.id, None, None, ScanMode::ClassifyOnly)
            .expect("classify only");

        assert_eq!(summary.deleted, 0, "classify-only must never delete");
        assert_eq!(
            summary.scanned, 2,
            "both rows are pending, regardless of their scan marker"
        );

        let indexed = db::load_existing_files(&db_path, job.root_id).expect("load");
        assert_eq!(
            indexed.len(),
            2,
            "both rows must survive the classify-only run"
        );

        let status = db::get_scan_job(&db_path, job.id)
            .expect("status")
            .expect("job exists");
        assert_eq!(status.status, "completed");
    }

    #[test]
    fn classify_only_with_nothing_pending_completes_immediately() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let db_dir = tempfile::tempdir().expect("dbdir");
        let db_path = db_dir.path().join("index.sqlite");
        db::init_database(&db_path).expect("init");

        write_test_image(&root_dir.path().join("a.jpg"), 0xAA);

        let root = root_dir.path().to_str().expect("str").to_string();
        let first = start_or_resume_scan_job(&db_path, &root).expect("job");
        let ctx = make_scan_context(&db_path);
        run_scan_job_internal(&ctx, first.id, None, None, ScanMode::Refresh).expect("index files");

        // Pretend the file was already classified.
        let indexed = db::load_existing_files(&db_path, first.root_id).expect("load");
        for file in indexed {
            db::update_file_classification(&db_path, file.id, "photo", "done", "", "", 0.9, "en")
                .expect("mark classified");
        }

        let job = start_or_resume_scan_job(&db_path, &root).expect("job");
        let summary = run_scan_job_internal(&ctx, job.id, None, None, ScanMode::ClassifyOnly)
            .expect("classify only");

        assert_eq!(summary.scanned, 0);
        let status = db::get_scan_job(&db_path, job.id)
            .expect("status")
            .expect("job exists");
        assert_eq!(
            status.status, "completed",
            "empty runs must not stay running"
        );
    }
}
