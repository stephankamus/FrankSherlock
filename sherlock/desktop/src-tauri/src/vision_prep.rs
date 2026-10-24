//! Image preparation for the vision model.
//!
//! Sending the original file to Ollama is the single largest cost in the
//! classification pipeline: a 12–14 MP JPEG keeps the image preprocessor busy for
//! ~10 s, while the same photo downscaled to 1024 px costs ~2.5 s and produces the
//! same classification. This module produces the (optionally) downscaled copy that
//! is handed to the model, applying EXIF orientation on the way so rotated phone
//! photos are not fed sideways.
//!
//! `max_side = 0` means "send the original bytes untouched".

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::config::RuntimeSettings;
use crate::error::{AppError, AppResult};

/// JPEG quality used for the downscaled copy. High enough that small text in
/// screenshots stays legible for OCR.
pub const JPEG_QUALITY: u8 = 88;

/// An image ready to be attached to an Ollama request.
///
/// When a downscaled copy was generated, the backing temp file is removed on drop.
#[derive(Debug)]
pub struct PreparedImage {
    /// File to attach to the request (a temp copy when downscaled).
    pub path: PathBuf,
    /// Original file. OCR engines that need full resolution use this instead.
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Size of the file on disk before preparation (used by tests and logs).
    #[allow(dead_code)]
    pub original_bytes: u64,
    /// Size of the bytes that will actually be uploaded.
    pub prepared_bytes: u64,
    temp: bool,
}

impl PreparedImage {
    /// True when `path` points at a temp file this struct owns.
    #[allow(dead_code)] // exercised by tests; kept as part of the public surface
    pub fn is_temp(&self) -> bool {
        self.temp
    }
}

impl Drop for PreparedImage {
    fn drop(&mut self) {
        if self.temp {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Read image dimensions from the file header without decoding pixels.
pub fn image_dimensions(path: &Path) -> AppResult<(u32, u32)> {
    image::image_dimensions(path).map_err(|e| {
        AppError::Config(format!(
            "cannot read image size for {}: {e}",
            path.display()
        ))
    })
}

/// Prepare `source` for the vision model.
///
/// Fast paths (no re-encode, no temp file):
/// - `max_side == 0` (user asked for originals)
/// - source is a JPEG that already fits within `max_side` and needs no rotation
pub fn prepare(source: &Path, max_side: u32, tmp_dir: &Path) -> AppResult<PreparedImage> {
    let max_side = RuntimeSettings::normalize_image_max_side(max_side);
    let original_bytes = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    let (src_w, src_h) = image_dimensions(source)?;

    if max_side == 0 {
        return Ok(PreparedImage {
            path: source.to_path_buf(),
            source: source.to_path_buf(),
            width: src_w,
            height: src_h,
            original_bytes,
            prepared_bytes: original_bytes,
            temp: false,
        });
    }

    let orientation = crate::exif::extract_orientation(source);
    let is_jpeg = source
        .extension()
        .map(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            e == "jpg" || e == "jpeg"
        })
        .unwrap_or(false);
    let needs_resize = src_w.max(src_h) > max_side;
    let needs_rotation = orientation != 1;

    if is_jpeg && !needs_resize && !needs_rotation {
        return Ok(PreparedImage {
            path: source.to_path_buf(),
            source: source.to_path_buf(),
            width: src_w,
            height: src_h,
            original_bytes,
            prepared_bytes: original_bytes,
            temp: false,
        });
    }

    let decoded = image::open(source)
        .map_err(|e| AppError::Config(format!("cannot decode {}: {e}", source.display())))?;
    let decoded = crate::exif::apply_orientation(decoded, orientation);

    let (w, h) = (decoded.width(), decoded.height());
    let resized = if w.max(h) > max_side {
        let scale = max_side as f64 / w.max(h) as f64;
        let new_w = ((w as f64 * scale).round() as u32).max(1);
        let new_h = ((h as f64 * scale).round() as u32).max(1);
        decoded.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
    } else {
        decoded
    };

    let (out_w, out_h) = (resized.width(), resized.height());
    let out_path = tmp_path(source, max_side, tmp_dir)?;
    let rgb = resized.to_rgb8();
    let mut file = std::fs::File::create(&out_path)?;
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, JPEG_QUALITY);
    rgb.write_with_encoder(encoder)
        .map_err(|e| AppError::Config(format!("cannot encode {}: {e}", out_path.display())))?;

    let prepared_bytes = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    Ok(PreparedImage {
        path: out_path,
        source: source.to_path_buf(),
        width: out_w,
        height: out_h,
        original_bytes,
        prepared_bytes,
        temp: true,
    })
}

/// Stable temp path for a (source, max_side) pair so repeated calls inside one
/// scan reuse the same file instead of spraying the temp directory.
fn tmp_path(source: &Path, max_side: u32, tmp_dir: &Path) -> AppResult<PathBuf> {
    let meta = std::fs::metadata(source)
        .map_err(|e| AppError::Config(format!("cannot stat {}: {e}", source.display())))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.to_string_lossy().hash(&mut hasher);
    meta.len().hash(&mut hasher);
    mtime.hash(&mut hasher);
    max_side.hash(&mut hasher);
    let hash = hasher.finish();

    std::fs::create_dir_all(tmp_dir)?;
    Ok(tmp_dir.join(format!("vp_{hash:016x}.jpg")))
}

/// Conservative estimate of the image token count Ollama will produce for a
/// `w × h` image with qwen2.5-VL (28 px patches, ~20% margin over what we
/// measured locally: 640→407-614, 768→530-814, 1024→918-1555, 1280→2302).
pub fn estimated_image_tokens(w: u32, h: u32) -> u32 {
    let patches = w.div_ceil(28) as u64 * h.div_ceil(28) as u64;
    (patches as f64 * 1.2) as u32 + 128
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 120]));
        img.save(path).expect("save test jpeg");
    }

    #[test]
    fn downscales_large_image_to_max_side() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("big.jpg");
        write_jpeg(&src, 2000, 1000);

        let prepared = prepare(&src, 1024, &dir.path().join("tmp")).expect("prepare");
        assert!(prepared.is_temp());
        assert_eq!(prepared.width, 1024);
        assert_eq!(prepared.height, 512);
        assert!(prepared.path.exists());
        assert_ne!(prepared.path, src);
        assert!(prepared.prepared_bytes < prepared.original_bytes);
    }

    #[test]
    fn keeps_small_jpeg_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("small.jpg");
        write_jpeg(&src, 800, 600);

        let prepared = prepare(&src, 1024, &dir.path().join("tmp")).expect("prepare");
        assert!(!prepared.is_temp());
        assert_eq!(prepared.path, src);
        assert_eq!((prepared.width, prepared.height), (800, 600));
    }

    #[test]
    fn max_side_zero_keeps_original() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("huge.jpg");
        write_jpeg(&src, 3000, 2000);

        let prepared = prepare(&src, 0, &dir.path().join("tmp")).expect("prepare");
        assert!(!prepared.is_temp());
        assert_eq!((prepared.width, prepared.height), (3000, 2000));
        assert_eq!(prepared.prepared_bytes, prepared.original_bytes);
    }

    #[test]
    fn reencodes_png_even_when_it_fits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("shot.png");
        RgbImage::from_fn(640, 480, |x, _| Rgb([x as u8, 10, 10]))
            .save(&src)
            .expect("save png");

        let prepared = prepare(&src, 1024, &dir.path().join("tmp")).expect("prepare");
        assert!(prepared.is_temp(), "PNG must be re-encoded to JPEG");
        assert!(prepared.path.to_string_lossy().ends_with(".jpg"));
    }

    #[test]
    fn temp_file_is_removed_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("big.jpg");
        write_jpeg(&src, 2000, 1000);

        let path = {
            let prepared = prepare(&src, 1024, &dir.path().join("tmp")).expect("prepare");
            prepared.path.clone()
        };
        assert!(!path.exists(), "temp copy must be cleaned up");
        assert!(src.exists(), "source must never be touched");
    }

    #[test]
    fn same_source_reuses_same_temp_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("big.jpg");
        write_jpeg(&src, 2000, 1000);
        let tmp = dir.path().join("tmp");

        let a = prepare(&src, 1024, &tmp).expect("prepare");
        let path_a = a.path.clone();
        let b = prepare(&src, 1024, &tmp).expect("prepare");
        assert_eq!(path_a, b.path);
    }

    #[test]
    fn estimated_image_tokens_track_measurements() {
        // Measured on Ollama 0.23.1 / qwen2.5vl at 640/768/1024 px.
        assert!(estimated_image_tokens(640, 640) >= 407);
        assert!(estimated_image_tokens(768, 768) >= 530);
        let t1024 = estimated_image_tokens(1024, 1024);
        assert!((1400..=1900).contains(&t1024), "got {t1024}");
        assert!(estimated_image_tokens(1280, 1280) > t1024);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = prepare(&dir.path().join("nope.jpg"), 1024, &dir.path().join("tmp"));
        assert!(err.is_err());
    }
}
