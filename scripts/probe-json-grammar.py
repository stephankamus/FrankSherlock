#!/usr/bin/env python3
"""Does Ollama's JSON grammar cause the runner crashes? Measure it.

The journal shows every crash of a scan as

    terminate called after throwing an instance of 'std::runtime_error'
      what():  Unexpected empty grammar stack after accepting piece: ! (0)

That is the constrained decoder (`format: "json"`) aborting, and our requests always
send it. Before dropping the grammar — which weakens the guarantee that the output
parses — this measures both arms **interleaved**, so a bad image cannot bias one
side:

    for each round:  request with `format: "json"`  → 1 request
                     request without it            → 1 request

For each arm it reports how many requests Ollama answered with 5xx, how many
responses still parse (directly, or after the same brace scan the app does), and the
latency. The app's request shape is mirrored: same prompt keys, `num_ctx = 4096`,
`num_batch = 128`, `temperature = 0`, `num_predict = 180`.

Usage:
    scripts/probe-json-grammar.py --image ./a.jpg --image ./b.jpg
    scripts/probe-json-grammar.py --root "/run/user/1000/gvfs/.../2020" --n 120
    scripts/probe-json-grammar.py --images-from-index 8 --n 120

Read the verdict: `500` only in the json arm means the grammar is the trigger.
Both arms clean with this many requests means the sample was too small — raise `--n`.
"""

from __future__ import annotations

import argparse
import base64
import glob
import json
import os
import sqlite3
import statistics
import sys
import time
import urllib.error
import urllib.request

OLLAMA = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434").rstrip("/")
DEFAULT_DB = os.path.expanduser("~/.local/share/frank_sherlock/db/index.sqlite")
MODEL = os.environ.get("PROBE_MODEL", "qwen2.5vl:3b")

# Mirrors PRIMARY_PROMPT_CONCISE in classify.rs: same keys, same "no markdown" rule.
PROMPT = (
    'Return only JSON: {"media_type":"photo|screenshot|document|meme|artwork|other",'
    '"contains_text":true,"is_anime_related":false,"is_document_like":false,'
    '"description":"max 12 words","confidence":0.0}'
    " Rules: describe what is visible; no markdown. No extra keys."
)

IMG_EXTS = (".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif")


def images_from_index(root_id: int, limit: int, db_path: str) -> list[str]:
    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    rows = con.execute(
        "select abs_path from files where root_id = ? and deleted_at is null"
        " and media_type in ('photo','screenshot') order by rel_path limit ?",
        (root_id, limit),
    ).fetchall()
    con.close()
    return [row[0] for row in rows]


def images_from_dir(root: str, limit: int) -> list[str]:
    found: list[str] = []
    for name in sorted(os.listdir(root)):
        if name.lower().endswith(IMG_EXTS):
            found.append(os.path.join(root, name))
            if len(found) >= limit:
                break
    return found


def request_once(image_path: str, grammar: bool) -> tuple[str, float, str]:
    """One /api/generate call. Returns (outcome, seconds, detail)."""
    with open(image_path, "rb") as handle:
        b64 = base64.b64encode(handle.read()).decode("ascii")
    body = {
        "model": MODEL,
        "prompt": PROMPT,
        "stream": False,
        "keep_alive": "10m",
        "options": {"temperature": 0, "num_predict": 180, "num_ctx": 4096, "num_batch": 128},
        "images": [b64],
    }
    if grammar:
        body["format"] = "json"
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        f"{OLLAMA}/api/generate", data=data, headers={"Content-Type": "application/json"}
    )
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as err:
        return (f"http {err.code}", time.monotonic() - started, str(err.code))
    except Exception as err:  # noqa: BLE001 - a timeout is just another outcome
        return ("error", time.monotonic() - started, str(err)[:60])
    elapsed = time.monotonic() - started
    raw = payload.get("response", "")
    if parses(raw):
        return ("parsed", elapsed, "")
    return ("text", elapsed, raw[:60].replace("\n", " "))


def parses(raw: str) -> bool:
    """Direct parse, then the same brace scan the app uses."""
    text = raw.strip()
    if not text:
        return False
    try:
        if isinstance(json.loads(text), dict):
            return True
    except json.JSONDecodeError:
        pass
    start = text.find("{")
    while start != -1:
        depth = 0
        for index, char in enumerate(text[start:], start=start):
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    try:
                        if isinstance(json.loads(text[start:index + 1]), dict):
                            return True
                    except json.JSONDecodeError:
                        pass
                    break
        start = text.find("{", start + 1)
    return False


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(fraction * (len(ordered) - 1))))]


def summarize(name: str, results: list[tuple[str, float, str]]) -> dict:
    outcomes: dict[str, int] = {}
    for outcome, _, _ in results:
        outcomes[outcome] = outcomes.get(outcome, 0) + 1
    times = [elapsed for _, elapsed, _ in results]
    failures = sum(count for outcome, count in outcomes.items() if outcome not in ("parsed", "text"))
    print(
        f"{name:<8} requests {len(results):>4}  5xx/failures {failures:>3}"
        f"  parsed {outcomes.get('parsed', 0):>4}  unsalvageable {outcomes.get('text', 0):>3}"
        f"  mean {statistics.fmean(times):5.2f} s  p95 {percentile(times, 0.95):5.2f} s"
    )
    return {"failures": failures, "parsed": outcomes.get("parsed", 0), "text": outcomes.get("text", 0)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", action="append", default=[], help="image to send (repeatable)")
    parser.add_argument("--root", help="folder with images to sample")
    parser.add_argument("--images-from-index", type=int, metavar="ROOT_ID")
    parser.add_argument("--db", default=os.environ.get("SHERLOCK_DB", DEFAULT_DB))
    parser.add_argument("--n", type=int, default=120, help="requests per arm")
    parser.add_argument("--max-images", type=int, default=4, help="distinct images to rotate")
    args = parser.parse_args()

    images = list(args.image)
    if args.root:
        images += images_from_dir(args.root, args.max_images)
    if args.images_from_index is not None:
        images += images_from_index(args.images_from_index, args.max_images, args.db)
    if not images:
        images = glob.glob(os.path.join(os.path.expanduser("~/Pictures"), "*"))[: args.max_images]
    images = [path for path in images if os.path.exists(path)]
    if not images:
        print("no image to send: pass --image, --root or --images-from-index", file=sys.stderr)
        return 1

    print(f"model     : {MODEL}")
    print(f"images    : {len(images)}  e.g. {os.path.basename(images[0])}")
    print(f"requests  : {args.n} per arm, interleaved")
    print()

    arms: dict[str, list[tuple[str, float, str]]] = {"json": [], "plain": []}
    for round_index in range(args.n):
        image = images[round_index % len(images)]
        # Alternate the order so a transient (a reload, a slow page) cannot favour
        # one arm: even rounds run with the grammar first, odd rounds without it.
        order = ("json", "plain") if round_index % 2 == 0 else ("plain", "json")
        for arm in order:
            outcome = request_once(image, grammar=(arm == "json"))
            arms[arm].append(outcome)
            if outcome[0] not in ("parsed", "text"):
                print(f"  {arm:<5} {outcome[1]:6.2f} s  {outcome[0]:<10} {outcome[2][:50]}")

    print()
    json_arm = summarize("json", arms["json"])
    plain_arm = summarize("plain", arms["plain"])

    print()
    if json_arm["failures"] and not plain_arm["failures"]:
        print("verdict: the JSON grammar is the trigger — the same images never fail without it.")
        print("         Dropping `format: json` (and relying on the salvage parser) fixes the crashes.")
    elif not json_arm["failures"] and not plain_arm["failures"]:
        print("verdict: no failure in either arm — the sample is too small. Raise --n.")
    elif plain_arm["failures"] >= json_arm["failures"]:
        print("verdict: the grammar is NOT the differentiator — look elsewhere (VRAM, driver, model).")
    else:
        print("verdict: mixed — the grammar makes it worse, but something else also fails.")
    if plain_arm["text"] > json_arm["text"]:
        print(
            f"cost   : without the grammar {plain_arm['text'] - json_arm['text']} more response(s)"
            " needed the brace scan (still recovered)."
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
