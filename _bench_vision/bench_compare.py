#!/usr/bin/env python3
"""Compare qwen2.5vl:7b vs qwen2.5vl:3b on the same images.

Configs measured (all with format=json):
  A) 7b, original bytes, PRIMARY_PROMPT, npred=500, default ctx  -> app baseline
  B) 7b, 1024px,       PRIMARY_PROMPT, npred=500, ctx=4096
  C) 3b, 1024px,       PRIMARY_PROMPT, npred=500, default ctx
  D) 3b, 1024px,       SHORT_PROMPT,   npred=180, default ctx
  E) 3b, original bytes, SHORT_PROMPT, npred=180, default ctx

Reports wall time, prompt/eval tokens, decode throughput and VRAM residency
(`size_vram` vs `size` from /api/ps): when size_vram < size the model spills to
system RAM and decode speed collapses.
"""

import base64
import glob
import io
import json
import statistics
import time
from pathlib import Path

import requests
from PIL import Image

OLLAMA = "http://localhost:11434"
ROOT = "/home/stephan/Imagens/Festa de Formatura"
LIMIT = 5
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

SHORT_PROMPT = (
    "Analyze this image. Return ONLY compact JSON, no markdown: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,"description":"max 12 words","confidence":0.0}'
    " Use contains_text=true only when the image clearly shows text."
)

files = sorted(glob.glob(f"{ROOT}/**/*.jpeg", recursive=True))[:LIMIT]


def prep(path, max_side=None):
    if max_side is None:
        return Path(path).read_bytes()
    im = Image.open(path).convert("RGB")
    w, h = im.size
    s = max_side / max(w, h)
    if s < 1.0:
        im = im.resize((round(w * s), round(h * s)), Image.LANCZOS)
    b = io.BytesIO()
    im.save(b, format="JPEG", quality=85, optimize=True)
    return b.getvalue()


def unload(model):
    S.post(f"{OLLAMA}/api/generate", json={"model": model, "keep_alive": 0}, timeout=180)
    time.sleep(2)


def ps(model):
    d = S.get(f"{OLLAMA}/api/ps", timeout=10).json()
    for m in d.get("models", []):
        if m["name"] == model:
            return m["size"], m.get("size_vram", 0), m.get("context_length")
    return None, None, None


def run(label, model, prompt, npred, max_side, num_ctx=None):
    unload(model)
    rows = []
    samples = []
    for f in files:
        raw = prep(f, max_side)
        opts = {"temperature": 0.1, "num_predict": npred}
        if num_ctx:
            opts["num_ctx"] = num_ctx
        payload = {
            "model": model,
            "prompt": prompt,
            "images": [base64.b64encode(raw).decode()],
            "stream": False,
            "keep_alive": "10m",
            "format": "json",
            "options": opts,
        }
        t = time.perf_counter()
        r = S.post(f"{OLLAMA}/api/generate", json=payload, timeout=600).json()
        wall = time.perf_counter() - t
        ns = 1e9
        rows.append(
            (
                wall,
                r["prompt_eval_duration"] / ns,
                r["prompt_eval_count"],
                r["eval_duration"] / ns,
                r["eval_count"],
                r.get("load_duration", 0) / ns,
            )
        )
        try:
            parsed = json.loads(r.get("response", ""))
            samples.append(
                (parsed.get("media_type"), (parsed.get("description") or "")[:60])
            )
        except Exception:
            samples.append(("INVALID", r.get("response", "")[:60]))

    size, vram, ctx = ps(model)
    avg = lambda i: statistics.mean(x[i] for x in rows)  # noqa: E731
    spill = (size - vram) / 1e9 if size else 0.0
    print(
        f"{label:26s} wall={avg(0):6.2f}s load={avg(5):5.1f}s "
        f"prefill={avg(1):5.2f}s({avg(2):5.0f}tok) "
        f"decode={avg(3):5.2f}s({avg(4):3.0f}tok)={avg(4)/avg(3):5.1f}t/s | "
        f"vram={vram/1e9:5.2f}/{size/1e9:5.2f}GB spill={spill:4.2f}GB ctx={ctx}",
        flush=True,
    )
    for mt, desc in samples[:3]:
        print(f"    -> {mt}: {desc}")
    return {
        "wall": avg(0),
        "vram": vram,
        "size": size,
        "decode_tps": avg(4) / avg(3),
        "samples": samples,
    }


out = {}
out["A_7b_original_baseline"] = run(
    "A 7b orig  baseline", "qwen2.5vl:7b", PRIMARY_PROMPT, 500, None
)
out["B_7b_1024_baseline_ctx4096"] = run(
    "B 7b 1024  baseline", "qwen2.5vl:7b", PRIMARY_PROMPT, 500, 1024, num_ctx=4096
)
out["C_3b_1024_baseline"] = run(
    "C 3b 1024  baseline", "qwen2.5vl:3b", PRIMARY_PROMPT, 500, 1024
)
out["D_3b_1024_short"] = run(
    "D 3b 1024  short", "qwen2.5vl:3b", SHORT_PROMPT, 180, 1024
)
out["E_3b_original_short"] = run(
    "E 3b orig  short", "qwen2.5vl:3b", SHORT_PROMPT, 180, None
)

Path("_bench_vision/compare.json").write_text(json.dumps(out, indent=2, ensure_ascii=False))
print("written _bench_vision/compare.json")
