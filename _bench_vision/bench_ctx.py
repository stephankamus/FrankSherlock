#!/usr/bin/env python3
"""Measure the effect of num_ctx / num_batch on VRAM residency and decode speed.

Ollama's /api/ps reports `size` (total model+ctx buffers) and `size_vram`.
When size_vram < size, part of the model runs on CPU and decode collapses.
"""

import base64
import glob
import io
import statistics
import sys
import time

import requests
from PIL import Image

OLLAMA = "http://localhost:11434"
MODEL = sys.argv[2] if len(sys.argv) > 2 else "qwen2.5vl:7b"
S = requests.Session()

PRIMARY_PROMPT = (
    "Analyze this image and respond ONLY with valid JSON. Schema: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,"is_anime_related":false,"is_document_like":false,'
    '"description":"short factual description","series_candidates":["name"],'
    '"character_candidates":["name"],"confidence":0.0}'
    " Rules: 1) Use null-like empty arrays when unknown. "
    "2) If image has visible text (UI, receipt, scan, subtitles), contains_text=true. "
    "3) is_document_like=true for receipts/invoices/forms/scanned docs/screenshots of documents. "
    "4) series_candidates and character_candidates must be unique and max 5 items each. "
    "5) Keep description under 24 words. 6) Favor precision over guesswork."
)

ROOT = "/home/stephan/Imagens/Festa de Formatura"
files = sorted(glob.glob(f"{ROOT}/**/*.jpeg", recursive=True))[:2]


def ds(p, max_side=1024):
    im = Image.open(p).convert("RGB")
    w, h = im.size
    s = max_side / max(w, h)
    if s < 1.0:
        im = im.resize((round(w * s), round(h * s)), Image.LANCZOS)
    b = io.BytesIO()
    im.save(b, format="JPEG", quality=85, optimize=True)
    return b.getvalue()


def unload():
    S.post(f"{OLLAMA}/api/generate", json={"model": MODEL, "keep_alive": 0}, timeout=180)
    time.sleep(2)


def ps():
    d = S.get(f"{OLLAMA}/api/ps", timeout=10).json()
    for m in d.get("models", []):
        return m["size"], m.get("size_vram", 0), m.get("context_length")
    return None, None, None


def run(num_ctx, num_batch=None, npred=500):
    unload()
    t_load = time.perf_counter()
    rows = []
    for idx, f in enumerate(files):
        raw = ds(f)
        opts = {"temperature": 0.1, "num_predict": npred, "num_ctx": num_ctx}
        if num_batch:
            opts["num_batch"] = num_batch
        payload = {
            "model": MODEL,
            "prompt": PRIMARY_PROMPT,
            "images": [base64.b64encode(raw).decode()],
            "stream": False,
            "keep_alive": "10m",
            "format": "json",
            "options": opts,
        }
        t = time.perf_counter()
        r = S.post(f"{OLLAMA}/api/generate", json=payload, timeout=300).json()
        wall = time.perf_counter() - t
        if idx == 0:
            load_s = r.get("load_duration", 0) / 1e9
        ns = 1e9
        rows.append(
            (
                wall,
                r["prompt_eval_duration"] / ns,
                r["prompt_eval_count"],
                r["eval_duration"] / ns,
                r["eval_count"],
            )
        )
    size, vram, ctx = ps()
    avg = lambda i: statistics.mean(x[i] for x in rows)  # noqa: E731
    print(
        f"num_ctx={num_ctx:6d} batch={num_batch or 'auto':>5} npred={npred:3d}: "
        f"wall={avg(0):5.2f}s prefill={avg(1):5.2f}s({avg(2):5.0f}tok) "
        f"decode={avg(3):5.2f}s({avg(4):3.0f}tok) = {avg(4)/avg(3):5.1f} t/s | "
        f"load={load_s:.1f}s | vram={vram/1e9:.2f}GB / total={size/1e9:.2f}GB | ctx={ctx}",
        flush=True,
    )


print(f"model={MODEL} files={len(files)}", flush=True)
for ctx in (4096, 8192):
    run(ctx)
run(4096, num_batch=1024)
run(4096, npred=180)
unload()
print("done", flush=True)
