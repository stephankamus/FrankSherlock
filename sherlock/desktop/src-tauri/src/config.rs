use std::env;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub const DATA_DIR_ENV: &str = "FRANK_SHERLOCK_DATA_DIR";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub base_dir: PathBuf,
    pub db_dir: PathBuf,
    pub db_file: PathBuf,
    pub cache_dir: PathBuf,
    pub classification_cache_dir: PathBuf,
    pub thumbnails_dir: PathBuf,
    pub scans_dir: PathBuf,
    pub surya_venv_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub models_dir: PathBuf,
    pub face_crops_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPathsView {
    pub base_dir: String,
    pub db_file: String,
    pub cache_dir: String,
}

impl AppPaths {
    pub fn view(&self) -> AppPathsView {
        AppPathsView {
            base_dir: self.base_dir.display().to_string(),
            db_file: self.db_file.display().to_string(),
            cache_dir: self.cache_dir.display().to_string(),
        }
    }
}

pub fn resolve_paths() -> AppResult<AppPaths> {
    let base_dir = match env::var(DATA_DIR_ENV) {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => default_base_dir()?,
    };

    let db_dir = base_dir.join("db");
    let cache_dir = base_dir.join("cache");
    let classification_cache_dir = cache_dir.join("classifications");
    let thumbnails_dir = cache_dir.join("thumbnails");
    let scans_dir = cache_dir.join("scans");
    let surya_venv_dir = base_dir.join("surya_venv");
    let tmp_dir = cache_dir.join("tmp");
    let db_file = db_dir.join("index.sqlite");
    let models_dir = base_dir.join("models");
    let face_crops_dir = cache_dir.join("face_crops");

    Ok(AppPaths {
        base_dir,
        db_dir,
        db_file,
        cache_dir,
        classification_cache_dir,
        thumbnails_dir,
        scans_dir,
        surya_venv_dir,
        tmp_dir,
        models_dir,
        face_crops_dir,
    })
}

pub fn prepare_dirs(paths: &AppPaths) -> AppResult<()> {
    std::fs::create_dir_all(&paths.base_dir)?;
    std::fs::create_dir_all(&paths.db_dir)?;
    std::fs::create_dir_all(&paths.cache_dir)?;
    std::fs::create_dir_all(&paths.classification_cache_dir)?;
    std::fs::create_dir_all(&paths.thumbnails_dir)?;
    std::fs::create_dir_all(&paths.scans_dir)?;
    std::fs::create_dir_all(&paths.surya_venv_dir)?;
    std::fs::create_dir_all(&paths.tmp_dir)?;
    std::fs::create_dir_all(&paths.models_dir)?;
    std::fs::create_dir_all(&paths.face_crops_dir)?;
    Ok(())
}

fn default_base_dir() -> AppResult<PathBuf> {
    if let Some(dir) = dirs::data_local_dir() {
        return Ok(dir.join("frank_sherlock"));
    }
    Err(AppError::Config(
        "could not resolve local data directory".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// UI config (~/.config/frank_sherlock/config.json)
// ---------------------------------------------------------------------------

fn user_config_path() -> AppResult<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        return Ok(config_dir.join("frank_sherlock").join("config.json"));
    }
    Err(AppError::Config(
        "could not resolve user config directory".to_string(),
    ))
}

pub fn load_user_config() -> AppResult<serde_json::Value> {
    let path = user_config_path()?;
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let data = std::fs::read_to_string(&path)?;
    serde_json::from_str(&data).map_err(|e| AppError::Config(format!("invalid config JSON: {e}")))
}

pub fn save_user_config(config: &serde_json::Value) -> AppResult<()> {
    let path = user_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| AppError::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(&path, data)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Runtime settings (~/.config/frank_sherlock/settings.toml)
// ---------------------------------------------------------------------------

const SETTINGS_TEMPLATE: &str = r#"# Frank Sherlock runtime settings.
#
# Restart Frank Sherlock after changing this file.
#
# Ollama vision model override.
# Leave empty to let the app choose from detected hardware.
#
# Recommended options:
# - qwen2.5vl:3b   Faster and safer for low VRAM, Windows NVIDIA <= 8GB, or CPU fallback.
# - qwen2.5vl:7b   Better quality when Ollama can load it fully on GPU.
# - qwen2.5vl:32b  Heavy; only realistic for large unified-memory systems.
#
# Experimental alternatives may work if installed in Ollama, but prompts are tuned for qwen2.5vl:
# - minicpm-v:8b
# - moondream
#
# When set, Frank Sherlock requires this exact model. If it is missing, first-run setup will
# offer to download it through Ollama before scanning.
model_override = ""
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub model_override: Option<String>,
}

fn runtime_settings_path() -> AppResult<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        return Ok(config_dir.join("frank_sherlock").join("settings.toml"));
    }
    Err(AppError::Config(
        "could not resolve user config directory".to_string(),
    ))
}

pub fn ensure_runtime_settings_file() -> AppResult<PathBuf> {
    let path = runtime_settings_path()?;
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, SETTINGS_TEMPLATE)?;
    Ok(path)
}

pub fn load_runtime_settings() -> AppResult<RuntimeSettings> {
    let path = ensure_runtime_settings_file()?;
    let data = std::fs::read_to_string(&path)?;
    parse_runtime_settings(&data)
}

fn parse_runtime_settings(data: &str) -> AppResult<RuntimeSettings> {
    let mut model_override = None;

    for raw_line in data.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model_override" {
            continue;
        }

        let parsed = parse_settings_string(value.trim())?;
        let parsed = parsed.trim();
        if !parsed.is_empty() {
            validate_model_tag(parsed)?;
            model_override = Some(parsed.to_string());
        }
    }

    Ok(RuntimeSettings { model_override })
}

fn parse_settings_string(value: &str) -> AppResult<String> {
    let without_comment = value.split('#').next().unwrap_or("").trim();
    if without_comment.starts_with('"') {
        let parsed: String = serde_json::from_str(without_comment)
            .map_err(|e| AppError::Config(format!("invalid quoted model_override value: {e}")))?;
        return Ok(parsed);
    }
    Ok(without_comment.to_string())
}

fn validate_model_tag(model: &str) -> AppResult<()> {
    let valid = !model.is_empty()
        && model.len() <= 128
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'));
    if valid {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "invalid model_override '{model}': use an Ollama model tag like qwen2.5vl:3b"
        )))
    }
}

/// Expand `~`, canonicalize, and validate that the result is a directory.
///
/// Handles:
/// - `~` and `~/...` (and `~\...` on Windows) expansion via `dirs::home_dir()`
/// - Trailing slashes, `.`, `..`, symlinks (via `canonicalize()`)
/// - Validates the resolved path is a directory
pub fn expand_and_canonicalize(raw: &str) -> AppResult<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidPath("empty path".into()));
    }

    let expanded = if trimmed == "~" {
        dirs::home_dir()
            .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?;
        home.join(rest)
    } else if cfg!(windows) {
        if let Some(rest) = trimmed.strip_prefix("~\\") {
            let home = dirs::home_dir()
                .ok_or_else(|| AppError::InvalidPath("cannot resolve home directory".into()))?;
            home.join(rest)
        } else {
            PathBuf::from(trimmed)
        }
    } else {
        PathBuf::from(trimmed)
    };

    let canonical = dunce::canonicalize(&expanded)
        .map_err(|e| AppError::InvalidPath(format!("cannot resolve path '{}': {}", raw, e)))?;

    if !canonical.is_dir() {
        return Err(AppError::InvalidPath(format!("not a directory: {}", raw)));
    }

    Ok(canonical)
}

pub fn canonical_root_path(path: &str) -> AppResult<PathBuf> {
    let root = Path::new(path);
    if !root.exists() {
        return Err(AppError::InvalidPath(format!(
            "path does not exist: {path}"
        )));
    }
    if !root.is_dir() {
        return Err(AppError::InvalidPath(format!(
            "path is not a directory: {path}"
        )));
    }
    Ok(dunce::canonicalize(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_from_env_override() {
        let _guard = ENV_LOCK.lock().expect("lock");
        env::remove_var(DATA_DIR_ENV);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("custom_data");
        env::set_var(DATA_DIR_ENV, path.as_os_str());
        let paths = resolve_paths().expect("paths");
        assert_eq!(paths.base_dir, path);
        env::remove_var(DATA_DIR_ENV);
    }

    #[test]
    fn expand_and_canonicalize_resolves_dot() {
        let cwd = dunce::canonicalize(".").expect("cwd");
        let result = expand_and_canonicalize(".").expect("dot");
        assert_eq!(result, cwd);
    }

    #[test]
    fn expand_and_canonicalize_strips_trailing_slash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let with_slash = format!("{}/", dir.path().display());
        let result = expand_and_canonicalize(&with_slash).expect("trailing slash");
        assert_eq!(result, dunce::canonicalize(dir.path()).expect("canon"));
    }

    /// GVFS mount names contain `:` and `,` and `=`, e.g.
    /// `smb-share:server=nas,share=fotos`. They must round-trip unchanged.
    #[test]
    fn expand_and_canonicalize_gvfs_style_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gvfs = dir.path().join("smb-share:server=nas,share=fotos");
        std::fs::create_dir_all(&gvfs).expect("create gvfs dir");

        let raw = gvfs.to_str().expect("utf8");
        let result = expand_and_canonicalize(raw).expect("gvfs path");
        assert_eq!(result, dunce::canonicalize(&gvfs).expect("canon"));
        assert!(result.to_string_lossy().contains("server=nas,share=fotos"));
    }

    #[test]
    fn expand_and_canonicalize_tilde() {
        if let Some(home) = dirs::home_dir() {
            let result = expand_and_canonicalize("~").expect("tilde");
            assert_eq!(result, dunce::canonicalize(&home).expect("canon"));
        }
    }

    #[test]
    fn expand_and_canonicalize_tilde_subdir() {
        // Only run if home dir exists and is a directory
        if let Some(home) = dirs::home_dir() {
            if home.is_dir() {
                // Just test that ~/. resolves to home
                let result = expand_and_canonicalize("~/.").expect("tilde dot");
                assert_eq!(result, dunce::canonicalize(&home).expect("canon"));
            }
        }
    }

    #[test]
    fn expand_and_canonicalize_nonexistent() {
        let err = expand_and_canonicalize("/nonexistent_path_that_should_not_exist_12345");
        assert!(err.is_err());
    }

    #[test]
    fn expand_and_canonicalize_empty() {
        let err = expand_and_canonicalize("");
        assert!(err.is_err());
    }

    #[test]
    fn expand_and_canonicalize_file_not_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("afile.txt");
        std::fs::write(&file, "hello").expect("write");
        let err = expand_and_canonicalize(file.to_str().unwrap());
        assert!(err.is_err());
    }

    #[test]
    fn prepare_dirs_creates_expected_structure() {
        let _guard = ENV_LOCK.lock().expect("lock");
        env::remove_var(DATA_DIR_ENV);
        let dir = tempfile::tempdir().expect("tempdir");
        env::set_var(DATA_DIR_ENV, dir.path().join("fs_data").as_os_str());
        let paths = resolve_paths().expect("paths");
        prepare_dirs(&paths).expect("prepare");

        assert!(paths.db_dir.exists());
        assert!(paths.thumbnails_dir.exists());
        assert!(paths.classification_cache_dir.exists());
        env::remove_var(DATA_DIR_ENV);
    }

    #[test]
    fn parse_runtime_settings_empty_override_uses_automatic() {
        let settings = parse_runtime_settings(r#"model_override = """#).expect("settings");
        assert_eq!(
            settings,
            RuntimeSettings {
                model_override: None
            }
        );
    }

    #[test]
    fn parse_runtime_settings_quoted_override() {
        let settings =
            parse_runtime_settings(r#"model_override = "qwen2.5vl:3b""#).expect("settings");
        assert_eq!(settings.model_override.as_deref(), Some("qwen2.5vl:3b"));
    }

    #[test]
    fn parse_runtime_settings_unquoted_override_with_comment() {
        let settings = parse_runtime_settings(
            r#"
            # local fallback
            model_override = qwen2.5vl:3b # exact Ollama tag
            "#,
        )
        .expect("settings");
        assert_eq!(settings.model_override.as_deref(), Some("qwen2.5vl:3b"));
    }

    #[test]
    fn parse_runtime_settings_rejects_invalid_model_tag() {
        let err = parse_runtime_settings(r#"model_override = "bad model""#);
        assert!(err.is_err());
    }

    #[test]
    fn runtime_settings_template_documents_model_override() {
        assert!(SETTINGS_TEMPLATE.contains("model_override"));
        assert!(SETTINGS_TEMPLATE.contains("qwen2.5vl:3b"));
        assert!(SETTINGS_TEMPLATE.contains("qwen2.5vl:7b"));
    }
}
