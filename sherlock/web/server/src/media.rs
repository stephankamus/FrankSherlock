//! Mapping from the paths the desktop recorded to the paths this container can open.
//!
//! The snapshot keeps the absolute paths of the machine that scanned the library
//! (`roots.root_path`, `files.rel_path`). Those paths do not exist in the container, and
//! they must not be trusted even if they did: every path this module produces comes from
//! a mapping the operator wrote plus a `rel_path` that is validated here.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapEntry {
    /// The root as the desktop recorded it.
    stored_path: String,
    /// Where that tree is mounted in this container.
    mounted_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct MediaMap {
    /// Longest `stored_path` first, so a root nested in another resolves to itself.
    entries: Vec<(String, PathBuf)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// The root exists in the index but no mount was configured for it.
    RootUnmapped(String),
    /// The relative path would escape its mount.
    Escapes(String),
    /// The file is not on disk (deleted, or a share that is not mounted).
    Missing(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::RootUnmapped(path) => write!(
                f,
                "no MEDIA_MAP entry for the root {path}: add it to the media map"
            ),
            MediaError::Escapes(path) => write!(f, "path escapes its mount: {path}"),
            MediaError::Missing(path) => write!(f, "not available on this host: {path}"),
        }
    }
}

impl MediaMap {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Read and validate `<storedPath, mountedPath>` pairs.
    ///
    /// Every mount must exist now: a mapping that points at a directory the container
    /// does not have is a typo, and failing at boot is the only way it gets noticed.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read MEDIA_MAP {}: {e}", path.display()))?;
        Self::from_json(&raw)
    }

    pub fn from_json(raw: &str) -> Result<Self, String> {
        let entries: Vec<MapEntry> = serde_json::from_str(raw)
            .map_err(|e| format!("MEDIA_MAP is not a JSON array of storedPath/mountedPath: {e}"))?;

        let mut pairs = Vec::with_capacity(entries.len());
        for entry in entries {
            let stored = normalize_stored(&entry.stored_path);
            if stored.is_empty() {
                return Err("MEDIA_MAP has an entry with an empty storedPath".to_string());
            }
            let mounted = PathBuf::from(&entry.mounted_path);
            if !mounted.is_dir() {
                return Err(format!(
                    "MEDIA_MAP maps {stored} to {}, which is not a directory in this container",
                    mounted.display()
                ));
            }
            pairs.push((stored, mounted));
        }
        // Longest first: `/mnt/nas` and `/mnt/nas/Anime` both match the second root.
        pairs.sort_by_key(|(stored, _)| std::cmp::Reverse(stored.len()));
        Ok(Self { entries: pairs })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Mount configured for a stored root path, if any.
    pub fn mount_for(&self, stored_root: &str) -> Option<&PathBuf> {
        let stored = normalize_stored(stored_root);
        self.entries
            .iter()
            .find(|(prefix, _)| stored == *prefix)
            .map(|(_, mounted)| mounted)
    }

    /// Full path on this host for a row, refusing anything that escapes the mount.
    pub fn resolve(&self, stored_root: &str, rel_path: &str) -> Result<PathBuf, MediaError> {
        let mount = self
            .mount_for(stored_root)
            .ok_or_else(|| MediaError::RootUnmapped(stored_root.to_string()))?;

        let rel = sherlock_core::paths::normalize_rel_path(rel_path);
        let mut joined = mount.clone();
        for component in Path::new(&rel).components() {
            match component {
                Component::Normal(part) => joined.push(part),
                Component::CurDir => {}
                // `rel_path` comes from the database, but a row written by a buggy or
                // older version must not be able to reach `/etc`.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(MediaError::Escapes(rel_path.to_string()))
                }
            }
        }
        Ok(joined)
    }
}

fn normalize_stored(raw: &str) -> String {
    let normalized = sherlock_core::paths::normalize_rel_path(raw.trim());
    normalized.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(json: &str) -> MediaMap {
        // `from_json` requires existing directories; the tests below use temp dirs, so
        // this helper only builds the entries for the pure resolution logic.
        let entries: Vec<MapEntry> = serde_json::from_str(json).expect("json");
        let mut pairs: Vec<(String, PathBuf)> = entries
            .into_iter()
            .map(|entry| {
                (
                    normalize_stored(&entry.stored_path),
                    PathBuf::from(entry.mounted_path),
                )
            })
            .collect();
        pairs.sort_by_key(|(stored, _)| std::cmp::Reverse(stored.len()));
        MediaMap { entries: pairs }
    }

    #[test]
    fn resolves_a_root_to_its_mount() {
        let map = map(r#"[{"storedPath":"/home/user/Fotos","mountedPath":"/mnt/fotos"}]"#);
        assert_eq!(
            map.resolve("/home/user/Fotos", "2024/IMG_1.jpg")
                .expect("resolve"),
            PathBuf::from("/mnt/fotos/2024/IMG_1.jpg")
        );
    }

    #[test]
    fn prefers_the_most_specific_root() {
        let map = map(r#"[{"storedPath":"/mnt/nas","mountedPath":"/mnt/nas"},
                {"storedPath":"/mnt/nas/Anime","mountedPath":"/mnt/anime"}]"#);
        assert_eq!(
            map.resolve("/mnt/nas/Anime", "ep1.jpg").expect("resolve"),
            PathBuf::from("/mnt/anime/ep1.jpg")
        );
    }

    #[test]
    fn reports_an_unmapped_root_instead_of_guessing() {
        let map = map(r#"[{"storedPath":"/a","mountedPath":"/mnt/a"}]"#);
        assert_eq!(
            map.resolve("/b", "x.jpg").unwrap_err(),
            MediaError::RootUnmapped("/b".to_string())
        );
    }

    #[test]
    fn refuses_a_relative_path_that_climbs_out() {
        let map = map(r#"[{"storedPath":"/a","mountedPath":"/mnt/a"}]"#);
        assert!(matches!(
            map.resolve("/a", "../../etc/passwd").unwrap_err(),
            MediaError::Escapes(_)
        ));
    }

    #[test]
    fn refuses_an_absolute_rel_path() {
        let map = map(r#"[{"storedPath":"/a","mountedPath":"/mnt/a"}]"#);
        assert!(matches!(
            map.resolve("/a", "/etc/passwd").unwrap_err(),
            MediaError::Escapes(_)
        ));
    }

    #[test]
    fn tolerates_a_trailing_slash_on_the_stored_path() {
        let map = map(r#"[{"storedPath":"/a/","mountedPath":"/mnt/a"}]"#);
        assert!(map.mount_for("/a").is_some());
    }
}
