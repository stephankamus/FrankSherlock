//! Live view of the application log file.
//!
//! `tauri_plugin_log` already writes every `log::info!`/`warn!`/`error!` to
//! `<app_log_dir>/<product>.log`, and that file is the same evidence the terminal
//! shows while a scan runs. Instead of adding a second logger (which would have to
//! mirror every target and level), the UI polls a window of the end of that file.
//! Polling also means the history is there when the modal opens, not only the lines
//! emitted after it — which is what makes it useful while classifying a folder.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tauri::Manager;

use crate::models::{LogEntry, LogSnapshot};

/// Lines returned when the caller does not ask for a specific count.
pub const DEFAULT_MAX_LINES: usize = 600;
/// Ceiling for `max_lines`: the modal is a viewer, not an export tool.
pub const MAX_LINES_LIMIT: usize = 5_000;
/// How much of the file tail is read per poll. A scan can log one line per file,
/// so 512 KiB holds a useful window without reading a 16 MiB file every second.
const TAIL_BYTES: u64 = 512 * 1024;

/// Snapshot of the newest log file of the application.
///
/// `max_lines` is applied after parsing, so multi-line messages still arrive
/// whole; `path` is `None` until the first line is written.
#[tauri::command]
pub fn read_logs(app: tauri::AppHandle, max_lines: Option<usize>) -> Result<LogSnapshot, String> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("cannot resolve the log directory: {e}"))?;
    let Some(path) = newest_log_file(&dir) else {
        return Ok(LogSnapshot {
            path: None,
            entries: Vec::new(),
            truncated: false,
            size_bytes: 0,
        });
    };

    let max_lines = max_lines
        .unwrap_or(DEFAULT_MAX_LINES)
        .clamp(1, MAX_LINES_LIMIT);
    let (text, truncated, size_bytes) = read_tail(&path, TAIL_BYTES)?;
    Ok(LogSnapshot {
        path: Some(path.display().to_string()),
        entries: parse_entries(&text, max_lines),
        truncated,
        size_bytes,
    })
}

/// The active file is always the one appended to last (rotation renames the old
/// one and creates a new active file), so modification time picks it out without
/// assuming the product name.
fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        let is_newer = match best.as_ref() {
            Some((current, _)) => modified > *current,
            None => true,
        };
        if is_newer {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

/// Reads at most `max_bytes` from the end of the file.
///
/// The first line after a mid-file seek is dropped (it is cut in half), and so is
/// a trailing line with no newline: that one is still being flushed and printing
/// it now would show a mangled record on every poll except the next.
fn read_tail(path: &Path, max_bytes: u64) -> Result<(String, bool, u64), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot open the log file: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("cannot stat the log file: {e}"))?
        .len();
    let start = len.saturating_sub(max_bytes);
    let truncated = start > 0;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("cannot seek the log file: {e}"))?;
    }

    // `take` + `read_to_end` instead of a fixed-size `read_exact`: rotation can
    // truncate the file between the stat and the read, and a short read should
    // return what is there instead of failing the whole poll.
    let mut buf = Vec::new();
    file.take(max_bytes)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read the log file: {e}"))?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();

    if truncated {
        if let Some(idx) = text.find('\n') {
            text.drain(..=idx);
        } else {
            text.clear();
        }
    }
    if !text.ends_with('\n') {
        match text.rfind('\n') {
            Some(idx) => text.truncate(idx + 1),
            None => text.clear(),
        }
    }

    Ok((text, truncated, len))
}

/// Turns the plugin's line format into entries.
///
/// Desktop records look like `[2026-10-27][17:17:15][target][INFO] message`.
/// A message can contain newlines; those continuation lines do not parse and are
/// appended to the previous entry instead of being dropped.
fn parse_entries(text: &str, max_lines: usize) -> Vec<LogEntry> {
    let mut entries: Vec<LogEntry> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(entry) => entries.push(entry),
            None => match entries.last_mut() {
                Some(last) => {
                    last.message.push('\n');
                    last.message.push_str(line);
                }
                None => entries.push(LogEntry {
                    date: String::new(),
                    time: String::new(),
                    target: String::new(),
                    level: "UNKNOWN".to_string(),
                    message: line.to_string(),
                }),
            },
        }
    }

    if entries.len() > max_lines {
        entries.drain(..entries.len() - max_lines);
    }
    entries
}

fn parse_line(line: &str) -> Option<LogEntry> {
    let rest = line.strip_prefix('[')?;
    let (date, rest) = rest.split_once("][")?;
    let (time, rest) = rest.split_once("][")?;
    let (target, rest) = rest.split_once("][")?;
    let (level, message) = rest.split_once("] ")?;
    Some(LogEntry {
        date: date.to_string(),
        time: time.to_string(),
        target: target.to_string(),
        level: level.to_string(),
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_plugin_line_format() {
        let entry = parse_line(
            "[2026-10-27][17:17:15][sherlock_app_lib::classify][INFO] Classifying: /photos/a.jpg",
        )
        .expect("parsed");

        assert_eq!(entry.date, "2026-10-27");
        assert_eq!(entry.time, "17:17:15");
        assert_eq!(entry.target, "sherlock_app_lib::classify");
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "Classifying: /photos/a.jpg");
    }

    #[test]
    fn a_message_with_brackets_keeps_the_rest_intact() {
        let entry =
            parse_line("[2026-10-27][10:00:00][app][WARN] row [42] failed: [x]").expect("parsed");
        assert_eq!(entry.level, "WARN");
        assert_eq!(entry.message, "row [42] failed: [x]");
    }

    #[test]
    fn continuation_lines_stay_with_the_previous_entry() {
        let text = "\
[2026-10-27][10:00:00][app][ERROR] first line
    second line
[2026-10-27][10:00:01][app][INFO] next
";
        let entries = parse_entries(text, 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "first line\n    second line");
        assert_eq!(entries[1].message, "next");
    }

    #[test]
    fn returns_only_the_last_lines_requested() {
        let text = (0..10)
            .map(|i| format!("[2026-10-27][10:00:0{i}][app][INFO] line {i}\n"))
            .collect::<String>();
        let entries = parse_entries(&text, 3);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "line 7");
        assert_eq!(entries[2].message, "line 9");
    }

    #[test]
    fn garbage_without_a_previous_entry_is_still_shown() {
        let entries = parse_entries("not a log line\n", 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, "UNKNOWN");
        assert_eq!(entries[0].message, "not a log line");
    }

    #[test]
    fn read_tail_drops_the_cut_first_line_and_the_unflushed_last_one() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("app.log");
        std::fs::write(
            &path,
            "[2026-10-27][10:00:00][app][INFO] aaa\n\
             [2026-10-27][10:00:01][app][INFO] bbb\n\
             [2026-10-27][10:00:02][app][INFO] cc",
        )
        .expect("write");

        // 76 bytes leaves the start inside the first line (both complete records
        // are 38 bytes), so that line is dropped; the third record has no newline
        // yet and must be dropped too.
        let (text, truncated, len) = read_tail(&path, 76).expect("tail");
        assert!(truncated);
        assert!(len > 76);
        assert_eq!(text, "[2026-10-27][10:00:01][app][INFO] bbb\n");
    }

    #[test]
    fn newest_log_file_prefers_the_file_being_appended_to() {
        let dir = tempfile::tempdir().expect("dir");
        let older = dir.path().join("frank_sherlock_2026-01-01.log");
        let active = dir.path().join("frank_sherlock.log");
        let other = dir.path().join("notes.txt");
        std::fs::write(&older, "old\n").expect("older");
        std::fs::write(&active, "new\n").expect("active");
        std::fs::write(&other, "ignore me\n").expect("other");
        // Make the active file unambiguously newer than the rotated one.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&active, "newer\n").expect("touch active");

        assert_eq!(newest_log_file(dir.path()), Some(active));
    }
}
