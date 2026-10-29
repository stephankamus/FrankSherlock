//! `?w=` derivatives: downscaled JPEG previews cached on disk.
//!
//! A phone does not need a 12 MP JPEG to fill a 400 px preview box, and the library
//! share should not be read for the full file on every scroll. The derivative is
//! built once per `<revision, id, width>` and served from `CACHE_DIR` afterwards.
//!
//! The cache is keyed by the published revision: publishing a new snapshot brings new
//! rows (or repaired thumbnails), and an old derivative must not survive that. Nothing
//! here is authoritative — the cache is disposable (a tmpfs in the container), so a
//! miss simply rebuilds it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;

/// Ceiling for `?w=`, mirroring `MAX_PREVIEW_WIDTH` in `sherlock/shared`.
pub const MAX_WIDTH: u32 = 4096;

const JPEG_QUALITY: u8 = 82;

/// Parse the query parameter.
///
/// A width that is absent, zero, negative or not a number means "serve the original":
/// the parameter is an optimisation, not a requirement, so a typo degrades instead of
/// failing the request. Anything above `MAX_WIDTH` is clamped.
pub fn requested_width(raw: Option<&str>) -> Option<u32> {
    let parsed: u32 = raw?.trim().parse().ok()?;
    if parsed == 0 {
        return None;
    }
    Some(parsed.min(MAX_WIDTH))
}

/// Whether this row is worth decoding.
///
/// The list is the set the `image` crate can actually decode without an external
/// helper: HEIC/NEF originals are published as JPEG copies by the desktop, and a video
/// or a PDF has no meaning here.
pub fn is_resizable(rel_path: &str) -> bool {
    let extension = rel_path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff"
    )
}

/// `<CACHE_DIR>/<revision>/<id>_<width>.jpg`.
pub fn cache_path(cache_dir: &Path, revision: &str, id: i64, width: u32) -> PathBuf {
    cache_dir.join(revision).join(format!("{id}_{width}.jpg"))
}

/// Decode, downscale and atomically publish a JPEG at `target`.
///
/// The caller runs this inside `spawn_blocking`: decoding a 12 MP photo is CPU work
/// that must not park a request thread.
pub fn render(source: &Path, target: &Path, width: u32) -> Result<(), String> {
    let image = image::ImageReader::open(source)
        .map_err(|error| format!("cannot open {}: {error}", source.display()))?
        .with_guessed_format()
        .map_err(|error| format!("cannot detect the format of {}: {error}", source.display()))?
        .decode()
        .map_err(|error| format!("cannot decode {}: {error}", source.display()))?;

    let (source_width, source_height) = (image.width(), image.height());
    let resized = if source_width > width {
        let height = ((source_height as f64) * (width as f64 / source_width as f64))
            .round()
            .max(1.0) as u32;
        image.resize(width, height, FilterType::Lanczos3)
    } else {
        // Small enough already: re-encoding still normalises EXIF orientation and
        // gives the browser a predictable JPEG instead of a HEIC-ish container.
        image
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }

    // A sibling temp file + rename: two phones asking for the same width at the same
    // time must never read a half-written JPEG, and a crashed render must not leave a
    // truncated file under the real name.
    let temp = target.with_file_name(format!(
        ".{}.{}-{}.tmp",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("preview"),
        std::process::id(),
        unique_suffix()
    ));

    match write_jpeg(&resized.to_rgb8(), &temp) {
        Ok(()) => std::fs::rename(&temp, target)
            .map_err(|error| format!("cannot publish {}: {error}", target.display())),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn write_jpeg(image: &image::RgbImage, target: &Path) -> Result<(), String> {
    let file = std::fs::File::create(target)
        .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY)
        .encode_image(image)
        .map_err(|error| format!("cannot encode {}: {error}", target.display()))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_is_optional_and_clamped() {
        assert_eq!(requested_width(None), None);
        assert_eq!(requested_width(Some("")), None);
        assert_eq!(requested_width(Some("abc")), None);
        assert_eq!(requested_width(Some("0")), None);
        assert_eq!(requested_width(Some(" 800 ")), Some(800));
        assert_eq!(requested_width(Some("99999")), Some(MAX_WIDTH));
    }

    #[test]
    fn only_decodable_images_are_resized() {
        assert!(is_resizable("a/b.JPG"));
        assert!(is_resizable("a/b.png"));
        assert!(!is_resizable("a/b.mp4"));
        assert!(!is_resizable("a/b.pdf"));
        assert!(!is_resizable("a/b.heic"));
        assert!(!is_resizable("a/no-extension"));
    }

    #[test]
    fn the_cache_path_is_namespaced_by_revision() {
        let path = cache_path(Path::new("/cache"), "rev-001", 42, 1600);
        assert_eq!(path, Path::new("/cache/rev-001/42_1600.jpg"));
    }

    #[test]
    fn render_downscales_preserving_the_aspect_ratio() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source.png");
        image::RgbImage::from_pixel(200, 100, image::Rgb([20, 30, 40]))
            .save_with_format(&source, image::ImageFormat::Png)
            .expect("source png");

        let target = temp.path().join("out").join("42_80.jpg");
        render(&source, &target, 80).expect("render");

        let preview = image::ImageReader::open(&target)
            .expect("open")
            .decode()
            .expect("decode");
        assert_eq!((preview.width(), preview.height()), (80, 40));
    }

    #[test]
    fn render_keeps_a_small_image_untouched_in_size() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("small.png");
        image::RgbImage::from_pixel(60, 40, image::Rgb([10, 20, 30]))
            .save_with_format(&source, image::ImageFormat::Png)
            .expect("source png");

        let target = temp.path().join("60_1600.jpg");
        render(&source, &target, 1600).expect("render");

        let preview = image::ImageReader::open(&target)
            .expect("open")
            .decode()
            .expect("decode");
        assert_eq!((preview.width(), preview.height()), (60, 40));
    }

    #[test]
    fn render_does_not_leave_a_file_behind_on_failure() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("not-an-image.jpg");
        std::fs::write(&source, b"plain text").expect("write");
        let target = temp.path().join("out.jpg");

        assert!(render(&source, &target, 100).is_err());
        assert!(!target.exists());
    }
}
