pub mod client;
pub mod management;
pub mod model_selection;

pub const OLLAMA_BASE: &str = "http://localhost:11434";

pub use client::{
    num_ctx_for, ollama_generate, parse_json_response, GenerateParams, OllamaTimings,
};
pub use management::{
    cleanup_loaded_models, list_installed_models, list_installed_models_with_sizes,
    list_loaded_models, loaded_model_memory, model_satisfies, resolve_installed_model,
    DownloadState,
};
pub use model_selection::recommended_model;
