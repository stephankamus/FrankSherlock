//! Opt-in conversion of videos a browser cannot play into MP4 (H.264/AAC) copies.
//!
//! Like the HEIC/NEF conversion, this writes next to the originals, into
//! `FrankSherlock-MP4/`, and only after an explicit confirmation in the modal. Unlike it,
//! the copies are *not* indexed (the scan skips the folder): a copy has the same content as
//! its source, so a second row would double the video in every search. The mapping lives in
//! `video_conversions` and the web server serves the copy in place of the original.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sherlock_core::db::{self, VideoConversionRecord, VideoWebCandidate};
use sherlock_core::video_compat::{self, WebPlan};


use crate::platform::process::silent_command;

/// Cap on the error lines kept in the status, so a bad share cannot grow it unbounded.
const MAX_REPORTED_ERRORS: usize = 25;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertOptions {
    /// Tallest output, in pixels, for videos that are re-encoded. `None` keeps the size.
    /// Copies of an H.264 stream are rewrapped and always keep theirs.
    pub max_height: Option<u32>,
    /// x264 constant rate factor: lower is better and bigger. 23 is the encoder default.
    pub crf: u8,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            max_height: Some(1080),
            crf: 23,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelCount {
    pub label: String,
    pub count: u64,
}

/// What the modal shows before the user agrees to write anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWebReport {
    pub total: u64,
    /// Videos that need a full re-encode.
    pub transcode: u64,
    /// Videos that only need their streams rewrapped (fast).
    pub remux: u64,
    pub total_bytes: u64,
    pub total_secs: f64,
    /// Seconds of video that need an encode, the number that drives the time estimate.
    pub transcode_secs: f64,
    pub groups: Vec<LabelCount>,
    pub ffmpeg_ready: bool,
    pub message: String,
}

pub fn build_report(candidates: &[VideoWebCandidate], support: &Support) -> VideoWebReport {
    let mut groups: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let (mut transcode, mut remux) = (0u64, 0u64);
    let (mut total_bytes, mut total_secs, mut transcode_secs) = (0u64, 0.0f64, 0.0f64);
    for candidate in candidates {
        *groups.entry(candidate.plan.label.clone()).or_insert(0) += 1;
        total_bytes += candidate.size_bytes.max(0) as u64;
        total_secs += candidate.duration_secs;
        if candidate.plan.is_remux_only() {
            remux += 1;
        } else {
            transcode += 1;
            transcode_secs += candidate.duration_secs;
        }
    }
    let mut groups: Vec<LabelCount> = groups
        .into_iter()
        .map(|(label, count)| LabelCount { label, count })
        .collect();
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    VideoWebReport {
        total: candidates.len() as u64,
        transcode,
        remux,
        total_bytes,
        total_secs,
        transcode_secs,
        groups,
        ffmpeg_ready: support.ready,
        message: support.message.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct Support {
    pub ready: bool,
    pub message: String,
}

/// ffmpeg must exist and carry the H.264 and AAC encoders; some distro builds do not.
pub fn support() -> Support {
    let Some(ffmpeg) = crate::video::ffmpeg_path() else {
        return Support {
            ready: false,
            message: "ffmpeg was not found. Install ffmpeg to convert videos.".to_string(),
        };
    };
    let output = silent_command(&ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let has = |name: &str| text.lines().any(|line| line.split_whitespace().nth(1) == Some(name));
            if has("libx264") && has("aac") {
                Support {
                    ready: true,
                    message: String::new(),
                }
            } else {
                Support {
                    ready: false,
                    message: "This ffmpeg build has no libx264 or aac encoder.".to_string(),
                }
            }
        }
        Err(e) => Support {
            ready: false,
            message: format!("cannot run ffmpeg: {e}"),
        },
    }
}

/// Arguments for one conversion; separate from the process so they can be tested.
pub fn ffmpeg_args(src: &Path, dst: &Path, plan: &WebPlan, options: &ConvertOptions) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |value: &str| args.push(OsString::from(value));
    for flag in [
        "-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats",
        "-fflags", "+genpts", "-i",
    ] {
        push(flag);
    }
    args.push(src.as_os_str().to_os_string());
    let mut push = |value: &str| args.push(OsString::from(value));
    for flag in ["-map", "0:v:0", "-sn", "-dn", "-map_metadata", "0"] {
        push(flag);
    }
    if plan.has_audio {
        push("-map");
        push("0:a:0");
    } else {
        push("-an");
    }
    if plan.copy_video {
        push("-c:v");
        push("copy");
    } else {
        let filter = match options.max_height {
            Some(height) => format!("scale=-2:'trunc(min(ih,{height})/2)*2'"),
            None => "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string(),
        };
        for flag in ["-c:v", "libx264", "-preset", "veryfast", "-crf"] {
            push(flag);
        }
        push(&options.crf.to_string());
        for flag in ["-pix_fmt", "yuv420p", "-vf"] {
            push(flag);
        }
        push(&filter);
    }
    if plan.has_audio {
        if plan.copy_audio {
            push("-c:a");
            push("copy");
        } else {
            for flag in ["-c:a", "aac", "-b:a", "160k", "-ac", "2"] {
                push(flag);
            }
        }
    }
    for flag in ["-movflags", "+faststart", "-f", "mp4"] {
        push(flag);
    }
    args.push(dst.as_os_str().to_os_string());
    args
}

/// Live state of the job, polled by the modal.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoJobStatus {
    pub running: bool,
    pub root_id: i64,
    pub done: u64,
    pub total: u64,
    /// File being converted, relative to the root.
    pub current: String,
    /// Seconds of video finished plus the progress of the file in flight.
    pub done_secs: f64,
    pub total_secs: f64,
    pub started_at_ms: i64,
    pub converted: u64,
    pub failed: u64,
    pub cancelled: bool,
    pub out_bytes: u64,
    pub errors: Vec<String>,
    /// Set when the job could not start at all (no ffmpeg, ...).
    pub fatal: Option<String>,
}

fn update<F: FnOnce(&mut VideoJobStatus)>(status: &Mutex<VideoJobStatus>, f: F) {
    if let Ok(mut guard) = status.lock() {
        f(&mut guard);
    }
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').filter(|p| !p.is_empty()).fold(root.to_path_buf(), |acc, part| acc.join(part))
}

/// Run ffmpeg for one file, reporting seconds encoded, and rename the result into place.
fn convert_one(
    ffmpeg: &Path,
    src: &Path,
    dst: &Path,
    plan: &WebPlan,
    options: &ConvertOptions,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64),
) -> Result<u64, String> {
    let parent = dst.parent().ok_or_else(|| "output has no folder".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let part = dst.with_file_name(format!(
        "{}.part",
        dst.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
    ));

    let mut child = silent_command(ffmpeg)
        .args(ffmpeg_args(src, &part, plan, options))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))?;

    let stdout = child.stdout.take().ok_or("no ffmpeg stdout")?;
    let (tx, rx) = mpsc::channel::<f64>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(us) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = us.trim().parse::<i64>() {
                    let _ = tx.send(us.max(0) as f64 / 1_000_000.0);
                }
            }
        }
    });
    let mut stderr = child.stderr.take().ok_or("no ffmpeg stderr")?;
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut cancelled = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(secs) => on_progress(secs),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            let _ = child.kill();
            break;
        }
    }
    let status = child.wait().map_err(|e| format!("ffmpeg wait failed: {e}"))?;
    let _ = reader.join();
    let err_text = err_reader.join().unwrap_or_default();

    if cancelled {
        let _ = std::fs::remove_file(&part);
        return Err("cancelled".to_string());
    }
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        let tail: String = err_text.trim().lines().last().unwrap_or("ffmpeg failed").to_string();
        return Err(tail);
    }
    std::fs::rename(&part, dst).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("cannot move the copy into place: {e}")
    })?;
    Ok(std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0))
}

/// Convert `candidates` one after another, recording each finished copy in the database.
pub fn run_conversion(
    db_path: &Path,
    root_id: i64,
    root_path: &Path,
    candidates: &[VideoWebCandidate],
    options: &ConvertOptions,
    cancel: &AtomicBool,
    status: &Mutex<VideoJobStatus>,
) {
    let Some(ffmpeg) = crate::video::ffmpeg_path() else {
        update(status, |s| {
            s.running = false;
            s.fatal = Some("ffmpeg was not found".to_string());
        });
        return;
    };

    let mut finished_secs = 0.0f64;
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            update(status, |s| s.cancelled = true);
            break;
        }
        let weight = candidate.duration_secs.max(1.0);
        update(status, |s| s.current = candidate.rel_path.clone());

        let src = join_rel(root_path, &candidate.rel_path);
        let out_rel = video_compat::output_rel_path(&candidate.rel_path);
        let dst = join_rel(root_path, &out_rel);

        let result = if !src.is_file() {
            Err("source not found".to_string())
        } else {
            convert_one(&ffmpeg, &src, &dst, &candidate.plan, options, cancel, |secs| {
                let partial = secs.min(weight);
                update(status, |s| s.done_secs = finished_secs + partial);
            })
        };

        match result {
            Ok(bytes) => {
                let recorded = db::upsert_video_conversion(
                    db_path,
                    &VideoConversionRecord {
                        root_id,
                        file_id: candidate.file_id,
                        output_rel_path: &out_rel,
                        source_mtime_ns: candidate.mtime_ns,
                        source_size_bytes: candidate.size_bytes,
                        output_size_bytes: bytes as i64,
                    },
                );
                match recorded {
                    Ok(()) => update(status, |s| {
                        s.converted += 1;
                        s.out_bytes += bytes;
                    }),
                    Err(e) => fail(status, &candidate.rel_path, &e.to_string()),
                }
            }
            Err(e) if e == "cancelled" => {
                update(status, |s| s.cancelled = true);
                break;
            }
            Err(e) => {
                log::warn!("video conversion failed for {}: {e}", candidate.rel_path);
                fail(status, &candidate.rel_path, &e);
            }
        }
        finished_secs += weight;
        update(status, |s| {
            s.done += 1;
            s.done_secs = finished_secs;
        });
    }
    update(status, |s| {
        s.running = false;
        s.current.clear();
    });
}

fn fail(status: &Mutex<VideoJobStatus>, rel_path: &str, error: &str) {
    update(status, |s| {
        s.failed += 1;
        if s.errors.len() < MAX_REPORTED_ERRORS {
            s.errors.push(format!("{rel_path}: {error}"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sherlock_core::video_compat::web_plan;

    fn args_of(ext: &str, video: &str, audio: Option<&str>, options: ConvertOptions) -> Vec<String> {
        let plan = web_plan(ext, Some(video), audio).expect("plan");
        ffmpeg_args(Path::new("/in/a.src"), Path::new("/out/a.mp4.part"), &plan, &options)
            .into_iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    fn has_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == key && w[1] == value)
    }

    #[test]
    fn hevc_is_encoded_to_h264_and_capped_in_height() {
        let args = args_of("mp4", "hevc", Some("aac"), ConvertOptions::default());
        assert!(has_pair(&args, "-c:v", "libx264"));
        assert!(has_pair(&args, "-c:a", "copy"));
        assert!(has_pair(&args, "-crf", "23"));
        assert!(has_pair(&args, "-vf", "scale=-2:'trunc(min(ih,1080)/2)*2'"));
        assert!(has_pair(&args, "-movflags", "+faststart"));
        assert_eq!(args.last().map(String::as_str), Some("/out/a.mp4.part"));
    }

    #[test]
    fn original_height_still_forces_even_dimensions() {
        let options = ConvertOptions { max_height: None, crf: 20 };
        let args = args_of("avi", "mjpeg", Some("pcm_s16le"), options);
        assert!(has_pair(&args, "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"));
        assert!(has_pair(&args, "-c:a", "aac"));
        assert!(has_pair(&args, "-crf", "20"));
    }

    #[test]
    fn h264_in_a_legacy_container_is_copied_not_reencoded() {
        let args = args_of("flv", "h264", Some("aac"), ConvertOptions::default());
        assert!(has_pair(&args, "-c:v", "copy"));
        assert!(has_pair(&args, "-c:a", "copy"));
        assert!(!args.iter().any(|a| a == "-vf" || a == "libx264"));
    }

    #[test]
    fn a_video_without_audio_maps_no_audio() {
        let args = args_of("mpg", "mpeg1video", None, ConvertOptions::default());
        assert!(args.iter().any(|a| a == "-an"));
        assert!(!has_pair(&args, "-map", "0:a:0"));
    }

    #[test]
    fn report_counts_remux_and_transcode_separately() {
        let candidate = |rel: &str, ext: &str, v: &str, secs: f64| VideoWebCandidate {
            file_id: 1,
            rel_path: rel.to_string(),
            abs_path: format!("/r/{rel}"),
            size_bytes: 1000,
            mtime_ns: 1,
            duration_secs: secs,
            video_codec: v.to_string(),
            audio_codec: Some("aac".to_string()),
            plan: web_plan(ext, Some(v), Some("aac")).expect("plan"),
        };
        let candidates = vec![
            candidate("a.mp4", "mp4", "hevc", 60.0),
            candidate("b.mp4", "mp4", "hevc", 30.0),
            candidate("c.flv", "flv", "h264", 10.0),
        ];
        let report = build_report(&candidates, &Support { ready: true, message: String::new() });
        assert_eq!((report.total, report.transcode, report.remux), (3, 2, 1));
        assert_eq!(report.transcode_secs, 90.0);
        assert_eq!(report.total_secs, 100.0);
        assert_eq!(report.groups[0].label, ".mp4 hevc");
        assert_eq!(report.groups[0].count, 2);
    }

    #[test]
    fn join_rel_builds_a_native_path() {
        let path = join_rel(Path::new("/root"), "Trip/FrankSherlock-MP4/x.mp4");
        assert_eq!(path, Path::new("/root").join("Trip").join("FrankSherlock-MP4").join("x.mp4"));
    }

    #[test]
    fn a_real_conversion_produces_a_playable_h264_copy() {
        let Some(ffmpeg) = crate::video::ffmpeg_path() else {
            return;
        };
        if !support().ready {
            return;
        }
        let dir = tempfile::tempdir().expect("tmp");
        let src = dir.path().join("src.avi");
        let made = silent_command(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=10:duration=1", "-c:v", "mjpeg", "-y"])
            .arg(&src)
            .status();
        if !matches!(made, Ok(s) if s.success()) {
            return;
        }
        let plan = web_plan("avi", Some("mjpeg"), None).expect("plan");
        let dst = dir.path().join("out").join("src__avi.mp4");
        let cancel = AtomicBool::new(false);
        let mut last = 0.0;
        let bytes = convert_one(&ffmpeg, &src, &dst, &plan, &ConvertOptions::default(), &cancel, |s| last = s)
            .expect("convert");
        assert!(bytes > 0 && dst.is_file());
        assert!(!dst.with_file_name("src__avi.mp4.part").exists());
        let meta = crate::video::extract_metadata(&dst).expect("metadata");
        assert_eq!(meta.video_codec.as_deref(), Some("h264"));
        assert!(last >= 0.0);
    }
}
