//! The published library: which revision is live and where its files are.
//!
//! The desktop replaces `current.json` atomically after it has uploaded a snapshot, so
//! this module only has to notice that the pointer changed. It stats the file on every
//! read (cheap, one syscall) and re-parses it when either its mtime or its size moved —
//! no watcher, no polling thread, and a publish that happens while the server is idle is
//! picked up by the next request.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::Deserialize;

pub const CURRENT_POINTER: &str = "current.json";
pub const THUMBS_DIR: &str = "thumbs";
pub const FACE_CROPS_DIR: &str = "face_crops";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pointer {
    #[serde(default)]
    pub format: u32,
    pub revision: String,
    #[serde(default)]
    pub generated_at_unix: i64,
    #[serde(default)]
    pub schema_version: u32,
    pub database: DatabasePointer,
    #[serde(default)]
    pub library: LibraryCounts,
    #[serde(default)]
    pub thumbnails: DirPointer,
    #[serde(default)]
    pub face_crops: DirPointer,
    #[serde(default)]
    pub roots: Vec<PointerRoot>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabasePointer {
    pub file: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCounts {
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub roots: u64,
    #[serde(default)]
    pub hidden_files: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirPointer {
    #[serde(default)]
    pub dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerRoot {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub root_key: String,
    #[serde(default)]
    pub files: u64,
}

/// A pointer plus the absolute paths derived from it.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub pointer: Pointer,
    pub db_path: PathBuf,
    pub thumbnails_dir: PathBuf,
    pub face_crops_dir: Option<PathBuf>,
}

impl Snapshot {
    pub fn root(&self, root_id: i64) -> Option<&PointerRoot> {
        self.pointer.roots.iter().find(|root| root.id == root_id)
    }
}

#[derive(Debug)]
pub struct Library {
    dir: PathBuf,
    cached: RwLock<Option<(Fingerprint, Snapshot)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    modified_secs: u64,
    len: u64,
}

impl Library {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cached: RwLock::new(None),
        }
    }

    /// The live snapshot, re-read when the pointer moved.
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let pointer_path = self.dir.join(CURRENT_POINTER);
        let metadata = std::fs::metadata(&pointer_path).map_err(|e| {
            format!(
                "no published library at {}: {e} (has the desktop published at least once?)",
                pointer_path.display()
            )
        })?;
        let fingerprint = Fingerprint {
            modified_secs: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            len: metadata.len(),
        };

        if let Ok(guard) = self.cached.read() {
            if let Some((cached_for, snapshot)) = guard.as_ref() {
                if *cached_for == fingerprint {
                    return Ok(snapshot.clone());
                }
            }
        }

        let raw = std::fs::read(&pointer_path)
            .map_err(|e| format!("could not read {}: {e}", pointer_path.display()))?;
        let pointer: Pointer = serde_json::from_slice(&raw)
            .map_err(|e| format!("{} is not a valid pointer: {e}", pointer_path.display()))?;
        let snapshot = self.build(pointer)?;

        if let Ok(mut guard) = self.cached.write() {
            *guard = Some((fingerprint, snapshot.clone()));
        }
        Ok(snapshot)
    }

    fn build(&self, pointer: Pointer) -> Result<Snapshot, String> {
        let db_path = self.dir.join(&pointer.database.file);
        if !db_path.is_file() {
            return Err(format!(
                "revision {} points at {}, which is not there",
                pointer.revision,
                db_path.display()
            ));
        }
        let thumbnails_dir = self
            .dir
            .join(dir_name_or(&pointer.thumbnails.dir, THUMBS_DIR));
        let face_crops_dir = if pointer.face_crops.dir.trim().is_empty() {
            None
        } else {
            Some(self.dir.join(&pointer.face_crops.dir))
        };
        Ok(Snapshot {
            pointer,
            db_path,
            thumbnails_dir,
            face_crops_dir,
        })
    }

    /// `current.json` as published, without opening the database.
    pub fn pointer(&self) -> Result<Pointer, String> {
        self.snapshot().map(|snapshot| snapshot.pointer)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn dir_name_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pointer(dir: &Path, revision: &str, db_file: &str) -> std::io::Result<()> {
        std::fs::write(dir.join(db_file), b"sqlite")?;
        let pointer = serde_json::json!({
            "format": 1,
            "revision": revision,
            "generatedAtUnix": 1_700_000_000,
            "schemaVersion": 21,
            "database": { "file": db_file, "bytes": 6, "sha256": "00" },
            "library": { "files": 3, "roots": 1, "hiddenFiles": 1 },
            "thumbnails": { "dir": "thumbs" },
            "faceCrops": { "dir": "" },
            "roots": [{ "id": 1, "name": "Fotos", "path": "/stored", "rootKey": "abc", "files": 3 }],
        });
        std::fs::write(dir.join(CURRENT_POINTER), serde_json::to_vec(&pointer)?)
    }

    #[test]
    fn reads_the_pointer_and_derives_the_paths() {
        let temp = tempfile::tempdir().expect("temp");
        write_pointer(temp.path(), "rev-001", "index.rev-001.sqlite").expect("pointer");
        let library = Library::new(temp.path().to_path_buf());

        let snapshot = library.snapshot().expect("snapshot");
        assert_eq!(snapshot.pointer.revision, "rev-001");
        assert_eq!(snapshot.pointer.library.hidden_files, 1);
        assert_eq!(snapshot.db_path, temp.path().join("index.rev-001.sqlite"));
        assert_eq!(snapshot.thumbnails_dir, temp.path().join("thumbs"));
        assert_eq!(
            snapshot.face_crops_dir, None,
            "an empty dir means crops were not published"
        );
        assert_eq!(snapshot.root(1).expect("root").root_key, "abc");
    }

    #[test]
    fn follows_a_new_revision_without_a_restart() {
        let temp = tempfile::tempdir().expect("temp");
        write_pointer(temp.path(), "rev-001", "index.rev-001.sqlite").expect("pointer");
        let library = Library::new(temp.path().to_path_buf());
        assert_eq!(
            library.snapshot().expect("first").pointer.revision,
            "rev-001"
        );

        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_pointer(temp.path(), "rev-002", "index.rev-002.sqlite").expect("pointer");

        let second = library.snapshot().expect("second");
        assert_eq!(second.pointer.revision, "rev-002");
        assert_eq!(second.db_path, temp.path().join("index.rev-002.sqlite"));
    }

    #[test]
    fn a_pointer_to_a_missing_snapshot_is_an_error() {
        let temp = tempfile::tempdir().expect("temp");
        let pointer = serde_json::json!({
            "revision": "rev-001",
            "database": { "file": "index.missing.sqlite" },
        });
        std::fs::write(
            temp.path().join(CURRENT_POINTER),
            serde_json::to_vec(&pointer).unwrap(),
        )
        .expect("pointer");
        let error = Library::new(temp.path().to_path_buf())
            .snapshot()
            .expect_err("must fail");
        assert!(error.contains("is not there"), "{error}");
    }

    #[test]
    fn a_missing_pointer_explains_that_nothing_was_published() {
        let temp = tempfile::tempdir().expect("temp");
        let error = Library::new(temp.path().to_path_buf())
            .snapshot()
            .expect_err("must fail");
        assert!(error.contains("has the desktop published"), "{error}");
    }
}
