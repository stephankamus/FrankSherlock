#!/usr/bin/env python3
"""Convert HEIC/NEF photos into JPEG copies for Frank Sherlock.

The app never decodes HEIC or NEF: the `image` crate does not support either, Ollama
does not accept them and the WebView cannot render NEF. This script is the one place
that knows how to read them, and it is run as an explicit, opt-in batch job (see
`plans/heic-nef-conversion.md`).

Input protocol (one pair per line on stdin):

    <source path>\t<output path>

Output protocol (one JSON object per line on stdout, flushed immediately):

    {"source": "...", "output": "...", "ok": true, "mode": "heic",
     "width": 4032, "height": 3024, "bytes": 3211234, "ms": 91}
    {"source": "...", "output": "...", "ok": false, "error": "..."}

A fatal error (missing dependency, bad arguments) is reported once as
`{"ok": false, "fatal": "..."}` with exit code 2, so the caller can tell "install the
support" apart from "this one file failed".

Guarantees:
  - never overwrites an existing output unless `--overwrite` is passed;
  - never deletes anything;
  - the JPEG is written with orientation baked into the pixels and the EXIF
    Orientation tag set to 1, so the Rust pipeline (which applies EXIF rotation)
    does not rotate it a second time;
  - EXIF (including GPS and DateTimeOriginal) is copied when the format exposes it;
  - the output mtime is set to the source mtime, because date search uses the file
    timestamp, not EXIF.

Usage examples:

    printf '%s\\t%s\\n' in.heic out.jpg | convert_raw_images.py --quality 90
    convert_raw_images.py --dry-run --max-side 2048 < pairs.txt
    convert_raw_images.py --self-check
"""

from __future__ import annotations

import argparse
import io
import json
import os
import sys
import time
from pathlib import Path

# EXIF tag ids used below.
TAG_ORIENTATION = 274
IFD_GPS = 34853

HEIC_EXTS = {".heic", ".heif"}
NEF_EXTS = {".nef"}

# rawpy's postprocess() parameter meaning "use the flip the camera recorded".
RAWPY_CAMERA_FLIP = None

# A NEF preview a couple of percent smaller than the raw (4608 of 4644 px on the
# tested camera) is still visually identical and costs milliseconds instead of a
# full demosaic, so `auto` accepts a small shortfall.
PREVIEW_SHORTFALL_TOLERANCE = 0.98


class Fatal(Exception):
    """An error that invalidates the whole run, not a single file."""


def emit(payload: dict) -> None:
    json.dump(payload, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")
    sys.stdout.flush()


def load_dependencies():
    """Import the heavy modules once, with a message that names what is missing."""
    try:
        import pillow_heif
    except Exception as exc:  # pragma: no cover - depends on the environment
        raise Fatal(f"pillow-heif not available: {exc}") from exc
    try:
        import rawpy
    except Exception as exc:  # pragma: no cover - depends on the environment
        raise Fatal(f"rawpy not available: {exc}") from exc
    try:
        from PIL import Image, ImageOps
    except Exception as exc:  # pragma: no cover - depends on the environment
        raise Fatal(f"Pillow not available: {exc}") from exc
    return pillow_heif, rawpy, Image, ImageOps


def exif_bytes(image) -> bytes | None:
    """EXIF bytes with Orientation forced to 1 and GPS materialised.

    `Image.Exif.tobytes()` only serialises sub-IFDs that were loaded, so the GPS IFD
    has to be touched explicitly or the location quietly disappears from the copy.
    """
    try:
        exif = image.getexif()
    except Exception:
        return None
    if not exif:
        return None
    try:
        if IFD_GPS in exif:
            exif.get_ifd(IFD_GPS)
    except Exception:
        pass
    try:
        exif[TAG_ORIENTATION] = 1
        return exif.tobytes()
    except Exception:
        return None


def resize_if_needed(image, max_side: int, Image):
    """Downscale the longest side to `max_side`. 0 means keep the original size."""
    if max_side <= 0:
        return image
    width, height = image.size
    largest = max(width, height)
    if largest <= max_side:
        return image
    scale = max_side / largest
    size = (max(1, round(width * scale)), max(1, round(height * scale)))
    return image.resize(size, Image.LANCZOS)


def flatten_to_rgb(image):
    """RGB without alpha, compositing transparency over white."""
    if image.mode == "RGB":
        return image
    if image.mode in ("RGBA", "LA", "PA") or (
        image.mode == "P" and "transparency" in image.info
    ):
        rgba = image.convert("RGBA")
        from PIL import Image as _Image

        background = _Image.new("RGB", rgba.size, (255, 255, 255))
        background.paste(rgba, mask=rgba.split()[-1])
        return background
    return image.convert("RGB")


def encode_jpeg(image, quality: int, exif: bytes | None) -> io.BytesIO:
    """Encode the copy. EXIF travels as an argument, never through `image.info`.

    `Image.resize()` and the RGBA flattening both return new images, and trusting
    `image.info` to survive those steps is exactly how GPS quietly disappears.
    """
    buffer = io.BytesIO()
    if exif:
        image.save(buffer, "JPEG", quality=quality, exif=exif)
    else:
        image.save(buffer, "JPEG", quality=quality)
    buffer.seek(0)
    return buffer


def convert_heic(path: Path, args, deps) -> tuple[object, str, bytes | None]:
    pillow_heif, _rawpy, _Image, ImageOps = deps
    heif = pillow_heif.open_heif(path, convert_hdr_to_8bit=True)
    image = heif.to_pillow()
    # EXIF is read while the pixels are still unrotated: `exif_bytes` forces
    # Orientation=1 and `exif_transpose` is what makes that tag true.
    exif = exif_bytes(image)
    return ImageOps.exif_transpose(image), "heic", exif


def convert_nef(path: Path, args, deps) -> tuple[object, str, bytes | None]:
    _pillow_heif, rawpy, Image, ImageOps = deps

    with rawpy.imread(str(path)) as raw:
        try:
            thumb = raw.extract_thumb()
        except Exception:
            thumb = None

        preview = thumb_to_image(thumb, Image, ImageOps)

        # Compare the preview against what the user actually asked for: taking a
        # 1620 px preview when the full file was requested would be a silent
        # downgrade, and demosaicing when a 1024 px copy was requested would be
        # minutes of work for nothing.
        full_side = max(raw.sizes.width, raw.sizes.height)
        requested = args.max_side if args.max_side > 0 else full_side
        wanted = max(1, int(requested * PREVIEW_SHORTFALL_TOLERANCE))
        floor = max(1, min(args.min_preview_side, wanted))
        usable_preview = (
            preview is not None and max(preview.size) >= max(wanted, floor)
        )

        if usable_preview and args.nef_mode in ("auto", "preview"):
            return preview, "preview", exif_bytes(preview)

        if args.nef_mode == "preview":
            if preview is None:
                raise ValueError("no embedded JPEG preview in this NEF")
            raise ValueError(
                f"embedded preview is {max(preview.size)}px, "
                f"below the {requested}px requested"
            )

        rgb = raw.postprocess(
            use_camera_wb=True, output_bps=8, user_flip=RAWPY_CAMERA_FLIP
        )
        image = Image.fromarray(rgb)
        # Demosaic output carries no metadata of its own: borrow the preview's.
        inherited = exif_bytes(preview) if preview is not None else None
        return image, "demosaic", inherited


def thumb_to_image(thumb, Image, ImageOps):
    """Turn a rawpy Thumbnail into an upright PIL image, or None."""
    if thumb is None:
        return None

    import numpy as np

    import rawpy

    try:
        # ThumbFormat is a plain Enum, so comparing to the integer 1 is silently
        # always false and pushes JPEG previews into the bitmap branch.
        if thumb.format == rawpy.ThumbFormat.JPEG:
            image = Image.open(io.BytesIO(thumb.data))
            image.load()
        else:
            image = Image.fromarray(np.asarray(thumb.data))
    except Exception:
        return None
    return ImageOps.exif_transpose(image)


def convert_one(source: Path, output: Path, args, deps) -> dict:
    started = time.monotonic()
    ext = source.suffix.lower()

    if ext in HEIC_EXTS:
        image, mode, exif = convert_heic(source, args, deps)
    elif ext in NEF_EXTS:
        image, mode, exif = convert_nef(source, args, deps)
    else:
        raise ValueError(f"unsupported extension: {ext}")

    image = flatten_to_rgb(image)
    image = resize_if_needed(image, args.max_side, deps[2])
    buffer = encode_jpeg(image, args.quality, exif)
    payload = buffer.getbuffer()
    size = len(payload)

    if not args.dry_run:
        if output.exists() and not args.overwrite:
            return {
                "source": str(source),
                "output": str(output),
                "ok": True,
                "skipped": "exists",
                "mode": mode,
                "width": image.size[0],
                "height": image.size[1],
                "bytes": output.stat().st_size,
                "ms": int((time.monotonic() - started) * 1000),
            }
        output.parent.mkdir(parents=True, exist_ok=True)
        with open(output, "wb") as handle:
            handle.write(payload)
        try:
            # Date search uses the file timestamp, so the copy has to carry the
            # original mtime; the conversion happens minutes or years later.
            stat = source.stat()
            os.utime(output, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        except OSError:
            pass

    return {
        "source": str(source),
        "output": str(output),
        "ok": True,
        "mode": mode,
        "dry_run": bool(args.dry_run),
        "width": image.size[0],
        "height": image.size[1],
        "bytes": size,
        "ms": int((time.monotonic() - started) * 1000),
    }


def read_pairs(stream) -> list[tuple[Path, Path]]:
    pairs: list[tuple[Path, Path]] = []
    for line in stream:
        line = line.rstrip("\n")
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) != 2:
            raise Fatal(f"expected '<source>\\t<output>', got: {line!r}")
        pairs.append((Path(parts[0]), Path(parts[1])))
    return pairs


def self_check(args, deps) -> int:
    pillow_heif, rawpy, _Image, _ImageOps = deps
    emit(
        {
            "ok": True,
            "pillow_heif": getattr(pillow_heif, "__version__", "unknown"),
            "rawpy": getattr(rawpy, "__version__", "unknown"),
        }
    )
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Convert HEIC/NEF files into JPEG copies (Frank Sherlock)",
    )
    parser.add_argument(
        "--max-side",
        type=int,
        default=0,
        help="longest side of the copy in pixels; 0 keeps the original size",
    )
    parser.add_argument("--quality", type=int, default=90, help="JPEG quality (1-100)")
    parser.add_argument(
        "--nef-mode",
        choices=("auto", "preview", "demosaic"),
        default="auto",
        help="auto uses the embedded JPEG preview when it is big enough for --max-side",
    )
    parser.add_argument(
        "--min-preview-side",
        type=int,
        default=1024,
        help="never use a NEF preview smaller than this, even if it is big enough",
    )
    parser.add_argument(
        "--overwrite",
        action="store_true",
        help="rewrite outputs that already exist (default: report as skipped)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="convert in memory and report the output size without writing",
    )
    parser.add_argument(
        "--self-check",
        action="store_true",
        help="only report the conversion dependency versions",
    )
    parser.add_argument(
        "--pair",
        action="append",
        default=[],
        metavar="SOURCE\\tOUTPUT",
        help="inline pair; stdin is used when no --pair is given",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not 1 <= args.quality <= 100:
        emit({"ok": False, "fatal": f"invalid --quality: {args.quality}"})
        return 2

    try:
        deps = load_dependencies()
    except Fatal as exc:
        emit({"ok": False, "fatal": str(exc)})
        return 2

    if args.self_check:
        return self_check(args, deps)

    try:
        if args.pair:
            pairs = read_pairs(iter(args.pair))
        else:
            pairs = read_pairs(sys.stdin)
    except Fatal as exc:
        emit({"ok": False, "fatal": str(exc)})
        return 2

    if not pairs:
        emit({"ok": False, "fatal": "no input pairs"})
        return 2

    for source, output in pairs:
        try:
            if not source.exists():
                raise ValueError(f"source not found: {source}")
            emit(convert_one(source, output, args, deps))
        except Fatal:
            raise
        except Exception as exc:
            emit(
                {
                    "source": str(source),
                    "output": str(output),
                    "ok": False,
                    "error": str(exc) or exc.__class__.__name__,
                }
            )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Fatal as fatal:
        emit({"ok": False, "fatal": str(fatal)})
        sys.exit(2)
    except KeyboardInterrupt:
        sys.exit(130)
