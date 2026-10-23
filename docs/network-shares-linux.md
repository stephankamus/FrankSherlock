# Network shares and NAS paths on Linux

Frank Sherlock stores one row per scan root in `roots.root_path`; every indexed file is identified by that root plus a relative path. Network shares must therefore be reachable when a scan runs, but they are **not** required for browsing an existing index anymore.

## How offline roots are handled

Historically the app deleted any root whose path was not a directory at startup, which wiped the index of a NAS share that was not mounted yet. That behavior is gone:

- Startup and refresh run a bounded reachability probe (`probe_roots`) and set `roots.availability` to `online` or `offline` with `last_seen_at` and `last_error`.
- Offline roots stay in the sidebar with an `Offline` badge showing how many files remain indexed, plus a `Locate…` action.
- A scan never starts when the root is unreachable. If discovery returns zero files, or fewer than half of the indexed files for a root with 100 or more entries, the job fails instead of soft-deleting the index.
- If discovery reports filesystem errors, the cleanup phase is skipped, so unreadable subtrees cannot cause deletions.

The remaining destructive action is explicit: `Remove folder`, which purges rows, thumbnails and face crops for that root. The database file is not shrunk by that; use `Compact DB`.

## Recommended: systemd automount over `/etc/fstab`

GVFS paths are tied to the login session and disappear on logout or reboot. A systemd automount gives a stable path that resolves lazily, so the app sees a normal directory.

`/etc/cifs-frank.cred`, owned `root:root` with mode `0600`:

```ini
username=stephan
password=CHANGE_ME
domain=WORKGROUP
```

`/etc/fstab`:

```
//nas.local/fotos  /mnt/nas/fotos  cifs  credentials=/etc/cifs-frank.cred,uid=1000,gid=1000,iocharset=utf8,vers=3.0,nofail,_netdev,x-systemd.automount,x-systemd.idle-timeout=600,x-systemd.mount-timeout=15  0  0
```

```bash
sudo mkdir -p /mnt/nas/fotos
sudo systemctl daemon-reload
sudo systemctl restart remote-fs.target
ls /mnt/nas/fotos   # triggers the automount
```

Why these options:

- `nofail` keeps boot from stalling when the NAS is down.
- `x-systemd.automount` mounts on first access, so `is_dir()` in the app triggers it.
- `x-systemd.mount-timeout=15` bounds a dead server. Combined with `soft` semantics at the kernel level, this stops indefinite I/O hangs.
- `uid`/`gid` make the files readable by your user without root.

Then add `/mnt/nas/fotos` as the scan root. Prefer this over GVFS for anything long-lived.

## Alternative: GNOME Files / GVFS

Browsing `smb://nas.local/fotos` in Files mounts it at:

```
/run/user/1000/gvfs/smb-share:server=nas.local,share=fotos,user=stephan
```

Notes:

- The path only exists while that mount is alive; it is gone after reboot, logout or `gio mount -u`.
- The name embeds the server, share and login user, so it changes if you connect as a different user.
- `gio mount -l` lists active mounts; `gio mount -u smb://nas.local/fotos` unmounts one for testing.

If a GVFS path changes, do not re-add the folder as a new root. Use `Locate…` on the offline root: it rewrites `roots.root_path`, `scan_jobs.root_path` and every `files.abs_path` prefix in one transaction, keeping classification, albums, duplicates and faces. A following `Refresh Metadata` revalidates files without calling the LLM.

## Storage size vs. rows

`DB size` is the physical file size. After a large delete or a purged root, SQLite keeps the freed pages, so the file can stay hundreds of MB while holding no rows. The sidebar also shows `In use`, and offers `Compact DB` when free pages exist. Compaction runs `wal_checkpoint(TRUNCATE)`, `FTS5 optimize` and `VACUUM`, then switches the database to incremental auto-vacuum.

Manual equivalent, with the app closed:

```bash
sqlite3 ~/.local/share/frank_sherlock/db/index.sqlite \
  "PRAGMA wal_checkpoint(TRUNCATE); \
   INSERT INTO files_fts(files_fts) VALUES('optimize'); \
   VACUUM;"
```

`VACUUM` needs free space roughly equal to the current file size in the same directory.

## Troubleshooting

| Symptom | Meaning | Fix |
|---|---|---|
| Root shows `Offline` | Path is not a mounted directory right now | Mount the share, then `Refresh`; or use `Locate…` if the path changed |
| Scan fails with `root unavailable` | Preflight could not read the root | Mount it; check `gio mount -l` or `mount \| grep cifs` |
| Scan fails with "found 0 files but N are indexed" | Mountpoint exists but the share is not mounted | Mount the share and rescan; no rows were changed |
| Sidebar shows `Files: 0` with a large `DB size` | The index was purged by an older build | Restore from `db/index.sqlite.bak` if present, otherwise run `Compact DB` to reclaim the space |
| Files flip to "modified" every scan | Server truncates timestamps | Prefer an fstab mount with `vers=3.0`; the scanner compares size and mtime |
