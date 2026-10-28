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
    /// Repetition guard. `None` keeps Ollama's default (1.1 / 64).
    ///
    /// Greedy decoding loves a loop, and on a flat image (`concrete slab,`,
    /// `concrete slab,`, …) Ollama now aborts the request with
    /// `prediction aborted, token repeat limit reached` instead of emitting it.
    /// Feeding the recovery attempt a little entropy and a wider penalty window is
    /// what breaks that loop.
    pub repeat_penalty: Option<f32>,
    pub repeat_last_n: Option<u32>,
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
            repeat_penalty: None,
            repeat_last_n: None,
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

/// Minimum context worth requesting.
///
/// 4096 rather than 2048 so that every image up to the default `image_max_side`
/// (1024) resolves to the same window. A smaller floor made a tiny image fall to
/// 2048 while the next photo stayed at 4096 — and each change of `num_ctx` makes
/// Ollama rebuild the runner. 4096 costs ~144 MiB of KV cache, which is cheap
/// next to a 7.3 GiB graph reallocation.
pub const MIN_NUM_CTX: u32 = 4096;

/// Longest output any stage of the pipeline may ask for (the LLM OCR pass wants
/// 1500 tokens).
///
/// The context size is part of Ollama's runner identity: when `num_ctx` changes
/// between requests, Ollama tears the runner down and loads the model again.
/// Measured on a mixed library at `image_max_side = 768`, the primary pass
/// (180 tokens) resolved to 2048 while the OCR pass (1500) resolved to 4096, and
/// the two alternated: **316 model loads in 15 minutes**, each ~2.1 s plus a
/// 7.3 GiB reallocation that sometimes failed with
/// `terminate called ... std::runtime_error` and killed the runner.
///
/// Reserving the largest output budget here makes every request of a run resolve
/// to the same context, which is what keeps one runner alive for the whole scan.
const NUM_PREDICT_RESERVE: u32 = 1500;
/// Upper bound. Images beyond ~1024 px are capped by Ollama anyway, so a bigger
/// window only wastes VRAM.
const MAX_NUM_CTX: u32 = 8192;

/// Context window needed for one image + prompt + output.
///
/// Derived from the image dimensions (not from VRAM): a request carries exactly
/// one image, so once it is downscaled the requirement is bounded and identical
/// on every machine. Measured on qwen2.5-VL: 640 px ≈ 407-614 tokens,
/// 768 px ≈ 530-814, 1024 px ≈ 918-1555, 1280 px ≈ 2302.
///
/// The output budget is floored at [`NUM_PREDICT_RESERVE`] rather than using the
/// caller's `num_predict`, so a 180-token classification and a 1500-token OCR
/// pass on the same image resolve to the *same* context. Reloading the model is
/// far more expensive than the ~144 MiB of KV cache the reservation costs.
pub fn num_ctx_for(img_w: u32, img_h: u32, prompt_chars: usize, num_predict: u32) -> u32 {
    let image_tokens = crate::vision_prep::estimated_image_tokens(img_w, img_h);
    let prompt_tokens = (prompt_chars / 4) as u32;
    let needed = image_tokens
        .saturating_add(prompt_tokens)
        .saturating_add(num_predict.max(NUM_PREDICT_RESERVE))
        .saturating_add(256);
    needed.next_power_of_two().clamp(MIN_NUM_CTX, MAX_NUM_CTX)
}

/// Prompt-evaluation batch size sent with every request.
///
/// Ollama allocates the prefill compute buffer in proportion to the batch, and the
/// default is 512. A single 768 px vision request then peaks near 7.3 GiB of
/// compute graph on top of ~3.0 GiB of weights, which does not fit reliably in
/// 12 GB: the runner aborts (`terminate called ... std::runtime_error`, SIGABRT,
/// `exit status 2`) and every later request pays a model reload. Measured on one
/// 191-file folder: **139 runner spawns and 6 aborts in 8 minutes**, with only
/// 534 MB of the 12 GB free.
///
/// 128 keeps the peak small enough to survive and costs a few percent of prefill
/// throughput. This must be sent per request: the `OLLAMA_NUM_BATCH` environment
/// variable had no effect on the runner used here (the journal still logged
/// `BatchSize:512` with the drop-in in place).
const NUM_BATCH: u32 = 128;

/// `options` block of the request body.
///
/// Split out so the wiring (`num_ctx` omitted vs present, batch always present)
/// is testable without an HTTP round trip.
fn options_payload(params: &GenerateParams) -> Value {
    let mut options = serde_json::json!({
        "temperature": params.temperature,
        "num_predict": params.num_predict,
        "num_batch": NUM_BATCH,
    });
    if let Some(ctx) = params.num_ctx {
        options["num_ctx"] = serde_json::json!(ctx);
    }
    if let Some(penalty) = params.repeat_penalty {
        options["repeat_penalty"] = serde_json::json!(penalty);
    }
    if let Some(last_n) = params.repeat_last_n {
        options["repeat_last_n"] = serde_json::json!(last_n);
    }
    options
}

/// Failure of one HTTP attempt at `/api/generate`.
struct AttemptError {
    message: String,
    status: Option<u16>,
    /// The response headers arrived but the body did not come through.
    body_lost: bool,
    /// Ollama answered 200 with its zero-value frame (`model: ""`, `done: false`).
    ///
    /// The runner answered without doing any work. Measured on Ollama 0.40.11: after
    /// some minutes of scanning the runner wedges, and from then on **every** request
    /// comes back like this while the journal logs either 200 or 500. A fresh model
    /// load fixes it, which is why this specific failure triggers a reload instead of
    /// a retry.
    degenerate: bool,
}

impl AttemptError {
    fn timed_out(&self) -> bool {
        self.message.contains("timed out") || self.message.contains("timeout")
    }

    /// Worth one more attempt **without** the JSON grammar.
    ///
    /// Three shapes reach here when the runner aborts mid-request: a 5xx, a
    /// transport error (no status at all) and a body that dies half way. Only the
    /// last two appear when the server closes the socket instead of answering, and
    /// Ollama 0.40 does exactly that when the grammar sampler fails — the journal
    /// logs a 500 while the client sees EOF. Timeouts are excluded: waiting another
    /// five minutes helps nobody.
    fn worth_retrying(&self) -> bool {
        if self.timed_out() {
            return false;
        }
        if self.body_lost {
            return true;
        }
        match self.status {
            Some(code) => (500..=504).contains(&code),
            None => true,
        }
    }
}

/// One `/api/generate` round trip.
///
/// The body is serialized here, compactly, and sent as bytes. `send_json` was doing
/// the same thing internally (`serde_json::to_vec_pretty` + `Content-Length`), but it
/// was the last variable left between a request that works outside the app and one
/// that made Ollama answer with a zero-value response (`"model":""`), so the wire
/// format is now explicit and identical to a `curl --data-binary`.
fn send_once(agent: &ureq::Agent, payload: &Value) -> Result<OllamaResponse, AttemptError> {
    let body = serde_json::to_vec(payload).map_err(|e| AttemptError {
        message: format!("serialize_error: {e}"),
        status: None,
        body_lost: false,
        degenerate: false,
    })?;
    let result = agent
        .post(&format!("{}/api/generate", super::ollama_base()))
        .header("Content-Type", "application/json")
        .send(&body);
    let mut resp = match result {
        Ok(resp) => resp,
        Err(e) => {
            return Err(AttemptError {
                message: format!("http_error: {e}"),
                status: None,
                body_lost: false,
                degenerate: false,
            });
        }
    };
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().map_err(|e| AttemptError {
        message: format!("read_error: {e}"),
        status: Some(status),
        body_lost: true,
        degenerate: false,
    })?;
    if !(200..300).contains(&status) {
        // Ollama answers with `{"error": "..."}` and the reason is the only thing
        // that makes a 500 debuggable — dropping the body left us guessing between
        // the grammar, VRAM and the payload.
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| body.trim().chars().take(300).collect());
        return Err(AttemptError {
            message: format!("http_error: status {status}: {detail}"),
            status: Some(status),
            body_lost: false,
            degenerate: false,
        });
    }
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let raw = parsed
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Ollama's zero-value frame: no model echoed, nothing generated, not done. The
    // runner answered without doing any work — it is wedged, and the only cure is a
    // fresh load. Measured on 0.40.11: after ~10 minutes of scanning this starts
    // happening on every request, and it stays that way until the model is reloaded.
    let degenerate = raw.is_empty()
        && parsed.get("model").and_then(|v| v.as_str()) == Some("")
        && !parsed
            .get("done")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    if degenerate {
        log::warn!(
            "Ollama answered 200 with its zero-value frame (sent model={:?} prompt={} B \
             image={} B body={} B); body: {}",
            payload.get("model").and_then(|v| v.as_str()).unwrap_or(""),
            payload
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(str::len)
                .unwrap_or(0),
            payload
                .get("images")
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_str())
                .map(str::len)
                .unwrap_or(0),
            serde_json::to_string(payload).map(|s| s.len()).unwrap_or(0),
            body.chars().take(200).collect::<String>()
        );
        return Err(AttemptError {
            message: "empty_frame: Ollama answered with its zero-value response".to_string(),
            status: Some(status),
            body_lost: false,
            degenerate: true,
        });
    }
    // A real answer: the runner is healthy again, so the next wedge may reload at
    // once instead of waiting out the throttle.
    super::management::note_success();
    const NS: f64 = 1_000_000_000.0;
    let ns = |key: &str| parsed.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) / NS;
    Ok(OllamaResponse {
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
    })
}

/// Run one generation request, retrying once without the JSON grammar.
///
/// Ollama's constrained sampler aborts often enough to matter: with 0.23.1 it
/// killed the runner (`Unexpected empty grammar stack after accepting piece`, 236
/// times in one morning, each a lost request plus a ~7.3 GiB reload) and with
/// 0.40.11 the same failure comes back as a 500 that kills a third of the requests
/// of a scan. The grammar only shapes the output syntax, so dropping it and relying
/// on [`parse_json_response`] + the salvage path in `classify` loses nothing but a
/// slightly longer free-form answer.
///
/// The retry is deliberately narrow: a 400 (bad payload) and a timeout are not
/// worth repeating — see [`AttemptError::worth_retrying`].
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
        "options": options_payload(params),
    });

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
        // We read the error body ourselves in `send_once`: ureq's status error
        // throws it away, and it is where Ollama explains the failure.
        .http_status_as_error(false)
        .build()
        .into();

    match send_once(&agent, &payload) {
        Ok(resp) => resp,
        // The runner is wedged: no retry of any kind helps, only a fresh model load.
        //
        // Two cycles, because one was not enough in practice: job #35 (2026-10-28) has
        // the request sent 3 s after a reload coming back with the zero-value frame
        // *again*, and the caller then burned a whole classify attempt on it. The first
        // cycle respects the reload throttle (a runner that wedges immediately has to
        // stop the pass, not spin); the second bypasses it, because by then the caller
        // has just seen the freshly loaded runner fail and reusing the old one is
        // strictly worse.
        Err(err) if err.degenerate => {
            let mut wedged = err;
            for cycle in 1..=2u8 {
                let reloaded = if cycle == 1 {
                    super::management::reload_model(model)
                } else {
                    super::management::reload_model_now(model)
                };
                if !reloaded {
                    // Throttled: the previous reload did not take. Retrying the same
                    // wedged runner would only burn the caller's attempts.
                    return OllamaResponse::error(wedged.message);
                }
                log::warn!(
                    "Reloaded {model} after a zero-value frame (cycle {cycle} of 2); \
                     retrying the request"
                );
                match send_once(&agent, &payload) {
                    Ok(resp) => return resp,
                    Err(again) if again.degenerate => {
                        log::warn!("Still getting the zero-value frame after the reload");
                        wedged = again;
                    }
                    Err(again) => {
                        log::error!("Retry after reload failed: {}", again.message);
                        return OllamaResponse::error(again.message);
                    }
                }
            }
            log::error!("Gave up on {model}: the zero-value frame survived two reloads");
            OllamaResponse::error(wedged.message)
        }
        Err(err) if params.json_mode && err.worth_retrying() => {
            match err.status {
                Some(code) => log::warn!(
                    "Ollama answered {code} for a json-constrained request; retrying once \
                     without `format: json` (see plans/pipeline-cpu-gpu-overlap.md §5.7)"
                ),
                // No status at all: nothing is listening where we are pointing.
                None => log::error!(
                    "Ollama is unreachable at {} ({}); retrying once without `format: json`",
                    super::ollama_base(),
                    err.message
                ),
            }
            let mut plain = payload.clone();
            if let Some(obj) = plain.as_object_mut() {
                obj.remove("format");
            }
            match send_once(&agent, &plain) {
                Ok(resp) => {
                    log::info!("Retry without the JSON grammar succeeded");
                    resp
                }
                Err(again) => {
                    log::error!("Retry without the JSON grammar failed: {}", again.message);
                    OllamaResponse::error(again.message)
                }
            }
        }
        Err(err) => OllamaResponse::error(err.message),
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
    fn num_ctx_for_downscaled_images_uses_4096() {
        // The 1500-token output reservation, not the image size, sets the floor:
        // 640 px and 768 px both land on 4096 so the runner is never rebuilt
        // between a photo and an OCR pass.
        assert_eq!(num_ctx_for(640, 640, 250, 180), 4096);
        assert_eq!(num_ctx_for(768, 768, 250, 180), 4096);
        assert_eq!(num_ctx_for(576, 1024, 400, 180), 4096);
    }

    #[test]
    fn num_ctx_for_1024px_uses_4096() {
        assert_eq!(num_ctx_for(1024, 1024, 400, 180), 4096);
    }

    #[test]
    fn num_ctx_for_is_clamped() {
        // The output reservation alone already exceeds the minimum.
        assert_eq!(num_ctx_for(16, 16, 0, 8), 4096);
        assert_eq!(num_ctx_for(4000, 4000, 4000, 4096), MAX_NUM_CTX);
    }

    /// Every stage of a run must resolve to the same context, otherwise Ollama
    /// reloads the whole model between requests (measured: 316 loads in 15 min).
    #[test]
    fn num_ctx_is_stable_across_stages() {
        // 768 px, the recommended default.
        let primary = num_ctx_for(768, 768, 1600, 180);
        let ocr = num_ctx_for(768, 768, 1600, 1500);
        let anime = num_ctx_for(768, 768, 1600, 500);
        assert_eq!(primary, ocr);
        assert_eq!(primary, anime);

        // 1024 px as well, and a small image that is not upscaled.
        assert_eq!(
            num_ctx_for(1024, 768, 1600, 180),
            num_ctx_for(1024, 768, 1600, 1500)
        );
        assert_eq!(
            num_ctx_for(640, 480, 1600, 180),
            num_ctx_for(640, 480, 1600, 1500)
        );
    }

    /// The batch must travel with every request: at the server default of 512 the
    /// runner aborted (SIGABRT) on a 12 GB card and reloaded the model each time.
    #[test]
    fn options_payload_always_carries_a_small_batch() {
        let without_ctx = options_payload(&GenerateParams::vision(180, None));
        assert_eq!(without_ctx["num_batch"], serde_json::json!(NUM_BATCH));
        assert_eq!(options_payload_batch(&without_ctx), 128);
        assert!(without_ctx.get("num_ctx").is_none());

        let with_ctx = options_payload(&GenerateParams::vision(180, Some(4096)));
        assert_eq!(with_ctx["num_ctx"], serde_json::json!(4096));
        assert_eq!(options_payload_batch(&with_ctx), 128);
    }

    fn options_payload_batch(options: &Value) -> u64 {
        options["num_batch"]
            .as_u64()
            .expect("num_batch must be a number")
    }

    /// A 5xx, an EOF and a half-read body all mean the runner died mid-request; a
    /// 400 (bad payload) or a timeout must not be retried.
    #[test]
    fn only_a_broken_response_earns_a_retry_without_the_grammar() {
        let attempt = |status: Option<u16>, body_lost: bool, message: &str| AttemptError {
            message: message.to_string(),
            status,
            body_lost,
            degenerate: false,
        };
        assert!(attempt(Some(500), false, "").worth_retrying());
        assert!(attempt(Some(503), false, "").worth_retrying());
        assert!(attempt(None, false, "http_error: connection reset").worth_retrying());
        assert!(
            attempt(Some(200), true, "read_error: eof").worth_retrying(),
            "a body that dies half way means the same thing as a 500"
        );
        assert!(!attempt(Some(400), false, "").worth_retrying());
        assert!(
            !attempt(None, true, "read_error: timed out").worth_retrying(),
            "a timeout already waited the full budget"
        );
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
