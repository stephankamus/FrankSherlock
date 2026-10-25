use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::platform::paths::normalize_rel_path;

/// Result of thumbnail generation, including optional perceptual hash.
pub struct ThumbnailResult {
    pub path: String,
    /// dHash computed from the image. `Some` when the thumbnail was freshly
    /// generated, `None` when the cached thumbnail was reused (image not decoded).
    pub dhash: Option<u64>,
}

/// Compute a 64-bit difference hash (dHash) from a decoded image.
///
/// Resizes to 9x8 with a fast bilinear filter, converts to grayscale, and
/// compares adjacent horizontal pixels to produce a 64-bit hash.
pub fn compute_dhash(img: &image::DynamicImage) -> u64 {
    let small = img.resize_exact(9, 8, image::imageops::FilterType::Triangle);
    let gray = small.to_luma8();
    let mut hash: u64 = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = gray.get_pixel(x, y).0[0];
            let right = gray.get_pixel(x + 1, y).0[0];
            if left > right {
                hash |= 1 << (y * 8 + x);
            }
        }
    }
    hash
}

/// Downscale so the longest side is at most `max_dim`.
///
/// `imageops::thumbnail` is a box filter specialized for large downscales: on a
/// 12-40 MP photo it is several times faster than `resize(.., Lanczos3)` and the
/// difference is invisible at 300 px. Images that already fit are returned as-is.
///
/// Takes the image by reference because the pipeline decodes once and feeds both
/// the thumbnail and the vision preparation from the same buffer; cloning a
/// 12-40 MP image just to shrink it would give the saving straight back.
fn downscale(img: &image::DynamicImage, max_dim: u32) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_dim && h <= max_dim {
        return img.clone();
    }
    let max_dim = max_dim.max(1);
    let max_side = w.max(h).max(1);
    let new_w = ((w as f64 * max_dim as f64 / max_side as f64).round() as u32).max(1);
    let new_h = ((h as f64 * max_dim as f64 / max_side as f64).round() as u32).max(1);
    image::DynamicImage::ImageRgba8(image::imageops::thumbnail(img, new_w, new_h))
}

/// Downscale to an exact target size (PDF pages are already small).
fn downscale_to(img: image::DynamicImage, new_w: u32, new_h: u32) -> image::DynamicImage {
    image::DynamicImage::ImageRgba8(image::imageops::thumbnail(&img, new_w.max(1), new_h.max(1)))
}

/// Directory prefix of the thumbnail cache that belongs to one root.
///
/// Derived from the canonical root path instead of `root_id`: removing a root and
/// adding it back reuses the cache, and two roots can never resolve to the same
/// file — which is what allowed one root's purge to delete another root's
/// thumbnails.
pub fn root_key(root_path: &str) -> String {
    let digest = Sha256::digest(normalize_rel_path(root_path).as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Canonical path of the thumbnail for `rel_path` under `<thumbnails>/<root_key>`.
pub fn thumb_path_for(thumbnails_dir: &Path, root_key: &str, rel_path: &str) -> PathBuf {
    thumb_path_in(&thumbnails_dir.join(root_key), rel_path)
}

/// Thumbnail path for `rel_path` inside an already root-scoped directory.
fn thumb_path_in(thumb_dir: &Path, rel_path: &str) -> PathBuf {
    let stem = normalize_rel_path(&Path::new(rel_path).with_extension("jpg").to_string_lossy());
    thumb_dir.join(stem)
}

/// Create `<thumb_dir>/<rel_path>.jpg` as a hard link to an existing thumbnail.
///
/// Used to deduplicate: byte-identical files never touch the source at all, and
/// near-duplicates reuse the JPEG that was already encoded. Each row keeps its own
/// path name (so deleting one row's thumbnail is safe) while the inode — and
/// therefore the disk space — is shared.
pub fn reuse_thumbnail(
    shared: &Path,
    dhash: Option<u64>,
    thumb_dir: &Path,
    rel_path: &str,
) -> Option<ThumbnailResult> {
    if !shared.exists() {
        return None;
    }
    let thumb_path = thumb_path_in(thumb_dir, rel_path);
    if thumb_path == shared {
        // Already the same file: nothing to link.
        return Some(ThumbnailResult {
            path: thumb_path.display().to_string(),
            dhash,
        });
    }
    if let Some(parent) = thumb_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("Failed to create thumbnail dir {}: {e}", parent.display());
            return None;
        }
    }
    if thumb_path.exists() {
        let _ = std::fs::remove_file(&thumb_path);
    }
    if let Err(link_err) = std::fs::hard_link(shared, &thumb_path) {
        // Different filesystem (cache moved between devices) — copy instead.
        if let Err(copy_err) = std::fs::copy(shared, &thumb_path) {
            log::warn!(
                "Failed to link thumbnail {} -> {}: {link_err} / {copy_err}",
                shared.display(),
                thumb_path.display()
            );
            return None;
        }
    }
    Some(ThumbnailResult {
        path: thumb_path.display().to_string(),
        dhash,
    })
}

/// A decoded image plus the perceptual hash computed from it.
///
/// Decoding is the expensive half of both the thumbnail and the vision
/// preparation, and a scan used to pay for it twice per file. This is the shared
/// artefact that removes the second decode
/// (`plans/pipeline-cpu-gpu-overlap.md`, Phase 1).
pub struct DecodedSource {
    /// The image with EXIF orientation already applied.
    pub image: image::DynamicImage,
    pub dhash: u64,
    /// Orientation read from the file (`1` = upright). Kept because
    /// `vision_prep::prepare_from_decoded` needs it to decide whether the
    /// original bytes can be uploaded untouched — Ollama ignores EXIF, so a
    /// rotated JPEG must be re-encoded even when it already fits.
    #[allow(dead_code)] // consumed by the pipeline producer (Phase 2)
    pub orientation: u16,
}

/// Decode `source`, apply EXIF orientation and compute the dHash — once.
pub fn decode_source(source_path: &Path) -> Option<DecodedSource> {
    let effective_path = first_frame_if_gif(source_path);

    let decode_started = Instant::now();
    let img = match image::open(&effective_path) {
        Ok(img) => img,
        Err(e) => {
            log::warn!(
                "Failed to open image for thumbnail {}: {e}",
                source_path.display()
            );
            return None;
        }
    };

    // Apply EXIF orientation so thumbnails and vision input match the display
    let orientation = crate::exif::extract_orientation(source_path);
    let img = crate::exif::apply_orientation(img, orientation);
    let decode_ms = decode_started.elapsed().as_millis();

    let dhash_started = Instant::now();
    let dhash = compute_dhash(&img);
    log::debug!(
        "decode_source {}: {}x{} decode={decode_ms}ms dhash={}ms",
        source_path.display(),
        img.width(),
        img.height(),
        dhash_started.elapsed().as_millis()
    );

    Some(DecodedSource {
        image: img,
        dhash,
        orientation,
    })
}

/// Encode the 300 px preview from an already decoded image.
///
/// Always writes. Reusing a cached file is the caller's decision, because only
/// the caller knows whether the source changed.
pub fn thumbnail_from_decoded(
    decoded: &DecodedSource,
    thumb_dir: &Path,
    rel_path: &str,
) -> Option<ThumbnailResult> {
    let thumb_path = thumb_path_in(thumb_dir, rel_path);

    if let Some(parent) = thumb_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("Failed to create thumbnail dir {}: {e}", parent.display());
            return None;
        }
    }

    let encode_started = Instant::now();
    // Images that already fit are kept as-is (just re-encoded as JPEG).
    let resized = downscale(&decoded.image, 300);

    let rgb = resized.to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
    if let Err(e) = rgb.write_with_encoder(encoder) {
        log::warn!("Failed to encode thumbnail: {e}");
        return None;
    }

    if let Err(e) = std::fs::write(&thumb_path, buf.into_inner()) {
        log::warn!("Failed to write thumbnail {}: {e}", thumb_path.display());
        return None;
    }
    log::debug!(
        "thumbnail {}: resize+encode={}ms",
        thumb_path.display(),
        encode_started.elapsed().as_millis()
    );

    Some(ThumbnailResult {
        path: thumb_path.display().to_string(),
        dhash: Some(decoded.dhash),
    })
}

/// Generate a thumbnail for the given source image.
///
/// Returns a `ThumbnailResult` with the path and optional dHash, or `None`
/// if generation fails. When `force` is `false` and the thumbnail already exists
/// it is reused without decoding anything (`dhash` is then `None`).
///
/// Validation never compares filesystem mtimes: SMB servers routinely report
/// timestamps in the future, which made `thumb_mtime >= source_mtime` always fail
/// and forced a full re-decode of the library on every scan. Whether the source
/// changed is decided by the scan's own size+mtime discovery, which passes
/// `force = true` for new/modified files only.
pub fn generate_thumbnail(
    source_path: &Path,
    thumb_dir: &Path,
    rel_path: &str,
    force: bool,
) -> Option<ThumbnailResult> {
    let thumb_path = thumb_path_in(thumb_dir, rel_path);

    if !force && thumb_path.exists() {
        return Some(ThumbnailResult {
            path: thumb_path.display().to_string(),
            dhash: None,
        });
    }

    let decoded = decode_source(source_path)?;
    thumbnail_from_decoded(&decoded, thumb_dir, rel_path)
}

/// Generate a thumbnail for a PDF: up to 2 content pages side-by-side, 300px max dimension.
pub fn generate_pdf_thumbnail(
    pdf_path: &Path,
    thumb_dir: &Path,
    rel_path: &str,
    pdfium_lib: &Path,
    password: Option<&str>,
    force: bool,
) -> Option<ThumbnailResult> {
    let thumb_path = thumb_path_in(thumb_dir, rel_path);

    if !force && thumb_path.exists() {
        return Some(ThumbnailResult {
            path: thumb_path.display().to_string(),
            dhash: None,
        });
    }

    // Ensure parent directory exists
    if let Some(parent) = thumb_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("Failed to create thumbnail dir {}: {e}", parent.display());
            return None;
        }
    }

    // Skip password-protected PDFs when no password is provided
    if crate::pdf::is_password_protected(pdf_path, pdfium_lib) && password.is_none() {
        return None;
    }

    // Find up to 2 non-blank content pages
    let content_pages = match crate::pdf::find_content_pages(pdf_path, 2, pdfium_lib, password) {
        Ok(pages) => pages,
        Err(e) => {
            log::warn!(
                "Failed to find content pages in PDF {}: {e}",
                pdf_path.display()
            );
            return None;
        }
    };

    if content_pages.is_empty() {
        return None;
    }

    // Render each content page at moderate resolution (150px width target)
    let mut page_images: Vec<image::DynamicImage> = Vec::new();
    for &page_idx in &content_pages {
        match crate::pdf::render_page(pdf_path, page_idx, 96, pdfium_lib, password) {
            Ok(img) => {
                // Scale page to ~150px width
                let target_w = 150u32;
                let scale = target_w as f64 / img.width().max(1) as f64;
                let new_h = (img.height() as f64 * scale).round() as u32;
                let resized = downscale_to(img, target_w, new_h);
                page_images.push(resized);
            }
            Err(e) => {
                log::warn!("Failed to render PDF page {page_idx}: {e}");
            }
        }
    }

    if page_images.is_empty() {
        return None;
    }

    // Stitch pages side-by-side if we have 2 pages
    let composite = if page_images.len() >= 2 {
        let gap = 2u32;
        let w = page_images[0].width() + gap + page_images[1].width();
        let h = page_images[0].height().max(page_images[1].height());
        let mut canvas = image::RgbImage::from_pixel(w, h, image::Rgb([40, 40, 40]));
        image::imageops::overlay(&mut canvas, &page_images[0].to_rgb8(), 0, 0);
        image::imageops::overlay(
            &mut canvas,
            &page_images[1].to_rgb8(),
            (page_images[0].width() + gap) as i64,
            0,
        );
        image::DynamicImage::ImageRgb8(canvas)
    } else {
        page_images.into_iter().next().unwrap()
    };

    // Scale composite so longest side is 300px
    let final_img = downscale(&composite, 300);

    let dhash = compute_dhash(&final_img);

    let rgb = final_img.to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
    if let Err(e) = rgb.write_with_encoder(encoder) {
        log::warn!("Failed to encode PDF thumbnail: {e}");
        return None;
    }

    if let Err(e) = std::fs::write(&thumb_path, buf.into_inner()) {
        log::warn!(
            "Failed to write PDF thumbnail {}: {e}",
            thumb_path.display()
        );
        return None;
    }

    Some(ThumbnailResult {
        path: thumb_path.display().to_string(),
        dhash: Some(dhash),
    })
}

/// Generate a thumbnail for a video: extract keyframes, pick the first
/// non-black one, resize and save as JPEG.
pub fn generate_video_thumbnail(
    video_path: &Path,
    thumb_dir: &Path,
    rel_path: &str,
    tmp_dir: &Path,
    force: bool,
) -> Option<ThumbnailResult> {
    let thumb_path = thumb_path_in(thumb_dir, rel_path);

    if !force && thumb_path.exists() {
        return Some(ThumbnailResult {
            path: thumb_path.display().to_string(),
            dhash: None,
        });
    }

    // Ensure parent directory exists
    if let Some(parent) = thumb_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("Failed to create thumbnail dir {}: {e}", parent.display());
            return None;
        }
    }

    // Use a per-video temp subdirectory to avoid collisions
    let video_tmp = tmp_dir.join(format!(
        "_vidthumb_{}",
        video_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "vid".to_string())
    ));
    let _ = std::fs::create_dir_all(&video_tmp);

    // Only the first non-black frame is used here, so ask for a single one: each
    // extra frame is another ffmpeg seek + decode over the network.
    let frames = crate::video::extract_keyframes(video_path, &video_tmp, 1);
    if frames.is_empty() {
        let _ = std::fs::remove_dir_all(&video_tmp);
        return None;
    }

    // Pick the first available frame (already filtered for non-black in extract_keyframes)
    let source_frame = &frames[0];
    let img = match image::open(source_frame) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("Failed to open keyframe for thumbnail: {e}");
            let _ = std::fs::remove_dir_all(&video_tmp);
            return None;
        }
    };

    let dhash = compute_dhash(&img);

    let resized = downscale(&img, 300);

    let rgb = resized.to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
    if let Err(e) = rgb.write_with_encoder(encoder) {
        log::warn!("Failed to encode video thumbnail: {e}");
        let _ = std::fs::remove_dir_all(&video_tmp);
        return None;
    }

    if let Err(e) = std::fs::write(&thumb_path, buf.into_inner()) {
        log::warn!(
            "Failed to write video thumbnail {}: {e}",
            thumb_path.display()
        );
        let _ = std::fs::remove_dir_all(&video_tmp);
        return None;
    }

    // Clean up temp keyframes
    let _ = std::fs::remove_dir_all(&video_tmp);

    Some(ThumbnailResult {
        path: thumb_path.display().to_string(),
        dhash: Some(dhash),
    })
}

/// For GIF files, extract the first frame. For other formats, return as-is.
fn first_frame_if_gif(path: &Path) -> PathBuf {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if ext.as_deref() != Some("gif") {
        return path.to_path_buf();
    }
    // For GIF, image::open already decodes the first frame
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_image(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let dynamic = image::DynamicImage::ImageRgb8(img);
        dynamic.save(&path).expect("save test image");
        path
    }

    /// Splitting the decode must not change the output: same dHash, same JPEG.
    #[test]
    fn thumbnail_from_decoded_matches_generate_thumbnail() {
        let src_dir = tempfile::tempdir().expect("srcdir");
        let first = tempfile::tempdir().expect("cache-a");
        let second = tempfile::tempdir().expect("cache-b");
        let source = create_test_image(src_dir.path(), "photo.png", 900, 600);

        let generated = generate_thumbnail(&source, first.path(), "photo.png", true)
            .expect("generate_thumbnail");
        let decoded = decode_source(&source).expect("decode_source");
        let shared = thumbnail_from_decoded(&decoded, second.path(), "photo.png")
            .expect("thumbnail_from_decoded");

        assert_eq!(shared.dhash, generated.dhash);
        assert_eq!(
            std::fs::read(&shared.path).expect("shared bytes"),
            std::fs::read(&generated.path).expect("generated bytes"),
            "same pixels must encode to the same JPEG"
        );
    }

    /// Manual benchmark over real files: `BENCH_PHOTOS=<paths, one per line>`.
    #[test]
    #[ignore = "manual benchmark"]
    fn bench_real_photos() {
        let paths = std::env::var("BENCH_PHOTOS").expect("BENCH_PHOTOS");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let (mut total, mut n) = (0.0_f64, 0_u32);
        for (i, line) in paths.lines().enumerate() {
            let p = Path::new(line.trim());
            if !p.exists() {
                continue;
            }
            let t0 = std::time::Instant::now();
            let ok = generate_thumbnail(p, thumb_dir.path(), &format!("f{i}.jpg"), true).is_some();
            let dt = t0.elapsed().as_secs_f64();
            if ok {
                total += dt;
                n += 1;
            }
        }
        eprintln!("bench: {n} files, avg {:.3} s/file", total / n as f64);
    }

    #[test]
    fn generates_thumbnail_at_expected_path() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "photo.png", 600, 400);
        let result = generate_thumbnail(&source, thumb_dir.path(), "subdir/photo.png", false);
        assert!(result.is_some());
        let tr = result.unwrap();
        let thumb_path = PathBuf::from(&tr.path);
        assert!(thumb_path.exists());
        assert!(thumb_path
            .display()
            .to_string()
            .ends_with("subdir/photo.jpg"));
        assert!(tr.dhash.is_some());

        // Verify the thumbnail is smaller
        let thumb_img = image::open(&thumb_path).expect("open thumb");
        assert!(thumb_img.width() <= 300);
    }

    #[test]
    fn skips_if_thumbnail_exists() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "pic.png", 400, 300);

        let r1 = generate_thumbnail(&source, thumb_dir.path(), "pic.png", false);
        assert!(r1.is_some());
        assert!(r1.as_ref().unwrap().dhash.is_some());

        let thumb_path = PathBuf::from(&r1.unwrap().path);
        let mtime1 = std::fs::metadata(&thumb_path).unwrap().modified().unwrap();

        // Not forced - reuses the file without decoding (dhash will be None)
        std::thread::sleep(std::time::Duration::from_millis(50));
        let r2 = generate_thumbnail(&source, thumb_dir.path(), "pic.png", false);
        assert!(r2.is_some());
        assert!(r2.as_ref().unwrap().dhash.is_none());

        let mtime2 = std::fs::metadata(&thumb_path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2);
    }

    /// SMB servers regularly report timestamps in the future. Cache validation used
    /// to compare the thumbnail mtime with the source mtime, so every scan re-decoded
    /// those files. Only the explicit `force` flag may invalidate now.
    #[test]
    fn future_source_mtime_does_not_invalidate_cache() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "future.png", 400, 300);

        let handle = std::fs::File::options()
            .write(true)
            .open(&source)
            .expect("open source");
        handle
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::SystemTime::now() + std::time::Duration::from_secs(86_400 * 365 * 11),
            ))
            .expect("set mtime");
        drop(handle);

        let r1 = generate_thumbnail(&source, thumb_dir.path(), "future.png", false);
        assert!(r1.is_some());
        assert!(r1.as_ref().unwrap().dhash.is_some());

        let r2 = generate_thumbnail(&source, thumb_dir.path(), "future.png", false);
        assert!(r2.is_some());
        assert!(
            r2.as_ref().unwrap().dhash.is_none(),
            "must reuse the cached thumbnail despite the future source mtime"
        );

        let r3 = generate_thumbnail(&source, thumb_dir.path(), "future.png", true);
        assert!(r3.is_some());
        assert!(
            r3.as_ref().unwrap().dhash.is_some(),
            "force must regenerate"
        );
    }

    #[test]
    fn root_key_is_stable_and_root_specific() {
        let a = root_key("/run/user/1000/gvfs/smb-share:server=nas,share=fotos/Fotos");
        assert_eq!(
            a,
            root_key("/run/user/1000/gvfs/smb-share:server=nas,share=fotos/Fotos")
        );
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            a,
            root_key("/run/user/1000/gvfs/smb-share:server=nas,share=fotos")
        );
    }

    #[test]
    fn thumb_path_for_namespaces_by_root() {
        let base = Path::new("/cache/thumbnails");
        let a = thumb_path_for(base, "aaa", "2012/foto.jpg");
        let b = thumb_path_for(base, "bbb", "2012/foto.jpg");
        assert_eq!(a, base.join("aaa").join("2012/foto.jpg"));
        assert_ne!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn reuse_thumbnail_hardlinks_shared_file() {
        use std::os::unix::fs::MetadataExt;

        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "orig.png", 400, 300);
        let first = generate_thumbnail(&source, thumb_dir.path(), "a/orig.png", false)
            .expect("first thumbnail");
        let dhash = first.dhash;

        let reused = reuse_thumbnail(
            Path::new(&first.path),
            dhash,
            thumb_dir.path(),
            "b/copy.png",
        )
        .expect("reused thumbnail");
        assert_eq!(reused.dhash, dhash);
        assert!(Path::new(&reused.path).exists());
        let shared_ino = std::fs::metadata(&first.path).expect("meta shared").ino();
        let reused_ino = std::fs::metadata(&reused.path).expect("meta reused").ino();
        assert_eq!(shared_ino, reused_ino, "must share the inode");
    }

    #[test]
    fn handles_missing_source_gracefully() {
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let missing = std::env::temp_dir().join("nonexistent_image_12345.png");
        let result = generate_thumbnail(&missing, thumb_dir.path(), "missing.png", false);
        assert!(result.is_none());
    }

    #[test]
    fn tall_image_constrained_by_height() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "tall.png", 200, 800);
        let result = generate_thumbnail(&source, thumb_dir.path(), "tall.png", false);
        assert!(result.is_some());
        let thumb_img = image::open(result.unwrap().path).expect("open");
        assert!(thumb_img.height() <= 300);
        assert!(thumb_img.width() < 200);
    }

    #[test]
    fn small_image_not_upscaled() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let thumb_dir = tempfile::tempdir().expect("tempdir");
        let source = create_test_image(src_dir.path(), "small.png", 100, 80);
        let result = generate_thumbnail(&source, thumb_dir.path(), "small.png", false);
        assert!(result.is_some());
        let thumb_img = image::open(result.unwrap().path).expect("open");
        assert_eq!(thumb_img.width(), 100);
    }

    #[test]
    fn dhash_stable_for_same_image() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let path = create_test_image(src_dir.path(), "stable.png", 200, 200);
        let img = image::open(&path).expect("open");
        let h1 = compute_dhash(&img);
        let h2 = compute_dhash(&img);
        assert_eq!(h1, h2);
    }

    #[test]
    fn dhash_different_for_different_images() {
        let src_dir = tempfile::tempdir().expect("tempdir");
        let path_a = create_test_image(src_dir.path(), "a.png", 200, 200);
        // Create a different image
        let path_b = src_dir.path().join("b.png");
        let img_b = image::RgbImage::from_fn(200, 200, |x, y| {
            image::Rgb([255 - (x % 256) as u8, (y % 256) as u8, 0])
        });
        image::DynamicImage::ImageRgb8(img_b).save(&path_b).unwrap();

        let img_a = image::open(&path_a).expect("open a");
        let img_b = image::open(&path_b).expect("open b");
        let ha = compute_dhash(&img_a);
        let hb = compute_dhash(&img_b);
        // Should be different (not guaranteed to be maximally different,
        // but with these patterns they should differ)
        assert_ne!(ha, hb);
    }
}
