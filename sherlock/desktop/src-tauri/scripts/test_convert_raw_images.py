#!/usr/bin/env python3
"""Tests for convert_raw_images.py.

Run with the conversion venv (needs Pillow; pillow-heif and rawpy for the
fixture-based tests):

    python -m unittest discover -s sherlock/desktop/src-tauri/scripts -p 'test_*.py'

The tests that need a real HEIC/NEF file are skipped unless `FRANK_RAW_SMOKE_DIR`
points at a directory that has some, mirroring the vision smoke test:

    FRANK_RAW_SMOKE_DIR="$HOME/Imagens/Fotos Grandes" \
        python -m unittest test_convert_raw_images -v
"""

from __future__ import annotations

import importlib.util
import io
import os
import tempfile
import unittest
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "convert_raw_images.py"


def load_module():
    spec = importlib.util.spec_from_file_location("convert_raw_images", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


conv = load_module()


def make_jpeg_with_metadata(path: Path, orientation: int = 6) -> None:
    """Write a JPEG carrying Orientation plus a GPS IFD, as a camera would."""
    image = Image.new("RGB", (64, 32), (10, 20, 30))
    exif = Image.Exif()
    exif[274] = orientation
    exif[271] = "TestCam"
    exif_ifd = exif.get_ifd(34665)
    exif_ifd[36867] = "2019:05:04 10:11:12"

    gps = exif.get_ifd(34853)
    gps[1] = "S"
    gps[2] = (23.0, 33.0, 0.0)
    gps[3] = "W"
    gps[4] = (46.0, 38.0, 0.0)

    image.save(path, "JPEG", exif=exif.tobytes())


class ExifBytesTests(unittest.TestCase):
    def test_orientation_is_forced_to_one_and_gps_survives(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "src.jpg"
            make_jpeg_with_metadata(source, orientation=6)

            with Image.open(source) as image:
                original = image.getexif()
                self.assertEqual(original.get(274), 6)
                self.assertTrue(dict(original.get_ifd(34853)))

                copied = conv.exif_bytes(image)

            self.assertIsNotNone(copied, "exif_bytes returned nothing for a JPEG with EXIF")

            # Round-trip through a real JPEG: `Image.Exif.tobytes()` only serialises
            # sub-IFDs that were materialised, and GPS is the one that silently
            # disappears when they were not.
            roundtrip = Path(tmp) / "out.jpg"
            with Image.open(source) as image:
                image.save(roundtrip, "JPEG", exif=copied)
            with Image.open(roundtrip) as image:
                exif = image.getexif()
                self.assertEqual(exif.get(274), 1, "orientation must be reset to upright")
                self.assertEqual(exif.get(271), "TestCam")
                self.assertEqual(dict(exif.get_ifd(34665)).get(36867), "2019:05:04 10:11:12")
                gps = dict(exif.get_ifd(34853))
                self.assertEqual(gps.get(1), "S", "GPS latitude ref must survive")
                self.assertEqual(gps.get(3), "W", "GPS longitude ref must survive")

    def test_image_without_exif_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "plain.png"
            Image.new("RGB", (8, 8), (1, 2, 3)).save(path)
            with Image.open(path) as image:
                self.assertIsNone(conv.exif_bytes(image))


class ReadPairsTests(unittest.TestCase):
    def test_parses_source_and_output(self):
        pairs = conv.read_pairs(iter(["a.heic\tout/a.jpg\n", "\n", "b.NEF\tout/b.jpg\n"]))
        self.assertEqual(
            pairs,
            [(Path("a.heic"), Path("out/a.jpg")), (Path("b.NEF"), Path("out/b.jpg"))],
        )

    def test_rejects_line_without_tab(self):
        with self.assertRaises(conv.Fatal):
            conv.read_pairs(iter(["a.heic out/a.jpg\n"]))

    def test_rejects_empty_input(self):
        self.assertEqual(conv.read_pairs(iter([])), [])


class ResizeTests(unittest.TestCase):
    def test_zero_keeps_size(self):
        image = Image.new("RGB", (4000, 2000))
        self.assertEqual(conv.resize_if_needed(image, 0, Image).size, (4000, 2000))

    def test_downscales_longest_side(self):
        image = Image.new("RGB", (4000, 2000))
        self.assertEqual(conv.resize_if_needed(image, 1000, Image).size, (1000, 500))

    def test_smaller_image_is_untouched(self):
        image = Image.new("RGB", (400, 200))
        self.assertEqual(conv.resize_if_needed(image, 1000, Image).size, (400, 200))


class FlattenTests(unittest.TestCase):
    def test_rgba_is_composited_over_white(self):
        image = Image.new("RGBA", (4, 4), (255, 0, 0, 0))
        flat = conv.flatten_to_rgb(image)
        self.assertEqual(flat.mode, "RGB")
        self.assertEqual(flat.getpixel((0, 0)), (255, 255, 255))

    def test_rgb_passes_through(self):
        image = Image.new("RGB", (2, 2), (5, 6, 7))
        self.assertEqual(conv.flatten_to_rgb(image).getpixel((0, 0)), (5, 6, 7))


SMOKE_DIR = os.environ.get("FRANK_RAW_SMOKE_DIR")


def first_with_suffix(suffix: str) -> Path | None:
    if not SMOKE_DIR:
        return None
    root = Path(SMOKE_DIR)
    if not root.is_dir():
        return None
    for entry in sorted(root.iterdir()):
        if entry.suffix.lower() == suffix:
            return entry
    return None


def args_for(**overrides):
    defaults = dict(
        max_side=0,
        quality=90,
        nef_mode="auto",
        min_preview_side=1024,
        overwrite=False,
        dry_run=False,
    )
    defaults.update(overrides)
    return type("Args", (), defaults)()


@unittest.skipUnless(first_with_suffix(".heic"), "FRANK_RAW_SMOKE_DIR with a HEIC file")
class HeicSmokeTests(unittest.TestCase):
    def test_converts_upright_with_preserved_mtime(self):
        source = first_with_suffix(".heic")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            result = conv.convert_one(source, output, args_for(), deps)
            self.assertTrue(result["ok"], result.get("error"))
            self.assertEqual(result["mode"], "heic")
            self.assertTrue(output.exists())
            self.assertEqual(
                int(output.stat().st_mtime), int(source.stat().st_mtime),
                "the copy must keep the source mtime",
            )
            with Image.open(output) as image:
                self.assertEqual(image.getexif().get(274), 1)

    def test_second_run_skips_existing_output(self):
        source = first_with_suffix(".heic")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            conv.convert_one(source, output, args_for(), deps)
            size_before = output.stat().st_size
            result = conv.convert_one(source, output, args_for(), deps)
            self.assertEqual(result.get("skipped"), "exists")
            self.assertEqual(output.stat().st_size, size_before)

    def test_max_side_is_honoured(self):
        source = first_with_suffix(".heic")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            result = conv.convert_one(source, output, args_for(max_side=1024), deps)
            self.assertTrue(result["ok"], result.get("error"))
            self.assertLessEqual(max(result["width"], result["height"]), 1024)


@unittest.skipUnless(first_with_suffix(".nef"), "FRANK_RAW_SMOKE_DIR with a NEF file")
class NefSmokeTests(unittest.TestCase):
    def test_auto_uses_the_embedded_preview_when_it_is_big_enough(self):
        source = first_with_suffix(".nef")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            result = conv.convert_one(source, output, args_for(), deps)
            self.assertTrue(result["ok"], result.get("error"))
            self.assertIn(result["mode"], ("preview", "demosaic"))
            self.assertTrue(output.exists())

    def test_preview_mode_never_demosaics(self):
        source = first_with_suffix(".nef")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            result = conv.convert_one(source, output, args_for(nef_mode="preview"), deps)
            if not result["ok"]:
                # A NEF without an embedded JPEG preview is a legitimate failure.
                self.assertIn("preview", result.get("error", ""))
            else:
                self.assertEqual(result["mode"], "preview")


class DryRunTests(unittest.TestCase):
    @unittest.skipUnless(first_with_suffix(".heic"), "FRANK_RAW_SMOKE_DIR with a HEIC file")
    def test_dry_run_writes_nothing(self):
        source = first_with_suffix(".heic")
        deps = conv.load_dependencies()
        assert source is not None
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out.jpg"
            result = conv.convert_one(source, output, args_for(dry_run=True), deps)
            self.assertTrue(result["ok"], result.get("error"))
            self.assertTrue(result["dry_run"])
            self.assertGreater(result["bytes"], 0)
            self.assertFalse(output.exists(), "dry-run must not create files")


if __name__ == "__main__":
    unittest.main(verbosity=2)
