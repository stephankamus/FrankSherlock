//! Where a thumbnail lives, which is part of the index format.
//!
//! The desktop writes the files and `sherlock_app_lib::thumbnail` owns the encoding, but
//! the path is derived from the root path and the row's `rel_path` — the web server has
//! to compute exactly the same one to serve it, so the rule lives here.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::paths::normalize_rel_path;

/// Directory prefix of the thumbnail cache that belongs to one root.
///
/// Derived from the canonical root path instead of `root_id`: removing a root and
/// adding it back reuses the cache, and two roots can never resolve to the same
/// file — which is what allowed one root's purge to delete another root's thumbnails.
pub fn root_key(root_path: &str) -> String {
    let digest = Sha256::digest(normalize_rel_path(root_path).as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Canonical path of the thumbnail for `rel_path` under `<thumbnails>/<root_key>`.
pub fn thumb_path_for(thumbnails_dir: &Path, root_key: &str, rel_path: &str) -> PathBuf {
    thumb_path_in(&thumbnails_dir.join(root_key), rel_path)
}

/// Thumbnail path for `rel_path` inside an already root-scoped directory.
pub fn thumb_path_in(thumb_dir: &Path, rel_path: &str) -> PathBuf {
    let stem = normalize_rel_path(&Path::new(rel_path).with_extension("jpg").to_string_lossy());
    thumb_dir.join(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_key_is_stable_and_short() {
        let key = root_key("/home/user/Photos");
        assert_eq!(key.len(), 16);
        assert_eq!(key, root_key("/home/user/Photos"));
    }

    #[test]
    fn root_key_separates_roots_that_share_a_name() {
        assert_ne!(root_key("/a/Photos"), root_key("/b/Photos"));
    }

    #[test]
    fn root_key_normalizes_separators() {
        assert_eq!(root_key("C:\\photos"), root_key("C:/photos"));
    }

    #[test]
    fn thumb_path_mirrors_the_relative_path_with_jpg_extension() {
        let path = thumb_path_for(Path::new("/cache/thumbs"), "abc", "Trip/IMG_1.HEIC");
        assert_eq!(path, PathBuf::from("/cache/thumbs/abc/Trip/IMG_1.jpg"));
    }
}
