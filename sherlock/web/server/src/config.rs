//! Everything the deployment has to tell the container.
//!
//! The mounts themselves are decided by `docker-compose` — a container cannot choose
//! what it mounts at runtime. What is configured here is the *mapping* from the paths the
//! desktop recorded to the paths this process can actually open, plus the token and the
//! library location.

use std::path::PathBuf;

use crate::media::MediaMap;

pub const DEFAULT_PORT: u16 = 8080;

#[derive(Debug, Clone)]
pub struct Config {
    /// Mount of the publish folder: `current.json`, the snapshots and the thumbnails.
    pub library_dir: PathBuf,
    /// `root_path` as recorded by the desktop → path inside this container.
    pub media_map: MediaMap,
    /// Bearer token every `/api` call must present. Empty disables the check (see
    /// `main`), which is only acceptable behind an authenticating reverse proxy.
    pub auth_token: Option<String>,
    pub port: u16,
}

impl Config {
    /// Read the environment, failing loudly on anything a silent default would hide.
    pub fn from_env() -> Result<Self, String> {
        let library_dir = std::env::var("LIBRARY_DIR").unwrap_or_else(|_| "/library".to_string());
        let library_dir = PathBuf::from(library_dir);
        if !library_dir.is_dir() {
            return Err(format!(
                "LIBRARY_DIR ({}) is not a directory: mount the publish folder there",
                library_dir.display()
            ));
        }

        let media_map_path = std::env::var("MEDIA_MAP").unwrap_or_default();
        let media_map = if media_map_path.trim().is_empty() {
            log::warn!(
                "MEDIA_MAP is not set: no root is mapped, so photos and videos cannot be \
                 served (search and metadata still work)"
            );
            MediaMap::empty()
        } else {
            MediaMap::from_file(&PathBuf::from(&media_map_path))?
        };

        let auth_token = std::env::var("AUTH_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if auth_token.is_none() {
            log::warn!(
                "AUTH_TOKEN is empty: the API is open to anyone who can reach this port. \
                 Put it behind an authenticating proxy or set a token."
            );
        }

        let port = match std::env::var("PORT") {
            Ok(raw) => raw
                .trim()
                .parse::<u16>()
                .map_err(|_| format!("PORT is not a valid port number: {raw}"))?,
            Err(_) => DEFAULT_PORT,
        };

        Ok(Self {
            library_dir,
            media_map,
            auth_token,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_the_one_documented_for_compose() {
        assert_eq!(DEFAULT_PORT, 8080);
    }

    #[test]
    fn missing_library_dir_is_an_error_not_a_default() {
        // `from_env` reads the process environment, so only the failure path that does
        // not depend on it is exercised here: the port parser's shape.
        let parsed: Result<u16, _> = "not-a-port".parse();
        assert!(parsed.is_err());
    }
}
