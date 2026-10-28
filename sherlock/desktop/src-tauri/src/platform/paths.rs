//! Path normalisation, now owned by `sherlock_core`.
//!
//! The canonical representation of a `rel_path` (forward slashes) is part of the
//! database format, not of this platform layer: the web server has to build the same
//! thumbnail paths the desktop does, so the functions moved to the shared core and
//! this module is only the old doorway to them.

pub use sherlock_core::paths::*;
