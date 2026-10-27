#!/usr/bin/env python3
"""Measure the cost of writing descriptions in PT-BR instead of English.

The app sends one vision request per image with a fixed prompt. This script
reproduces that request exactly (same downscale, same JSON grammar, same
`num_predict`/`num_ctx`) and runs it three ways on real images from your index:

  en           the current concise prompt (descriptions in English)
  pt           the same schema, description asked in PT-BR
  translate    the `en` request plus a second, text-only request that translates
               the resulting description

Reported per variant: output tokens (`eval_count`), decode time and total time.
The interesting number is the *delta* against `en`, because the image prefill is
identical across the three and cancels out.

Usage:
    python3 scripts/bench-description-language.py --count 8
    python3 scripts/bench-description-language.py --paths a.jpg b.jpg
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import sqlite3
import statistics
import sys
import tempfile
import urllib.request
from io import BytesIO
from pathlib import Path

from PIL import Image

OLLAMA = os.environ.get("OLLAMA_BASE", "http://localhost:11434")
DB = Path.home() / ".local/share/frank_sherlock/db/index.sqlite"

# Mirrors PRIMARY_PROMPT_CONCISE in sherlock/desktop/src-tauri/src/classify.rs.
PROMPT_EN = (
    "Analyze this image. Return ONLY compact JSON, no markdown. Schema: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,'
    '"document_photo":"none|receipt|invoice|statement|other",'
    '"description":"max 12 words",'
    '"confidence":0.0}'
    " Rules: "
    "1) contains_text=true only when text is clearly visible (UI, receipt, scan, subtitles). "
    "2) document_photo=receipt|invoice|statement|other only when the picture IS that document; "
    'a scene showing signs, logos or prices is "none". '
    "3) description must be factual and under 12 words. "
    "4) Favor precision over guesswork."
)

# The same schema and the same English enum values; only the free-text field is
# asked in Portuguese. The enum values must stay English or classify.rs stops
# recognising them.
PROMPT_PT = (
    "Analise esta imagem. Responda SOMENTE com JSON compacto, sem markdown. Schema: "
    '{"media_type":"screenshot|anime|manga|photo|document|artwork|other",'
    '"contains_text":true,'
    '"document_photo":"none|receipt|invoice|statement|other",'
    '"description":"máximo 12 palavras",'
    '"confidence":0.0}'
    " Regras: "
    "1) contains_text=true apenas quando houver texto claramente visível "
    "(interface, recibo, digitalização, legendas). "
    "2) document_photo=receipt|invoice|statement|other apenas quando a imagem FOR esse documento; "
    'uma cena com placas, logotipos ou preços é "none". '
    "3) description deve ser factual, em português do Brasil, com no máximo 12 palavras. "
    "4) Prefira precisão a adivinhação."
)

TRANSLATE_PROMPT = (
    "Traduza a descrição abaixo para português do Brasil, mantendo-a curta e factual. "
    "Responda apenas com a tradução, sem aspas e sem comentários.\n\n"
)

NUM_CTX = 4096
NUM_PREDICT = 180


def post(path: str, payload: dict, timeout: int = 300) -> dict:
    req = urllib.request.Request(
        f"{OLLAMA}{path}",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode())


def downscale(path: Path, max_side: int, out_dir: Path) -> Path:
    """Mirror the app: longest side capped at `max_side`, re-encoded to JPEG."""
    dst = out_dir / (path.stem + "-" + str(max_side) + ".jpg")
    if dst.exists():
        return dst
    with Image.open(path) as img:
        img = img.convert("RGB")
        img.thumbnail((max_side, max_side), Image.LANCZOS)
        img.save(dst, "JPEG", quality=90)
    return dst


def generate(model: str, prompt: str, image: Path | None, num_predict: int) -> dict:
    payload = {
        "model": model,
        "prompt": prompt,
        "stream": False,
        "keep_alive": "10m",
        "options": {
            "temperature": 0.0,
            "num_predict": num_predict,
            "num_ctx": NUM_CTX,
            "num_batch": 128,
        },
        "format": "json" if image is not None else None,
    }
    if image is not None:
        payload["images"] = [base64.b64encode(image.read_bytes()).decode()]
    resp = post("/api/generate", payload)
    ns = 1e9
    return {
        "ok": bool(resp.get("response")),
        "raw": resp.get("response", ""),
        "prompt_tokens": resp.get("prompt_eval_count", 0),
        "eval_tokens": resp.get("eval_count", 0),
        "prefill_s": resp.get("prompt_eval_duration", 0) / ns,
        "decode_s": resp.get("eval_duration", 0) / ns,
        "total_s": resp.get("total_duration", 0) / ns,
        "load_s": resp.get("load_duration", 0) / ns,
    }


def pick_paths(count: int) -> list[Path]:
    conn = sqlite3.connect(DB)
    rows = conn.execute(
        "SELECT abs_path FROM files WHERE deleted_at IS NULL "
        "AND media_type IN ('photo','screenshot','document') AND description <> '' "
        "ORDER BY RANDOM() LIMIT 400"
    ).fetchall()
    paths = [Path(r[0]) for r in rows if os.path.exists(r[0])]
    if not paths:
        sys.exit("no reachable images in the index; pass --paths")
    return paths[:count]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=8)
    ap.add_argument("--paths", nargs="*", default=None)
    ap.add_argument("--model", default="qwen2.5vl:3b")
    ap.add_argument("--max-side", type=int, default=768)
    args = ap.parse_args()

    paths = [Path(p) for p in args.paths] if args.paths else pick_paths(args.count)
    tmp = Path(tempfile.mkdtemp(prefix="bench-lang-"))

    rows = []
    with Image.open(paths[0]) as probe:  # warm the runner once, untimed
        generate(args.model, PROMPT_EN, downscale(paths[0], args.max_side, tmp), NUM_PREDICT)

    def desc_of(raw: str) -> str:
        try:
            return json.loads(raw).get("description", "") or ""
        except Exception:
            return ""

    # Pass 1: the two vision variants only. A text-only request between them
    # makes the next vision call redo the image prefill (~0.9 s, measured), so
    # the translate pass cannot be interleaved without poisoning the numbers.
    for idx, path in enumerate(paths):
        img = downscale(path, args.max_side, tmp)
        if idx % 2 == 0:
            en = generate(args.model, PROMPT_EN, img, NUM_PREDICT)
            pt = generate(args.model, PROMPT_PT, img, NUM_PREDICT)
        else:
            pt = generate(args.model, PROMPT_PT, img, NUM_PREDICT)
            en = generate(args.model, PROMPT_EN, img, NUM_PREDICT)
        rows.append((path.name, en, pt, None, desc_of(en["raw"]), desc_of(pt["raw"])))

    # Pass 2: what "translate afterwards" would add — one text-only request per
    # image, at the same moment in the pipeline as if it ran after the vision call.
    for idx, row in enumerate(rows):
        tr = generate(args.model, TRANSLATE_PROMPT + row[4], None, 120)
        rows[idx] = row[:3] + (tr, row[4], row[5])

    fmt = lambda r: (
        f"{r['eval_tokens']:>3} tok dec {r['decode_s']:4.2f} "
        f"pre {r['prefill_s']:4.2f} tot {r['total_s']:4.2f}"
    )
    for name, en, pt, tr, en_desc, pt_desc in rows:
        print(f"{name[:22]:22} en {fmt(en)}")
        print(f"{'':22} pt {fmt(pt)}  {pt_desc}")
        print(f"{'':22} tr {fmt(tr)}  {tr['raw'].strip()[:80]}")
        print(f"{'':22} EN {en_desc}")

    def avg(sel) -> float:
        return statistics.fmean(sel) if sel else 0.0

    print("\n=== summary (n=%d, model=%s, max_side=%d) ===" % (
        len(rows), args.model, args.max_side))
    en_tok = avg([r[1]["eval_tokens"] for r in rows])
    pt_tok = avg([r[2]["eval_tokens"] for r in rows])
    tr_tok = avg([r[3]["eval_tokens"] for r in rows])
    en_dec = avg([r[1]["decode_s"] for r in rows])
    pt_dec = avg([r[2]["decode_s"] for r in rows])
    tr_dec = avg([r[3]["decode_s"] for r in rows])
    # `total_s` minus the model load: a load is runner churn, not per-image cost.
    work = lambda r: r["total_s"] - r["load_s"]
    en_tot = avg([work(r[1]) for r in rows])
    pt_tot = avg([work(r[2]) for r in rows])
    tr_tot = avg([work(r[1]) + work(r[3]) for r in rows])
    tok_s = pt_tok / pt_dec if pt_dec else 0.0

    print(f"output tokens   en {en_tok:6.1f} | pt {pt_tok:6.1f} | translate en+tr {en_tok + tr_tok:6.1f}")
    print(f"decode seconds  en {en_dec:6.2f} | pt {pt_dec:6.2f} | translate {en_dec + tr_dec:6.2f}")
    print(f"work seconds    en {en_tot:6.2f} | pt {pt_tot:6.2f} | translate {tr_tot:6.2f}")
    if en_tot:
        print(f"overhead vs en  pt {100 * (pt_tot - en_tot) / en_tot:+5.1f}% "
              f"| translate {100 * (tr_tot - en_tot) / en_tot:+5.1f}%")
    print(f"decode throughput {tok_s:.1f} tok/s")
    print(f"extra tokens    pt {pt_tok - en_tok:+.1f} per image, "
          f"~{(pt_tok - en_tok) / tok_s if tok_s else 0:.2f}s per 5000 images "
          f"{5000 * (pt_tok - en_tok) / tok_s / 3600 if tok_s else 0:.2f}h")
    failures = [r[0] for r in rows if not (r[1]["ok"] and r[2]["ok"])]
    print(f"parse failures  {failures or 'none'}")


if __name__ == "__main__":
    main()
