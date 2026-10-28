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

Two bookkeeping traps are worked around here, because both of them made the
summary describe something other than the run:

- `started_at` is the start of the *run* only if the job was never resumed. The
  column used to be inherited across a resume, so a job created in April and
  resumed in October reported a 189-day wall — and the journal window was derived
  from it, mixing six months of unrelated requests (35k generates, 5.8k model
  loads, four different `KvSize`) into "one run". When the span is impossible the
  window is derived from the last contiguous block of `/api/generate` instead.
- `processed_files` is a checkpoint written every 25 files, so it lags what the
  run actually did (886 came out as 875). The file count printed here is the
  larger of the counter and the distinct files in the log.

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
from datetime import datetime, timedelta, timezone

DEFAULT_LOG = os.path.expanduser(
    "~/.local/share/dev.frank.sherlock/logs/frank_sherlock.log"
)
DEFAULT_DB = os.path.expanduser("~/.local/share/frank_sherlock/db/index.sqlite")

# `Classifying: x` for a still image, `Classifying video: x` and `Classifying PDF: x`
# for the legacy paths. Matching only the first one made the log count impossible to
# reconcile with the job counter and hid both legacy paths from every report.
CLASSIFY = re.compile(
    r"^\[(\d{4}-\d{2}-\d{2})\]\[(\d{2}:\d{2}:\d{2})\].*Classifying(?: (video|PDF|pdf))?: (.+)$"
)
GIN = re.compile(
    r"\[GIN\].*?\|\s*(\d{3})\s*\|\s*([0-9.]+)(ns|µs|us|ms|s|m|h)\s*\|.*\"(/api/[^\"]+)\""
)
# The pass summary the scan already logs. It is the only place that reports `deduped`,
# `failed`, `deferred` and `unreadable`, so the report stops guessing them.
BACKLOG_SUMMARY = re.compile(
    r"Backlog(?: \(retry failed\))?: (\d+) processed, (\d+) classified \((\d+) deduped\), "
    r"(\d+) thumbnailed, (\d+) failed, (\d+) deferred to the next pass, "
    r"(\d+) unreadable, (\d+) face\(s\) stored"
)
DOC_BUCKET = re.compile(r"Scan job \d+: (\d+) document row\(s\) in the bucket \((.+)\)$")
# Rows that inherited a classification from a near-duplicate. They never reach the model,
# so they have no `Classifying:` line — before this they were invisible in the log and the
# only place that knew about them was the summary's `deduped`.
REUSED_CLASSIFICATION = "Reused classification: "
# A file that takes longer than this is what makes the mean meaningless: the median is
# ~2 s while the tail is where most of the wall goes.
SLOW_FILE_SECONDS = 30.0
# One `Operation:fit` line per model load, and it carries the runner identity:
# `BatchSize:512 FlashAttention:Enabled KvSize:4096`.
LOAD = re.compile(r"Operation:fit.*?BatchSize:(\d+).*?KvSize:(\d+)")
RUNNER_START = "starting runner"
RUNNER_CRASH = "SIGABRT"

UNIT_SECONDS = {"ns": 1e-9, "µs": 1e-6, "us": 1e-6, "ms": 1e-3, "s": 1.0, "m": 60.0, "h": 3600.0}

# No single run lasts a day. A bigger span means `started_at` belongs to a
# previous run of the same job row, so neither the wall nor the journal window
# can be derived from it.
MAX_TRUSTED_WALL = 24 * 3600
# A quiet stretch this long between two `/api/generate` requests separates two
# runs (the longest single request measured is 57 s, preparation included).
RUN_GAP_SECONDS = 600
# How far back to look for the real window when `started_at` cannot be trusted.
STALE_LOOKBACK = 48 * 3600


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


def failed_breakdown(con, root_id: int) -> list[tuple[str, int, int]]:
    """Rows the pass gave up on, by reason — and they are still counted as pending.

    `pending` counts `description = ''` and does not look at `classify_state`, so a row
    that burned every attempt appears in it forever and the "did this pass finish"
    number can never reach zero. Printing the two apart is the only way to read it.
    """
    try:
        return con.execute(
            "select coalesce(failure_kind, ''), count(*), max(classify_attempts)"
            " from files where root_id = ? and deleted_at is null"
            " and classify_state = 'failed' group by 1 order by 2 desc",
            (root_id,),
        ).fetchall()
    except sqlite3.Error:
        return []


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
            # Timestamps have to be machine-readable and timezone-free: the default
            # short format prints the month name in the system locale ("out 25").
            "--output=short-iso",
            "--utc",
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


def log_entries(path: str) -> tuple[list[tuple[datetime, str, str]], dict]:
    """Every `Classifying*` line as (timestamp, kind, path), plus the pass summary.

    Files that come out of the dedupe index never log a line at all, so reconciling the
    log against the job counter has to subtract `deduped` from `processed` — the summary
    line reports both, which is why it is parsed here instead of being guessed.
    """
    entries: list[tuple[datetime, str, str]] = []
    summary: dict = {}
    if not os.path.exists(path):
        return entries, summary
    with open(path, encoding="utf-8", errors="replace") as handle:
        for raw in handle:
            line = raw.strip()
            match = CLASSIFY.match(line)
            if match is not None:
                day, clock, kind, file_path = match.groups()
                entries.append(
                    (
                        datetime.strptime(f"{day} {clock}", "%Y-%m-%d %H:%M:%S").replace(
                            tzinfo=timezone.utc
                        ),
                        (kind or "image").lower(),
                        file_path.strip(),
                    )
                )
                continue
            backlog = BACKLOG_SUMMARY.search(line)
            if backlog is not None:
                (
                    processed,
                    classified,
                    deduped,
                    thumbnailed,
                    failed,
                    deferred,
                    unreadable,
                    faces,
                ) = (int(value) for value in backlog.groups())
                summary["backlog"] = {
                    "processed": processed,
                    "classified": classified,
                    "deduped": deduped,
                    "thumbnailed": thumbnailed,
                    "failed": failed,
                    "deferred": deferred,
                    "unreadable": unreadable,
                    "faces": faces,
                }
                continue
            bucket = DOC_BUCKET.search(line)
            if bucket is not None:
                summary["documents"] = (int(bucket.group(1)), bucket.group(2))
                continue
            if REUSED_CLASSIFICATION in line:
                summary["reused_logged"] = summary.get("reused_logged", 0) + 1
    return entries, summary


def line_stamp(line: str) -> datetime | None:
    """Timestamp of a `--output=short-iso --utc` journal line (`2026-10-25T21:47:40+0000`)."""
    try:
        return datetime.strptime(line[:19], "%Y-%m-%dT%H:%M:%S").replace(
            tzinfo=timezone.utc
        )
    except ValueError:
        return None


def clip_lines(lines: list[str], start: datetime) -> list[str]:
    """Drop what the fetch had to reach past the real start of the run.

    The fetch window is deliberately wider than the run (48 h) so a stale
    `started_at` can be replaced by evidence; the statistics have to be computed
    on the clipped block, or the summary counts requests from other runs again.
    """
    return [
        line
        for line in lines
        if (stamp := line_stamp(line)) is not None and stamp >= start
    ]


def run_window_from_journal(lines: list[str], end: datetime) -> datetime | None:
    """Start of the last contiguous block of `/api/generate` requests.

    Used when `started_at` predates the run (a resumed job). The journal is the
    only source that knows when the GPU actually started working: `/api/ps` and
    `/api/tags` cannot be used, because the UI polls both the whole time the app
    is open, whether a pass is running or not.
    """
    stamps = [
        stamp
        for line in lines
        if "/api/generate" in line
        for stamp in [line_stamp(line)]
        if stamp is not None
    ]
    if not stamps:
        return None
    stamps.sort()
    start = stamps[-1]
    for stamp in reversed(stamps[:-1]):
        if (start - stamp).total_seconds() > RUN_GAP_SECONDS:
            break
        start = stamp
    return start


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
    end_dt = datetime.fromtimestamp(finished, timezone.utc)
    reported_start = datetime.fromtimestamp(started, timezone.utc)
    stale_start = finished - started > MAX_TRUSTED_WALL or finished < started
    lookback = STALE_LOOKBACK if stale_start else max(0, finished - started)
    lines = journal_window(end_dt - timedelta(seconds=lookback), end_dt)
    if stale_start:
        start_dt = run_window_from_journal(lines, end_dt) or end_dt
        lines = clip_lines(lines, start_dt)
    else:
        start_dt = reported_start
    wall = max(0.0, (end_dt - start_dt).total_seconds())

    stats = journal_stats(lines)
    durations = stats["durations"]
    entries, summary = log_entries(args.log)
    in_window = [
        (stamp, kind, path) for stamp, kind, path in entries if start_dt <= stamp <= end_dt
    ]
    logged_files = len({path for _, _, path in in_window})
    counted = job.get("processed_files") or 0
    processed = max(counted, logged_files)
    pending = pending_rows(con, job["root_id"])
    total = total_rows(con, job["root_id"])
    low_conf = low_confidence_rows(con, job["root_id"])

    print(f"job #{job['id']}            : {job.get('phase')}  {job.get('status')}")
    print(f"root                : #{job['root_id']}  {os.path.basename(job.get('root_path') or '')}")
    window = f"{start_dt:%H:%M:%S} -> {end_dt:%H:%M:%S}"
    if stale_start:
        window += "   (derived from the journal)"
    print(f"window (UTC)        : {window}")
    files = f"{processed} / {job.get('total_files') or 0} processed"
    if processed != counted:
        files += f"   (job counter {counted}, log {logged_files})"
    print(f"files               : {files}")
    print(f"wall                : {format_duration(wall)}  ({int(wall)} s)")
    if processed > 0:
        print(f"per file (wall)     : {wall / processed:.2f} s")
        print(f"requests per file   : {len(durations) / processed:.2f}")
    if stale_start:
        print(
            f"  note: scan_jobs.started_at is {reported_start:%Y-%m-%d %H:%M:%S} UTC and belongs"
            " to an earlier run of this job; the window above comes from the journal"
        )

    print()
    print(f"pending now         : {pending} of {total} rows still have no description")
    failed = failed_breakdown(con, job["root_id"])
    if failed:
        total_failed = sum(count for _, count, _ in failed)
        print(
            f"  of which failed   : {total_failed} row(s) with classify_state='failed'"
            " (only `Retry failed` brings them back)"
        )
        for kind, count, attempts in failed:
            print(f"    {kind or 'unknown':<16} {count:>6}   max attempts {attempts}")
    if summary.get("backlog"):
        backlog = summary["backlog"]
        print(
            "job summary (log)   : "
            f"{backlog['processed']} processed, {backlog['classified']} classified "
            f"({backlog['deduped']} deduped), {backlog['thumbnailed']} thumbnailed, "
            f"{backlog['failed']} failed, {backlog['deferred']} deferred, "
            f"{backlog['unreadable']} unreadable, {backlog['faces']} face(s)"
        )
    if summary.get("documents"):
        doc_total, doc_breakdown = summary["documents"]
        print(f"documents bucket    : {doc_total} row(s) ({doc_breakdown})")
    if summary.get("reused_logged"):
        # Cross-check for `deduped` in the summary: these rows now log one line each.
        print(
            f"reused lines        : {summary['reused_logged']} near-duplicate row(s)"
            " that skipped the model"
        )
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
    if stats["loads"] == 0:
        # A pass that never loads a model is the healthy case: the runner was
        # already up and never changed identity. Printing "-" for KvSize read like
        # missing data instead of "nothing changed".
        print("KvSize              : not loaded again in this run (runner kept its identity)")
        print("BatchSize           : not loaded again in this run")
    else:
        print(f"KvSize              : {table(stats['kv_sizes'])}   (more than one = reloads)")
        print(f"BatchSize           : {table(stats['batches'])}   (512 is Ollama's default)")
    if stats["loads"] and stats["runner_starts"] > stats["loads"] * 2:
        print(
            f"  note: {stats['runner_starts']} spawns for {stats['loads']} loads — the runner is"
            " being retried in a loop after crashing"
        )

    print()
    kinds: dict[str, int] = {}
    for _, kind, _ in in_window:
        kinds[kind] = kinds.get(kind, 0) + 1
    print(
        f"log lines           : {len(entries)} Classifying line(s) total,"
        f" {len(in_window)} inside this run ({table(kinds)})"
    )
    stamps = [stamp for stamp, _, _ in in_window]
    if len(stamps) > 1:
        # delta[i] is the time spent on the file that logged stamps[i]
        deltas = [(b - a).total_seconds() for a, b in zip(stamps, stamps[1:])]
        print(
            "per file (log)      : "
            f"mean {sum(deltas) / len(deltas):.2f} s, "
            f"p50/p95 {percentile(deltas, 0.5):.2f}/{percentile(deltas, 0.95):.2f} s, "
            f"max {max(deltas):.2f} s"
        )
        slow = [delta for delta in deltas if delta >= SLOW_FILE_SECONDS]
        if slow:
            share = 100 * sum(slow) / max(1.0, sum(deltas))
            print(
                f"  tail              : {len(slow)} file(s) at or above"
                f" {SLOW_FILE_SECONDS:.0f} s = {sum(slow):.0f} s of wall ({share:.0f} %)"
            )
        ranked = sorted(zip(deltas, in_window[:-1]), key=lambda pair: pair[0], reverse=True)
        print("  slowest files     :")
        for delta, (_, kind, path) in ranked[:10]:
            print(f"    {delta:8.1f} s  {kind:<5} {path}")
    if len(stamps) < 2 and entries:
        print("  note: the log keeps no lines from this run — it is truncated on start")
    else:
        # Files served from the dedupe index never log a line, so the number the log can
        # be reconciled against is `processed - deduped`, not `processed`.
        expected = None
        if summary.get("backlog"):
            expected = summary["backlog"]["processed"] - summary["backlog"]["deduped"]
        if expected and len(in_window) < expected * 0.9:
            print(
                f"  note: {len(in_window)} of ~{expected} classified file(s) logged — the rest"
                " are in a rotated log or were deduped"
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
