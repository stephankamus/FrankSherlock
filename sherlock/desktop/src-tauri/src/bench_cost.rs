//! Phase 0 of `plans/pipeline-cpu-gpu-overlap.md`.
//!
//! Measures, per file, the cost of the two stages that currently decode the same
//! image twice — thumbnail generation and vision preparation — so the decision
//! to build a CPU/GPU pipeline rests on the user's own library instead of an
//! assumption.
//!
//! How the duplicate is priced: the file is read **once, first**, into memory
//! (`read_ms`, the real I/O cost) and decoded from those bytes (`decode_ms`, a
//! pure decompress with no I/O). One read + one decode is exactly what the
//! pipeline removes, because the thumbnail and `vision_prep::prepare` currently
//! each do their own. The production calls are then run in scan order
//! (`thumb` before `prepare`) and their timings include their own cached reads,
//! which is why the derived numbers subtract `read_ms + decode_ms` rather than
//! reusing `open_ms`.
//!
//! Manual run, sampling the real root in terciles by file size (a mixed library
//! has a ~5× spread in per-image cost, so a random handful is not enough):
//!
//! ```sh
//! BENCH_ROOT=/mnt/nas/fotos BENCH_LIMIT=200 BENCH_MODEL=qwen2.5vl:3b \
//!   cargo test --release bench_cost_split -- --ignored --nocapture
//! ```
//!
//! `BENCH_PHOTOS` (a file with one path per line) takes precedence over
//! `BENCH_ROOT`. Optional `BENCH_MAX_SIDE` overrides the configured resolution.
//! Without `BENCH_MODEL` the Ollama columns stay empty and only CPU/IO is
//! measured.
//!
//! `BENCH_QUALITY=1` (requires `BENCH_MODEL`) additionally classifies **both**
//! prepared images of each file and prints the two responses side by side, so a
//! faster kernel can be checked against the one it replaces. Use a small
//! `BENCH_LIMIT` for that run: it doubles the requests.
//!
//! `BENCH_ROOT` accepts a plain directory or an `smb://host/share/...` URI. The
//! URI is *not* a filesystem path: it is mapped onto the GVFS mount the app
//! itself indexes (`$XDG_RUNTIME_DIR/gvfs/smb-share:server=host,share=share`),
//! so a path copied from the file manager can be pasted as-is.
//!
//! Caveat: the second and third reads of the same file hit the page cache on a
//! local disk, so only `read_ms` reflects the first touch. Run this on the real
//! network share, where every read costs the same.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::{ImageFilter, PromptProfile};
use crate::models::VisionSettings;
use crate::{classify, thumbnail, vision_estimate, vision_prep};

#[derive(Default)]
struct Sample {
    bytes: u64,
    /// First-touch read of the whole file (`std::fs::read`), the I/O unit.
    read_ms: f64,
    /// Pure decode from those bytes (`image::load_from_memory`), no I/O.
    decode_ms: f64,
    /// `thumbnail::compute_dhash` on the decoded image (informational; the
    /// thumbnail timing below already includes it).
    dhash_ms: f64,
    /// `generate_thumbnail`: read + decode + dHash + resize 300 + JPEG q80.
    thumb_ms: f64,
    /// `prepare_with(.., Box)`: read + decode + orient + resize + JPEG q88.
    prepare_box_ms: f64,
    /// `prepare_with(.., Lanczos3)`: the kernel `prepare` used before Phase 0.5.
    prepare_lanczos_ms: f64,
    /// `image::open` — the read+decode entry point production uses. Over SMB it
    /// may cost far more than `read + load_from_memory` because it reads through
    /// a `BufReader`; this column decides how the shared decode should read.
    open_ms: f64,
    /// Ollama request wall time (`None` without `BENCH_MODEL` or on failure).
    request_ms: Option<f64>,
    prefill_ms: Option<f64>,
    decode_gpu_ms: Option<f64>,
}

impl Sample {
    /// Wall time not spent on the GPU: HTTP upload + Ollama's own image decode
    /// and tokenization, which run on the CPU.
    fn ollama_cpu_ms(&self) -> Option<f64> {
        let request = self.request_ms?;
        let prefill = self.prefill_ms?;
        let decode = self.decode_gpu_ms?;
        Some((request - prefill - decode).max(0.0))
    }

    /// One read + one decode — what a shared decode removes per file.
    fn duplicated_ms(&self) -> f64 {
        self.read_ms + self.decode_ms
    }
}

fn mean(samples: &[Sample], get: impl Fn(&Sample) -> Option<f64>) -> Option<f64> {
    let values: Vec<f64> = samples.iter().filter_map(get).collect();
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn fmt_ms(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.1}")).unwrap_or_default()
}

/// One-line, length-capped view of a model response, for `BENCH_QUALITY` runs.
fn summarize_raw(raw: &str) -> String {
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 240 {
        compact
    } else {
        compact.chars().take(240).collect::<String>() + "…"
    }
}

/// Pick `limit` images spread over three size terciles.
///
/// A real library mixes screenshots, phone photos and RAW; averaging a random
/// handful hides the ~5× spread in per-image cost. `vision_estimate` samples the
/// same way for the same reason.
fn sample_by_tercile(mut images: Vec<(u64, PathBuf)>, limit: usize) -> Vec<PathBuf> {
    if images.len() <= limit {
        return images.into_iter().map(|(_, path)| path).collect();
    }
    images.sort_by_key(|(size, _)| *size);

    const BUCKETS: usize = 3;
    let per_bucket = limit.div_ceil(BUCKETS);
    let mut out = Vec::with_capacity(limit);
    for bucket in 0..BUCKETS {
        let start = bucket * images.len() / BUCKETS;
        let end = ((bucket + 1) * images.len() / BUCKETS).max(start);
        let slice_len = end - start;
        let take = per_bucket.min(slice_len).min(limit - out.len());
        if take == 0 {
            continue;
        }
        let step = slice_len as f64 / take as f64;
        for i in 0..take {
            let index = start + (i as f64 * step) as usize;
            out.push(images[index].1.clone());
        }
    }
    out
}

fn resolve_paths(limit: usize) -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("BENCH_PHOTOS") {
        let mut paths: Vec<PathBuf> = std::fs::read_to_string(&list)
            .expect("read BENCH_PHOTOS")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(PathBuf::from)
            .collect();
        paths.truncate(limit);
        return paths;
    }

    let raw = std::env::var("BENCH_ROOT")
        .expect("BENCH_PHOTOS (list of paths) or BENCH_ROOT (directory to sample) is required");
    let root = resolve_root(&raw);
    let (_, images) = vision_estimate::collect_files(&root);
    if images.is_empty() {
        panic!(
            "no supported image >= 10 KB under {} (the scan counts jpg/jpeg/png/gif/bmp/webp/tiff/tif; \
             videos and PDFs are not sampled by this bench)",
            root.display()
        );
    }
    eprintln!(
        "bench: {} candidate image(s) under {}",
        images.len(),
        root.display()
    );
    sample_by_tercile(images, limit)
}

/// Accept a real directory, or map an `smb://host/share/rest` URI onto the GVFS
/// mount the application indexes. The URI itself is not a filesystem path, which
/// is the mistake this guards against: `Path::new("smb://…").is_dir()` is simply
/// false and would otherwise surface as an empty scan.
fn resolve_root(raw: &str) -> PathBuf {
    let direct = PathBuf::from(raw);
    if direct.is_dir() {
        return direct;
    }

    if let Some(mapped) = gvfs_path_for_smb_uri(raw) {
        if mapped.is_dir() {
            return mapped;
        }
    }

    if raw.contains("://") {
        panic!(
            "{} is a URI, not a filesystem path. The app indexes the mounted location \
             ($XDG_RUNTIME_DIR/gvfs/...), so pass that path or a share that is currently \
             mounted. Visible network mounts: {}",
            raw,
            gvfs_mounts().join(", ")
        );
    }

    let mounts = gvfs_mounts();
    panic!(
        "BENCH_ROOT is not a readable directory: {}\nvisible network mounts: {}",
        direct.display(),
        if mounts.is_empty() {
            "<none>".to_string()
        } else {
            mounts.join(", ")
        }
    );
}

fn gvfs_dir() -> Option<PathBuf> {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(|dir| Path::new(&dir).join("gvfs"))
        .filter(|dir| dir.is_dir())
}

fn gvfs_mounts() -> Vec<String> {
    let Some(dir) = gvfs_dir() else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `smb://host/share/sub/dir` →
/// `$XDG_RUNTIME_DIR/gvfs/smb-share:server=host,share=share/sub/dir`.
fn gvfs_path_for_smb_uri(raw: &str) -> Option<PathBuf> {
    let rest = raw.strip_prefix("smb://")?;
    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let (share, sub) = tail.split_once('/').unwrap_or((tail, ""));
    if authority.is_empty() || share.is_empty() {
        return None;
    }

    let gvfs = gvfs_dir()?;
    let mut mount = gvfs.join(format!("smb-share:server={authority},share={share}"));
    if !mount.is_dir() {
        // Some servers publish the share name percent-encoded.
        mount = gvfs.join(format!(
            "smb-share:server={authority},share={}",
            percent_decode(share)
        ));
    }
    if !mount.is_dir() {
        return None;
    }

    let mut path = mount;
    for segment in percent_decode(sub).split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    Some(path)
}

/// Minimal `%XX` decoder: URIs copied from the file manager encode spaces
/// (`%20`) and accented names.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let is_escape = bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit();
        if is_escape {
            let high = (bytes[index + 1] as char).to_digit(16).unwrap_or(0) as u8;
            let low = (bytes[index + 2] as char).to_digit(16).unwrap_or(0) as u8;
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
#[ignore = "manual measurement; needs BENCH_PHOTOS or BENCH_ROOT"]
fn bench_cost_split() {
    let limit: usize = std::env::var("BENCH_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let model = std::env::var("BENCH_MODEL")
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let quality = std::env::var("BENCH_QUALITY")
        .map(|v| v.trim() == "1")
        .unwrap_or(false);

    let mut settings = VisionSettings::default();
    if let Some(side) = std::env::var("BENCH_MAX_SIDE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        settings.image_max_side = side;
    }
    if let Some(profile) = std::env::var("BENCH_PROFILE")
        .ok()
        .and_then(|v| PromptProfile::parse(&v))
    {
        settings.prompt_profile = profile;
    }

    let paths = resolve_paths(limit);
    assert!(!paths.is_empty(), "no files to measure");

    // Everything is written under one temp root so the source files are never
    // touched and the measurement cannot be skewed by an existing cache.
    let tmp = tempfile::tempdir().expect("tempdir");
    let thumb_dir = tmp.path().join("thumbs");
    let prep_dir = tmp.path().join("prep");

    eprintln!(
        "bench: {} file(s), model={}, image_max_side={}",
        paths.len(),
        model.as_deref().unwrap_or("<none: CPU/IO only>"),
        settings.image_max_side
    );

    println!(
        "file,bytes,read_ms,decode_ms,dhash_ms,thumb_ms,prepare_box_ms,prepare_lanczos_ms,open_ms,request_ms,prefill_ms,decode_gpu_ms,ollama_cpu_ms"
    );

    let mut samples: Vec<Sample> = Vec::new();
    let mut failed_thumb = 0u32;
    let mut failed_prepare = 0u32;
    let mut failed_request = 0u32;
    let mut probed = 0u32;

    for (index, path) in paths.iter().enumerate() {
        let rel = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("f{index}.jpg"));

        // First touch: the only read whose cost a local page cache cannot hide.
        let started = Instant::now();
        let raw = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("bench: cannot read {}: {e}", path.display());
                continue;
            }
        };
        let read_ms = started.elapsed().as_secs_f64() * 1000.0;

        let started = Instant::now();
        let decoded = match image::load_from_memory(&raw) {
            Ok(image) => image,
            Err(e) => {
                eprintln!("bench: cannot decode {}: {e}", path.display());
                continue;
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;

        let started = Instant::now();
        let dhash_ms = {
            thumbnail::compute_dhash(&decoded);
            started.elapsed().as_secs_f64() * 1000.0
        };
        drop(decoded);
        drop(raw);

        let started = Instant::now();
        let thumb = thumbnail::generate_thumbnail(path, &thumb_dir, &rel, true);
        let thumb_ms = started.elapsed().as_secs_f64() * 1000.0;
        if thumb.is_none() {
            failed_thumb += 1;
        }

        // Phase 0.5 A/B: same source, same target size, two kernels. Box is the
        // area average the thumbnail already uses; Lanczos3 is what `prepare`
        // used until now. Different temp paths, so both can be alive at once.
        let started = Instant::now();
        let prepared_box = vision_prep::prepare_with(
            path,
            settings.image_max_side,
            &prep_dir,
            ImageFilter::Box,
        );
        let prepare_box_ms = started.elapsed().as_secs_f64() * 1000.0;

        let started = Instant::now();
        let prepared_lanczos = vision_prep::prepare_with(
            path,
            settings.image_max_side,
            &prep_dir,
            ImageFilter::Lanczos,
        );
        let prepare_lanczos_ms = started.elapsed().as_secs_f64() * 1000.0;

        if prepared_box.is_err() && prepared_lanczos.is_err() {
            failed_prepare += 1;
        }

        // What production does with `Auto`: box for photos, Lanczos for
        // screenshots/documents.
        let prepared = if vision_prep::prefers_box(path, ImageFilter::Auto) {
            &prepared_box
        } else {
            &prepared_lanczos
        };

        // Same file, read again through `image::open`'s `BufReader`. On SMB this
        // is the entry point production uses, so the gap against `read + decode`
        // tells us which read strategy the shared decode should adopt.
        let started = Instant::now();
        let open_ms = match image::open(path) {
            Ok(_) => started.elapsed().as_secs_f64() * 1000.0,
            Err(e) => {
                eprintln!("bench: image::open failed for {}: {e}", path.display());
                0.0
            }
        };

        let mut sample = Sample {
            bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            read_ms,
            decode_ms,
            dhash_ms,
            thumb_ms,
            prepare_box_ms,
            prepare_lanczos_ms,
            open_ms,
            ..Sample::default()
        };

        if let Some(model_name) = model.as_deref() {
            if let Ok(prepared) = prepared {
                let probe = classify::probe_primary(model_name, prepared, settings.prompt_profile);
                if probe.ok {
                    // A cold model load (~50 s) happens once per session, not per
                    // image, so it must not enter the per-image average. Same rule
                    // as `vision_estimate`.
                    let cold = index == 0 || probe.timings.load_s > 0.5;
                    if !cold {
                        sample.request_ms = Some(probe.wall_s * 1000.0);
                        sample.prefill_ms = Some(probe.timings.prefill_s * 1000.0);
                        sample.decode_gpu_ms = Some(probe.timings.decode_s * 1000.0);
                    }
                    probed += 1;
                } else {
                    failed_request += 1;
                }
            }
        }

        if quality {
            if let Some(model_name) = model.as_deref() {
                if let (Ok(box_img), Ok(lanc_img)) = (&prepared_box, &prepared_lanczos) {
                    let boxed =
                        classify::probe_primary(model_name, box_img, settings.prompt_profile);
                    let lanczos =
                        classify::probe_primary(model_name, lanc_img, settings.prompt_profile);
                    eprintln!(
                        "quality [{rel}]\n  box     : {}\n  lanczos : {}",
                        summarize_raw(&boxed.raw),
                        summarize_raw(&lanczos.raw)
                    );
                }
            }
        }

        println!(
            "{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{},{}",
            path.display(),
            sample.bytes,
            sample.read_ms,
            sample.decode_ms,
            sample.dhash_ms,
            sample.thumb_ms,
            sample.prepare_box_ms,
            sample.prepare_lanczos_ms,
            sample.open_ms,
            fmt_ms(sample.request_ms),
            fmt_ms(sample.prefill_ms),
            fmt_ms(sample.decode_gpu_ms),
            fmt_ms(sample.ollama_cpu_ms()),
        );

        samples.push(sample);
    }

    if samples.is_empty() {
        panic!("bench: no readable files");
    }

    let bytes = mean(&samples, |s| Some(s.bytes as f64)).unwrap_or(0.0);
    let read = mean(&samples, |s| Some(s.read_ms)).unwrap_or(0.0);
    let decode = mean(&samples, |s| Some(s.decode_ms)).unwrap_or(0.0);
    let dhash = mean(&samples, |s| Some(s.dhash_ms)).unwrap_or(0.0);
    let duplicated = mean(&samples, |s| Some(s.duplicated_ms())).unwrap_or(0.0);
    let thumb = mean(&samples, |s| Some(s.thumb_ms)).unwrap_or(0.0);
    let prepare_box = mean(&samples, |s| Some(s.prepare_box_ms)).unwrap_or(0.0);
    let prepare_lanczos = mean(&samples, |s| Some(s.prepare_lanczos_ms)).unwrap_or(0.0);
    // The derived model follows the production `Auto` policy, which picks box
    // for photos. A mixed folder lands between the two means.
    let prepare = prepare_box;
    let open = mean(&samples, |s| Some(s.open_ms)).unwrap_or(0.0);
    let request = mean(&samples, |s| s.request_ms);
    let prefill = mean(&samples, |s| s.prefill_ms).unwrap_or(0.0);
    let decode_gpu = mean(&samples, |s| s.decode_gpu_ms).unwrap_or(0.0);
    let ollama_cpu = mean(&samples, |s| s.ollama_cpu_ms());

    println!("\n=== summary (mean per file, ms) ===");
    println!("files             : {}", samples.len());
    println!("avg file size     : {:.2} MB", bytes / (1024.0 * 1024.0));
    println!("read (1st touch)  : {read:.1}");
    println!("decode (pure)     : {decode:.1}");
    println!("dhash             : {dhash:.1}");
    println!("duplicated unit   : {duplicated:.1}  (read + decode)");
    println!("thumb (total)     : {thumb:.1}");
    println!("prepare box       : {prepare_box:.1}");
    println!(
        "prepare lanczos   : {prepare_lanczos:.1}  ({:+.1} ms, {:+.0} %)",
        prepare_lanczos - prepare_box,
        if prepare_box > 0.0 {
            (prepare_lanczos / prepare_box - 1.0) * 100.0
        } else {
            0.0
        }
    );
    println!(
        "image::open       : {open:.1}  (BufReader read + decode = {:.0} % of read+decode)",
        open / duplicated * 100.0
    );
    match request {
        Some(request) => {
            println!("request (total)   : {request:.1}");
            println!("  prefill (GPU)   : {prefill:.1}");
            println!("  decode  (GPU)   : {decode_gpu:.1}");
            println!("  ollama cpu      : {:.1}", ollama_cpu.unwrap_or(0.0));
        }
        None => println!("request           : n/a (probes={probed}, failures={failed_request})"),
    }
    if failed_thumb > 0 || failed_prepare > 0 {
        println!("stage failures    : thumb={failed_thumb}, prepare={failed_prepare}");
    }

    // Per-file cost today: both decodes plus the request. This is the baseline
    // the pipeline has to beat by at least 20 % (`plans/pipeline-cpu-gpu-overlap.md`).
    let seq = thumb + prepare + request.unwrap_or(0.0);
    let cpu_after_shared_decode = (thumb + prepare + ollama_cpu.unwrap_or(0.0) - duplicated).max(0.0);
    let gpu_active = prefill + decode_gpu;
    // Ceiling only: it assumes the pipeline overlaps CPU and GPU perfectly and
    // that the workers do not steal CPU from Ollama's preprocessing. Real runs
    // will land below this.
    let ceiling = cpu_after_shared_decode.max(gpu_active);

    println!("\n=== derived (per file, ms) ===");
    println!("sequential (today): {seq:.1}");
    println!(
        "duplicated share  : {:.0} % of sequential",
        duplicated / seq * 100.0
    );
    println!(
        "shared decode only: {:.1} ({:.0} % cheaper)",
        seq - duplicated,
        duplicated / seq * 100.0
    );
    if request.is_none() {
        println!(
            "note: no BENCH_MODEL, so these percentages are relative to CPU only. \
             The real per-file cost also contains the Ollama request, which usually \
             dominates — set BENCH_MODEL to price the pipeline against it."
        );
    }
    println!(
        "pipeline ceiling  : {ceiling:.1} ({:.0} % faster than today)",
        (1.0 - ceiling / seq) * 100.0
    );
    println!(
        "CPU critical path : {cpu_after_shared_decode:.1} vs GPU {gpu_active:.1} ({})",
        if cpu_after_shared_decode >= gpu_active {
            "CPU-bound: overlap alone will not help, only the shared decode will"
        } else {
            "GPU-bound: overlap has room to help"
        }
    );
}
