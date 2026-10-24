#!/usr/bin/env python3
"""Benchmark real large photos (HEIC from phone, NEF from a DSLR).

HEIC/NEF cannot be decoded by the tools available on this box (no libheif, ffmpeg
only exposes a 160x120 tiff thumb inside the NEF), so this script uses an isolated
venv with pillow-heif + rawpy to produce full-resolution JPEGs first:

    python3 -m venv /tmp/rawvenv
    /tmp/rawvenv/bin/pip install pillow-heif rawpy pillow
    /tmp/rawvenv/bin/python _bench_vision/bench_big.py

Stage 1 converts 2 HEIC + 2 NEF to /tmp/bigjpgs/*.jpg at full resolution.
Stage 2 measures the app's current config against the proposed ones.
"""

import base64
import io
import json
import statistics
import time
import urllib.request
from pathlib import Path

import pillow_heif
import rawpy
from PIL import Image

OLLAMA = "http://localhost:11434"
SRC = Path("/home/stephan/Imagens/Fotos Grandes")
OUT = Path("/tmp/bigjpgs")
OUT.mkdir(exist_ok=True)

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


# ---------------------------------------------------------------- stage 1
def convert_sources() -> list[Path]:
    made = []
    heics = sorted(SRC.glob("*.heic"))[:2]
    nefs = sorted(SRC.glob("*.NEF"))[:2]
    for src in heics:
        dst = OUT / (src.stem + ".jpg")
        if not dst.exists():
            im = pillow_heif.open_heif(src, convert_hdr_to_8bit=True).to_pillow()
            im.convert("RGB").save(dst, "JPEG", quality=95)
        made.append(dst)
    for src in nefs:
        dst = OUT / (src.stem + ".jpg")
        if not dst.exists():
            with rawpy.imread(str(src)) as raw:
                rgb = raw.postprocess(use_camera_wb=True, output_bps=8)
            Image.fromarray(rgb).save(dst, "JPEG", quality=95)
        made.append(dst)
    return made


# ---------------------------------------------------------------- stage 2
def post(path: str, payload: dict, timeout: int = 600) -> dict:
    req = urllib.request.Request(
        f"{OLLAMA}{path}",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    return json.load(urllib.request.urlopen(req, timeout=timeout))


def get(path: str, timeout: int = 30) -> dict:
    return json.load(urllib.request.urlopen(f"{OLLAMA}{path}", timeout=timeout))


def unload(model: str) -> None:
    post("/api/generate", {"model": model, "keep_alive": 0}, timeout=180)
    time.sleep(2)


def ps(model: str):
    d = get("/api/ps")
    for m in d.get("models", []):
        if m["name"] == model:
            return m["size"], m.get("size_vram", 0), m.get("context_length")
    return None, None, None


def prep(path: Path, max_side, quality=85) -> bytes:
    if max_side is None:
        return path.read_bytes()
    im = Image.open(path).convert("RGB")
    w, h = im.size
    s = max_side / max(w, h)
    if s < 1.0:
        im = im.resize((round(w * s), round(h * s)), Image.LANCZOS)
    buf = io.BytesIO()
    im.save(buf, "JPEG", quality=quality, optimize=True)
    return buf.getvalue()


def run(label, model, prompt, npred, max_side, num_ctx=None, files=None):
    unload(model)
    rows, samples = [], []
    for f in files:
        raw = prep(f, max_side)
        opts = {"temperature": 0.1, "num_predict": npred}
        if num_ctx:
            opts["num_ctx"] = num_ctx
        t = time.perf_counter()
        r = post(
            "/api/generate",
            {
                "model": model,
                "prompt": prompt,
                "images": [base64.b64encode(raw).decode()],
                "stream": False,
                "keep_alive": "10m",
                "format": "json",
                "options": opts,
            },
        )
        wall = time.perf_counter() - t
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
        try:
            p = json.loads(r.get("response", ""))
            samples.append((p.get("media_type"), (p.get("description") or "")[:50]))
        except Exception:
            samples.append(("INVALID", r.get("response", "")[:50]))

    size, vram, ctx = ps(model)
    a = lambda i: statistics.mean(x[i] for x in rows)  # noqa: E731
    print(
        f"{label:32s} wall={a(0):6.2f}s prefill={a(1):6.2f}s({a(2):5.0f}tok) "
        f"decode={a(3):6.2f}s({a(4):3.0f}tok) gap={a(0)-a(1)-a(3):5.2f}s | "
        f"vram={vram/1e9:5.2f}/{size/1e9:5.2f}GB ctx={ctx}",
        flush=True,
    )
    for mt, d in samples:
        print(f"    -> {mt}: {d}")
    return {"wall": a(0), "prompt_tokens": a(2), "eval_tokens": a(4), "samples": samples}


def main() -> None:
    files = convert_sources()
    print("== converted (full resolution) ==")
    for f in files:
        with Image.open(f) as im:
            print(f"  {f.name}: {im.size[0]}x{im.size[1]}  {f.stat().st_size/1e6:.1f} MB")

    out = {}
    print("\n== 7b, original bytes, prompt atual (o que o app faz hoje) ==")
    out["7b_original_primary_ctx16384"] = run(
        "7b orig PRIMARY ctx16384", "qwen2.5vl:7b", PRIMARY_PROMPT, 500, None, files=files
    )
    print("\n== 7b otimizado ==")
    out["7b_1024_primary_ctx4096"] = run(
        "7b 1024 PRIMARY ctx4096", "qwen2.5vl:7b", PRIMARY_PROMPT, 500, 1024, 4096, files=files
    )
    print("\n== 3b, prompt curto, varredura de resolução ==")
    for ms in (1024, 768, 640):
        out[f"3b_{ms}_short"] = run(
            f"3b {ms} SHORT", "qwen2.5vl:3b", SHORT_PROMPT, 180, ms, files=files
        )
    Path("_bench_vision/big.json").write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print("\nwritten _bench_vision/big.json")


if __name__ == "__main__":
    main()
