//! Filesystem helpers that stay responsive on network mounts.
//!
//! A `stat()` or `read_dir()` on a dead CIFS/SMB mount can block for minutes
//! inside the kernel. The rest of the application must never wait on that
//! when it only needs to know whether a scan root is reachable, so probes run
//! on a worker thread and are abandoned after a short timeout.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Result of a bounded reachability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirProbe {
    pub is_dir: bool,
    /// Reason the probe failed (missing, not a directory, timed out).
    pub error: Option<String>,
}

impl DirProbe {
    fn ok() -> Self {
        Self {
            is_dir: true,
            error: None,
        }
    }

    fn failed(error: impl Into<String>) -> Self {
        Self {
            is_dir: false,
            error: Some(error.into()),
        }
    }
}

/// Check whether `path` is a directory without blocking longer than `timeout`.
///
/// The worker thread is intentionally detached: if the kernel call is stuck on
/// an unreachable mount there is no portable way to cancel it. It finishes (or
/// the process exits) without being joined, which is preferable to freezing the
/// caller's thread forever.
pub fn probe_dir(path: &Path, timeout: Duration) -> DirProbe {
    let (tx, rx) = mpsc::channel();
    let owned = path.to_path_buf();
    std::thread::spawn(move || {
        let result = if owned.is_dir() {
            Ok(())
        } else {
            Err(format!(
                "not a directory or unavailable: {}",
                owned.display()
            ))
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => DirProbe::ok(),
        Ok(Err(error)) => DirProbe::failed(error),
        Err(_) => DirProbe::failed(format!(
            "timed out after {}s waiting for {}",
            timeout.as_secs(),
            path.display()
        )),
    }
}

/// Filesystem type for the mount that contains `path`.
///
/// Used to tell network filesystems (`cifs`, `smb3`, `nfs`, gvfs fuse mounts)
/// apart from local disks when choosing tolerances for change detection.
#[cfg(target_os = "linux")]
pub fn mount_kind(path: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    mount_kind_from(&mounts, &path.to_string_lossy())
}

#[cfg(not(target_os = "linux"))]
pub fn mount_kind(_path: &Path) -> Option<String> {
    // macOS and Windows expose volume information through other APIs; the
    // tolerant-mtime policy falls back to exact comparison there.
    None
}

/// True when `path` lives on a network or FUSE-backed mount.
pub fn is_network_mount(path: &Path) -> bool {
    match mount_kind(path) {
        Some(kind) => {
            let kind = kind.to_ascii_lowercase();
            kind.contains("cifs")
                || kind.contains("smb")
                || kind.starts_with("nfs")
                || kind.contains("fuse")
                || kind.contains("sshfs")
                || kind.contains("davfs")
        }
        None => false,
    }
}

/// Longest-prefix mount lookup over `/proc/mounts` content.
///
/// Kept separate from the file read so it stays unit-testable and works the
/// same on every platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mount_kind_from(mounts: &str, target: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;

    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(_device) = fields.next() else {
            continue;
        };
        let Some(mount_point) = fields.next() else {
            continue;
        };
        let Some(fs_type) = fields.next() else {
            continue;
        };

        let mount_point = unescape_mount_path(mount_point);
        let mount_point = mount_point.trim_end_matches('/');
        let mount_point = if mount_point.is_empty() {
            "/"
        } else {
            mount_point
        };

        let matches = if mount_point == "/" {
            target.starts_with('/')
        } else {
            target == mount_point
                || (target.starts_with(mount_point) && target[mount_point.len()..].starts_with('/'))
        };
        if !matches {
            continue;
        }

        let len = mount_point.len();
        let is_longer = match best.as_ref() {
            Some((best_len, _)) => len > *best_len,
            None => true,
        };
        if is_longer {
            best = Some((len, fs_type.to_string()));
        }
    }

    best.map(|(_, fs_type)| fs_type)
}

/// `/proc/mounts` escapes spaces and a few control characters as octal.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unescape_mount_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
                let value = (digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0');
                out.push(value as char);
                i += 4;
                continue;
            }
        }
        // Copy one whole UTF-8 scalar so multi-byte mount paths stay valid.
        let ch = raw[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_existing_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let probe = probe_dir(dir.path(), Duration::from_secs(5));
        assert!(probe.is_dir);
        assert!(probe.error.is_none());
    }

    #[test]
    fn probe_missing_dir() {
        let probe = probe_dir(
            Path::new("/nonexistent_path_for_probe_test_12345"),
            Duration::from_secs(5),
        );
        assert!(!probe.is_dir);
        assert!(probe.error.is_some());
    }

    #[test]
    fn probe_file_is_not_a_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").expect("write");
        let probe = probe_dir(&file, Duration::from_secs(5));
        assert!(!probe.is_dir);
    }

    #[test]
    fn mount_kind_picks_longest_prefix() {
        let mounts = "\
/dev/sda1 / ext4 rw 0 0
nas:/fotos /mnt/nas nfs4 rw 0 0
//nas/fotos /mnt/nas/fotos cifs rw 0 0
";
        assert_eq!(mount_kind_from(mounts, "/"), Some("ext4".into()));
        assert_eq!(
            mount_kind_from(mounts, "/mnt/nas/fotos/file.jpg"),
            Some("cifs".into())
        );
        assert_eq!(mount_kind_from(mounts, "/mnt/nas"), Some("nfs4".into()));
        assert_eq!(
            mount_kind_from(mounts, "/home/stephan"),
            Some("ext4".into())
        );
    }

    #[test]
    fn mount_kind_handles_gvfs_paths() {
        let mounts = "\
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw,nosuid,nodev 0 0
";
        assert_eq!(
            mount_kind_from(
                mounts,
                "/run/user/1000/gvfs/smb-share:server=nas,share=fotos/Fotos"
            ),
            Some("fuse.gvfsd-fuse".into())
        );
    }

    #[test]
    fn mount_kind_unescapes_spaces() {
        let mounts = "//nas/fotos /mnt/My\\040Photos cifs rw 0 0\n";
        assert_eq!(
            mount_kind_from(mounts, "/mnt/My Photos/a.jpg"),
            Some("cifs".into())
        );
    }

    #[test]
    fn network_mount_detection_on_gvfs_kind() {
        assert!(is_network_mount_from_kind("fuse.gvfsd-fuse"));
        assert!(is_network_mount_from_kind("cifs"));
        assert!(is_network_mount_from_kind("nfs4"));
        assert!(!is_network_mount_from_kind("ext4"));
        assert!(!is_network_mount_from_kind("btrfs"));
    }

    fn is_network_mount_from_kind(kind: &str) -> bool {
        let kind = kind.to_ascii_lowercase();
        kind.contains("cifs")
            || kind.contains("smb")
            || kind.starts_with("nfs")
            || kind.contains("fuse")
            || kind.contains("sshfs")
            || kind.contains("davfs")
    }
}
