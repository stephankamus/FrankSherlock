# Draft: upstream issue + PR split

## Issue title

Unmounted network roots are purged at startup, and scans of an empty mountpoint soft-delete the whole index

## Environment

- Frank Sherlock 0.8.2 (AppImage)
- Ubuntu 24.04.5, photos on a SMB share opened through GNOME Files
- Root path was `/run/user/1000/gvfs/smb-share:server=openmediavault.local,share=fotos/Fotos`
- Library size: 35,882 indexed files

## Summary

Two independent code paths turn a transient network outage into permanent data loss. Both were reproduced on a real library; the `.bak` file later proved the index was intact and recoverable.

## Steps to reproduce

### A. Startup purge

1. Add a folder on a network share as a scan root, e.g. a GVFS path under `/run/user/1000/gvfs/smb-share:...` or an fstab mount under `/mnt/nas`.
2. Let a scan complete, so `files` is populated.
3. Reboot, log out, or run `gio mount -u smb://nas/fotos` so the path disappears.
4. Launch the app.

Actual: the root and every row in `files` are deleted, thumbnails and face crops are removed from disk, and the sidebar shows `Files: 0` while `DB size` stays large.

Expected: the root is kept and marked unreachable; the index remains browsable.

### B. Scan over an existing but empty mountpoint

1. Keep the root index populated.
2. Make the mountpoint directory exist but leave the share unmounted (an empty `/mnt/nas/fotos`).
3. Start a scan.

Actual: discovery returns zero files, `mark_missing_as_deleted` soft-deletes every row, and `cleanup_deleted_caches` deletes the thumbnails. A later scan re-inserts the files as new with `confidence = 0`, discarding every LLM description and OCR result.

Expected: the scan fails with a clear "root unavailable" error and changes nothing.

## Root cause

- `run()` calls `validate_and_purge_stale_roots` on every startup. It runs `!Path::new(&root.root_path).is_dir()` and then `purge_root`, which hard-deletes `files` (CASCADE also removes `face_detections`), `scan_jobs`, the `roots` row, thumbnails and face crops.
- `collect_image_probes_incremental` swallows `WalkDir` errors with `filter_map(|entry| entry.ok())`, so an unreachable root is indistinguishable from an empty one.
- Phase 4 then calls `mark_missing_as_deleted` unconditionally.
- `load_existing_files` filters `deleted_at IS NULL`, so soft-deleted rows are invisible to the next scan, and `upsert_file_record` overwrites `description`, `extracted_text`, `canonical_mentions`, `confidence` and `lang_hint` with the minimal thumbnail record.
- Roots are keyed by the exact `root_path` string. A share remounted at a different path creates a new root with no reuse and no way to relink.
- `DB size` reports the raw file length; SQLite never shrinks after `DELETE`, and nothing runs `VACUUM`, so a purged database still looks full.

## Impact

- Total loss of classification, OCR, dHash, album membership and face data for a root, from an event as ordinary as rebooting without the NAS mounted.
- Rebuilding costs a full LLM pass over the library.

## Proposed fix

Keep the behavior change small and reviewable:

1. Never delete roots automatically. Add `roots.availability`, `last_seen_at`, `last_error` (appended migration), probe with a bounded timeout, and surface an `Offline` badge plus a `Locate…` action.
2. Abort a scan before discovery when the root is unreachable, and abort again after discovery when zero files are found for a root that has indexed rows, or when fewer than half are found for roots with 100 or more rows.
3. Skip the cleanup phase whenever discovery reported filesystem errors.
4. Revive soft-deleted rows on rescan instead of re-inserting them, preserving classification.
5. Add `relink_root`, which rewrites `roots.root_path`, `scan_jobs.root_path` and `files.abs_path` in one transaction so a remount at a new path keeps the index.
6. Report logical size and freelist bytes, and offer a `Compact DB` action that runs `wal_checkpoint(TRUNCATE)`, FTS5 `optimize` and `VACUUM`, then enables incremental auto-vacuum.

## Workaround until released

- Mount the share before launching, or use an fstab entry with `x-systemd.automount,nofail`.
- Do not start a scan while the share is unreachable.
- If the index was already purged, check `~/.local/share/frank_sherlock/db/index.sqlite.bak` (plus any `-wal`) before rebuilding.

## PR split

Branch each from `main`, one topic per branch, and reference this issue.

| PR | Scope | Main files | Tests |
|---|---|---|---|
| 1 | Non-destructive root probing and the offline state | `db.rs`, `models.rs`, `lib.rs`, `platform/fs.rs`, `RootCard.tsx`, `Sidebar.tsx`, `types.ts` | probe keeps rows and flags offline; gvfs path round-trip; sidebar badge |
| 2 | Scan abort guards | `scan.rs` | empty discovery with indexed rows aborts; cleanup skipped on walk errors |
| 3 | Soft-delete revival | `db.rs`, `scan.rs` | restore keeps description, confidence and FTS |
| 4 | `relink_root` and the `Locate…` flow | `db.rs`, `lib.rs`, `App.tsx`, `api.ts` | prefix rewrite keeps classification; duplicate path rejected |
| 5 | Storage stats and compaction | `db.rs`, `lib.rs`, `Sidebar.tsx` | freelist cleared by VACUUM; logical size reported |
| 6 | Optional: per-root thumbnail layout | `thumbnail.rs`, `scan.rs` | separate PR, moves cache paths |

Gates before opening any PR:

```bash
cd sherlock/desktop/src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
cd sherlock/desktop && npx tsc --noEmit && npm run test
```

## Evidence from a real recovery

```text
live database after the purge : file 664 MB, roots 0, files 0, freelist 100% of pages
backup database               : file 34 MB, roots 1, files 35,882, quick_check ok
recovered copy after VACUUM    : file 33 MB, roots 1, files 35,882, FTS rows 35,882
```

After applying the fix, launching with the share unmounted produced:

```text
user_version : 14
roots        : 1
files        : 35,882
availability : [(2, 'offline', 'not a directory or unavailable: /run/user/1000/gvfs/...')]
```
