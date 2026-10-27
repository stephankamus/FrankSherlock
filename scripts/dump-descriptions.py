#!/usr/bin/env python3
"""Snapshot and compare the descriptions of a folder between two runs.

Why this exists: an A/B over models replaces the description of every row **in
place** — the first arm is gone the moment the second one starts. The only honest
way to compare them is to write one CSV per arm and put them side by side, which
is what this script does; `measure-run.py` answers "how long", this one answers
"did the words get better".

Usage:

    # arm A (3b), right after the job finished
    scripts/dump-descriptions.py dump "Fotos celular Stéphan/2020" -o arm-a.csv

    # arm B (7b, run through ScanOverrides so settings.toml is untouched)
    scripts/dump-descriptions.py dump "Fotos celular Stéphan/2020" -o arm-b.csv
    scripts/dump-descriptions.py compare arm-a.csv arm-b.csv

The folder is matched by suffix against `roots.root_path`, so the GVFS mount
prefix does not have to be typed. `--db` overrides the index path.
"""

from __future__ import annotations

import argparse
import csv
import os
import sqlite3
import sys

DEFAULT_DB = os.path.expanduser("~/.local/share/frank_sherlock/db/index.sqlite")
COLUMNS = ["rel_path", "media_type", "confidence", "document_hint", "description"]


def open_db(path: str) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)


def in_scope(rel_path: str, needle: str) -> bool:
    """True when the row lives in `needle`, a folder path or a suffix of one."""
    return (
        rel_path == needle
        or rel_path.startswith(f"{needle}/")
        or f"/{needle}/" in rel_path
    )


def run_dump(args: argparse.Namespace) -> int:
    """Write every row of `--folder`, matched by root path or by rel_path."""
    connection = open_db(args.db)
    needle = args.folder.rstrip("/")
    # An index written before migration 16 has no `document_hint`, and reading one is
    # better than failing: the A/B can run against a database the app has not opened
    # since the upgrade.
    known = {row[1] for row in connection.execute("pragma table_info(files)")}
    if "document_hint" in known:
        hint_column = "document_hint"
    else:
        hint_column = "'' as document_hint"
        print("note  : this index predates document_hint; the column reads empty")
    rows = connection.execute(
        f"select root_id, rel_path, media_type, confidence, {hint_column}, description"
        " from files where deleted_at is null order by rel_path"
    ).fetchall()
    # `--folder` names either a root (".../Fotos") or a folder inside one, in which
    # case only its rel_path prefix is dumped.
    root_paths = dict(connection.execute("select id, root_path from roots"))
    scoped = [
        row
        for row in rows
        if in_scope(row[1], needle)
        or root_paths.get(row[0], "").rstrip("/").endswith(needle)
    ]

    if not scoped:
        print(f"no indexed file matches {args.folder!r}", file=sys.stderr)
        print("roots in the index:", file=sys.stderr)
        for root_id, root_path in connection.execute("select id, root_path from roots"):
            print(f"  #{root_id} {root_path}", file=sys.stderr)
        return 1

    roots = {row[0] for row in scoped}
    if len(roots) > 1:
        print(f"{args.folder!r} spans more than one root: {sorted(roots)}", file=sys.stderr)
        return 1

    with open(args.out, "w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(COLUMNS)
        for row in scoped:
            writer.writerow(row[1:])

    print(f"folder: {args.folder} (root #{scoped[0][0]})")
    print(f"rows  : {len(scoped)} written to {args.out}")
    return 0


def read_arm(path: str) -> dict[str, dict[str, str]]:
    with open(path, encoding="utf-8", newline="") as handle:
        return {row["rel_path"]: row for row in csv.DictReader(handle)}


def one_line(text: str, width: int) -> str:
    flat = " ".join((text or "").split())
    return flat if len(flat) <= width else flat[: width - 1] + "…"


def mean_words(arm: dict[str, dict[str, str]]) -> float:
    lengths = [len((row["description"] or "").split()) for row in arm.values()]
    return sum(lengths) / len(lengths) if lengths else 0.0


def run_compare(args: argparse.Namespace) -> int:
    first = read_arm(args.first)
    second = read_arm(args.second)
    shared = sorted(set(first) & set(second))
    different = [
        path
        for path in shared
        if first[path]["description"] != second[path]["description"]
        or first[path]["document_hint"] != second[path]["document_hint"]
    ]

    print(f"{args.first:<28}: {len(first)} rows, mean {mean_words(first):.1f} words")
    print(f"{args.second:<28}: {len(second)} rows, mean {mean_words(second):.1f} words")
    print(f"identical / different        : {len(shared) - len(different)} / {len(different)}")
    only_first = len(set(first) - set(second))
    only_second = len(set(second) - set(first))
    if only_first or only_second:
        print(f"only in the first / second   : {only_first} / {only_second}")
    if args.only_different:
        shared = different
    print()
    column = max(12, args.width // 2)
    for path in shared:
        left = one_line(first[path]["description"], column)
        right = one_line(second[path]["description"], column)
        print(path)
        print(f"  A | {left}")
        print(f"  B | {right}")
        if first[path]["document_hint"] != second[path]["document_hint"]:
            print(
                f"  doc | {first[path]['document_hint'] or '-'} -> "
                f"{second[path]['document_hint'] or '-'}"
            )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default=os.environ.get("SHERLOCK_DB", DEFAULT_DB))
    subparsers = parser.add_subparsers(dest="command", required=True)

    dump = subparsers.add_parser("dump", help="write one arm to a CSV")
    dump.add_argument("folder", help="path suffix of the indexed root (or a folder inside it)")
    dump.add_argument("-o", "--out", required=True, help="CSV to write")
    dump.set_defaults(func=run_dump)

    compare = subparsers.add_parser("compare", help="print two arms side by side")
    compare.add_argument("first")
    compare.add_argument("second")
    compare.add_argument("--width", type=int, default=84, help="characters per column")
    compare.add_argument(
        "--only-different",
        action="store_true",
        help="hide the rows both arms described the same way",
    )
    compare.set_defaults(func=run_compare)

    args = parser.parse_args()
    if args.command == "dump" and not os.path.exists(args.db):
        print(f"index not found: {args.db}", file=sys.stderr)
        return 1
    try:
        return args.func(args)
    except BrokenPipeError:
        # `… compare a.csv b.csv | head` closes the pipe early; that is not an error.
        return 0


if __name__ == "__main__":
    sys.exit(main())
