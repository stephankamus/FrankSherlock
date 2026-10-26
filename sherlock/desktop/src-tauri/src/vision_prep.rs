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
use std::time::Instant;

use crate::config::{ImageFilter, RuntimeSettings};
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

/// Size [`prepare_from_decoded`] will produce for a source of `width × height`.
///
/// Extracted so the pipeline can predict the context window of a row **without**
/// decoding it: `num_ctx` is derived from the prepared size, and getting it wrong
/// is what makes Ollama rebuild the runner between two files (§5.5 of
/// `plans/pipeline-cpu-gpu-overlap.md`). `prepare_from_decoded` calls this same
/// function, so the prediction cannot drift from the real output.
pub fn prepared_size(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let max_side = RuntimeSettings::normalize_image_max_side(max_side);
    let largest = width.max(height);
    if max_side == 0 || largest <= max_side {
        return (width, height);
    }
    let scale = max_side as f64 / largest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// Prepare `source` for the vision model with the default kernel policy.
///
/// Callers that carry [`crate::models::VisionSettings`] use [`prepare_with`] so
/// the user's `image_filter` is honoured; this wrapper exists for tests and for
/// any future caller that has no settings at hand.
#[allow(dead_code)]
pub fn prepare(source: &Path, max_side: u32, tmp_dir: &Path) -> AppResult<PreparedImage> {
    prepare_with(source, max_side, tmp_dir, ImageFilter::Auto)
}

/// True when `filter` resolves to the area-average kernel for this file.
///
/// `Auto` picks Lanczos only for the formats that are usually screenshots or
/// documents, where thin text benefits from a sharpening kernel. Photos are
/// dominated by chroma noise, and a 3–4× downscale is exactly what an area
/// average is for. Measured on a 174-file SMB library, `prepare` cost
/// 606 ms/file with Lanczos3 against 300 ms for the thumbnail, which does the
/// same resize + encode with the box filter.
pub fn prefers_box(source: &Path, filter: ImageFilter) -> bool {
    match filter {
        ImageFilter::Box => true,
        ImageFilter::Lanczos => false,
        ImageFilter::Auto => !is_text_like(source),
    }
}

fn is_text_like(source: &Path) -> bool {
    source
        .extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            matches!(ext.as_str(), "png" | "bmp" | "tif" | "tiff")
        })
        .unwrap_or(false)
}

fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            ext == "jpg" || ext == "jpeg"
        })
        .unwrap_or(false)
}

/// A request that will upload the original file untouched.
fn passthrough(source: &Path, original_bytes: u64, width: u32, height: u32) -> PreparedImage {
    PreparedImage {
        path: source.to_path_buf(),
        source: source.to_path_buf(),
        width,
        height,
        original_bytes,
        prepared_bytes: original_bytes,
        temp: false,
    }
}

/// Prepare `source` for the vision model, reading and decoding it here.
///
/// Fast paths (no re-encode, no temp file):
/// - `max_side == 0` (user asked for originals)
/// - source is a JPEG that already fits within `max_side` and needs no rotation
pub fn prepare_with(
    source: &Path,
    max_side: u32,
    tmp_dir: &Path,
    filter: ImageFilter,
) -> AppResult<PreparedImage> {
    let max_side = RuntimeSettings::normalize_image_max_side(max_side);
    let original_bytes = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    let (src_w, src_h) = image_dimensions(source)?;

    if max_side == 0 {
        return Ok(passthrough(source, original_bytes, src_w, src_h));
    }

    let orientation = crate::exif::extract_orientation(source);
    let needs_resize = src_w.max(src_h) > max_side;
    let needs_rotation = orientation != 1;

    if is_jpeg(source) && !needs_resize && !needs_rotation {
        return Ok(passthrough(source, original_bytes, src_w, src_h));
    }

    let decode_started = Instant::now();
    let decoded = image::open(source)
        .map_err(|e| AppError::Config(format!("cannot decode {}: {e}", source.display())))?;
    let decoded = crate::exif::apply_orientation(decoded, orientation);
    log::debug!(
        "vision_prep {}: decode={}ms (exif orientation {orientation})",
        source.display(),
        decode_started.elapsed().as_millis()
    );

    prepare_from_decoded(
        source,
        original_bytes,
        orientation,
        &decoded,
        max_side,
        tmp_dir,
        filter,
    )
}

/// Prepare `source` when the image is **already decoded**.
///
/// This is the entry point of the CPU/GPU pipeline: the thumbnail pass has
/// already paid for the decode and the EXIF rotation, so reading and decoding the
/// file a second time is pure waste (`plans/pipeline-cpu-gpu-overlap.md`,
/// Phase 1). `decoded` must have orientation already applied; `orientation` is
/// the value read from the file, still needed because Ollama ignores EXIF and a
/// rotated JPEG therefore cannot travel untouched.
#[allow(clippy::too_many_arguments)]
pub fn prepare_from_decoded(
    source: &Path,
    original_bytes: u64,
    orientation: u16,
    decoded: &image::DynamicImage,
    max_side: u32,
    tmp_dir: &Path,
    filter: ImageFilter,
) -> AppResult<PreparedImage> {
    let max_side = RuntimeSettings::normalize_image_max_side(max_side);
    let (w, h) = (decoded.width(), decoded.height());

    // A JPEG that already fits and is upright travels untouched: re-encoding it
    // would only add a generation loss.
    if max_side == 0 || (is_jpeg(source) && w.max(h) <= max_side && orientation == 1) {
        return Ok(passthrough(source, original_bytes, w, h));
    }

    let box_filter = prefers_box(source, filter);
    let resize_started = Instant::now();
    // Same function the pipeline uses to predict the context window (see
    // [`prepared_size`]); the resize below must stay exactly this.
    let (new_w, new_h) = prepared_size(w, h, max_side);
    let resized = if (new_w, new_h) != (w, h) {
        if box_filter {
            image::DynamicImage::ImageRgba8(image::imageops::thumbnail(decoded, new_w, new_h))
        } else {
            decoded.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
        }
    } else {
        decoded.clone()
    };
    let resize_ms = resize_started.elapsed().as_millis();

    let (out_w, out_h) = (resized.width(), resized.height());
    let out_path = tmp_path(source, max_side, tmp_dir, box_filter)?;
    let encode_started = Instant::now();
    let rgb = resized.to_rgb8();
    let mut file = std::fs::File::create(&out_path)?;
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, JPEG_QUALITY);
    rgb.write_with_encoder(encoder)
        .map_err(|e| AppError::Config(format!("cannot encode {}: {e}", out_path.display())))?;
    let encode_ms = encode_started.elapsed().as_millis();

    // Phase 0 of `plans/pipeline-cpu-gpu-overlap.md`: `prepare_with` used to do a
    // second full decode here, and a scan runs the thumbnail just before it.
    log::debug!(
        "vision_prep {}: {w}x{h} -> {out_w}x{out_h} filter={} resize={resize_ms}ms encode={encode_ms}ms",
        source.display(),
        if box_filter { "box" } else { "lanczos3" }
    );

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

/// Stable temp path for a (source, max_side, kernel) tuple so repeated calls
/// inside one scan reuse the same file instead of spraying the temp directory,
/// and two kernels never resolve to the same file.
fn tmp_path(source: &Path, max_side: u32, tmp_dir: &Path, box_filter: bool) -> AppResult<PathBuf> {
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
    box_filter.hash(&mut hasher);
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
    fn auto_picks_box_for_photos_and_lanczos_for_screenshots() {
        assert!(prefers_box(Path::new("photo.jpg"), ImageFilter::Auto));
        assert!(!prefers_box(Path::new("shot.png"), ImageFilter::Auto));
        assert!(!prefers_box(Path::new("scan.tiff"), ImageFilter::Auto));
        assert!(prefers_box(Path::new("shot.png"), ImageFilter::Box));
        assert!(!prefers_box(Path::new("photo.jpg"), ImageFilter::Lanczos));
    }

    /// Both kernels must land on the same geometry and never share a temp file,
    /// so a `BENCH_QUALITY` comparison really compares the kernels.
    #[test]
    fn box_and_lanczos_agree_on_size_but_not_on_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("big.png");
        RgbImage::from_fn(2000, 1500, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .save(&src)
        .expect("save png");
        let tmp = dir.path().join("tmp");

        let boxed = prepare_with(&src, 1024, &tmp, ImageFilter::Box).expect("box");
        let lanczos = prepare_with(&src, 1024, &tmp, ImageFilter::Lanczos).expect("lanczos");

        assert_eq!((boxed.width, boxed.height), (1024, 768));
        assert_eq!((boxed.width, boxed.height), (lanczos.width, lanczos.height));
        assert_ne!(
            boxed.path, lanczos.path,
            "kernels must not resolve to the same temp file"
        );
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

    /// Feeding a decoded image must give exactly what reading the file gives.
    #[test]
    fn prepare_from_decoded_matches_prepare_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("big.png");
        RgbImage::from_fn(1600, 1200, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        })
        .save(&src)
        .expect("save png");
        let tmp = dir.path().join("tmp");
        let bytes = std::fs::metadata(&src).expect("metadata").len();

        let read_path = prepare_with(&src, 1024, &tmp, ImageFilter::Auto).expect("prepare_with");
        let expected = (read_path.width, read_path.height, read_path.prepared_bytes);

        let decoded = image::open(&src).expect("open");
        let decoded = crate::exif::apply_orientation(decoded, 1);
        let shared = prepare_from_decoded(&src, bytes, 1, &decoded, 1024, &tmp, ImageFilter::Auto)
            .expect("prepare_from_decoded");

        assert_eq!(
            (shared.width, shared.height, shared.prepared_bytes),
            expected,
            "the shared decode must produce the same JPEG as the reading path"
        );
    }

    /// An upright JPEG that already fits is uploaded untouched — no re-encode.
    #[test]
    fn prepare_from_decoded_passes_through_a_fitting_jpeg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("small.jpg");
        write_jpeg(&src, 800, 600);
        let tmp = dir.path().join("tmp");
        let bytes = std::fs::metadata(&src).expect("metadata").len();

        let decoded = image::open(&src).expect("open");
        let prepared =
            prepare_from_decoded(&src, bytes, 1, &decoded, 1024, &tmp, ImageFilter::Auto)
                .expect("prepare");

        assert!(!prepared.is_temp());
        assert_eq!(prepared.path, src);
        assert_eq!((prepared.width, prepared.height), (800, 600));
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

    /// The pipeline predicts the context window from `prepared_size`; if that ever
    /// disagrees with what `prepare` produces, the prediction silently starts
    /// requesting the wrong `num_ctx` and Ollama reloads the model between files.
    #[test]
    fn prepared_size_matches_what_prepare_produces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("tmp");

        let wide = dir.path().join("wide.jpg");
        write_jpeg(&wide, 2000, 1000);
        let prepared = prepare(&wide, 1024, &tmp).expect("prepare");
        assert_eq!(
            prepared_size(2000, 1000, 1024),
            (prepared.width, prepared.height)
        );

        let tall = dir.path().join("tall.jpg");
        write_jpeg(&tall, 700, 1500);
        let prepared = prepare(&tall, 1024, &tmp).expect("prepare");
        assert_eq!(
            prepared_size(700, 1500, 1024),
            (prepared.width, prepared.height)
        );

        // Already small enough, and `max_side = 0` (originals): untouched.
        assert_eq!(prepared_size(640, 480, 1024), (640, 480));
        assert_eq!(prepared_size(4000, 3000, 0), (4000, 3000));
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = prepare(&dir.path().join("nope.jpg"), 1024, &dir.path().join("tmp"));
        assert!(err.is_err());
    }
}
