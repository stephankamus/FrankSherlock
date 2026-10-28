use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::AppResult;
use crate::models::SetupDownloadStatus;

use super::OLLAMA_BASE;

#[derive(Clone, Debug)]
pub struct DownloadState {
    pub status: String,
    pub model: Option<String>,
    pub progress_pct: f32,
    pub message: String,
}

impl DownloadState {
    pub fn idle() -> Self {
        Self {
            status: "idle".to_string(),
            model: None,
            progress_pct: 0.0,
            message: "No download in progress".to_string(),
        }
    }

    pub fn as_view(&self) -> SetupDownloadStatus {
        SetupDownloadStatus {
            status: self.status.clone(),
            model: self.model.clone(),
            progress_pct: self.progress_pct,
            message: self.message.clone(),
        }
    }
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_recv_body(Some(Duration::from_secs(5)))
        .timeout_send_body(Some(Duration::from_secs(5)))
        .build()
        .into()
}

/// List models installed locally via Ollama HTTP API (`/api/tags`).
pub fn list_installed_models() -> Option<Vec<String>> {
    let agent = http_agent();
    let mut resp = agent.get(&format!("{OLLAMA_BASE}/api/tags")).call().ok()?;
    let body: String = resp.body_mut().read_to_string().ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = parsed
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            m.get("name")
                .or_else(|| m.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    Some(models)
}

/// List installed models with their on-disk size in bytes (`/api/tags`).
///
/// Size is the weights blob, not the runtime footprint: KV cache and compute
/// buffers add to it. Used only as a rough "will this fit?" hint in the model
/// picker; the calibrated estimate reports the real numbers from `/api/ps`.
pub fn list_installed_models_with_sizes() -> Option<Vec<(String, u64)>> {
    let agent = http_agent();
    let mut resp = agent.get(&format!("{OLLAMA_BASE}/api/tags")).call().ok()?;
    let body: String = resp.body_mut().read_to_string().ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = parsed
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            let name = m
                .get("name")
                .or_else(|| m.get("model"))
                .and_then(|v| v.as_str())?;
            let size = m.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            Some((name.to_string(), size))
        })
        .collect();
    Some(models)
}

/// Runtime memory footprint reported by Ollama for a loaded model.
///
/// Returns `(total_bytes, vram_bytes)`. `total > vram` means part of the model
/// spilled to system RAM, which collapses decode throughput.
pub fn loaded_model_memory(model: &str) -> Option<(u64, u64)> {
    let agent = http_agent();
    let mut resp = agent.get(&format!("{OLLAMA_BASE}/api/ps")).call().ok()?;
    let body: String = resp.body_mut().read_to_string().ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = parsed.get("models")?.as_array()?;
    for m in models {
        let name = m
            .get("name")
            .or_else(|| m.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name == model {
            let size = m.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let vram = m.get("size_vram").and_then(|v| v.as_u64()).unwrap_or(0);
            return Some((size, vram));
        }
    }
    None
}

/// List models currently loaded in Ollama via HTTP API (`/api/ps`).
/// Returns (ollama_available, loaded_model_names).
pub fn list_loaded_models() -> (bool, Vec<String>) {
    let agent = http_agent();
    let resp = agent.get(&format!("{OLLAMA_BASE}/api/ps")).call();
    match resp {
        Ok(mut resp) => {
            let body: String = match resp.body_mut().read_to_string() {
                Ok(s) => s,
                Err(_) => return (true, Vec::new()),
            };
            let parsed: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => return (true, Vec::new()),
            };
            let models = parsed
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            m.get("name")
                                .or_else(|| m.get("model"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            (true, models)
        }
        Err(_) => (false, Vec::new()),
    }
}

/// Unload all currently loaded Ollama models via HTTP API.
/// Shared state of the "runner wedged" recovery.
fn reload_state() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

/// A request went through: a wedge that comes later deserves its own reload.
///
/// Without this the throttle below turned into a 10 s blackout in which a genuinely
/// wedged runner was retried without a reload and the file was marked as failed —
/// measured: 13 of 107 files lost to attempts that were throttled out.
pub fn note_success() {
    if let Ok(mut last) = reload_state().lock() {
        *last = None;
    }
}

/// Unload a model so the next request loads a fresh runner.
///
/// Ollama 0.40.11 answers requests with its zero-value frame once a runner has
/// wedged (measured: ~1-2 minutes into a scan, and then on every request until the
/// model is reloaded). Unloading is the only cure; the next `generate` pays the load.
/// Throttled to once every 10 s only to survive a runner that wedges immediately
/// after every reload — [`note_success`] clears the throttle as soon as one request
/// goes through.
///
/// Returns whether a reload was actually performed.
pub fn reload_model(model: &str) -> bool {
    reload_model_inner(model, true)
}

/// The same reload, bypassing the 10 s throttle.
///
/// Used by the second recovery cycle of a request that came back with the zero-value
/// frame *again* after the first reload. The throttle exists to survive a runner that
/// wedges immediately after every load, but in that case the answer is to stop sending
/// requests, not to reuse a runner the caller has just seen fail.
pub fn reload_model_now(model: &str) -> bool {
    reload_model_inner(model, false)
}

fn reload_model_inner(model: &str, respect_throttle: bool) -> bool {
    use std::time::{Duration, Instant};
    let state = reload_state();
    let mut last = match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    if respect_throttle
        && last.is_some_and(|previous| now.duration_since(previous) < Duration::from_secs(10))
    {
        return false;
    }
    *last = Some(now);
    drop(last);

    let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
    let payload = serde_json::json!({ "model": model, "keep_alive": 0 });
    let sent = agent
        .post(&format!("{}/api/generate", super::ollama_base()))
        .send_json(&payload);
    match sent {
        Ok(_) => log::info!("Unloaded {model} to reset a wedged runner"),
        Err(e) => log::warn!("Could not unload {model}: {e}"),
    }
    // The unload is asynchronous, and a fixed 400 ms sleep was not enough: measured on
    // job #35 (2026-10-28), the retry arrived 3 s after the unload request and Ollama
    // still answered with the **same** zero-value frame, because the wedged runner was
    // the one that served it. `/api/ps` no longer listing the model is the condition
    // that actually has to hold before the caller retries.
    if !wait_until_unloaded(model, UNLOAD_TIMEOUT) {
        log::warn!(
            "{model} was still listed by /api/ps after {UNLOAD_TIMEOUT:?}; the retry may \
             hit the old runner"
        );
    }
    true
}

/// How long the unload may take before the caller retries anyway.
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// True once `/api/ps` no longer lists `model`.
///
/// The poll starts with a 100 ms pause so a healthy unload costs one short wait. It
/// returns early when Ollama itself is unreachable: waiting proves nothing in that
/// case, and the retry will produce the honest transport error instead.
fn wait_until_unloaded(model: &str, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(100));
    while started.elapsed() < timeout {
        let (reachable, models) = list_loaded_models();
        if !reachable {
            return false;
        }
        if !models.iter().any(|loaded| loaded == model) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

pub fn cleanup_loaded_models() -> AppResult<()> {
    let (_, models) = list_loaded_models();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .timeout_send_body(Some(Duration::from_secs(10)))
        .build()
        .into();

    for model in &models {
        let payload = serde_json::json!({
            "model": model,
            "keep_alive": 0,
        });
        let _ = agent
            .post(&format!("{OLLAMA_BASE}/api/generate"))
            .send_json(&payload);
    }

    Ok(())
}

/// Pull a model via Ollama HTTP API (`/api/pull`), streaming progress.
pub async fn run_model_download(setup_state: Arc<Mutex<DownloadState>>, model: String) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_recv_body(None) // no timeout — downloads can take a long time
        .timeout_send_body(Some(Duration::from_secs(30)))
        .build()
        .into();

    let payload = serde_json::json!({
        "name": &model,
        "stream": true,
    });

    let resp = agent
        .post(&format!("{OLLAMA_BASE}/api/pull"))
        .send_json(&payload);

    let Ok(resp) = resp else {
        let mut state = setup_state.lock().expect("setup download mutex poisoned");
        state.status = "failed".to_string();
        state.message = "Could not connect to Ollama HTTP API for pull.".to_string();
        return;
    };

    let reader = BufReader::new(resp.into_body().into_reader());
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let mut state = setup_state.lock().expect("setup download mutex poisoned");
        state.model = Some(model.clone());
        state.status = "running".to_string();

        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(status_text) = obj.get("status").and_then(|v| v.as_str()) {
                state.message = status_text.to_string();
            }
            // Progress: {"status":"pulling ...","completed":123456,"total":789000}
            let completed = obj.get("completed").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let total = obj.get("total").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if total > 0.0 {
                state.progress_pct = ((completed / total) * 100.0).clamp(0.0, 100.0) as f32;
            }
            // Check for error
            if let Some(err) = obj.get("error").and_then(|v| v.as_str()) {
                state.status = "failed".to_string();
                state.message = err.to_string();
                return;
            }
        } else {
            // Fallback: try parsing percent from raw text
            if let Some(pct) = parse_progress_percent(&line) {
                state.progress_pct = pct;
            }
            state.message = line;
        }
    }

    let mut state = setup_state.lock().expect("setup download mutex poisoned");
    if state.status == "running" {
        state.status = "completed".to_string();
        state.progress_pct = 100.0;
        state.message = format!("Model {model} downloaded.");
    }
}

fn parse_progress_percent(line: &str) -> Option<f32> {
    let percent_pos = line.find('%')?;
    let prefix = &line[..percent_pos];
    let start = prefix
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let number = prefix.get(start..)?.trim();
    number.parse::<f32>().ok().map(|v| v.clamp(0.0, 100.0))
}

/// Extract the base model name (without tag) from an Ollama model identifier.
///
/// `"qwen2.5vl:7b"` → `"qwen2.5vl"`, `"qwen2.5vl:latest"` → `"qwen2.5vl"`.
pub fn model_base_name(model: &str) -> &str {
    model.split(':').next().unwrap_or(model)
}

/// Check if an installed model satisfies a required model tag.
///
/// Returns true when the models share the same base name — any variant from the
/// same family will work (e.g. `qwen2.5vl:3b` satisfies `qwen2.5vl:7b`).
/// The scan context will resolve to the actual installed tag at runtime.
pub fn model_satisfies(installed: &str, required: &str) -> bool {
    if installed == required {
        return true;
    }
    model_base_name(installed) == model_base_name(required)
}

/// Find the best installed model matching a recommended tag.
///
/// Preference order:
/// 1. Exact match (e.g. `qwen2.5vl:7b`)
/// 2. `:latest` alias with same base name
/// 3. Any variant with the same base name (e.g. `qwen2.5vl:3b` when `7b` is recommended)
///
/// Returns the installed model tag to use, or the original recommended tag if nothing matches.
pub fn resolve_installed_model(recommended: &str) -> String {
    match list_installed_models() {
        Some(models) => resolve_model_from_list(recommended, &models),
        None => recommended.to_string(),
    }
}

/// Pure-logic model resolution against a known list of installed models.
fn resolve_model_from_list(recommended: &str, installed: &[String]) -> String {
    let base = model_base_name(recommended);

    // 1. Exact match
    if installed.iter().any(|m| m == recommended) {
        return recommended.to_string();
    }

    // 2. :latest alias
    if let Some(m) = installed
        .iter()
        .find(|m| model_base_name(m) == base && m.ends_with(":latest"))
    {
        return m.clone();
    }

    // 3. Any variant from the same family
    if let Some(m) = installed.iter().find(|m| model_base_name(m) == base) {
        return m.clone();
    }

    recommended.to_string()
}

/// Parse the first whitespace-delimited column from each non-header line.
/// Works for both `ollama ps` and `ollama list` CLI output.
/// Kept for backward compatibility with tests.
#[allow(dead_code)]
fn parse_ollama_table_output(text: &str) -> Vec<String> {
    text.lines()
        .skip(1) // skip header row
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ollama_ps_rows() {
        let sample = "NAME ID SIZE PROCESSOR UNTIL\nqwen2.5vl:7b abc 6.0 GB 100% GPU 4 minutes\n";
        let models = parse_ollama_table_output(sample);
        assert_eq!(models, vec!["qwen2.5vl:7b".to_string()]);
    }

    #[test]
    fn parses_ollama_list_rows() {
        let sample = "NAME ID SIZE MODIFIED\nqwen2.5vl:7b abc 5 GB 1 day ago\n";
        let models = parse_ollama_table_output(sample);
        assert_eq!(models, vec!["qwen2.5vl:7b".to_string()]);
    }

    #[test]
    fn cleanup_handles_empty_ps_output() {
        let mock = "NAME\tID\tSIZE\tPROCESSOR\tUNTIL\n";
        let models = parse_ollama_table_output(mock);
        assert!(models.is_empty());
    }

    #[test]
    fn extracts_progress_percent() {
        assert_eq!(parse_progress_percent("pulling ... 34%"), Some(34.0));
        assert_eq!(parse_progress_percent("12.5% complete"), Some(12.5));
        assert_eq!(parse_progress_percent("done"), None);
    }

    #[test]
    fn download_state_idle() {
        let state = DownloadState::idle();
        assert_eq!(state.status, "idle");
        assert!(state.model.is_none());
        assert_eq!(state.progress_pct, 0.0);
    }

    #[test]
    fn download_state_as_view() {
        let state = DownloadState {
            status: "running".to_string(),
            model: Some("test:7b".to_string()),
            progress_pct: 50.0,
            message: "Downloading...".to_string(),
        };
        let view = state.as_view();
        assert_eq!(view.status, "running");
        assert_eq!(view.model, Some("test:7b".to_string()));
        assert_eq!(view.progress_pct, 50.0);
    }

    #[test]
    fn parses_api_tags_response() {
        let json =
            r#"{"models":[{"name":"qwen2.5vl:7b","model":"qwen2.5vl:7b","size":5000000000}]}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let models: Vec<String> = parsed
            .get("models")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| {
                m.get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert_eq!(models, vec!["qwen2.5vl:7b"]);
    }

    #[test]
    fn parses_api_ps_response() {
        let json = r#"{"models":[{"name":"qwen2.5vl:7b","model":"qwen2.5vl:7b"}]}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let models: Vec<String> = parsed
            .get("models")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| {
                m.get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert_eq!(models, vec!["qwen2.5vl:7b"]);
    }

    #[test]
    fn parses_empty_api_ps_response() {
        let json = r#"{"models":[]}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let models: Vec<String> = parsed
            .get("models")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| {
                m.get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(models.is_empty());
    }

    #[test]
    fn model_base_name_strips_tag() {
        assert_eq!(model_base_name("qwen2.5vl:7b"), "qwen2.5vl");
        assert_eq!(model_base_name("qwen2.5vl:latest"), "qwen2.5vl");
        assert_eq!(model_base_name("qwen2.5vl"), "qwen2.5vl");
    }

    #[test]
    fn model_satisfies_exact_match() {
        assert!(model_satisfies("qwen2.5vl:7b", "qwen2.5vl:7b"));
    }

    #[test]
    fn model_satisfies_latest_alias() {
        assert!(model_satisfies("qwen2.5vl:latest", "qwen2.5vl:7b"));
    }

    #[test]
    fn model_satisfies_rejects_different_base() {
        assert!(!model_satisfies("llama3:latest", "qwen2.5vl:7b"));
    }

    #[test]
    fn model_satisfies_accepts_different_variant_same_family() {
        // "qwen2.5vl:3b" is a different size but same family — acceptable
        assert!(model_satisfies("qwen2.5vl:3b", "qwen2.5vl:7b"));
    }

    #[test]
    fn resolve_exact_match() {
        let installed = vec!["qwen2.5vl:7b".to_string(), "llama3:8b".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:7b"
        );
    }

    #[test]
    fn resolve_latest_alias() {
        let installed = vec!["qwen2.5vl:latest".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:latest"
        );
    }

    #[test]
    fn resolve_different_variant() {
        let installed = vec!["qwen2.5vl:3b".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:3b"
        );
    }

    #[test]
    fn resolve_no_match_returns_recommended() {
        let installed = vec!["llama3:8b".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:7b"
        );
    }

    #[test]
    fn resolve_prefers_exact_over_latest() {
        let installed = vec!["qwen2.5vl:latest".to_string(), "qwen2.5vl:7b".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:7b"
        );
    }

    #[test]
    fn resolve_prefers_latest_over_other_variant() {
        let installed = vec!["qwen2.5vl:3b".to_string(), "qwen2.5vl:latest".to_string()];
        assert_eq!(
            resolve_model_from_list("qwen2.5vl:7b", &installed),
            "qwen2.5vl:latest"
        );
    }
}
