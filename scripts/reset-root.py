#!/usr/bin/env python3
"""Hard-delete the index of one folder, so it can be scanned from scratch.

Why this is needed: removing a root in the UI does **not** always delete anything.
When the folder sits inside another root that is already indexed, `remove_root`
hands its rows to the parent ([`reassign_to_parent_root`]) and adding the folder
again adopts them straight back ([`adopt_child_files`]) — descriptions,
confidence and thumbnails included. The next scan then finds everything already
classified and only works on the few rows that never got a description. That is
the "it only classifies 3 files" symptom.

For every row under `--path` this script removes:

- the `files_fts` entry and the `files` row (plus album links and face rows);
- the root row and the scan jobs of that exact root (the parent root is kept);
- the thumbnail and face-crop files on disk.

Usage:
    scripts/reset-root.py --path "/run/user/1000/gvfs/smb-share:server=.../2020"
    scripts/reset-root.py --path "..." --apply

Without `--apply` it only reports what it would delete. Close the app first: a
scan writing to the same index while this runs will be a bad time for everyone.
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys

DEFAULT_DB = os.path.expanduser("~/.local/share/frank_sherlock/db/index.sqlite")


def connect(path: str) -> sqlite3.Connection:
    con = sqlite3.connect(path, timeout=2.0)
    con.execute("pragma busy_timeout = 2000")
    con.execute("pragma foreign_keys = on")
    return con


def unlink(paths: list[str]) -> int:
    """Remove files on disk; a missing file is not an error."""
    removed = 0
    for path in paths:
        try:
            os.remove(path)
            removed += 1
        except OSError:
            pass
    return removed


def under(con: sqlite3.Connection, sql_prefix: str, prefix: str) -> list:
    """Rows whose stored path is exactly `prefix` or lives inside it."""
    pattern = prefix + "/"
    return con.execute(
        f"{sql_prefix} where root_path = ? or substr(root_path, 1, ?) = ?",
        (prefix, len(pattern), pattern),
    ).fetchall()


def under_files(con: sqlite3.Connection, prefix: str) -> list:
    pattern = prefix + "/"
    return con.execute(
        "select id, root_id, thumb_path from files"
        " where abs_path = ? or substr(abs_path, 1, ?) = ?",
        (prefix, len(pattern), pattern),
    ).fetchall()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", required=True, help="folder to forget (absolute path)")
    parser.add_argument("--db", default=os.environ.get("SHERLOCK_DB", DEFAULT_DB))
    parser.add_argument("--apply", action="store_true", help="actually delete")
    args = parser.parse_args()

    prefix = os.path.normpath(args.path.rstrip("/"))
    if not os.path.exists(args.db):
        print(f"index not found: {args.db}", file=sys.stderr)
        return 1

    con = connect(args.db)
    roots = under(con, "select id, root_path from roots", prefix)
    files = under_files(con, prefix)
    if not roots and not files:
        print(f"nothing indexed under {prefix}")
        return 0

    root_ids = [row[0] for row in roots]
    file_ids = [row[0] for row in files]
    places = ",".join("?" for _ in file_ids) or "null"
    thumbs = [row[2] for row in files if row[2]]
    crops: list[str] = []
    if file_ids:
        crops = [
            row[0]
            for row in con.execute(
                f"select crop_path from face_detections"
                f" where crop_path is not null and file_id in ({places})",
                file_ids,
            )
        ]

    print(f"path            : {prefix}")
    print(f"roots           : {len(roots)}  {[row[0] for row in roots]}")
    for root_id, root_path in roots:
        print(f"  #{root_id} {root_path}")
    print(f"rows            : {len(files)}")
    if file_ids:
        classified = con.execute(
            f"select count(*) from files where id in ({places})"
            " and description is not null and description <> ''",
            file_ids,
        ).fetchone()[0]
        print(f"  with description: {classified} (these are what make a rescan skip them)")
        jobs = con.execute(
            f"select id, phase, status from scan_jobs where root_id in"
            f" ({','.join('?' for _ in root_ids)})",
            root_ids,
        ).fetchall()
        print(f"scan jobs      : {len(jobs)}  {jobs}")
    print(f"thumbnails      : {len(thumbs)} file(s)")
    print(f"face crops      : {len(crops)} file(s)")

    if not args.apply:
        print()
        print("dry run — nothing changed. Re-run with --apply to delete (close the app first).")
        return 0

    try:
        con.execute("begin immediate")
        if file_ids:
            con.execute(f"delete from files_fts where rowid in ({places})", file_ids)
            con.execute(f"delete from album_files where file_id in ({places})", file_ids)
            con.execute(f"delete from face_detections where file_id in ({places})", file_ids)
            con.execute(f"delete from files where id in ({places})", file_ids)
        if root_ids:
            root_places = ",".join("?" for _ in root_ids)
            con.execute(f"delete from scan_jobs where root_id in ({root_places})", root_ids)
            con.execute(f"delete from roots where id in ({root_places})", root_ids)
        con.commit()
    except sqlite3.OperationalError as err:
        con.rollback()
        print(f"failed: {err}", file=sys.stderr)
        print("close the app (or stop the scan) and try again", file=sys.stderr)
        return 1

    removed_thumbs = unlink(thumbs)
    removed_crops = unlink(crops)

    print()
    print(f"deleted         : {len(files)} row(s), {len(roots)} root(s)")
    print(f"removed         : {removed_thumbs} thumbnail(s), {removed_crops} face crop(s)")
    print("next            : add the folder again; it starts a `full` from zero")
    return 0


if __name__ == "__main__":
    sys.exit(main())
