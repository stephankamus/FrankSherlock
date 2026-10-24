#!/usr/bin/env python3
"""Speed benchmark for the Sherlock vision-classification path.

Reproduces the exact prompt/options used by
`sherlock/desktop/src-tauri/src/classify.rs` (baseline) and compares it against
the proposed optimizations:

  baseline_7b   original bytes + PRIMARY_PROMPT + num_predict=500
  downscale_7b  max side 1024 + JPEG q85 + PRIMARY_PROMPT + num_predict=500
  short_7b      max side 1024 + JPEG q85 + SHORT_PROMPT + num_predict=180
  huge_7b       synthetic 4000x3000 + PRIMARY_PROMPT + num_predict=500
  baseline_3b   original bytes + PRIMARY_PROMPT + num_predict=500 (if installed)

Usage:
  python3 _bench_vision/bench_vision.py --dir "/path/to/images" --limit 10
"""

import argparse
import base64
import io
import json
import statistics
import sys
import time
from pathlib import Path

import requests
from PIL import Image

OLLAMA = "http://localhost:11434"

# Exact copy of classify.rs PRIMARY_PROMPT
PRIMARY_PROMPT = (
    "Analyze this image and respond ONLY with valid JSON. Schema: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,'
    '"is_anime_related":false,'
    '"is_document_like":false,'
    '"description":"short factual description",'
    '"series_candidates":["name"],'
    '"character_candidates":["name"],'
    '"confidence":0.0}'
    " Rules: "
    "1) Use null-like empty arrays when unknown. "
    "2) If image has visible text (UI, receipt, scan, subtitles), contains_text=true. "
    "3) is_document_like=true for receipts/invoices/forms/scanned docs/screenshots of documents. "
    "4) series_candidates and character_candidates must be unique and max 5 items each. "
    "5) Keep description under 24 words. "
    "6) Favor precision over guesswork."
)

# Proposed shorter schema: keeps only the fields that drive routing + search
SHORT_PROMPT = (
    "Analyze this image. Return ONLY compact JSON, no markdown: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,'
    '"description":"max 12 words",'
    '"confidence":0.0}'
    " Use contains_text=true only when the image clearly shows text."
)

IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"}


def downscale_bytes(path: Path, max_side: int, quality: int = 85) -> bytes:
    with Image.open(path) as im:
        im = im.convert("RGB")
        w, h = im.size
        scale = max_side / max(w, h)
        if scale < 1.0:
            im = im.resize((round(w * scale), round(h * scale)), Image.LANCZOS)
        buf = io.BytesIO()
        im.save(buf, format="JPEG", quality=quality, optimize=True)
        return buf.getvalue()


def upscale_bytes(path: Path, width: int, height: int, quality: int = 90) -> bytes:
    with Image.open(path) as im:
        im = im.convert("RGB").resize((width, height), Image.LANCZOS)
        buf = io.BytesIO()
        im.save(buf, format="JPEG", quality=quality, optimize=True)
        return buf.getvalue()


def installed_models() -> list[str]:
    try:
        r = requests.get(f"{OLLAMA}/api/tags", timeout=5)
        return [m["name"] for m in r.json().get("models", [])]
    except Exception:
        return []


def call(session, model, prompt, raw: bytes, num_predict: int, keep_alive="10m") -> dict:
    """Send one /api/generate request and return timings in seconds."""
    t0 = time.perf_counter()
    b64 = base64.b64encode(raw).decode()
    t_encode = time.perf_counter() - t0

    payload = {
        "model": model,
        "prompt": prompt,
        "images": [b64],
        "stream": False,
        "keep_alive": keep_alive,
        "format": "json",
        "options": {"temperature": 0.1, "num_predict": num_predict},
    }
    t1 = time.perf_counter()
    try:
        resp = session.post(f"{OLLAMA}/api/generate", json=payload, timeout=300)
        resp.raise_for_status()
        data = resp.json()
    except Exception as exc:  # noqa: BLE001
        return {"error": str(exc)}
    wall = time.perf_counter() - t1

    ns = 1e9
    out = {
        "wall_s": round(wall, 3),
        "read_encode_s": round(t_encode, 4),
        "payload_kb": round(len(b64) / 1024, 1),
        "total_s": round(data.get("total_duration", 0) / ns, 3),
        "load_s": round(data.get("load_duration", 0) / ns, 3),
        "prompt_tokens": data.get("prompt_eval_count", 0),
        "prefill_s": round(data.get("prompt_eval_duration", 0) / ns, 3),
        "out_tokens": data.get("eval_count", 0),
        "decode_s": round(data.get("eval_duration", 0) / ns, 3),
        "response": data.get("response", ""),
    }
    out["decode_tps"] = (
        round(out["out_tokens"] / out["decode_s"], 1) if out["decode_s"] > 0 else 0.0
    )
    try:
        parsed = json.loads(out["response"])
        out["json_ok"] = isinstance(parsed, dict) and parsed.get("media_type") is not None
        out["media_type"] = parsed.get("media_type")
        out["description"] = parsed.get("description")
    except Exception:  # noqa: BLE001
        out["json_ok"] = False
        out["media_type"] = None
        out["description"] = None
    return out


def summarize(rows: list[dict]) -> dict:
    ok = [r for r in rows if "error" not in r]
    if not ok:
        return {"n": 0}
    def avg(key):
        vals = [r[key] for r in ok if key in r]
        return round(statistics.mean(vals), 3) if vals else 0.0
    return {
        "n": len(ok),
        "errors": len(rows) - len(ok),
        "wall_s": avg("wall_s"),
        "total_s": avg("total_s"),
        "load_s": avg("load_s"),
        "prompt_tokens": round(avg("prompt_tokens")),
        "prefill_s": avg("prefill_s"),
        "out_tokens": round(avg("out_tokens")),
        "decode_s": avg("decode_s"),
        "decode_tps": avg("decode_tps"),
        "read_encode_s": avg("read_encode_s"),
        "payload_kb": avg("payload_kb"),
        "json_ok_rate": round(sum(1 for r in ok if r["json_ok"]) / len(ok), 3),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--limit", type=int, default=10)
    ap.add_argument("--model", default="qwen2.5vl:7b")
    ap.add_argument("--small-model", default="qwen2.5vl:3b")
    ap.add_argument("--out", default="_bench_vision/results.json")
    args = ap.parse_args()

    root = Path(args.dir)
    files = sorted(p for p in root.rglob("*") if p.suffix.lower() in IMAGE_EXTS)
    if not files:
        print(f"no images under {root}", file=sys.stderr)
        return 1
    files = files[: args.limit]

    models = installed_models()
    print(f"ollama models: {', '.join(models) or '(none)'}")
    print(f"images: {len(files)} from {root}\n")

    session = requests.Session()
    results: dict[str, dict] = {}

    configs = [
        ("baseline_" + args.model.split(":")[-1], args.model, PRIMARY_PROMPT, 500, "original"),
        ("downscale_" + args.model.split(":")[-1], args.model, PRIMARY_PROMPT, 500, "downscale"),
        ("short_" + args.model.split(":")[-1], args.model, SHORT_PROMPT, 180, "downscale"),
        ("huge_" + args.model.split(":")[-1], args.model, PRIMARY_PROMPT, 500, "huge"),
    ]
    if args.small_model in models:
        configs.append(("baseline_" + args.small_model.split(":")[-1], args.small_model, PRIMARY_PROMPT, 500, "original"))
        configs.append(("short_" + args.small_model.split(":")[-1], args.small_model, SHORT_PROMPT, 180, "downscale"))
    else:
        print(f"[skip] {args.small_model} not installed\n")

    # Preload the model so load_duration does not pollute the first measurement
    print(f"warming up {args.model} ...")
    warm = call(session, args.model, "Reply with {\"ok\":true}", downscale_bytes(files[0], 512), 8)
    print(f"  load={warm.get('load_s')}s total={warm.get('total_s')}s\n")

    for name, model, prompt, num_predict, kind in configs:
        rows = []
        print(f"=== {name} (model={model}, num_predict={num_predict}) ===")
        for f in files:
            if kind == "original":
                raw = f.read_bytes()
            elif kind == "downscale":
                raw = downscale_bytes(f, 1024)
            else:
                raw = upscale_bytes(f, 4000, 3000)
            row = call(session, model, prompt, raw, num_predict)
            row["file"] = f.name
            rows.append(row)
            if "error" in row:
                print(f"  {f.name}: ERROR {row['error']}")
            else:
                print(
                    f"  {f.name[:34]:34s} wall={row['wall_s']:6.2f}s "
                    f"prefill={row['prefill_s']:5.2f}s/{row['prompt_tokens']:4d}tok "
                    f"decode={row['decode_s']:5.2f}s/{row['out_tokens']:3d}tok "
                    f"({row['decode_tps']:5.1f}t/s) type={row['media_type']}"
                )
        summary = summarize(rows)
        results[name] = {"summary": summary, "rows": rows}
        print(f"  -> avg wall={summary['wall_s']}s total={summary['total_s']}s "
              f"prefill={summary['prefill_s']}s ({summary['prompt_tokens']} tok) "
              f"decode={summary['decode_s']}s ({summary['out_tokens']} tok, {summary['decode_tps']} t/s) "
              f"json_ok={summary['json_ok_rate']}\n")

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(results, indent=2, ensure_ascii=False))
    print(f"results written to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
