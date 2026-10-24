//! Pre-scan time projection for the vision classification phase.
//!
//! Instead of shipping a hardcoded throughput table (which would be wrong on
//! every machine but the one it was measured on), the estimate is calibrated by
//! running the *real* first-pass request on a few images from the user's own
//! library, with the exact model, resolution, prompt profile and context the
//! scan will use.
//!
//! Images are sampled in terciles by file size — a mixed library (screenshots,
//! phone photos, RAW) has a ~5× spread in per-image cost, so averaging a random
//! handful of files is not enough.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::PromptProfile;
use crate::error::AppResult;
use crate::models::VisionSettings;
use crate::vision_prep;

/// Up to this many images are actually classified during calibration.
pub const MAX_PROBES: usize = 3;

/// Conservative allowance for `ffprobe` + keyframe extraction per video. The
/// per-video estimate is one vision request (same as a photo) plus this.
pub const VIDEO_OVERHEAD_S: f64 = 3.0;

/// Files below this size are skipped by the scanner, so they must not be counted.
const MIN_IMAGE_BYTES: u64 = 10 * 1024;

const IMAGE_EXTS: [&str; 8] = [
    ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tiff", ".tif",
];
const VIDEO_EXTS: [&str; 11] = [
    ".mp4", ".mkv", ".avi", ".mov", ".wmv", ".flv", ".webm", ".m4v", ".ts", ".mpg", ".mpeg",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCounts {
    pub images: u64,
    pub videos: u64,
    pub pdfs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    /// Wall time not spent on prefill/decode: HTTP upload plus Ollama's image
    /// preprocessing, which is CPU-bound and the reason resolution matters.
    pub gap_s: f64,
    pub prefill_s: f64,
    pub decode_s: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionEstimate {
    pub counts: FileCounts,
    pub per_image_s: f64,
    pub total_s: f64,
    /// `calibrated`, `cached` or `rough`.
    pub source: String,
    pub samples: u32,
    pub breakdown: Breakdown,
    /// Model bytes that did not fit in VRAM during calibration (0 = fully on GPU).
    pub spill_bytes: u64,
    /// True when the probes were skipped because the model is not installed.
    pub model_missing: bool,
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .map(|e| {
            let ext = format!(".{}", e.to_string_lossy().to_ascii_lowercase());
            exts.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

/// Count countable files under `root` and collect image paths (with their size)
/// for calibration sampling. Unreadable entries are ignored, mirroring the scan.
pub fn collect_files(root: &Path) -> (FileCounts, Vec<(u64, PathBuf)>) {
    let mut counts = FileCounts::default();
    let mut images: Vec<(u64, PathBuf)> = Vec::new();

    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let size = meta.len();

        if has_ext(path, &IMAGE_EXTS) {
            if size < MIN_IMAGE_BYTES {
                continue;
            }
            counts.images += 1;
            images.push((size, path.to_path_buf()));
        } else if has_ext(path, &VIDEO_EXTS) {
            counts.videos += 1;
        } else if path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            counts.pdfs += 1;
        }
    }

    (counts, images)
}

/// Bucket rows coming from the index (`filename`, `abs_path`, `size_bytes`)
/// into counts plus the image sample pool.
fn bucket_rows(rows: Vec<(String, String, i64)>) -> (FileCounts, Vec<(u64, PathBuf)>) {
    let mut counts = FileCounts::default();
    let mut images: Vec<(u64, PathBuf)> = Vec::new();

    for (filename, abs_path, size) in rows {
        let name = Path::new(&filename);
        let size = size.max(0) as u64;
        if has_ext(name, &IMAGE_EXTS) {
            if size < MIN_IMAGE_BYTES {
                continue;
            }
            counts.images += 1;
            images.push((size, PathBuf::from(abs_path)));
        } else if has_ext(name, &VIDEO_EXTS) {
            counts.videos += 1;
        } else if filename.to_ascii_lowercase().ends_with(".pdf") {
            counts.pdfs += 1;
        }
    }

    (counts, images)
}

/// Counts and samples from the SQLite index, when the root is indexed.
fn collect_from_index(db_path: &Path, root: &Path) -> Option<(FileCounts, Vec<(u64, PathBuf)>)> {
    let rows = crate::db::vision_estimate_rows(db_path, &root.display().to_string())
        .ok()
        .flatten()?;
    Some(bucket_rows(rows))
}

/// Pick up to `wanted` evenly spaced indices over a size-sorted list.
///
/// Pure so it can be tested without touching the filesystem.
pub fn stratified_indices(len: usize, wanted: usize) -> Vec<usize> {
    if len == 0 || wanted == 0 {
        return Vec::new();
    }
    let wanted = wanted.min(len);
    let mut out = Vec::with_capacity(wanted);
    for i in 0..wanted {
        // Spread across (0, len-1) so we sample the small, median and large terciles.
        let pos = ((i as f64 + 0.5) / wanted as f64 * (len as f64 - 1.0)).round() as usize;
        if !out.contains(&pos) {
            out.push(pos);
        }
    }
    out
}

/// Choose sample paths spanning the size distribution.
pub fn sample_paths(mut images: Vec<(u64, PathBuf)>, wanted: usize) -> Vec<PathBuf> {
    images.sort_by_key(|(size, _)| *size);
    stratified_indices(images.len(), wanted)
        .into_iter()
        .map(|idx| images[idx].1.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Math
// ---------------------------------------------------------------------------

pub fn per_video_s(per_image_s: f64) -> f64 {
    per_image_s + VIDEO_OVERHEAD_S
}

/// Total projection from counts and a per-image cost. Pure.
pub fn total_seconds(counts: FileCounts, per_image_s: f64) -> f64 {
    let images = counts.images as f64 * per_image_s;
    let videos = counts.videos as f64 * per_video_s(per_image_s);
    let pdfs = counts.pdfs as f64 * per_image_s;
    images + videos + pdfs
}

/// Wall time attributable to a single image: total minus one-off model load.
pub fn active_wall_s(wall_s: f64, load_s: f64) -> f64 {
    (wall_s - load_s).max(0.0)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Rough fallback when calibration is not possible (model missing, user in a
/// hurry). Intentionally pessimistic and clearly labelled in the UI.
pub fn rough_per_image_s(profile: PromptProfile) -> f64 {
    match profile {
        PromptProfile::Concise => 6.0,
        PromptProfile::Detailed => 12.0,
    }
}

// ---------------------------------------------------------------------------
// Calibration cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedCalibration {
    per_image_s: f64,
    breakdown: Breakdown,
    samples: u32,
}

fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("frank_sherlock").join("vision_calibration.json"))
}

fn cache_key(root: &Path, model: &str, settings: &VisionSettings, gpu_signature: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        root.display(),
        model,
        settings.image_max_side,
        settings.prompt_profile.as_str(),
        gpu_signature
    )
}

fn load_cache() -> BTreeMap<String, CachedCalibration> {
    let Some(path) = cache_path() else {
        return BTreeMap::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn store_cache(key: &str, entry: CachedCalibration) {
    let Some(path) = cache_path() else { return };
    let mut cache = load_cache();
    // Bounded so the file cannot grow without limit across many roots.
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(key.to_string(), entry);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(&cache) {
        let _ = std::fs::write(path, data);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Calibrate (or read from cache) and project the classification phase.
pub fn estimate(
    root: &Path,
    model: &str,
    settings: VisionSettings,
    tmp_dir: &Path,
    gpu_signature: &str,
    db_path: Option<&Path>,
) -> AppResult<VisionEstimate> {
    // Prefer the index: walking an SMB root with 50k files takes minutes.
    let (counts, images) = match db_path.and_then(|db| collect_from_index(db, root)) {
        Some(found) => found,
        None => collect_files(root),
    };
    let key = cache_key(root, model, &settings, gpu_signature);

    if let Some(cached) = load_cache().get(&key) {
        return Ok(VisionEstimate {
            counts,
            per_image_s: cached.per_image_s,
            total_s: total_seconds(counts, cached.per_image_s),
            source: "cached".to_string(),
            samples: cached.samples,
            breakdown: cached.breakdown.clone(),
            spill_bytes: 0,
            model_missing: false,
        });
    }

    let samples = sample_paths(images, MAX_PROBES);
    let mut walls = Vec::new();
    let mut gaps = Vec::new();
    let mut prefills = Vec::new();
    let mut decodes = Vec::new();
    let mut probed = 0u32;
    let mut any_ok = false;

    for sample in &samples {
        let prepared = match vision_prep::prepare(sample, settings.image_max_side, tmp_dir) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("estimate: cannot prepare {}: {e}", sample.display());
                continue;
            }
        };
        let probe = crate::classify::probe_primary(model, &prepared, settings.prompt_profile);
        if !probe.ok {
            continue;
        }
        any_ok = true;
        probed += 1;
        // The first probe pays for loading the model (~50 s cold). That happens
        // once per scan, not once per image, so it must not enter the per-image
        // cost — otherwise a 53k-image projection would be off by orders of
        // magnitude.
        let per_image_wall = active_wall_s(probe.wall_s, probe.timings.load_s);
        walls.push(per_image_wall);
        prefills.push(probe.timings.prefill_s);
        decodes.push(probe.timings.decode_s);
        gaps.push((per_image_wall - probe.timings.prefill_s - probe.timings.decode_s).max(0.0));
    }

    if !any_ok {
        // Model missing / Ollama down: fall back to a labelled rough number.
        let per_image_s = rough_per_image_s(settings.prompt_profile);
        return Ok(VisionEstimate {
            counts,
            per_image_s,
            total_s: total_seconds(counts, per_image_s),
            source: "rough".to_string(),
            samples: 0,
            breakdown: Breakdown::default(),
            spill_bytes: 0,
            model_missing: true,
        });
    }

    let breakdown = Breakdown {
        gap_s: mean(&gaps),
        prefill_s: mean(&prefills),
        decode_s: mean(&decodes),
    };
    let per_image_s = mean(&walls);

    store_cache(
        &key,
        CachedCalibration {
            per_image_s,
            breakdown: breakdown.clone(),
            samples: probed,
        },
    );

    // After probing, Ollama reports the real footprint: this is the honest
    // answer to "does this model fit in my VRAM?", no formula required.
    let spill_bytes = crate::llm::loaded_model_memory(model)
        .map(|(total, vram)| total.saturating_sub(vram))
        .unwrap_or(0);

    Ok(VisionEstimate {
        counts,
        per_image_s,
        total_s: total_seconds(counts, per_image_s),
        source: "calibrated".to_string(),
        samples: probed,
        breakdown,
        spill_bytes,
        model_missing: false,
    })
}

// ---------------------------------------------------------------------------
// Model list for the picker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionModelOption {
    pub name: String,
    pub size_bytes: u64,
    pub recommended: bool,
    pub active: bool,
    /// `true` / `false` when it can be decided, `None` when the model is not
    /// loaded and only a rough weights-vs-memory comparison is possible.
    pub fits_vram: Option<bool>,
    /// Measured footprint when the model is currently loaded.
    pub loaded_bytes: Option<u64>,
    pub loaded_vram_bytes: Option<u64>,
    pub note: String,
}

/// Bytes of KV cache to reserve when guessing whether a model fits. Only used
/// for the pre-estimate hint; the calibration reports the real number.
const KV_ALLOWANCE_BYTES: u64 = 1024 * 1024 * 1024;

pub fn list_models(
    gpu: &crate::platform::gpu::GpuInfo,
    recommended_tag: &str,
    active_model: &str,
) -> Vec<VisionModelOption> {
    let Some(models) = crate::llm::list_installed_models_with_sizes() else {
        return Vec::new();
    };

    let effective = if gpu.unified_memory {
        (gpu.system_ram_mib as f64 * 0.75) as u64 * 1024 * 1024
    } else if let Some(vram) = gpu.vram_total_mib {
        vram * 1024 * 1024
    } else {
        gpu.system_ram_mib * 1024 * 1024
    };

    let mut out: Vec<VisionModelOption> = models
        .into_iter()
        .map(|(name, size_bytes)| {
            let loaded = crate::llm::loaded_model_memory(&name);
            let (fits_vram, note) = match loaded {
                Some((total, vram)) if total > vram => {
                    let spill_gb = (total - vram) as f64 / 1e9;
                    (
                        Some(false),
                        format!("{spill_gb:.1} GB em RAM — cerca de 2x mais lento"),
                    )
                }
                Some(_) => (Some(true), "cabe inteiro na GPU".to_string()),
                None => {
                    let needed = size_bytes + KV_ALLOWANCE_BYTES;
                    if needed <= effective {
                        (None, "estimativa: deve caber na GPU".to_string())
                    } else {
                        (
                            None,
                            "estimativa: pode transbordar para a RAM (mais lento)".to_string(),
                        )
                    }
                }
            };
            VisionModelOption {
                recommended: name.contains(recommended_tag),
                active: name == active_model,
                name,
                size_bytes,
                fits_vram,
                loaded_bytes: loaded.map(|(total, _)| total),
                loaded_vram_bytes: loaded.map(|(_, vram)| vram),
                note,
            }
        })
        .collect();

    out.sort_by_key(|option| option.size_bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stratified_indices_spans_the_range() {
        let idx = stratified_indices(100, 3);
        assert_eq!(idx.len(), 3);
        // First / median / last tercile of a 0..99 range.
        assert_eq!(idx[0], 17);
        assert_eq!(idx[1], 50);
        assert_eq!(idx[2], 83);
    }

    #[test]
    fn stratified_indices_handles_small_inputs() {
        assert!(stratified_indices(0, 3).is_empty());
        assert_eq!(stratified_indices(1, 3), vec![0]);
        assert_eq!(stratified_indices(2, 3), vec![0, 1]);
    }

    #[test]
    fn sample_paths_picks_small_median_large() {
        let images: Vec<(u64, PathBuf)> = (1..=9)
            .map(|i| (i * 1000, PathBuf::from(format!("img{i}.jpg"))))
            .collect();
        let picked = sample_paths(images, 3);
        assert_eq!(
            picked,
            vec![
                PathBuf::from("img2.jpg"),
                PathBuf::from("img5.jpg"),
                PathBuf::from("img8.jpg")
            ]
        );
    }

    #[test]
    fn bucket_rows_counts_and_skips_tiny_files() {
        let rows = vec![
            ("a.jpg".to_string(), "/r/a.jpg".to_string(), 200_000),
            ("b.JPEG".to_string(), "/r/b.jpeg".to_string(), 300_000),
            ("tiny.png".to_string(), "/r/tiny.png".to_string(), 512),
            ("c.pdf".to_string(), "/r/c.pdf".to_string(), 1_000_000),
            ("d.MP4".to_string(), "/r/d.mp4".to_string(), 9_000_000),
            ("notes.txt".to_string(), "/r/notes.txt".to_string(), 10),
        ];
        let (counts, images) = bucket_rows(rows);
        assert_eq!(counts.images, 2);
        assert_eq!(counts.videos, 1);
        assert_eq!(counts.pdfs, 1);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].1, PathBuf::from("/r/a.jpg"));
    }

    #[test]
    fn totals_add_video_overhead() {
        let counts = FileCounts {
            images: 100,
            videos: 2,
            pdfs: 0,
        };
        let total = total_seconds(counts, 2.0);
        // 100 images * 2.0 + 2 videos * (2.0 + 3.0)
        assert!((total - 210.0).abs() < 1e-9, "got {total}");
    }

    #[test]
    fn totals_with_no_files_is_zero() {
        assert_eq!(total_seconds(FileCounts::default(), 3.5), 0.0);
    }

    #[test]
    fn active_wall_excludes_model_load() {
        assert!((active_wall_s(52.0, 48.5) - 3.5).abs() < 1e-9);
        assert_eq!(active_wall_s(3.0, 48.5), 0.0, "negative must clamp to zero");
        assert_eq!(active_wall_s(3.0, 0.0), 3.0);
    }

    #[test]
    fn rough_estimates_get_slower_with_detail() {
        assert!(
            rough_per_image_s(PromptProfile::Concise) < rough_per_image_s(PromptProfile::Detailed)
        );
    }

    #[test]
    fn cache_key_changes_with_settings() {
        let root = Path::new("/photos");
        let concise = VisionSettings::default();
        let detailed = VisionSettings {
            prompt_profile: PromptProfile::Detailed,
            ..concise
        };
        let k1 = cache_key(root, "qwen2.5vl:3b", &concise, "gpu-12gb");
        let k2 = cache_key(root, "qwen2.5vl:3b", &detailed, "gpu-12gb");
        let k3 = cache_key(root, "qwen2.5vl:7b", &concise, "gpu-12gb");
        let k4 = cache_key(root, "qwen2.5vl:3b", &concise, "gpu-8gb");
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
    }

    #[test]
    fn collect_files_counts_by_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("a.jpg"), vec![0u8; 20 * 1024]).expect("write");
        std::fs::write(root.join("b.JPG"), vec![0u8; 20 * 1024]).expect("write");
        std::fs::write(root.join("tiny.jpg"), vec![0u8; 512]).expect("write");
        std::fs::write(root.join("c.pdf"), b"%PDF-1.4").expect("write");
        std::fs::write(root.join("d.mp4"), b"xxxx").expect("write");
        std::fs::write(root.join("e.txt"), b"hello").expect("write");

        let (counts, images) = collect_files(root);
        assert_eq!(
            counts.images, 2,
            "tiny.jpg must be skipped like the scanner does"
        );
        assert_eq!(counts.pdfs, 1);
        assert_eq!(counts.videos, 1);
        assert_eq!(images.len(), 2);
    }
}
