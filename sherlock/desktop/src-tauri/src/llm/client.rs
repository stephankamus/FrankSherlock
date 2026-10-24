use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// Timing/counters returned by Ollama for a single request.
///
/// Everything is in seconds (Ollama reports nanoseconds). They are kept so the
/// scan estimator can calibrate per-image cost on the user's own hardware
/// instead of relying on hardcoded throughput tables.
#[derive(Debug, Clone, Copy, Default)]
pub struct OllamaTimings {
    pub total_s: f64,
    pub load_s: f64,
    pub prompt_tokens: u32,
    pub prefill_s: f64,
    pub eval_tokens: u32,
    pub decode_s: f64,
}

pub struct OllamaResponse {
    pub ok: bool,
    pub raw: String,
    pub timings: OllamaTimings,
}

impl OllamaResponse {
    pub fn error(msg: String) -> Self {
        Self {
            ok: false,
            raw: msg,
            timings: OllamaTimings::default(),
        }
    }
}

/// Per-request generation parameters.
#[derive(Debug, Clone)]
pub struct GenerateParams {
    pub num_predict: u32,
    /// Context window. `None` leaves Ollama's default (which is far larger than
    /// a single image needs and makes the model spill out of VRAM).
    pub num_ctx: Option<u32>,
    pub temperature: f32,
    pub json_mode: bool,
    pub keep_alive: String,
    pub timeout_secs: u64,
}

impl Default for GenerateParams {
    fn default() -> Self {
        Self {
            num_predict: 180,
            num_ctx: None,
            temperature: 0.0,
            json_mode: true,
            keep_alive: "10m".to_string(),
            timeout_secs: 300,
        }
    }
}

impl GenerateParams {
    /// Parameters for a vision classification request.
    pub fn vision(num_predict: u32, num_ctx: Option<u32>) -> Self {
        Self {
            num_predict,
            num_ctx,
            ..Self::default()
        }
    }
}

/// Minimum context worth requesting. Below 2048 the prompt + image + output of a
/// single frame do not fit.
const MIN_NUM_CTX: u32 = 2048;
/// Upper bound. Images beyond ~1024 px are capped by Ollama anyway, so a bigger
/// window only wastes VRAM.
const MAX_NUM_CTX: u32 = 8192;

/// Context window needed for one image + prompt + output.
///
/// Derived from the image dimensions (not from VRAM): a request carries exactly
/// one image, so once it is downscaled the requirement is bounded and identical
/// on every machine. Measured on qwen2.5-VL: 640 px ≈ 407-614 tokens,
/// 768 px ≈ 530-814, 1024 px ≈ 918-1555, 1280 px ≈ 2302.
pub fn num_ctx_for(img_w: u32, img_h: u32, prompt_chars: usize, num_predict: u32) -> u32 {
    let image_tokens = crate::vision_prep::estimated_image_tokens(img_w, img_h);
    let prompt_tokens = (prompt_chars / 4) as u32;
    let needed = image_tokens
        .saturating_add(prompt_tokens)
        .saturating_add(num_predict)
        .saturating_add(256);
    needed.next_power_of_two().clamp(MIN_NUM_CTX, MAX_NUM_CTX)
}

pub fn ollama_generate(
    model: &str,
    prompt: &str,
    image_path: Option<&Path>,
    params: &GenerateParams,
) -> OllamaResponse {
    let mut payload = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "keep_alive": params.keep_alive,
        "options": {
            "temperature": params.temperature,
            "num_predict": params.num_predict,
        }
    });

    if let Some(ctx) = params.num_ctx {
        payload["options"]["num_ctx"] = serde_json::json!(ctx);
    }

    if params.json_mode {
        payload["format"] = serde_json::json!("json");
    }

    if let Some(img_path) = image_path {
        match std::fs::read(img_path) {
            Ok(bytes) => {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                payload["images"] = serde_json::json!([b64]);
            }
            Err(e) => {
                return OllamaResponse::error(format!("image_read_error: {e}"));
            }
        }
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_recv_body(Some(Duration::from_secs(params.timeout_secs)))
        .timeout_send_body(Some(Duration::from_secs(30)))
        .build()
        .into();

    let result = agent
        .post(&format!("{}/api/generate", super::OLLAMA_BASE))
        .send_json(&payload);

    match result {
        Ok(mut resp) => {
            let body: String = match resp.body_mut().read_to_string() {
                Ok(s) => s,
                Err(e) => {
                    return OllamaResponse::error(format!("read_error: {e}"));
                }
            };
            let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let raw = parsed
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            const NS: f64 = 1_000_000_000.0;
            let ns = |key: &str| parsed.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) / NS;
            OllamaResponse {
                ok: true,
                raw,
                timings: OllamaTimings {
                    total_s: ns("total_duration"),
                    load_s: ns("load_duration"),
                    prompt_tokens: parsed
                        .get("prompt_eval_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    prefill_s: ns("prompt_eval_duration"),
                    eval_tokens: parsed
                        .get("eval_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    decode_s: ns("eval_duration"),
                },
            }
        }
        Err(e) => OllamaResponse::error(format!("http_error: {e}")),
    }
}

/// Extract the first balanced JSON object from free-form text.
pub fn extract_first_json_object(text: &str) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if v.is_object() {
                return Some(v);
            }
        }
    }
    for (start, _) in text.char_indices().filter(|(_, c)| *c == '{') {
        let mut depth = 0i32;
        for (i, ch) in text[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &text[start..start + i + 1];
                        if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                            if v.is_object() {
                                return Some(v);
                            }
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

pub fn parse_json_response(raw: &str) -> Option<Value> {
    if raw.is_empty() {
        return None;
    }
    extract_first_json_object(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_first_json_object_valid() {
        let input = r#"{"media_type":"anime","confidence":0.9}"#;
        let result = extract_first_json_object(input).unwrap();
        assert_eq!(result["media_type"], "anime");
    }

    #[test]
    fn extract_first_json_object_garbage_prefix() {
        let input = r#"Here is my answer: {"media_type":"photo","confidence":0.5} done"#;
        let result = extract_first_json_object(input).unwrap();
        assert_eq!(result["media_type"], "photo");
    }

    #[test]
    fn extract_first_json_object_nested_braces() {
        let input = r#"{"a":{"b":1},"c":2}"#;
        let result = extract_first_json_object(input).unwrap();
        assert_eq!(result["c"], 2);
    }

    #[test]
    fn extract_first_json_object_empty() {
        assert!(extract_first_json_object("").is_none());
    }

    #[test]
    fn extract_first_json_object_no_json() {
        assert!(extract_first_json_object("just some text").is_none());
    }

    #[test]
    fn num_ctx_for_downscaled_images_uses_2048() {
        // 640/768 px never need more than a 2k window for one frame.
        assert_eq!(num_ctx_for(640, 640, 250, 180), 2048);
        assert_eq!(num_ctx_for(768, 768, 250, 180), 2048);
    }

    #[test]
    fn num_ctx_for_1024px_uses_4096() {
        assert_eq!(num_ctx_for(1024, 1024, 400, 180), 4096);
        // Portrait 9 MP downscaled to 576x1024 needs less, and asking for less
        // is what keeps the model inside VRAM.
        assert_eq!(num_ctx_for(576, 1024, 400, 180), 2048);
    }

    #[test]
    fn num_ctx_for_is_clamped() {
        assert_eq!(num_ctx_for(16, 16, 0, 8), MIN_NUM_CTX);
        assert_eq!(num_ctx_for(4000, 4000, 4000, 4096), MAX_NUM_CTX);
    }

    #[test]
    fn num_ctx_for_grows_with_resolution() {
        let small = num_ctx_for(768, 768, 400, 180);
        let large = num_ctx_for(1280, 1280, 400, 180);
        assert!(large >= small);
    }

    #[test]
    fn vision_params_default_to_json_and_deterministic() {
        let p = GenerateParams::vision(180, Some(4096));
        assert_eq!(p.temperature, 0.0);
        assert!(p.json_mode);
        assert_eq!(p.num_ctx, Some(4096));
        assert_eq!(p.num_predict, 180);
    }
}
