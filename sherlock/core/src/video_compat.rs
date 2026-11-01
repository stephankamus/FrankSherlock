//! Which videos a browser can play, and what it takes to make the others playable.
//!
//! The desktop plays whatever its webview decodes, but the web client has to live with
//! what Chrome, Firefox and Safari agree on: H.264/VP8/VP9/AV1 video, AAC/MP3/Opus/Vorbis
//! audio, in an MP4 or WebM container. Everything else (HEVC, MPEG-1/4, WMV, MJPEG, PCM
//! audio, AVI/MOV/FLV/MPG containers) needs a converted copy.

use serde::Serialize;

const PLAYABLE_CONTAINERS: [&str; 3] = ["mp4", "m4v", "webm"];
const PLAYABLE_VIDEO: [&str; 4] = ["h264", "vp8", "vp9", "av1"];
const PLAYABLE_AUDIO: [&str; 4] = ["aac", "mp3", "opus", "vorbis"];

/// How to build the browser-friendly copy of one video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebPlan {
    /// The video stream is already H.264: copy it instead of re-encoding.
    pub copy_video: bool,
    /// The audio stream is already AAC/MP3: copy it instead of re-encoding.
    pub copy_audio: bool,
    /// The source has an audio stream at all.
    pub has_audio: bool,
    /// Short human label of what is incompatible, e.g. `.avi mjpeg`.
    pub label: String,
}

impl WebPlan {
    /// A copy that only rewraps the streams is orders of magnitude faster than an encode.
    pub fn is_remux_only(&self) -> bool {
        self.copy_video && (self.copy_audio || !self.has_audio)
    }
}

fn norm(value: Option<&str>) -> String {
    value.unwrap_or("").trim().to_ascii_lowercase()
}

/// Plan for a video that a browser cannot play, or `None` when it needs no copy.
///
/// `None` also covers a video whose codec is unknown (ffprobe could not read it): a copy
/// of a file we cannot describe would be a guess, and it would not be listed as pending
/// forever either.
pub fn web_plan(ext: &str, video_codec: Option<&str>, audio_codec: Option<&str>) -> Option<WebPlan> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let video = norm(video_codec);
    let audio = norm(audio_codec);
    if video.is_empty() {
        return None;
    }
    let has_audio = !audio.is_empty();

    let container_ok = PLAYABLE_CONTAINERS.contains(&ext.as_str());
    let video_ok = PLAYABLE_VIDEO.contains(&video.as_str());
    let audio_ok = !has_audio || PLAYABLE_AUDIO.contains(&audio.as_str());
    if container_ok && video_ok && audio_ok {
        return None;
    }

    // Only H.264 is rewrapped as is: it is the one codec every browser decodes that the
    // MP4 output can carry from any of the legacy containers.
    let copy_video = video == "h264";
    let copy_audio = has_audio && (audio == "aac" || audio == "mp3");
    let label = if !video_ok {
        format!(".{ext} {video}")
    } else if !audio_ok {
        format!(".{ext} {audio} audio")
    } else {
        format!(".{ext} container")
    };
    Some(WebPlan {
        copy_video,
        copy_audio,
        has_audio,
        label,
    })
}

/// Name of the folder that holds the copies, next to the originals.
pub const OUTPUT_DIR_NAME: &str = "FrankSherlock-MP4";

/// Path of the copy, relative to the root: `<dir>/FrankSherlock-MP4/<stem>__<ext>.mp4`.
///
/// A pure function of the source, so the job is idempotent, and the `__<ext>` suffix keeps
/// `clip.avi` and `clip.mov` from sharing a copy.
pub fn output_rel_path(source_rel_path: &str) -> String {
    let normalized = crate::paths::normalize_rel_path(source_rel_path);
    let (dir, file) = match normalized.rfind('/') {
        Some(idx) => (&normalized[..idx], &normalized[idx + 1..]),
        None => ("", normalized.as_str()),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(idx) if idx > 0 => (&file[..idx], file[idx + 1..].to_ascii_lowercase()),
        _ => (file, "video".to_string()),
    };
    let name = format!("{stem}__{ext}.mp4");
    if dir.is_empty() {
        format!("{OUTPUT_DIR_NAME}/{name}")
    } else {
        format!("{dir}/{OUTPUT_DIR_NAME}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainstream_mp4_needs_no_copy() {
        assert!(web_plan("mp4", Some("h264"), Some("aac")).is_none());
        assert!(web_plan("MP4", Some("h264"), None).is_none());
        assert!(web_plan("webm", Some("vp9"), Some("opus")).is_none());
        assert!(web_plan("mp4", Some("vp9"), Some("aac")).is_none());
    }

    #[test]
    fn hevc_in_mp4_is_a_full_transcode() {
        let plan = web_plan("mp4", Some("hevc"), Some("aac")).expect("plan");
        assert!(!plan.copy_video);
        assert!(plan.copy_audio);
        assert!(!plan.is_remux_only());
        assert_eq!(plan.label, ".mp4 hevc");
    }

    #[test]
    fn h264_in_a_legacy_container_is_only_rewrapped() {
        let plan = web_plan("flv", Some("h264"), Some("aac")).expect("plan");
        assert!(plan.is_remux_only());
        assert_eq!(plan.label, ".flv container");

        let mpg = web_plan("mpg", Some("h264"), Some("aac")).expect("plan");
        assert!(mpg.is_remux_only());
    }

    #[test]
    fn pcm_audio_is_reencoded_but_the_video_is_kept() {
        let plan = web_plan("mov", Some("h264"), Some("pcm_s16le")).expect("plan");
        assert!(plan.copy_video);
        assert!(!plan.copy_audio);
        assert!(plan.has_audio);
        assert!(!plan.is_remux_only());
        assert_eq!(plan.label, ".mov pcm_s16le audio");

        let in_mp4 = web_plan("mp4", Some("h264"), Some("pcm_s16le")).expect("plan");
        assert_eq!(in_mp4.label, ".mp4 pcm_s16le audio");
    }

    #[test]
    fn old_codecs_are_transcoded() {
        for (ext, v, a) in [
            ("mpg", "mpeg1video", "mp2"),
            ("avi", "mjpeg", "pcm_s16le"),
            ("wmv", "wmv2", "wmav2"),
            ("mp4", "mpeg4", "aac"),
        ] {
            let plan = web_plan(ext, Some(v), Some(a)).expect("plan");
            assert!(!plan.copy_video, "{ext} {v}");
        }
    }

    #[test]
    fn an_unreadable_video_gets_no_plan() {
        assert!(web_plan("mp4", None, None).is_none());
        assert!(web_plan("avi", Some(""), Some("aac")).is_none());
    }

    #[test]
    fn output_path_is_next_to_the_source_and_keeps_the_extension() {
        assert_eq!(
            output_rel_path("Trip/clip.AVI"),
            "Trip/FrankSherlock-MP4/clip__avi.mp4"
        );
        assert_eq!(
            output_rel_path("clip.mov"),
            "FrankSherlock-MP4/clip__mov.mp4"
        );
        assert_eq!(
            output_rel_path("a\\b\\v.mp4"),
            "a/b/FrankSherlock-MP4/v__mp4.mp4"
        );
        assert_ne!(output_rel_path("x.avi"), output_rel_path("x.mov"));
    }
}
