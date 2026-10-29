//! Filesystem probing, now owned by `sherlock_core`.
//!
//! Bounded `stat`/`read_dir` checks and mount-kind detection moved with `db.rs`: the
//! read side of the index answers "is this root reachable", and the web server asks the
//! same question of its mounts. Nothing here was OS-specific beyond a `cfg` on the mount
//! table path, so the module moved whole and this file is only the old doorway.

pub use sherlock_core::fs::*;
