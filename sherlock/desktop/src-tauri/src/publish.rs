//! Publishes a read-only snapshot of the library for the web client.
//!
//! The desktop owns the index; the web app only reads. Between the two there is a
//! network share, and SQLite must never be opened there while it is being written:
//! WAL plus a share means advisory locking that the server may not honour, plus a
//! `-wal`/`-shm` pair nobody is cleaning up. So this module never exports the live
//! database — it exports a **snapshot** (`VACUUM INTO`, one file, no journal) and
//! points at it with an atomically replaced `current.json`.
//!
//! Layout of the publish folder:
//!
//! ```text
//! <PUBLISH_DIR>/
//!   current.json                       the revision that is live
//!   index.<rev>.sqlite                 immutable snapshot
//!   thumbs/<root_key>/<rel_path>.jpg   same tree as the local cache
//!   face_crops/<file_id>_<n>.jpg       flattened: the basename is unique
//! ```
//!
//! Every upload is written next to its destination first and renamed into place, so
//! a reader either sees the previous revision or the new one — never a torn file and
//! never a half-copied tree. Older revisions are pruned only *after* the pointer has
//! moved, and a failure to delete one is a warning, not an error: the web may still
//! have it open.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::config::AppPaths;
use crate::error::{AppError, AppResult};

/// Pointer file the web reads to discover which revision is live.
pub const CURRENT_POINTER: &str = "current.json";
pub const THUMBS_DIR: &str = "thumbs";
pub const FACE_CROPS_DIR: &str = "face_crops";
const SNAPSHOT_PREFIX: &str = "index.";
const SNAPSHOT_SUFFIX: &str = ".sqlite";
/// Bumped when the meaning of a field in `current.json` changes.
const PUBLISH_FORMAT: u32 = 1;

#[derive(Debug, Clone)]
pub struct PublishOptions {
    /// Destination folder (usually a mount of the network share).
    pub dir: PathBuf,
    /// Publish the face crops as well. Off saves space; the web then has no face
    /// cards, and the people gallery falls back to whole photos.
    pub include_face_crops: bool,
    /// How many snapshots to keep, newest included. 1 is enough when the web is
    /// allowed to reopen immediately; 2 is the default so a reader that is mid-query
    /// during a publish is never the reason a file has to be deleted.
    pub keep_revisions: u32,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    pub revision: String,
    pub cancelled: bool,
    pub database_bytes: u64,
    pub database_sha256: String,
    pub thumbnails_copied: u64,
    pub thumbnails_unchanged: u64,
    pub thumbnails_removed: u64,
    pub thumbnails_bytes: u64,
    pub face_crops_copied: u64,
    pub face_crops_removed: u64,
    pub face_crops_bytes: u64,
    pub revisions_pruned: u64,
    pub elapsed_ms: u64,
}

/// What the UI shows while a publish runs.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishProgress {
    /// `snapshot`, `copy`, `thumbs`, `faces`, `pointer` or `prune`.
    pub phase: String,
    pub done: u64,
    pub total: u64,
    /// File currently being written, for the log line under the progress bar.
    pub current: String,
}

impl PublishOptions {
    fn keep(&self) -> usize {
        // A retention of 0 would delete the revision that is being published.
        self.keep_revisions.max(1) as usize
    }
}

/// Copy the index and the caches to `options.dir` and move the pointer.
///
/// `cancel` is checked between files: a publish that is interrupted has already
/// written a snapshot nobody points at, so it removes it instead of leaving a
/// revision that the next prune would keep over a referenced one.
pub fn publish_library(
    paths: &AppPaths,
    options: &PublishOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(PublishProgress),
) -> AppResult<PublishReport> {
    let started = Instant::now();
    prepare_destination(&options.dir, &paths.base_dir)?;

    let revision = next_revision(&options.dir)?;
    let mut report = PublishReport {
        revision: revision.clone(),
        ..Default::default()
    };

    report_phase(on_progress, "snapshot", 0, 1, String::new());
    let local_snapshot = paths.tmp_dir.join(format!("publish-{revision}.sqlite"));
    let _ = fs::remove_file(&local_snapshot);
    snapshot_database(&paths.db_file, &local_snapshot)?;

    report_phase(on_progress, "copy", 0, 1, snapshot_file_name(&revision));
    let destination = options.dir.join(snapshot_file_name(&revision));
    let partial = destination.with_extension("sqlite.tmp");
    let (bytes, sha256) = copy_hashed(&local_snapshot, &partial)?;
    fs::rename(&partial, &destination)?;
    let _ = fs::remove_file(&local_snapshot);
    report.database_bytes = bytes;
    report.database_sha256 = sha256;

    if cancel.load(Ordering::Relaxed) {
        return Ok(cancelled(report, &destination, started));
    }

    let thumbs = sync_tree(
        &paths.thumbnails_dir,
        &options.dir.join(THUMBS_DIR),
        "thumbs",
        cancel,
        on_progress,
    )?;
    report.thumbnails_copied = thumbs.copied;
    report.thumbnails_unchanged = thumbs.unchanged;
    report.thumbnails_removed = thumbs.removed;
    report.thumbnails_bytes = thumbs.bytes;

    if cancel.load(Ordering::Relaxed) {
        return Ok(cancelled(report, &destination, started));
    }

    if options.include_face_crops {
        let faces = sync_tree(
            &paths.face_crops_dir,
            &options.dir.join(FACE_CROPS_DIR),
            "faces",
            cancel,
            on_progress,
        )?;
        report.face_crops_copied = faces.copied;
        report.face_crops_removed = faces.removed;
        report.face_crops_bytes = faces.bytes;
    } else {
        // A folder left behind from an earlier publish with crops enabled would keep
        // serving portraits the current configuration says not to publish.
        let _ = fs::remove_dir_all(options.dir.join(FACE_CROPS_DIR));
    }

    if cancel.load(Ordering::Relaxed) {
        return Ok(cancelled(report, &destination, started));
    }

    report_phase(on_progress, "pointer", 0, 1, CURRENT_POINTER.to_string());
    let pointer = build_pointer(paths, options, &revision, &report)?;
    write_pointer(&options.dir, &pointer)?;

    report_phase(on_progress, "prune", 0, 1, String::new());
    report.revisions_pruned = prune_revisions(&options.dir, options.keep())?;
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

fn cancelled(mut report: PublishReport, snapshot: &Path, started: Instant) -> PublishReport {
    // Nobody points at this revision yet, so removing it is safe and keeps the next
    // prune from evicting a revision the web is actually reading.
    if let Err(e) = fs::remove_file(snapshot) {
        log::warn!(
            "publish: could not remove cancelled snapshot {}: {e}",
            snapshot.display()
        );
    }
    report.cancelled = true;
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report
}

fn report_phase(
    on_progress: &mut dyn FnMut(PublishProgress),
    phase: &str,
    done: u64,
    total: u64,
    current: String,
) {
    on_progress(PublishProgress {
        phase: phase.to_string(),
        done,
        total,
        current,
    });
}

/// Reject destinations that would publish on top of the app's own data.
fn prepare_destination(dir: &Path, base_dir: &Path) -> AppResult<()> {
    if dir.as_os_str().is_empty() {
        return Err(AppError::Config(
            "no publish folder configured; pick one in Settings".to_string(),
        ));
    }
    fs::create_dir_all(dir).map_err(|e| {
        AppError::Config(format!(
            "cannot use {} as the publish folder: {e}",
            dir.display()
        ))
    })?;
    let destination = fs::canonicalize(dir)?;
    if let Ok(base) = fs::canonicalize(base_dir) {
        if destination.starts_with(&base) {
            return Err(AppError::Config(format!(
                "refusing to publish inside the app data folder ({}): pick a folder on the \
                 shared drive, outside it",
                base.display()
            )));
        }
    }
    Ok(())
}

fn snapshot_file_name(revision: &str) -> String {
    format!("{SNAPSHOT_PREFIX}{revision}{SNAPSHOT_SUFFIX}")
}

/// Pick a revision id that does not collide with a snapshot already in the folder.
///
/// The id is always `<timestamp>-<sequence>`, never a bare timestamp: the sequence is
/// what keeps two publishes inside the same second ordered, and `prune_revisions`
/// decides what to delete by sorting these ids as strings. A suffix only appended on
/// collision (`…Z-2`) would sort *before* `…Z.sqlite`, because `-` comes before `.`,
/// and the first publish of the second would be the one pruned.
fn next_revision(dir: &Path) -> AppResult<String> {
    let base = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for sequence in 1..1000 {
        let candidate = format!("{base}-{sequence:03}");
        if !dir.join(snapshot_file_name(&candidate)).exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Config(
        "could not allocate a revision id in the publish folder".to_string(),
    ))
}

/// `VACUUM INTO` the live index and verify the result before it leaves the machine.
///
/// The app keeps the database in WAL, so a plain file copy can miss committed pages
/// that are still in `-wal`. `VACUUM INTO` writes a single, consistent file with no
/// journal — exactly the shape the web can open read-only over a share.
fn snapshot_database(db_path: &Path, target: &Path) -> AppResult<()> {
    {
        let conn = Connection::open(db_path)?;
        conn.execute("VACUUM INTO ?1", params![target.to_string_lossy()])?;
    }

    let snapshot = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    // The output of VACUUM INTO is already in delete mode; setting it again costs
    // nothing and makes the invariant explicit for whoever edits this next.
    snapshot.pragma_update(None, "journal_mode", "DELETE")?;
    let integrity: String = snapshot.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(AppError::Config(format!(
            "the snapshot failed quick_check ({integrity}); nothing was published"
        )));
    }
    Ok(())
}

/// Stream `src` to `dst`, hashing as it goes.
///
/// One read pass serves both the copy and the checksum the pointer publishes, which
/// is the difference between "verify this 300 MB file" and "read it twice over SMB".
fn copy_hashed(src: &Path, dst: &Path) -> AppResult<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut reader = BufReader::new(fs::File::open(src)?);
    let mut writer = fs::File::create(dst)?;
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total = 0u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
        total += read as u64;
    }
    writer.sync_all()?;
    Ok((total, hex::encode(hasher.finalize())))
}

#[derive(Debug, Default)]
struct SyncOutcome {
    copied: u64,
    unchanged: u64,
    removed: u64,
    bytes: u64,
}

/// Mirror `src` into `dst`, copying only what changed and deleting what disappeared.
///
/// Thumbnails and crops are immutable once written — a changed photo gets a new file
/// with a new mtime — so size plus second-resolution mtime is enough to skip a file,
/// and a full re-hash would cost more than the copy it avoids over SMB.
fn sync_tree(
    src: &Path,
    dst: &Path,
    phase: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(PublishProgress),
) -> AppResult<SyncOutcome> {
    let mut outcome = SyncOutcome::default();
    if !src.is_dir() {
        // Nothing to publish yet (no thumbnail was ever generated). An empty mirror is
        // the correct answer, and it also removes a stale one from a previous run.
        remove_tree_contents(dst)?;
        return Ok(outcome);
    }

    let entries: Vec<PathBuf> = WalkDir::new(src)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect();
    let total = entries.len() as u64;

    let mut seen = std::collections::HashSet::with_capacity(entries.len());
    for (index, source) in entries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let rel = source.strip_prefix(src).map_err(|_| {
            AppError::Config(format!("{} is outside {}", source.display(), src.display()))
        })?;
        seen.insert(rel.to_path_buf());

        let target = dst.join(rel);
        let source_meta = fs::metadata(source)?;
        if is_up_to_date(&source_meta, &target) {
            outcome.unchanged += 1;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("creating", parent, error))?;
            }
            // The copy brings contents only, **not** the modification time. Without the
            // stamp the destination carries the copy's own timestamp, so the very next
            // publish sees a difference and copies the file again — the incremental sync
            // would rewrite the whole cache every time.
            copy_file(source, &target, source_meta.modified().ok())?;
            outcome.copied += 1;
            outcome.bytes += source_meta.len();
        }

        if index % 25 == 0 || index + 1 == entries.len() {
            on_progress(PublishProgress {
                phase: phase.to_string(),
                done: index as u64 + 1,
                total,
                current: rel.to_string_lossy().to_string(),
            });
        }
    }

    if !cancel.load(Ordering::Relaxed) {
        outcome.removed = sweep_tree(dst, &seen, total, on_progress)?;
    }
    Ok(outcome)
}

/// Copy `source` over `target`, keeping the kernel's fast path when the filesystem has
/// one and falling back to a read/write loop when it does not.
///
/// `fs::copy` starts with `copy_file_range`/`sendfile`, and a GVFS mount over SMB — the
/// usual home of the publish folder — refuses it with `EOPNOTSUPP` (os error 95). The
/// publish died on its very first thumbnail with nothing but "io error: Operation not
/// supported": the snapshot uses its own read/write loop, so that copy worked and the
/// failure looked like a broken share. Streaming works on every filesystem, and these
/// files are small enough that the syscall count does not matter.
fn copy_file(source: &Path, target: &Path, modified: Option<SystemTime>) -> AppResult<()> {
    match fs::copy(source, target) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
            log::debug!(
                "publish: {} refused the kernel copy ({error}), streaming it instead",
                target.display()
            );
            stream_copy(source, target)?;
        }
        Err(error) => return Err(copy_error(source, target, error)),
    }
    // Only once the destination is closed: see `preserve_mtime`.
    preserve_mtime(modified, target);
    Ok(())
}

fn stream_copy(source: &Path, target: &Path) -> AppResult<()> {
    let mut reader =
        BufReader::new(fs::File::open(source).map_err(|e| copy_error(source, target, e))?);
    let mut writer = fs::File::create(target).map_err(|e| copy_error(source, target, e))?;
    std::io::copy(&mut reader, &mut writer).map_err(|e| copy_error(source, target, e))?;
    Ok(())
}

/// Name both ends of a failed copy: the bare `io::Error` used to reach the UI as
/// "Operation not supported", with no hint of which of 34 000 thumbnails caused it.
fn copy_error(source: &Path, target: &Path, error: std::io::Error) -> AppError {
    AppError::Io(std::io::Error::new(
        error.kind(),
        format!(
            "copying {} to {}: {error}",
            source.display(),
            target.display()
        ),
    ))
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> AppError {
    AppError::Io(std::io::Error::new(
        error.kind(),
        format!("{action} {}: {error}", path.display()),
    ))
}

/// Stamp `target` with the source's modification time, best effort.
///
/// A failure only means the next publish copies that file again; it must never fail
/// the publish itself.
///
/// Two details are load-bearing on a network share, and both were measured against a
/// gvfs/SMB mount:
///
/// * the stamp has to happen **after the writer is closed**. Doing it on the handle that
///   wrote the file returns `Ok` and reads back correctly, and then the gvfs writeback
///   replaces it with the copy time — the incremental sync silently degrades to a full
///   copy of 34 000 thumbnails on every publish;
/// * that reopen cannot be write-only, which gvfs refuses with `EOPNOTSUPP`. A read
///   handle is enough for `futimens` (ownership, not write access, is what it needs), so
///   read-only and then append are the fallbacks.
fn preserve_mtime(modified: Option<SystemTime>, target: &Path) {
    let Some(modified) = modified else {
        return;
    };
    let stamp = |file: &fs::File| file.set_times(fs::FileTimes::new().set_modified(modified));

    // Lazily, one at a time: opening all three up front would cost 68 000 extra syscalls
    // on a first publish.
    let openers: [(&str, fn(&Path) -> std::io::Result<fs::File>); 3] = [
        ("write", |path| fs::File::options().write(true).open(path)),
        ("read", |path| fs::File::open(path)),
        ("append", |path| fs::File::options().append(true).open(path)),
    ];
    let mut error = None;
    for (mode, open) in openers {
        match open(target) {
            Ok(file) => match stamp(&file) {
                Ok(()) => return,
                Err(e) => error = Some(format!("setting the mtime through a {mode} handle: {e}")),
            },
            Err(e) => error = Some(format!("reopening it {mode}-only: {e}")),
        }
    }
    log::debug!(
        "publish: could not stamp {}: {}",
        target.display(),
        error.unwrap_or_else(|| "no reopen mode worked".to_string())
    );
}

fn is_up_to_date(source: &fs::Metadata, target: &Path) -> bool {
    let Ok(target_meta) = fs::metadata(target) else {
        return false;
    };
    if target_meta.len() != source.len() {
        return false;
    }
    let seconds = |time: std::io::Result<SystemTime>| {
        time.ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    // A network share may round mtimes; comparing at second resolution avoids
    // rewriting the whole cache on every publish for a 1 ns difference.
    seconds(source.modified()) == seconds(target_meta.modified())
}

/// Delete files under `dst` that are not in `keep` (relative paths seen in the source),
/// then drop the directories the sweep left empty.
///
/// This walks the entire destination — 34 000 files on a real library — immediately after
/// the copy reported `done == total`. It used to run inside the `thumbs`/`faces` phase
/// with no progress at all, so the screen sat at 100 % while the share answered one
/// `readdir`/`unlink` at a time, which is indistinguishable from a hang and got the app
/// closed mid-publish. It now reports a `sweep` phase, and collects the directories during
/// the same walk instead of walking the tree a second time.
fn sweep_tree(
    dst: &Path,
    keep: &std::collections::HashSet<PathBuf>,
    expected: u64,
    on_progress: &mut dyn FnMut(PublishProgress),
) -> AppResult<u64> {
    if !dst.is_dir() {
        return Ok(0);
    }
    // Announced even when the tree is small, so the UI leaves "thumbs 34196/34196"
    // behind the moment the sweep starts instead of showing a frozen 100 %.
    on_progress(PublishProgress {
        phase: "sweep".to_string(),
        done: 0,
        total: expected,
        current: String::new(),
    });
    let mut removed = 0u64;
    let mut visited = 0u64;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(dst).into_iter().filter_map(|entry| entry.ok()) {
        if entry.file_type().is_dir() {
            if entry.path() != dst {
                dirs.push(entry.path().to_path_buf());
            }
            continue;
        }
        let path = entry.into_path();
        let Ok(rel) = path.strip_prefix(dst) else {
            continue;
        };
        visited += 1;
        if visited % 200 == 0 {
            on_progress(PublishProgress {
                phase: "sweep".to_string(),
                done: visited,
                total: expected,
                current: rel.to_string_lossy().to_string(),
            });
        }
        if keep.contains(rel) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            // A reader on the other side may hold it open for a second; the next
            // publish retries and nothing is corrupted by waiting.
            Err(e) => log::warn!("publish: could not remove orphan {}: {e}", path.display()),
        }
    }
    remove_empty_dirs(&mut dirs);
    Ok(removed)
}

fn remove_tree_contents(dst: &Path) -> AppResult<()> {
    if dst.is_dir() {
        sweep_tree(dst, &std::collections::HashSet::new(), 0, &mut |_| {})?;
    }
    Ok(())
}

/// Drop directories left empty after an orphan sweep, deepest first.
fn remove_empty_dirs(dirs: &mut Vec<PathBuf>) {
    // Deepest first: a parent only becomes empty after its children are gone.
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in dirs.iter() {
        let is_empty = fs::read_dir(dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if is_empty {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// Everything the web needs to know about this revision without opening the database.
fn build_pointer(
    paths: &AppPaths,
    options: &PublishOptions,
    revision: &str,
    report: &PublishReport,
) -> AppResult<serde_json::Value> {
    let roots = crate::db::list_roots(&paths.db_file)?;
    let schema_version = snapshot_schema_version(&options.dir.join(snapshot_file_name(revision)))?;
    let stats = crate::db::database_stats(&paths.db_file)?;
    let hidden = crate::db::hidden_count(&paths.db_file, &[])?;

    let roots_json: Vec<serde_json::Value> = roots
        .iter()
        .map(|root| {
            serde_json::json!({
                "id": root.id,
                "name": root.root_name,
                "path": root.root_path,
                "rootKey": crate::thumbnail::root_key(&root.root_path),
                "files": root.file_count,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "format": PUBLISH_FORMAT,
        "revision": revision,
        "generatedAtUnix": crate::db::now_epoch_secs_pub(),
        "schemaVersion": schema_version,
        "database": {
            "file": snapshot_file_name(revision),
            "bytes": report.database_bytes,
            "sha256": report.database_sha256,
        },
        "library": {
            "files": stats.files,
            "roots": stats.roots,
            "hiddenFiles": hidden,
        },
        "thumbnails": {
            "dir": THUMBS_DIR,
            "copied": report.thumbnails_copied,
            "unchanged": report.thumbnails_unchanged,
            "removed": report.thumbnails_removed,
            "bytes": report.thumbnails_bytes,
        },
        "faceCrops": {
            "dir": if options.include_face_crops { FACE_CROPS_DIR } else { "" },
            "copied": report.face_crops_copied,
            "bytes": report.face_crops_bytes,
        },
        "roots": roots_json,
        "publisher": {
            "app": "frank-sherlock",
            "version": env!("CARGO_PKG_VERSION"),
        },
    }))
}

fn snapshot_schema_version(snapshot: &Path) -> AppResult<u32> {
    let conn = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(version as u32)
}

/// Replace the pointer atomically: write, flush, rename.
///
/// The rename is what makes a reader see either the previous revision or this one.
/// `sync_all` before it is the difference between "the pointer exists" and "the
/// pointer exists and its bytes reached the server".
fn write_pointer(dir: &Path, pointer: &serde_json::Value) -> AppResult<()> {
    let data = serde_json::to_vec_pretty(pointer)
        .map_err(|e| AppError::Config(format!("could not serialize the pointer: {e}")))?;
    let partial = dir.join("current.json.tmp");
    {
        let mut file = fs::File::create(&partial)?;
        file.write_all(&data)?;
        file.sync_all()?;
    }
    fs::rename(&partial, dir.join(CURRENT_POINTER))?;
    Ok(())
}

/// Keep the newest `keep` snapshots, remove the rest.
///
/// Failures are warnings: the file may be open by the web app, and the next publish
/// tries again. Losing an unreferenced snapshot is not worth failing a publish over.
fn prune_revisions(dir: &Path, keep: usize) -> AppResult<u64> {
    let mut snapshots: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(SNAPSHOT_SUFFIX) {
            snapshots.push((name.clone(), entry.path()));
        } else if name.ends_with(".sqlite.tmp") {
            // Leftover of a publish that died between create and rename.
            let _ = fs::remove_file(entry.path());
        }
    }

    snapshots.sort_by(|a, b| a.0.cmp(&b.0));
    let mut removed = 0u64;
    if snapshots.len() > keep {
        for (name, path) in snapshots.iter().take(snapshots.len() - keep) {
            match fs::remove_file(path) {
                Ok(()) => removed += 1,
                Err(e) => log::warn!("publish: could not prune {name}: {e}"),
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FileRecordUpsert;

    fn test_paths(base: &Path) -> AppPaths {
        let base_dir = base.join("app");
        let cache_dir = base_dir.join("cache");
        let paths = AppPaths {
            db_dir: base_dir.join("db"),
            db_file: base_dir.join("db").join("index.sqlite"),
            cache_dir: cache_dir.clone(),
            classification_cache_dir: cache_dir.join("classifications"),
            thumbnails_dir: cache_dir.join("thumbnails"),
            scans_dir: cache_dir.join("scans"),
            surya_venv_dir: base_dir.join("surya_venv"),
            raw_venv_dir: base_dir.join("raw_venv"),
            tmp_dir: cache_dir.join("tmp"),
            models_dir: base_dir.join("models"),
            face_crops_dir: cache_dir.join("face_crops"),
            base_dir,
        };
        crate::config::prepare_dirs(&paths).expect("dirs");
        paths
    }

    fn seed(paths: &AppPaths, thumbs: &[&str]) -> (i64, i64) {
        crate::db::init_database(&paths.db_file).expect("init");
        let root_id = crate::db::upsert_root(&paths.db_file, "/tmp/photos").expect("root");
        let root_key = crate::thumbnail::root_key("/tmp/photos");

        let mut last = 0;
        for (index, rel) in thumbs.iter().enumerate() {
            let record = FileRecordUpsert {
                root_id,
                rel_path: rel.to_string(),
                abs_path: format!("/tmp/photos/{rel}"),
                filename: format!("file{index}.jpg"),
                media_type: "photo".to_string(),
                description: format!("photo {index}"),
                extracted_text: String::new(),
                canonical_mentions: String::new(),
                confidence: 0.5,
                lang_hint: "en".to_string(),
                mtime_ns: 1_700_000_000_000_000_000 + index as i64,
                size_bytes: 100,
                fingerprint: format!("fp-{index}"),
                scan_marker: 1,
                location_text: String::new(),
                dhash: None,
                duration_secs: None,
                video_width: None,
                video_height: None,
                video_codec: None,
                audio_codec: None,
            };
            last = crate::db::upsert_file_record(&paths.db_file, &record).expect("row");

            let thumb = paths.thumbnails_dir.join(&root_key).join(rel);
            fs::create_dir_all(thumb.parent().expect("thumb parent")).expect("thumb dir");
            fs::write(&thumb, format!("thumb-{index}")).expect("thumb");
        }
        (root_id, last)
    }

    fn options(dir: &Path) -> PublishOptions {
        PublishOptions {
            dir: dir.to_path_buf(),
            include_face_crops: true,
            keep_revisions: 2,
        }
    }

    #[test]
    fn publishes_a_readable_snapshot_and_a_pointer() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg", "Trip/b.jpg"]);
        let publish_dir = temp.path().join("share");

        let report = publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("publish");

        let snapshot = publish_dir.join(snapshot_file_name(&report.revision));
        assert!(snapshot.is_file(), "the snapshot must exist");
        assert!(
            !publish_dir
                .join(format!("{}.tmp", snapshot.display()))
                .exists(),
            "no partial file may be left behind"
        );

        let conn = Connection::open_with_flags(&snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open snapshot");
        let files: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(files, 2);
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal.to_lowercase(), "delete");

        let pointer: serde_json::Value =
            serde_json::from_slice(&fs::read(publish_dir.join(CURRENT_POINTER)).expect("pointer"))
                .expect("json");
        assert_eq!(pointer["revision"], report.revision);
        assert_eq!(
            pointer["database"]["file"],
            snapshot_file_name(&report.revision)
        );
        assert_eq!(pointer["library"]["files"], 2);
        assert_eq!(
            pointer["roots"][0]["rootKey"],
            crate::thumbnail::root_key("/tmp/photos")
        );
        assert!(report.database_sha256.len() == 64, "sha256 of the snapshot");
        assert_eq!(report.thumbnails_copied, 2);
    }

    /// The incremental sync depends on the destination carrying the source's mtime.
    ///
    /// `fs::copy` does not preserve it, so this pins the fix: without the explicit
    /// timestamp the comparison always fails and every publish rewrites the cache.
    #[test]
    fn copied_files_keep_the_source_mtime() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("a.jpg");
        let target = temp.path().join("out").join("a.jpg");
        fs::create_dir_all(target.parent().expect("parent")).expect("dir");
        fs::write(&source, b"thumb").expect("write");

        // An old, whole-second timestamp, so the copy (which happens now) cannot
        // accidentally land in the same second and hide the bug.
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        fs::File::options()
            .write(true)
            .open(&source)
            .expect("open")
            .set_times(fs::FileTimes::new().set_modified(old))
            .expect("set mtime");

        copy_file(
            &source,
            &target,
            fs::metadata(&source).expect("meta").modified().ok(),
        )
        .expect("copy");

        let source_secs = fs::metadata(&source)
            .expect("meta")
            .modified()
            .expect("mtime")
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("epoch")
            .as_secs();
        let target_secs = fs::metadata(&target)
            .expect("meta")
            .modified()
            .expect("mtime")
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("epoch")
            .as_secs();
        assert_eq!(source_secs, target_secs);
    }

    /// The fallback taken on a share that refuses `copy_file_range`: exact bytes, and a
    /// pre-existing longer file is truncated, not left with a tail of the old copy.
    #[test]
    fn stream_copy_replaces_a_longer_target() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("a.jpg");
        let target = temp.path().join("b.jpg");
        fs::write(&source, b"new").expect("write source");
        fs::write(&target, b"a much longer previous copy").expect("write target");

        stream_copy(&source, &target).expect("stream copy");

        assert_eq!(fs::read(&target).expect("read"), b"new");
    }

    #[test]
    fn copy_file_reports_the_path_it_could_not_copy() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("a.jpg");
        let target = temp.path().join("b.jpg");
        fs::write(&source, b"bytes").expect("write");
        copy_file(&source, &target, None).expect("copy");
        assert_eq!(fs::read(&target).expect("read"), b"bytes");

        let missing = temp.path().join("gone.jpg");
        let error = copy_file(&missing, &target, None).expect_err("missing source must fail");
        assert!(
            error.to_string().contains("gone.jpg"),
            "the message has to name the file: {error}"
        );
    }

    #[test]
    fn second_publish_copies_only_what_changed() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg", "b.jpg"]);
        let publish_dir = temp.path().join("share");

        let first = publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("first");
        assert_eq!(first.thumbnails_copied, 2);

        let second = publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("second");
        assert_eq!(second.thumbnails_copied, 0, "nothing changed");
        assert_eq!(second.thumbnails_unchanged, 2);
        assert_ne!(
            first.revision, second.revision,
            "each publish is a revision"
        );
    }

    #[test]
    fn deletes_orphaned_thumbnails_from_the_destination() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg", "b.jpg"]);
        let publish_dir = temp.path().join("share");
        publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("first");

        let root_key = crate::thumbnail::root_key("/tmp/photos");
        fs::remove_file(paths.thumbnails_dir.join(&root_key).join("b.jpg")).expect("remove source");

        let second = publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("second");

        assert_eq!(second.thumbnails_removed, 1);
        assert!(!publish_dir
            .join(THUMBS_DIR)
            .join(&root_key)
            .join("b.jpg")
            .exists());
        assert!(publish_dir
            .join(THUMBS_DIR)
            .join(&root_key)
            .join("a.jpg")
            .exists());
    }

    #[test]
    /// The sweep walks the whole destination after the copy says 100 %, so it has to say
    /// what it is doing — that phase is what keeps the screen from looking hung.
    #[test]
    fn the_orphan_sweep_reports_its_own_phase() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg", "b.jpg"]);
        let publish_dir = temp.path().join("share");
        publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("first");

        let mut phases: Vec<String> = Vec::new();
        publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |update| {
                if !phases.contains(&update.phase) {
                    phases.push(update.phase);
                }
            },
        )
        .expect("second");

        assert!(
            phases.iter().any(|phase| phase == "sweep"),
            "the sweep must announce itself: {phases:?}"
        );
    }

    #[test]
    fn keeps_the_configured_number_of_revisions() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg"]);
        let publish_dir = temp.path().join("share");
        let mut opts = options(&publish_dir);
        opts.keep_revisions = 1;

        let first =
            publish_library(&paths, &opts, &AtomicBool::new(false), &mut |_| {}).expect("first");
        let second =
            publish_library(&paths, &opts, &AtomicBool::new(false), &mut |_| {}).expect("second");

        assert_eq!(second.revisions_pruned, 1);
        assert!(!publish_dir
            .join(snapshot_file_name(&first.revision))
            .exists());
        assert!(publish_dir
            .join(snapshot_file_name(&second.revision))
            .exists());
    }

    #[test]
    fn refuses_to_publish_inside_the_app_data_folder() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg"]);
        let inside = paths.cache_dir.join("publish");

        let error = publish_library(
            &paths,
            &options(&inside),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect_err("must refuse");
        assert!(error.to_string().contains("app data folder"), "{error}");
    }

    #[test]
    fn cancelling_leaves_no_unreferenced_revision() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        seed(&paths, &["a.jpg"]);
        let publish_dir = temp.path().join("share");
        let cancel = AtomicBool::new(true);

        let report =
            publish_library(&paths, &options(&publish_dir), &cancel, &mut |_| {}).expect("publish");

        assert!(report.cancelled);
        assert!(
            !publish_dir.join(CURRENT_POINTER).exists(),
            "pointer untouched"
        );
        let leftovers: Vec<_> = fs::read_dir(&publish_dir)
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(SNAPSHOT_PREFIX)
            })
            .collect();
        assert!(leftovers.is_empty(), "the cancelled snapshot is removed");
    }

    #[test]
    fn publishes_hidden_rows_in_the_snapshot() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = test_paths(temp.path());
        let (_, file_id) = seed(&paths, &["a.jpg", "b.jpg"]);
        crate::db::set_files_hidden(&paths.db_file, &[file_id], true).expect("hide");
        let publish_dir = temp.path().join("share");

        publish_library(
            &paths,
            &options(&publish_dir),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("publish");

        let pointer: serde_json::Value =
            serde_json::from_slice(&fs::read(publish_dir.join(CURRENT_POINTER)).expect("pointer"))
                .expect("json");
        assert_eq!(pointer["library"]["hiddenFiles"], 1);

        // The row travels in the snapshot with its timestamp: the web filters it, the
        // desktop keeps its index untouched.
        let revision = pointer["revision"].as_str().expect("revision");
        let conn = Connection::open_with_flags(
            publish_dir.join(snapshot_file_name(revision)),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open");
        let hidden: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE hidden_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(hidden, 1);
    }
}
