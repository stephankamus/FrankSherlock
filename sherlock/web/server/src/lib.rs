//! Read-only web server for a library published by the Frank Sherlock desktop app.
//!
//! The desktop owns the pipeline; this crate owns nothing but HTTP. It reads the
//! snapshot the desktop uploads (`current.json` plus one immutable `.sqlite` file) and
//! answers from it with the **same** query code the desktop runs, because both link
//! `sherlock_core`. That is the whole point of the shared crate: the two clients cannot
//! disagree about what a search means.

pub mod api;
pub mod config;
pub mod library;
pub mod media;
pub mod preview;

use std::sync::Arc;

pub use config::Config;
pub use library::Library;
pub use media::MediaMap;

pub struct AppState {
    pub config: Config,
    pub library: Library,
}

impl AppState {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            library: Library::new(config.library_dir.clone()),
            config,
        })
    }
}
