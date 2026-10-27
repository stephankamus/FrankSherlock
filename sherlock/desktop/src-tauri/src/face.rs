//! Native ONNX face detection (InsightFace SCRFD) and recognition (ArcFace).
//!
//! Uses the `ort` crate with `load-dynamic` to call ONNX Runtime at inference
//! time without linking at compile time.  Model files (from the InsightFace
//! buffalo_l pack) are downloaded on first use to `~/.local/share/frank_sherlock/models/buffalo_l/`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use image::imageops::FilterType;
use image::RgbImage;
use ndarray::{Array2, Array4};
use ort::session::Session;

use crate::error::{AppError, AppResult};

// ── Public types ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub bbox: [f32; 4], // [x1, y1, x2, y2] in original image coords
    pub confidence: f32,
    #[allow(dead_code)] // Used during alignment, stored for future face crop display
    pub keypoints: [[f32; 2]; 5], // left_eye, right_eye, nose, left_mouth, right_mouth
    pub embedding: Vec<f32>, // 512-dim ArcFace embedding
}

pub struct FaceDetector {
    scrfd_session: Session,
    arcface_session: Session,
}

// ── Constants ───────────────────────────────────────────────────────

const SCRFD_INPUT_SIZE: u32 = 640;
const ARCFACE_INPUT_SIZE: u32 = 112;
const SCORE_THRESHOLD: f32 = 0.5;
const NMS_THRESHOLD: f32 = 0.4;
const STRIDES: [u32; 3] = [8, 16, 32];

/// Smallest face kept, in original-image pixels.
///
/// Below this the ArcFace embedding is noise and what it does is pollute a person with
/// a smear from a background. `_face_ab_test/docs/RESULTS.md` §4 measured 32 px as the
/// floor for the faces this library actually contains.
const MIN_FACE_SIZE_PX: f32 = 32.0;

/// Smallest variance of the Laplacian, over the 112×112 aligned crop, that is still a
/// face rather than motion blur. The same A/B keeps 70 % of the detections at
/// `confidence ≥ 0.5, size ≥ 32 px, blur ≥ 50` and calls that a good starting point.
const MIN_BLUR: f32 = 50.0;

/// Thread budget of a detector built without an explicit one.
const DEFAULT_INTRA_THREADS: usize = 4;

/// Threads one session asks ONNX Runtime for, per worker.
///
/// ORT parallelizes inside a session, and that scaling flattens quickly on a graph the
/// size of SCRFD: 1→2 threads is a big step, 4→8 is small, 8→16 is noise plus memory
/// pressure. Four threads per session is the knee; the rest of the budget buys more
/// sessions instead, which is what actually puts more images in flight.
const THREADS_PER_WORKER: usize = 4;

/// Most workers the pool will ever start.
///
/// Each worker owns a full copy of both ONNX sessions — roughly 180 MB of weights plus
/// ORT arenas — so this is a memory ceiling as much as a CPU one: 8 workers ≈ 1,5 GB.
pub const MAX_FACE_WORKERS: usize = 8;

/// Largest value accepted for `face_threads`.
pub const MAX_FACE_THREADS: u32 = 32;

/// Total threads the face engine may use, resolved from the configured ceiling.
///
/// `0` means "everything the machine has". Callers decide how much of that budget each
/// context may take: one thread while the GPU is classifying, the whole budget when the
/// GPU is idle.
pub fn thread_budget(configured: u32) -> usize {
    if configured == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        (configured as usize).clamp(1, MAX_FACE_THREADS as usize)
    }
}

/// How many sessions to run and how many threads each one gets.
///
/// `workers * intra` stays inside `budget`. One worker per `THREADS_PER_WORKER` of
/// budget, capped at [`MAX_FACE_WORKERS`]; whatever is left over goes to the sessions.
pub fn plan_pool(budget: usize) -> (usize, usize) {
    let budget = budget.max(1);
    let workers = (budget / THREADS_PER_WORKER).clamp(1, MAX_FACE_WORKERS);
    let intra = (budget / workers).clamp(1, 8);
    (workers, intra)
}

/// Serializes the whole face engine across the app.
///
/// Two passes at once — the Detect Faces button and a scan with `face_detection` on —
/// would each load their own copies of the models and write `face_detections` for the
/// same rows. Whichever starts first wins and the other one backs off for that run.
static FACE_ENGINE_BUSY: AtomicBool = AtomicBool::new(false);

/// Held for the duration of a face pass.
pub struct FaceEngineGuard;

impl FaceEngineGuard {
    /// `None` when another pass already holds the engine.
    pub fn try_acquire() -> Option<Self> {
        FACE_ENGINE_BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| FaceEngineGuard)
    }
}

impl Drop for FaceEngineGuard {
    fn drop(&mut self) {
        FACE_ENGINE_BUSY.store(false, Ordering::SeqCst);
    }
}

const BUFFALO_L_URL: &str =
    "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip";

/// Standard ArcFace alignment reference points for 112×112 crop.
const ARCFACE_REF: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

// ── Model management ────────────────────────────────────────────────

/// The buffalo_l directory, when both ONNX files are already on disk.
///
/// `ensure_models` downloads ~200 MB, which a scan must never do behind the user's
/// back: the pipeline uses this check and simply skips faces when the models are not
/// there yet.
pub fn models_present(models_dir: &Path) -> Option<PathBuf> {
    let buffalo_dir = models_dir.join("buffalo_l");
    let det = buffalo_dir.join("det_10g.onnx");
    let rec = buffalo_dir.join("w600k_r50.onnx");
    (det.exists() && rec.exists()).then_some(buffalo_dir)
}

/// Everything a face pass needs from the filesystem, checked without side effects.
///
/// Returns the buffalo_l directory when both the models and the ONNX Runtime library
/// are present. The runtime check is not paranoia: `ort` with `load-dynamic` falls
/// back to loading `libonnxruntime.so` from the loader search path when nobody calls
/// `init_from`, and a missing library there is `expect("Failed to load ONNX Runtime
/// dylib")` — a panic inside `setup_api`, not an `Err`. That panic took the scan
/// producer down and the pass ended after 26 files.
pub fn engine_ready(models_dir: &Path, ort_lib_dir: &Path) -> Result<PathBuf, String> {
    let models = models_present(models_dir).ok_or_else(|| {
        format!(
            "the buffalo_l models are not installed in {} (Detect Faces downloads them)",
            models_dir.display()
        )
    })?;

    let lib = ort_lib_dir.join(onnxruntime_lib_name());
    if !lib.exists() {
        return Err(format!(
            "ONNX Runtime is missing at {} (run scripts/download-onnxruntime.sh)",
            lib.display()
        ));
    }

    Ok(models)
}

pub fn ensure_models(models_dir: &Path) -> AppResult<PathBuf> {
    let buffalo_dir = models_dir.join("buffalo_l");
    let det_path = buffalo_dir.join("det_10g.onnx");
    let rec_path = buffalo_dir.join("w600k_r50.onnx");

    if det_path.exists() && rec_path.exists() {
        return Ok(buffalo_dir);
    }

    std::fs::create_dir_all(&buffalo_dir)?;

    log::info!("Downloading InsightFace buffalo_l models...");
    let mut resp = ureq::get(BUFFALO_L_URL)
        .call()
        .map_err(|e| AppError::Config(format!("Failed to download buffalo_l models: {e}")))?;

    // buffalo_l.zip is ~200MB; ureq defaults to 10MB limit
    let body = resp
        .body_mut()
        .with_config()
        .limit(300 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| AppError::Config(format!("Failed to read buffalo_l download: {e}")))?;

    let cursor = std::io::Cursor::new(body);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| AppError::Config(format!("Failed to open buffalo_l zip: {e}")))?;

    let wanted = ["det_10g.onnx", "w600k_r50.onnx"];
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| AppError::Config(format!("zip entry error: {e}")))?;
        let name = file.name().to_string();
        for w in &wanted {
            if name.ends_with(w) {
                let out_path = buffalo_dir.join(w);
                let mut out = std::fs::File::create(&out_path)?;
                std::io::copy(&mut file, &mut out)?;
                log::info!("Extracted {w} to {}", out_path.display());
            }
        }
    }

    if !det_path.exists() || !rec_path.exists() {
        return Err(AppError::Config(
            "buffalo_l archive did not contain expected ONNX files".into(),
        ));
    }

    Ok(buffalo_dir)
}

// ── FaceDetector ────────────────────────────────────────────────────

impl FaceDetector {
    #[allow(dead_code)]
    pub fn new(models_dir: &Path) -> AppResult<Self> {
        let buffalo_dir = ensure_models(models_dir)?;
        Self::from_model_dir(&buffalo_dir)
    }

    /// Create a detector from a directory that already contains the ONNX model files.
    /// Call `ensure_models()` first to download them if needed.
    pub fn from_model_dir(buffalo_dir: &Path) -> AppResult<Self> {
        Self::from_model_dir_with_threads(buffalo_dir, DEFAULT_INTRA_THREADS)
    }

    /// Same, with an explicit thread budget per session.
    ///
    /// A worker pool runs one detector per thread (see the caller in `lib.rs`), so the
    /// budget is per worker: `workers × threads` must not exceed the cores, or the
    /// pool competes with itself. Inside a scan it must also leave room for Ollama's
    /// image preprocessor, which is the actual bottleneck of that pass.
    pub fn from_model_dir_with_threads(
        buffalo_dir: &Path,
        intra_threads: usize,
    ) -> AppResult<Self> {
        let det_path = buffalo_dir.join("det_10g.onnx");
        let rec_path = buffalo_dir.join("w600k_r50.onnx");
        let intra_threads = intra_threads.max(1);

        let scrfd_session = Session::builder()
            .map_err(|e| AppError::Config(format!("SCRFD session builder: {e}")))?
            .with_intra_threads(intra_threads)
            .map_err(|e| AppError::Config(format!("SCRFD intra threads: {e}")))?
            .commit_from_file(&det_path)
            .map_err(|e| AppError::Config(format!("SCRFD load failed: {e}")))?;

        let arcface_session = Session::builder()
            .map_err(|e| AppError::Config(format!("ArcFace session builder: {e}")))?
            .with_intra_threads(intra_threads)
            .map_err(|e| AppError::Config(format!("ArcFace intra threads: {e}")))?
            .commit_from_file(&rec_path)
            .map_err(|e| AppError::Config(format!("ArcFace load failed: {e}")))?;

        Ok(Self {
            scrfd_session,
            arcface_session,
        })
    }

    /// Detect and embed every face of a file, reading and decoding it here.
    ///
    /// Kept for tests and tools. Every in-app caller already holds the decoded image
    /// (the scan producer decodes it for the thumbnail) and must use
    /// [`FaceDetector::detect_decoded`] instead: decoding a 12–14 MP photo twice is
    /// the largest single cost of a face pass.
    #[allow(dead_code)]
    pub fn detect(&mut self, image_path: &Path) -> AppResult<Vec<FaceDetection>> {
        let raw_img = image::open(image_path)
            .map_err(|e| AppError::Config(format!("Failed to open image: {e}")))?;
        let orientation = crate::exif::extract_orientation(image_path);
        let img = crate::exif::apply_orientation(raw_img, orientation);
        self.detect_decoded(&img)
    }

    /// Detect and embed every face of an image that is already decoded and oriented.
    ///
    /// Bounding boxes and keypoints are in the coordinates of `img`, which are the
    /// original image coordinates because orientation was applied before the call.
    pub fn detect_decoded(&mut self, img: &image::DynamicImage) -> AppResult<Vec<FaceDetection>> {
        let rgb = as_rgb8(img);
        let (orig_w, orig_h) = (rgb.width(), rgb.height());

        // 1. Preprocess for SCRFD
        let (tensor, scale, pad_x, pad_y) = preprocess_scrfd(&rgb);

        // 2. Create input Value and run SCRFD
        let input_value = ort::value::Tensor::from_array(tensor)
            .map_err(|e| AppError::Config(format!("SCRFD tensor creation: {e}")))?;

        // Run detection and extract raw detections in a scoped block
        // so that `outputs` is dropped before we borrow self again for embeddings.
        let kept = {
            let outputs = self
                .scrfd_session
                .run(ort::inputs!["input.1" => input_value])
                .map_err(|e| AppError::Config(format!("SCRFD inference error: {e}")))?;

            // 3. Post-process detections
            let raw_dets = postprocess_scrfd(&outputs, scale, pad_x, pad_y, orig_w, orig_h)?;

            // 4. NMS
            nms(&raw_dets, NMS_THRESHOLD)
        }; // outputs dropped here

        // 5. Align, gate and embed each kept face
        let mut results = Vec::with_capacity(kept.len());
        for det in &kept {
            let face_w = det.bbox[2] - det.bbox[0];
            let face_h = det.bbox[3] - det.bbox[1];
            if face_w.min(face_h) < MIN_FACE_SIZE_PX {
                continue;
            }

            // The aligned crop is what ArcFace consumes, so measuring its sharpness
            // costs nothing extra and keeps a motion-blurred smudge out of a person.
            let aligned = align_face(&rgb, &det.keypoints);
            if laplacian_variance(&aligned) < MIN_BLUR {
                continue;
            }

            let embedding = self.embed(&aligned)?;
            results.push(FaceDetection {
                bbox: det.bbox,
                confidence: det.confidence,
                keypoints: det.keypoints,
                embedding,
            });
        }

        Ok(results)
    }

    fn embed(&mut self, aligned: &RgbImage) -> AppResult<Vec<f32>> {
        // Preprocess for ArcFace
        let tensor = preprocess_arcface(aligned);

        // Create input Value and run ArcFace
        let input_value = ort::value::Tensor::from_array(tensor)
            .map_err(|e| AppError::Config(format!("ArcFace tensor creation: {e}")))?;

        let outputs = self
            .arcface_session
            .run(ort::inputs!["input.1" => input_value])
            .map_err(|e| AppError::Config(format!("ArcFace inference error: {e}")))?;

        // Extract and L2-normalize the 512-dim vector
        let (_shape, raw_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Config(format!("ArcFace output extract: {e}")))?;

        let norm = raw_data
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(1e-10);
        let normalized: Vec<f32> = raw_data.iter().map(|v| v / norm).collect();

        Ok(normalized)
    }
}

/// Borrow an RGB buffer when the decoded image already is one.
///
/// `to_rgb8()` on a 12 MP RGBA photo allocates and copies 36 MB; the pipeline producer
/// hands us whatever the decoder produced.
fn as_rgb8(img: &image::DynamicImage) -> std::borrow::Cow<'_, RgbImage> {
    use std::borrow::Cow;
    match img {
        image::DynamicImage::ImageRgb8(rgb) => Cow::Borrowed(rgb),
        other => Cow::Owned(other.to_rgb8()),
    }
}

/// Variance of the Laplacian over the aligned crop, the standard sharpness proxy.
fn laplacian_variance(img: &RgbImage) -> f32 {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let gray = |x: i32, y: i32| -> f32 {
        let px = img.get_pixel(x as u32, y as u32);
        0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32
    };

    let mut sum = 0f64;
    let mut sum_sq = 0f64;
    let mut count = 0u32;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let lap = -4.0 * gray(x, y)
                + gray(x - 1, y)
                + gray(x + 1, y)
                + gray(x, y - 1)
                + gray(x, y + 1);
            sum += lap as f64;
            sum_sq += (lap * lap) as f64;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    let mean = sum / count as f64;
    (sum_sq / count as f64 - mean * mean).max(0.0) as f32
}

// ── SCRFD preprocessing ─────────────────────────────────────────────

fn preprocess_scrfd(img: &RgbImage) -> (Array4<f32>, f32, f32, f32) {
    let (w, h) = (img.width(), img.height());
    let target = SCRFD_INPUT_SIZE;

    // Letterbox: scale to fit target keeping aspect ratio, pad with zeros
    let scale = (target as f32 / w as f32).min(target as f32 / h as f32);
    let new_w = (w as f32 * scale).round() as u32;
    let new_h = (h as f32 * scale).round() as u32;

    let resized = image::imageops::resize(img, new_w, new_h, FilterType::Triangle);

    let pad_x = (target - new_w) as f32 / 2.0;
    let pad_y = (target - new_h) as f32 / 2.0;
    let pad_left = pad_x.floor() as u32;
    let pad_top = pad_y.floor() as u32;

    // Build NCHW tensor [1, 3, 640, 640] over one flat slice: the padding stays at
    // 0.0, which after this normalization is the 127.5 grey SCRFD expects.
    let side = target as usize;
    let plane = side * side;
    let mut tensor = Array4::<f32>::zeros((1, 3, side, side));
    {
        let flat = tensor
            .as_slice_mut()
            .expect("freshly built Array4 is contiguous");
        let row_bytes = new_w as usize * 3;
        for (y, row) in resized.as_raw().chunks_exact(row_bytes.max(1)).enumerate() {
            let ty = y + pad_top as usize;
            if ty >= side {
                break;
            }
            let row_start = ty * side + pad_left as usize;
            for (x, px) in row.chunks_exact(3).enumerate() {
                let index = row_start + x;
                if index >= plane {
                    break;
                }
                flat[index] = (px[0] as f32 - 127.5) / 128.0;
                flat[plane + index] = (px[1] as f32 - 127.5) / 128.0;
                flat[2 * plane + index] = (px[2] as f32 - 127.5) / 128.0;
            }
        }
    }

    (tensor, scale, pad_x.floor(), pad_y.floor())
}

// ── SCRFD postprocessing ────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RawDetection {
    bbox: [f32; 4],
    confidence: f32,
    keypoints: [[f32; 2]; 5],
}

/// Index into a tensor that may be 2D `[rows, cols]` or 3D `[1, rows, cols]`.
/// Called as `tidx(shape, row, col)` — for 3D tensors the batch dim is assumed 0.
fn tidx(shape: &[usize], row: usize, col: usize) -> usize {
    match shape.len() {
        2 => row * shape[1] + col,
        3 => row * shape[2] + col, // batch dim 0, offset is the same
        _ => panic!("unexpected SCRFD tensor rank: {}", shape.len()),
    }
}

fn postprocess_scrfd(
    outputs: &ort::session::SessionOutputs<'_>,
    scale: f32,
    pad_x: f32,
    pad_y: f32,
    orig_w: u32,
    orig_h: u32,
) -> AppResult<Vec<RawDetection>> {
    // SCRFD outputs 9 tensors: for each stride {8,16,32} → (scores, bboxes, keypoints)
    // Output ordering: scores_8, scores_16, scores_32, bbox_8, bbox_16, bbox_32, kps_8, kps_16, kps_32
    let mut detections = Vec::new();

    for (stride_idx, &stride) in STRIDES.iter().enumerate() {
        let feat_h = SCRFD_INPUT_SIZE / stride;
        let feat_w = SCRFD_INPUT_SIZE / stride;
        let num_anchors = 2u32;

        let (scores_shape, scores_data) = outputs[stride_idx]
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Config(format!("SCRFD scores extract: {e}")))?;
        let (bbox_shape, bbox_data) = outputs[stride_idx + 3]
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Config(format!("SCRFD bbox extract: {e}")))?;
        let (kps_shape, kps_data) = outputs[stride_idx + 6]
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Config(format!("SCRFD kps extract: {e}")))?;

        let s_dims: Vec<usize> = scores_shape.iter().map(|d| *d as usize).collect();
        let b_dims: Vec<usize> = bbox_shape.iter().map(|d| *d as usize).collect();
        let k_dims: Vec<usize> = kps_shape.iter().map(|d| *d as usize).collect();

        let mut anchor_idx: usize = 0;
        for row in 0..feat_h {
            for col in 0..feat_w {
                let anchor_cx = (col as f32 + 0.5) * stride as f32;
                let anchor_cy = (row as f32 + 0.5) * stride as f32;

                for _a in 0..num_anchors {
                    let score = scores_data[tidx(&s_dims, anchor_idx, 0)];
                    if score > SCORE_THRESHOLD {
                        // distance2bbox
                        let x1 =
                            anchor_cx - bbox_data[tidx(&b_dims, anchor_idx, 0)] * stride as f32;
                        let y1 =
                            anchor_cy - bbox_data[tidx(&b_dims, anchor_idx, 1)] * stride as f32;
                        let x2 =
                            anchor_cx + bbox_data[tidx(&b_dims, anchor_idx, 2)] * stride as f32;
                        let y2 =
                            anchor_cy + bbox_data[tidx(&b_dims, anchor_idx, 3)] * stride as f32;

                        // distance2kps
                        let mut keypoints = [[0f32; 2]; 5];
                        for k in 0..5 {
                            keypoints[k][0] = anchor_cx
                                + kps_data[tidx(&k_dims, anchor_idx, k * 2)] * stride as f32;
                            keypoints[k][1] = anchor_cy
                                + kps_data[tidx(&k_dims, anchor_idx, k * 2 + 1)] * stride as f32;
                        }

                        // Map back from letterboxed coords to original image
                        let bbox = [
                            ((x1 - pad_x) / scale).clamp(0.0, orig_w as f32),
                            ((y1 - pad_y) / scale).clamp(0.0, orig_h as f32),
                            ((x2 - pad_x) / scale).clamp(0.0, orig_w as f32),
                            ((y2 - pad_y) / scale).clamp(0.0, orig_h as f32),
                        ];
                        for kp in &mut keypoints {
                            kp[0] = ((kp[0] - pad_x) / scale).clamp(0.0, orig_w as f32);
                            kp[1] = ((kp[1] - pad_y) / scale).clamp(0.0, orig_h as f32);
                        }

                        detections.push(RawDetection {
                            bbox,
                            confidence: score,
                            keypoints,
                        });
                    }
                    anchor_idx += 1;
                }
            }
        }
    }

    Ok(detections)
}

// ── NMS ─────────────────────────────────────────────────────────────

fn nms(dets: &[RawDetection], threshold: f32) -> Vec<RawDetection> {
    if dets.is_empty() {
        return Vec::new();
    }

    let mut indices: Vec<usize> = (0..dets.len()).collect();
    indices.sort_by(|&a, &b| {
        dets[b]
            .confidence
            .partial_cmp(&dets[a].confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = Vec::new();
    let mut suppressed = vec![false; dets.len()];

    for &i in &indices {
        if suppressed[i] {
            continue;
        }
        keep.push(dets[i].clone());
        for &j in &indices {
            if suppressed[j] || i == j {
                continue;
            }
            if iou(&dets[i].bbox, &dets[j].bbox) > threshold {
                suppressed[j] = true;
            }
        }
    }

    keep
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = a[2].min(b[2]);
    let y2 = a[3].min(b[3]);

    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a[2] - a[0]) * (a[3] - a[1]);
    let area_b = (b[2] - b[0]) * (b[3] - b[1]);
    let union = area_a + area_b - inter;

    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

// ── ArcFace alignment ───────────────────────────────────────────────

fn align_face(img: &RgbImage, keypoints: &[[f32; 2]; 5]) -> RgbImage {
    // Estimate similarity transform from detected keypoints to reference points.
    // We use least-squares to solve for [s*cos, -s*sin, tx; s*sin, s*cos, ty].
    let src = Array2::from_shape_fn((5, 2), |(i, j)| keypoints[i][j]);
    let dst = Array2::from_shape_fn((5, 2), |(i, j)| ARCFACE_REF[i][j]);

    let (tfm_a, tfm_b) = estimate_similarity_transform(src.view(), dst.view());

    // Apply inverse transform to produce the aligned 112×112 crop
    let out_size = ARCFACE_INPUT_SIZE;
    let mut aligned = RgbImage::new(out_size, out_size);

    // Inverse: src = A^-1 * (dst - b)
    // A = [[a, -b], [b, a]], A^-1 = 1/(a²+b²) * [[a, b], [-b, a]]
    let det = tfm_a[0] * tfm_a[0] + tfm_a[1] * tfm_a[1];
    if det < 1e-10 {
        // Degenerate transform — return a center crop instead
        return crop_center(img, out_size);
    }
    let inv_det = 1.0 / det;
    let ia = tfm_a[0] * inv_det;
    let ib = tfm_a[1] * inv_det;

    // Bilinear instead of nearest: the transform is a rotation plus a scale, so every
    // source pixel lands between four destination pixels. Nearest kept the aliasing
    // and it ends up in the embedding, which is the only thing ArcFace sees.
    for dy in 0..out_size {
        for dx in 0..out_size {
            let px = dx as f32 - tfm_b[0];
            let py = dy as f32 - tfm_b[1];
            let sx = ia * px + ib * py;
            let sy = -ib * px + ia * py;
            aligned.put_pixel(dx, dy, sample_bilinear(img, sx, sy));
        }
    }

    aligned
}

/// Bilinear sample of `img` at `(x, y)`, black outside the image.
fn sample_bilinear(img: &RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
        return image::Rgb([0, 0, 0]);
    }

    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = img.get_pixel(x0 as u32, y0 as u32);
    let p10 = img.get_pixel(x1 as u32, y0 as u32);
    let p01 = img.get_pixel(x0 as u32, y1 as u32);
    let p11 = img.get_pixel(x1 as u32, y1 as u32);

    let mut out = [0u8; 3];
    for channel in 0..3 {
        let top = p00[channel] as f32 * (1.0 - fx) + p10[channel] as f32 * fx;
        let bottom = p01[channel] as f32 * (1.0 - fx) + p11[channel] as f32 * fx;
        out[channel] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    image::Rgb(out)
}

fn estimate_similarity_transform(
    src: ndarray::ArrayView2<f32>,
    dst: ndarray::ArrayView2<f32>,
) -> ([f32; 2], [f32; 2]) {
    // Solve for [a, b, tx, ty] where:
    //   dst_x = a * src_x - b * src_y + tx
    //   dst_y = b * src_x + a * src_y + ty
    let n = src.nrows();
    let mut ata = [0f32; 16]; // 4x4
    let mut atb_vec = [0f32; 4];

    for i in 0..n {
        let sx = src[[i, 0]];
        let sy = src[[i, 1]];
        let dx = dst[[i, 0]];
        let dy = dst[[i, 1]];

        // Row for x: [sx, -sy, 1, 0] * [a, b, tx, ty]' = dx
        let r1 = [sx, -sy, 1.0, 0.0];
        // Row for y: [sy, sx, 0, 1] * [a, b, tx, ty]' = dy
        let r2 = [sy, sx, 0.0, 1.0];

        for j in 0..4 {
            for k in 0..4 {
                ata[j * 4 + k] += r1[j] * r1[k] + r2[j] * r2[k];
            }
            atb_vec[j] += r1[j] * dx + r2[j] * dy;
        }
    }

    // Solve 4x4 system via Gauss elimination
    let params = solve_4x4(&ata, &atb_vec);

    ([params[0], params[1]], [params[2], params[3]])
}

#[allow(clippy::needless_range_loop)] // Gauss elimination naturally uses index-based access
fn solve_4x4(ata: &[f32; 16], atb: &[f32; 4]) -> [f32; 4] {
    // Augmented matrix [A|b]
    let mut m = [[0f32; 5]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = ata[i * 4 + j];
        }
        m[i][4] = atb[i];
    }

    // Forward elimination with partial pivoting
    for col in 0..4 {
        // Find pivot
        let mut max_row = col;
        let mut max_val = m[col][col].abs();
        for row in (col + 1)..4 {
            if m[row][col].abs() > max_val {
                max_val = m[row][col].abs();
                max_row = row;
            }
        }
        m.swap(col, max_row);

        let pivot = m[col][col];
        if pivot.abs() < 1e-10 {
            return [1.0, 0.0, 0.0, 0.0]; // fallback: identity-ish
        }

        for row in (col + 1)..4 {
            let factor = m[row][col] / pivot;
            for k in col..5 {
                m[row][k] -= factor * m[col][k];
            }
        }
    }

    // Back substitution
    let mut x = [0f32; 4];
    for i in (0..4).rev() {
        let mut sum = m[i][4];
        for j in (i + 1)..4 {
            sum -= m[i][j] * x[j];
        }
        x[i] = sum / m[i][i];
    }

    x
}

fn crop_center(img: &RgbImage, size: u32) -> RgbImage {
    let (w, h) = (img.width(), img.height());
    let min_dim = w.min(h);
    let x0 = (w - min_dim) / 2;
    let y0 = (h - min_dim) / 2;
    let cropped = image::imageops::crop_imm(img, x0, y0, min_dim, min_dim).to_image();
    image::imageops::resize(&cropped, size, size, FilterType::Triangle)
}

// ── ArcFace preprocessing ───────────────────────────────────────────

fn preprocess_arcface(img: &RgbImage) -> Array4<f32> {
    let size = ARCFACE_INPUT_SIZE as usize;
    let plane = size * size;
    let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
    {
        let flat = tensor
            .as_slice_mut()
            .expect("freshly built Array4 is contiguous");
        for (index, px) in img.as_raw().chunks_exact(3).enumerate() {
            if index >= plane {
                break;
            }
            flat[index] = (px[0] as f32 - 127.5) / 128.0;
            flat[plane + index] = (px[1] as f32 - 127.5) / 128.0;
            flat[2 * plane + index] = (px[2] as f32 - 127.5) / 128.0;
        }
    }
    tensor
}

// ── ONNX Runtime initialization ─────────────────────────────────────

/// Platform-specific ONNX Runtime shared library filename.
pub fn onnxruntime_lib_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "libonnxruntime.so"
    }
    #[cfg(target_os = "macos")]
    {
        "libonnxruntime.dylib"
    }
    #[cfg(target_os = "windows")]
    {
        "onnxruntime.dll"
    }
}

/// Initialize ONNX Runtime from a directory containing the shared library.
/// Must be called before any `ort::Session` creation.
/// Returns Ok(true) if initialized, Ok(false) if lib not found.
pub fn init_ort_from_dir(lib_dir: &Path) -> AppResult<bool> {
    let lib_path = lib_dir.join(onnxruntime_lib_name());
    if !lib_path.exists() {
        return Ok(false);
    }
    ort::init_from(lib_path)
        .map_err(|e| AppError::Config(format!("ONNX Runtime init failed: {e}")))?
        .commit();
    Ok(true)
}

// ── Face crop generation ────────────────────────────────────────────

/// Crop a single face from an already-decoded image (avoids re-opening the file).
/// Takes bbox `[x1, y1, x2, y2]`, adds 20% padding, resizes longest side to 150px JPEG.
pub fn crop_face_from_image(
    img: &image::DynamicImage,
    bbox: &[f32; 4],
    output_dir: &Path,
    face_id: i64,
) -> AppResult<PathBuf> {
    crop_face_to_file(img, bbox, output_dir, &face_id.to_string())
}

/// Crop one face to `<output_dir>/<stem>.jpg`.
///
/// The stem is chosen by the caller: a face id when the id already exists (crop
/// regeneration), or a deterministic `<file_id>_<index>` when the crops are written
/// before the rows, which is what lets the whole file be recorded in one transaction.
pub fn crop_face_to_file(
    img: &image::DynamicImage,
    bbox: &[f32; 4],
    output_dir: &Path,
    stem: &str,
) -> AppResult<PathBuf> {
    let (img_w, img_h) = (img.width() as f32, img.height() as f32);
    let face_w = bbox[2] - bbox[0];
    let face_h = bbox[3] - bbox[1];
    let pad_x = face_w * 0.2;
    let pad_y = face_h * 0.2;

    let x1 = (bbox[0] - pad_x).max(0.0) as u32;
    let y1 = (bbox[1] - pad_y).max(0.0) as u32;
    let x2 = ((bbox[2] + pad_x).min(img_w)) as u32;
    let y2 = ((bbox[3] + pad_y).min(img_h)) as u32;

    let crop_w = x2.saturating_sub(x1).max(1);
    let crop_h = y2.saturating_sub(y1).max(1);

    let cropped = img.crop_imm(x1, y1, crop_w, crop_h);

    let longest = crop_w.max(crop_h) as f32;
    let scale = 150.0 / longest;
    let new_w = ((crop_w as f32 * scale).round() as u32).max(1);
    let new_h = ((crop_h as f32 * scale).round() as u32).max(1);
    let resized = cropped.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);

    let out_path = output_dir.join(format!("{stem}.jpg"));
    resized
        .to_rgb8()
        .save_with_format(&out_path, image::ImageFormat::Jpeg)
        .map_err(|e| AppError::Config(format!("Failed to save face crop: {e}")))?;
    Ok(out_path)
}

/// Detect the faces of an already decoded image and write their crops.
///
/// The whole point of the pair: one decode produces the detection, the embedding and
/// the crop files. Returns the detections and one crop path per face (`None` when a
/// crop could not be written), ready for a single `record_faces` transaction.
///
/// Crop names are `<file_id>_<index>` rather than the face id, because the rows are
/// written *after* the files and using the id would need an UPDATE per face.
pub fn detect_and_crop(
    detector: &mut FaceDetector,
    image: &image::DynamicImage,
    output_dir: &Path,
    file_id: i64,
) -> AppResult<(Vec<FaceDetection>, Vec<Option<String>>)> {
    let faces = detector.detect_decoded(image)?;
    if faces.is_empty() {
        return Ok((faces, Vec::new()));
    }

    let mut crops = Vec::with_capacity(faces.len());
    for (index, face) in faces.iter().enumerate() {
        let stem = format!("{file_id}_{index}");
        match crop_face_to_file(image, &face.bbox, output_dir, &stem) {
            Ok(path) => crops.push(Some(path.to_string_lossy().into_owned())),
            Err(e) => {
                log::warn!("Failed to write face crop {index} of file {file_id}: {e}");
                crops.push(None);
            }
        }
    }

    Ok((faces, crops))
}

/// Result from a single face crop within a batch.
pub struct BatchCropResult {
    pub face_id: i64,
    pub result: AppResult<PathBuf>,
}

/// Generate face crops grouped by source image — each image is decoded only once.
/// `on_progress` is called after each face is processed with the running total.
pub fn generate_face_crops_batch<F>(
    jobs: &[crate::models::FaceCropJob],
    output_dir: &Path,
    mut on_progress: F,
) -> Vec<BatchCropResult>
where
    F: FnMut(usize),
{
    use std::collections::HashMap;

    // Group jobs by source image path
    let mut by_image: HashMap<&str, Vec<&crate::models::FaceCropJob>> = HashMap::new();
    for job in jobs {
        by_image.entry(&job.abs_path).or_default().push(job);
    }

    let mut results = Vec::with_capacity(jobs.len());
    let mut done = 0usize;

    for (abs_path, faces) in &by_image {
        let path = Path::new(abs_path);
        if !path.exists() {
            for face_job in faces {
                results.push(BatchCropResult {
                    face_id: face_job.face_id,
                    result: Err(AppError::Config(format!(
                        "Source image not found: {abs_path}"
                    ))),
                });
                done += 1;
                on_progress(done);
            }
            continue;
        }

        // Open + EXIF-orient the image ONCE
        let img = match image::open(path) {
            Ok(raw) => {
                let orientation = crate::exif::extract_orientation(path);
                crate::exif::apply_orientation(raw, orientation)
            }
            Err(e) => {
                for face_job in faces {
                    results.push(BatchCropResult {
                        face_id: face_job.face_id,
                        result: Err(AppError::Config(format!("Failed to open image: {e}"))),
                    });
                    done += 1;
                    on_progress(done);
                }
                continue;
            }
        };

        // Crop all faces from this one decoded image
        for face_job in faces {
            let result = crop_face_from_image(&img, &face_job.bbox, output_dir, face_job.face_id);
            results.push(BatchCropResult {
                face_id: face_job.face_id,
                result,
            });
            done += 1;
            on_progress(done);
        }
    }

    results
}

// ── Embedding helpers ───────────────────────────────────────────────

pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_identical_boxes() {
        let a = [10.0, 10.0, 50.0, 50.0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn iou_no_overlap() {
        let a = [0.0, 0.0, 10.0, 10.0];
        let b = [20.0, 20.0, 30.0, 30.0];
        assert!((iou(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn iou_partial_overlap() {
        let a = [0.0, 0.0, 20.0, 20.0];
        let b = [10.0, 10.0, 30.0, 30.0];
        // Intersection: 10x10=100, Union: 400+400-100=700
        let expected = 100.0 / 700.0;
        assert!((iou(&a, &b) - expected).abs() < 1e-5);
    }

    #[test]
    fn nms_removes_overlapping() {
        let dets = vec![
            RawDetection {
                bbox: [0.0, 0.0, 100.0, 100.0],
                confidence: 0.9,
                keypoints: [[0.0; 2]; 5],
            },
            RawDetection {
                bbox: [5.0, 5.0, 105.0, 105.0],
                confidence: 0.8,
                keypoints: [[0.0; 2]; 5],
            },
            RawDetection {
                bbox: [200.0, 200.0, 300.0, 300.0],
                confidence: 0.7,
                keypoints: [[0.0; 2]; 5],
            },
        ];
        let kept = nms(&dets, 0.4);
        // First two overlap heavily, only highest confidence kept. Third is separate.
        assert_eq!(kept.len(), 2);
        assert!((kept[0].confidence - 0.9).abs() < 1e-6);
        assert!((kept[1].confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn nms_empty_input() {
        let kept = nms(&[], 0.4);
        assert!(kept.is_empty());
    }

    #[test]
    fn nms_single_detection() {
        let dets = vec![RawDetection {
            bbox: [10.0, 10.0, 50.0, 50.0],
            confidence: 0.95,
            keypoints: [[0.0; 2]; 5],
        }];
        let kept = nms(&dets, 0.4);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn embedding_roundtrip() {
        let original: Vec<f32> = (0..512).map(|i| i as f32 * 0.001).collect();
        let blob = embedding_to_blob(&original);
        assert_eq!(blob.len(), 2048);
        let recovered = blob_to_embedding(&blob);
        assert_eq!(recovered.len(), 512);
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-7);
        }
    }

    #[test]
    fn cosine_similarity_identical() {
        let v: Vec<f32> = (0..512).map(|i| (i as f32).sin()).collect();
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn solve_4x4_identity() {
        // A = I, b = [1,2,3,4] => x = [1,2,3,4]
        #[rustfmt::skip]
        let ata: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let atb = [1.0, 2.0, 3.0, 4.0];
        let x = solve_4x4(&ata, &atb);
        for i in 0..4 {
            assert!(
                (x[i] - atb[i]).abs() < 1e-5,
                "x[{i}] = {} != {}",
                x[i],
                atb[i]
            );
        }
    }

    #[test]
    fn preprocess_scrfd_output_shape() {
        let img = RgbImage::new(200, 300);
        let (tensor, scale, _, _) = preprocess_scrfd(&img);
        assert_eq!(tensor.shape(), &[1, 3, 640, 640]);
        assert!(scale > 0.0);
    }

    #[test]
    fn preprocess_arcface_output_shape() {
        let img = RgbImage::new(112, 112);
        let tensor = preprocess_arcface(&img);
        assert_eq!(tensor.shape(), &[1, 3, 112, 112]);
    }

    #[test]
    fn crop_center_produces_correct_size() {
        let img = RgbImage::new(200, 300);
        let cropped = crop_center(&img, 112);
        assert_eq!(cropped.width(), 112);
        assert_eq!(cropped.height(), 112);
    }

    /// The ort crate must complete its OrtApi handshake with the bundled
    /// ONNX Runtime library. A crate/library version mismatch panics deep
    /// inside ort at first API use, which killed face detection silently
    /// (#12). CI downloads the bundled runtime before running tests on all
    /// platforms, so this guards the pairing everywhere we ship.
    #[test]
    fn bundled_onnxruntime_is_compatible_with_ort_crate() {
        let lib_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib");
        if !lib_dir.join(onnxruntime_lib_name()).exists() {
            eprintln!("skipping: ONNX Runtime library not present in lib/");
            return;
        }
        let loaded = init_ort_from_dir(&lib_dir).expect("ONNX Runtime init must succeed");
        assert!(loaded, "library exists but was not loaded");
        // SessionBuilder::new() performs the OrtApi version handshake.
        ort::session::builder::SessionBuilder::new()
            .expect("OrtApi handshake with bundled ONNX Runtime must succeed");
    }

    /// The budget is the ceiling, and the split keeps `workers * intra` inside it.
    #[test]
    fn plan_pool_splits_the_budget_into_sessions() {
        // A single core gets one session with one thread.
        assert_eq!(plan_pool(1), (1, 1));
        // One session until there is enough budget for a second one.
        assert_eq!(plan_pool(4), (1, 4));
        assert_eq!(plan_pool(8), (2, 4));
        // 16 cores: four sessions of four, the shape the old hardcoded numbers gave.
        assert_eq!(plan_pool(16), (4, 4));
        // Above the worker cap the extra threads stay inside the sessions.
        assert_eq!(plan_pool(32), (MAX_FACE_WORKERS, 4));
        // A huge budget cannot start more workers than the memory ceiling allows.
        assert_eq!(plan_pool(64).0, MAX_FACE_WORKERS);
    }

    #[test]
    fn thread_budget_zero_means_every_core() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(thread_budget(0), cores);
        assert_eq!(thread_budget(3), 3);
        assert_eq!(thread_budget(MAX_FACE_THREADS + 10), 32);
    }

    /// A pass may only start when both halves exist. The runtime half is the one that
    /// bit: `ort` panics — not errors — when it has to look for the dylib on its own.
    #[test]
    fn engine_ready_requires_the_models_and_the_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");

        let error = engine_ready(dir.path(), dir.path()).expect_err("no models, no runtime");
        assert!(error.contains("buffalo_l"), "{error}");

        let buffalo = dir.path().join("buffalo_l");
        std::fs::create_dir_all(&buffalo).expect("buffalo");
        std::fs::write(buffalo.join("det_10g.onnx"), b"stub").expect("det");
        std::fs::write(buffalo.join("w600k_r50.onnx"), b"stub").expect("rec");

        let error = engine_ready(dir.path(), dir.path()).expect_err("models but no runtime");
        assert!(error.contains("ONNX Runtime"), "{error}");

        std::fs::write(dir.path().join(onnxruntime_lib_name()), b"stub").expect("lib");
        assert!(engine_ready(dir.path(), dir.path()).is_ok());
    }

    #[test]
    fn onnxruntime_lib_name_has_correct_extension() {
        let name = onnxruntime_lib_name();
        assert!(
            name.ends_with(".so") || name.ends_with(".dylib") || name.ends_with(".dll"),
            "unexpected lib name: {name}"
        );
    }

    #[test]
    fn tidx_3d() {
        let shape = vec![1, 3, 4]; // [batch=1, rows=3, cols=4]
        assert_eq!(tidx(&shape, 0, 0), 0);
        assert_eq!(tidx(&shape, 1, 0), 4);
        assert_eq!(tidx(&shape, 2, 3), 11);
    }

    #[test]
    fn tidx_2d() {
        let shape = vec![3, 4]; // [rows=3, cols=4]
        assert_eq!(tidx(&shape, 0, 0), 0);
        assert_eq!(tidx(&shape, 1, 0), 4);
        assert_eq!(tidx(&shape, 2, 3), 11);
    }
}
