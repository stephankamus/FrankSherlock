#!/usr/bin/env python3
"""Summarise one scan/backlog run: index (SQLite) + Ollama journal + app log.

Why three sources: the app log is truncated every time the app starts, so a run
that outlives a restart loses most of its `Classifying:` lines (a 191-file folder
left 23 of them). The index knows how many files the job touched and how many are
*still* pending, and the Ollama journal is append-only and keeps every request with
its duration. The log is only used for the detail it uniquely has (which file) and
to show how much of it survived.

What it reports:

- the job: mode, status, files, wall time, mean per file;
- how many rows of that root are still pending (no description) — the honest
  "did it actually finish" check, since `confidence` comes from the model;
- Ollama: `/api/generate` count, per-request p50/p95/max, runner spawns, aborts
  (`SIGABRT` / `terminated`), `KvSize` and `BatchSize` values. More than one
  `KvSize` in a run means the runner is being rebuilt; more than one `BatchSize`
  or any abort means the GPU side is not stable;
- the `Classifying:` lines that survived in the log.

Usage:
    scripts/measure-run.py                 # last job, 300 log lines
    scripts/measure-run.py --job 4         # a specific job
    scripts/measure-run.py 1000            # last job, 1000 log lines
    SHERLOCK_LOG=/path/to/log SHERLOCK_DB=/path/to/index.sqlite scripts/measure-run.py
"""

from __future__ import annotations

import argparse
import os
import re
import sqlite3
import subprocess
import sys
from datetime import datetime, timezone

DEFAULT_LOG = os.path.expanduser(
    "~/.local/share/dev.frank.sherlock/logs/frank_sherlock.log"
)
DEFAULT_DB = os.path.expanduser("~/.local/share/frank_sherlock/db/index.sqlite")

CLASSIFY = re.compile(r"^\[(\d{4}-\d{2}-\d{2})\]\[(\d{2}:\d{2}:\d{2})\].*Classifying: (.+)$")
GIN = re.compile(
    r"\[GIN\].*?\|\s*(\d{3})\s*\|\s*([0-9.]+)(ns|µs|us|ms|s|m|h)\s*\|.*\"(/api/[^\"]+)\""
)
# One `Operation:fit` line per model load, and it carries the runner identity:
# `BatchSize:512 FlashAttention:Enabled KvSize:4096`.
LOAD = re.compile(r"Operation:fit.*?BatchSize:(\d+).*?KvSize:(\d+)")
RUNNER_START = "starting runner"
RUNNER_CRASH = "SIGABRT"

UNIT_SECONDS = {"ns": 1e-9, "µs": 1e-6, "us": 1e-6, "ms": 1e-3, "s": 1.0, "m": 60.0, "h": 3600.0}


# --------------------------------------------------------------------------- index


def open_db(path: str):
    try:
        return sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error:
        return None


def last_job(con, job_id: int | None) -> dict | None:
    """The requested job, or the most recent one."""
    sql = "select * from scan_jobs"
    args: tuple = ()
    if job_id is not None:
        sql += " where id = ?"
        args = (job_id,)
    sql += " order by id desc limit 1"
    cur = con.execute(sql, args)
    row = cur.fetchone()
    if row is None:
        return None
    return dict(zip([d[0] for d in cur.description], row))


def pending_rows(con, root_id: int) -> int:
    """Rows of the root with no description yet — the real "not classified" count."""
    try:
        return con.execute(
            "select count(*) from files where root_id = ?"
            " and deleted_at is null and (description is null or description = '')",
            (root_id,),
        ).fetchone()[0]
    except sqlite3.Error:
        return -1


def total_rows(con, root_id: int) -> int:
    try:
        return con.execute(
            "select count(*) from files where root_id = ? and deleted_at is null",
            (root_id,),
        ).fetchone()[0]
    except sqlite3.Error:
        return -1


def low_confidence_rows(con, root_id: int) -> int:
    """Rows the model answered with confidence 0 — classified, but worth knowing."""
    try:
        return con.execute(
            "select count(*) from files where root_id = ? and deleted_at is null"
            " and confidence = 0 and description is not null and description <> ''",
            (root_id,),
        ).fetchone()[0]
    except sqlite3.Error:
        return -1


# ------------------------------------------------------------------------- journal


def journal_window(start: datetime, end: datetime) -> list[str]:
    if end <= start:
        end = start
    out = subprocess.run(
        [
            "journalctl",
            "-u",
            "ollama",
            "--since",
            f"@{int(start.timestamp())}",
            "--until",
            f"@{int(end.timestamp()) + 1}",
            "--no-pager",
        ],
        capture_output=True,
        text=True,
    ).stdout
    return out.splitlines()


def journal_stats(lines: list[str]) -> dict:
    statuses: list[int] = []
    durations: list[float] = []
    paths: dict[str, int] = {}
    loads = 0
    runner_starts = 0
    crashes = 0
    kv_sizes: dict[str, int] = {}
    batches: dict[str, int] = {}

    for line in lines:
        if RUNNER_START in line:
            runner_starts += 1
        if RUNNER_CRASH in line:
            crashes += 1
        load = LOAD.search(line)
        if load is not None:
            batch, kv = load.groups()
            loads += 1
            batches[batch] = batches.get(batch, 0) + 1
            kv_sizes[kv] = kv_sizes.get(kv, 0) + 1
        match = GIN.search(line)
        if match is None:
            continue
        code, value, unit, path = match.groups()
        statuses.append(int(code))
        paths[path] = paths.get(path, 0) + 1
        if path == "/api/generate":
            durations.append(float(value) * UNIT_SECONDS[unit])

    return {
        "statuses": statuses,
        "durations": durations,
        "paths": paths,
        "loads": loads,
        "runner_starts": runner_starts,
        "crashes": crashes,
        "kv_sizes": kv_sizes,
        "batches": batches,
    }


# ----------------------------------------------------------------------------- log


def classify_stamps(path: str) -> list[datetime]:
    stamps: list[datetime] = []
    if not os.path.exists(path):
        return stamps
    with open(path, encoding="utf-8", errors="replace") as handle:
        for raw in handle:
            match = CLASSIFY.match(raw.strip())
            if match is None:
                continue
            day, clock, _ = match.groups()
            stamps.append(
                datetime.strptime(f"{day} {clock}", "%Y-%m-%d %H:%M:%S").replace(
                    tzinfo=timezone.utc
                )
            )
    return stamps


# ------------------------------------------------------------------------- helpers


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, int(round(fraction * (len(ordered) - 1))))
    return ordered[index]


def format_duration(seconds: float) -> str:
    total = int(max(0, seconds))
    hours, rest = divmod(total, 3600)
    minutes, secs = divmod(rest, 60)
    return f"{hours}h{minutes:02d}m{secs:02d}s" if hours else f"{minutes}m{secs:02d}s"


def table(counts: dict[str, int]) -> str:
    if not counts:
        return "-"
    return " ".join(f"{key}×{value}" for key, value in sorted(counts.items()))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("limit", nargs="?", type=int, default=300, help="log lines to read")
    parser.add_argument("--job", type=int, default=None, help="scan_jobs.id (default: last)")
    parser.add_argument("--log", default=os.environ.get("SHERLOCK_LOG", DEFAULT_LOG))
    parser.add_argument("--db", default=os.environ.get("SHERLOCK_DB", DEFAULT_DB))
    args = parser.parse_args()

    if not os.path.exists(args.db):
        print(f"index not found: {args.db}", file=sys.stderr)
        return 1
    con = open_db(args.db)
    if con is None:
        print(f"cannot read the index: {args.db}", file=sys.stderr)
        return 1

    job = last_job(con, args.job)
    if job is None:
        print("no scan job in the index", file=sys.stderr)
        return 1

    started = job.get("started_at") or 0
    finished = job.get("completed_at") or job.get("updated_at") or started
    start_dt = datetime.fromtimestamp(started, timezone.utc)
    end_dt = datetime.fromtimestamp(finished, timezone.utc)
    wall = max(0, finished - started)
    processed = job.get("processed_files") or 0

    stats = journal_stats(journal_window(start_dt, end_dt))
    durations = stats["durations"]
    pending = pending_rows(con, job["root_id"])
    total = total_rows(con, job["root_id"])
    low_conf = low_confidence_rows(con, job["root_id"])

    print(f"job #{job['id']}            : {job.get('phase')}  {job.get('status')}")
    print(f"root                : #{job['root_id']}  {os.path.basename(job.get('root_path') or '')}")
    print(f"window (UTC)        : {start_dt:%H:%M:%S} -> {end_dt:%H:%M:%S}")
    print(f"files               : {processed} / {job.get('total_files') or 0} processed")
    print(f"wall                : {format_duration(wall)}  ({wall} s)")
    if processed > 0:
        print(f"per file (wall)     : {wall / processed:.2f} s")
        print(f"requests per file   : {len(durations) / processed:.2f}")

    print()
    print(f"pending now         : {pending} of {total} rows still have no description")
    if low_conf > 0:
        print(f"confidence 0        : {low_conf} classified row(s) the model scored 0.0")

    print()
    print(f"ollama requests     : {len(durations)}  (normally 1-3 per file)")
    if durations:
        print(
            "request p50/p95/max : "
            f"{percentile(durations, 0.5):.2f} s / "
            f"{percentile(durations, 0.95):.2f} s / {max(durations):.2f} s"
        )
        print(f"request total       : {sum(durations):.1f} s of GPU time")
    bad = [code for code in stats["statuses"] if code >= 400]
    if bad:
        codes: dict[str, int] = {}
        for code in bad:
            codes[str(code)] = codes.get(str(code), 0) + 1
        print(f"failed responses    : {len(bad)} ({table(codes)})")

    print(f"model loads         : {stats['loads']}")
    print(f"runner spawns       : {stats['runner_starts']}")
    print(f"runner crashes      : {stats['crashes']}")
    print(f"KvSize              : {table(stats['kv_sizes'])}   (more than one = reloads)")
    print(f"BatchSize           : {table(stats['batches'])}   (512 is Ollama's default)")
    if stats["loads"] and stats["runner_starts"] > stats["loads"] * 2:
        print(
            f"  note: {stats['runner_starts']} spawns for {stats['loads']} loads — the runner is"
            " being retried in a loop after crashing"
        )

    stamps = classify_stamps(args.log)
    in_window = [s for s in stamps if start_dt <= s <= end_dt]
    print()
    print(f"log lines           : {len(stamps)} total, {len(in_window)} inside this run")
    if in_window and len(in_window) > 1:
        deltas = [
            (b - a).total_seconds() for a, b in zip(in_window, in_window[1:])
        ]
        print(
            "per file (log)      : "
            f"mean {sum(deltas) / len(deltas):.2f} s, "
            f"p50/p95 {percentile(deltas, 0.5):.2f}/{percentile(deltas, 0.95):.2f} s, "
            f"max {max(deltas):.2f} s"
        )
    if len(in_window) < 2 and stamps:
        print("  note: the log keeps no lines from this run — it is truncated on start")
    elif processed and len(in_window) < processed // 2:
        print(
            f"  note: {len(in_window)} of {processed} files logged — the log is only a tail,"
            " trust the index and the journal"
        )
    if stats["crashes"]:
        print(
            f"  note: the runner crashed {stats['crashes']} time(s) during this run; those"
            " requests were lost and the model was reloaded"
            " (see plans/pipeline-cpu-gpu-overlap.md §5.5)"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
