use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("task join error: {0}")]
    Join(String),
    /// The scan root exists in the index but cannot be reached right now
    /// (unmounted share, disconnected network drive, stale GVFS path, ...).
    /// Aborts the scan without touching any indexed rows.
    #[error("root unavailable: {0}")]
    RootUnavailable(String),
}

pub type AppResult<T> = Result<T, AppError>;
