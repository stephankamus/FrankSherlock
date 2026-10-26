pub mod client;
pub mod management;
pub mod model_selection;

pub const OLLAMA_BASE: &str = "http://localhost:11434";

/// Address of the Ollama API this process talks to.
///
/// `SHERLOCK_OLLAMA_BASE` overrides it, which is how the app is pointed at the
/// logging proxy in `scripts/ollama-proxy.py` to capture the exact bytes of a
/// request without rebuilding anything.
pub fn ollama_base() -> String {
    std::env::var("SHERLOCK_OLLAMA_BASE").unwrap_or_else(|_| OLLAMA_BASE.to_string())
}

pub use client::{
    num_ctx_for, ollama_generate, parse_json_response, GenerateParams, OllamaTimings, MIN_NUM_CTX,
};
pub use management::{
    cleanup_loaded_models, list_installed_models, list_installed_models_with_sizes,
    list_loaded_models, loaded_model_memory, model_satisfies, resolve_installed_model,
    DownloadState,
};
pub use model_selection::recommended_model;
