use crate::*;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ModelTestMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Serialize)]
pub(crate) struct ModelTestReply {
    pub(crate) content: String,
    pub(crate) first_token_ms: u64,
    pub(crate) latency_ms: u64,
}

#[derive(Clone, Serialize)]
pub(crate) struct ModelTestStreamEvent {
    pub(crate) request_id: String,
    pub(crate) delta: String,
    pub(crate) first_token_ms: Option<u64>,
}

#[derive(Default)]
pub(crate) struct ModelTestStreamRegistry {
    pub(crate) active: BTreeMap<String, CancelToken>,
    pub(crate) pending_cancellations: BTreeSet<String>,
}

impl ModelTestStreamRegistry {
    pub(crate) fn register(&mut self, request_id: &str, token: CancelToken) -> Result<(), String> {
        if self.pending_cancellations.remove(request_id) {
            return Err("Model test cancelled".to_owned());
        }
        if self.active.contains_key(request_id) {
            return Err("This model test request is already active".to_owned());
        }
        if self.active.len() >= MODEL_TEST_MAX_ACTIVE_STREAMS {
            return Err("Too many model test requests are active".to_owned());
        }
        self.active.insert(request_id.to_owned(), token);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, request_id: String) {
        if let Some(token) = self.active.get(&request_id).cloned() {
            token.cancel();
            return;
        }
        if self.pending_cancellations.len() >= MODEL_TEST_MAX_PENDING_CANCELLATIONS {
            let eviction = self.pending_cancellations.iter().next().cloned();
            if let Some(eviction) = eviction {
                self.pending_cancellations.remove(&eviction);
            }
        }
        self.pending_cancellations.insert(request_id);
    }
}

#[derive(Clone, Default)]
pub(crate) struct ModelTestStreamState(pub(crate) Arc<Mutex<ModelTestStreamRegistry>>);

pub(crate) struct ModelTestStreamRegistration {
    pub(crate) registry: Arc<Mutex<ModelTestStreamRegistry>>,
    pub(crate) request_id: String,
}

impl Drop for ModelTestStreamRegistration {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        registry.active.remove(&self.request_id);
        registry.pending_cancellations.remove(&self.request_id);
    }
}

pub(crate) const MODEL_TEST_MAX_MESSAGES: usize = 20;
pub(crate) const MODEL_TEST_MAX_MESSAGE_BYTES: usize = 16_000;
pub(crate) const MODEL_TEST_MAX_TOTAL_BYTES: usize = 64_000;
pub(crate) const MODEL_TEST_MAX_REQUEST_ID_BYTES: usize = 64;
pub(crate) const MODEL_TEST_MAX_ACTIVE_STREAMS: usize = 4;
pub(crate) const MODEL_TEST_MAX_PENDING_CANCELLATIONS: usize = 32;
pub(crate) const MODEL_TEST_MAX_SSE_BUFFER_BYTES: usize = 1_048_576;
pub(crate) const MODEL_TEST_MAX_RESPONSE_BYTES: usize = 16_000;
pub(crate) const MODEL_TEST_MAX_STREAM_EVENTS: usize = 1_024;
pub(crate) const MODEL_TEST_MAX_STREAM_BYTES: usize = 4 * 1_048_576;
pub(crate) const MODEL_TEST_STREAM_EVENT: &str = "model-test-stream";

#[derive(Default)]
pub(crate) struct ModelTestOutputBudget {
    pub(crate) bytes: usize,
    pub(crate) events: usize,
    pub(crate) wire_bytes: usize,
}

impl ModelTestOutputBudget {
    pub(crate) fn accept_wire(&mut self, bytes: usize) -> Result<(), String> {
        let next_bytes = self.wire_bytes.saturating_add(bytes);
        if next_bytes > MODEL_TEST_MAX_STREAM_BYTES {
            return Err("The model stream exceeded the wire response limit".to_owned());
        }
        self.wire_bytes = next_bytes;
        Ok(())
    }

    pub(crate) fn accept(&mut self, delta: &str) -> Result<(), String> {
        let next_bytes = self.bytes.saturating_add(delta.len());
        if next_bytes > MODEL_TEST_MAX_RESPONSE_BYTES {
            return Err("The model output exceeded the response limit".to_owned());
        }
        if self.events >= MODEL_TEST_MAX_STREAM_EVENTS {
            return Err("The model output exceeded the stream event limit".to_owned());
        }
        self.bytes = next_bytes;
        self.events += 1;
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct ModelTestSseDecoder {
    pub(crate) buffer: Vec<u8>,
}

impl ModelTestSseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MODEL_TEST_MAX_SSE_BUFFER_BYTES {
            return Err("The model stream exceeded the response buffer limit".to_owned());
        }
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some((boundary, delimiter_len)) = find_model_test_sse_boundary(&self.buffer) {
            let frame = self.buffer.drain(..boundary).collect::<Vec<_>>();
            self.buffer.drain(..delimiter_len);
            let frame = String::from_utf8(frame)
                .map_err(|_| "The model stream returned invalid UTF-8".to_owned())?;
            if !frame.trim().is_empty() {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<String>, String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            self.buffer.clear();
            return Ok(Vec::new());
        }
        let frame = String::from_utf8(std::mem::take(&mut self.buffer))
            .map_err(|_| "The model stream ended with invalid UTF-8".to_owned())?;
        Ok(vec![frame])
    }
}

pub(crate) fn find_model_test_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

pub(crate) fn validate_model_test_request_id(request_id: &str) -> Result<(), String> {
    if request_id.is_empty()
        || request_id.len() > MODEL_TEST_MAX_REQUEST_ID_BYTES
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("The model test request ID is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn model_test_stream_delta(frame: &str) -> Result<Option<String>, String> {
    let data = frame
        .lines()
        .filter_map(|line| {
            let data = line.strip_prefix("data:")?;
            Some(data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|_| "The model stream returned invalid JSON".to_owned())?;
    if value.get("error").is_some() {
        return Err("The Provider returned a stream error".to_owned());
    }
    let content = value.pointer("/choices/0/delta/content");
    if let Some(text) = content
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Ok(Some(text.to_owned()));
    }
    if let Some(parts) = content.and_then(Value::as_array) {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        if !text.is_empty() {
            return Ok(Some(text));
        }
    }
    Ok(value
        .pointer("/choices/0/text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned))
}

pub(crate) fn emit_model_test_frames<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    started: Instant,
    frames: Vec<String>,
    content: &mut String,
    first_token_ms: &mut Option<u64>,
    output_budget: &mut ModelTestOutputBudget,
) -> Result<(), String> {
    for frame in frames {
        let Some(delta) = model_test_stream_delta(&frame)? else {
            continue;
        };
        output_budget.accept(&delta)?;
        if first_token_ms.is_none() {
            *first_token_ms =
                Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
        content.push_str(&delta);
        app.emit(
            MODEL_TEST_STREAM_EVENT,
            ModelTestStreamEvent {
                request_id: request_id.to_owned(),
                delta,
                first_token_ms: *first_token_ms,
            },
        )
        .map_err(|_| "The model stream could not reach the test console".to_owned())?;
    }
    Ok(())
}

pub(crate) fn validate_model_test_messages(messages: &[ModelTestMessage]) -> Result<(), String> {
    if messages.is_empty() {
        return Err("Enter a message before sending".to_owned());
    }
    if messages.len() > MODEL_TEST_MAX_MESSAGES {
        return Err(format!(
            "A model test supports at most {MODEL_TEST_MAX_MESSAGES} messages"
        ));
    }

    let mut total_bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("Model test messages support only user and assistant roles".to_owned());
        }
        let expected_role = if index % 2 == 0 { "user" } else { "assistant" };
        if message.role != expected_role {
            return Err("Model test messages must alternate user and assistant roles".to_owned());
        }
        let message_bytes = message.content.len();
        if message_bytes == 0 || message.content.trim().is_empty() {
            return Err("Model test messages cannot be empty".to_owned());
        }
        if message_bytes > MODEL_TEST_MAX_MESSAGE_BYTES {
            return Err(format!(
                "One model test message exceeds {MODEL_TEST_MAX_MESSAGE_BYTES} bytes"
            ));
        }
        total_bytes = total_bytes.saturating_add(message_bytes);
        if total_bytes > MODEL_TEST_MAX_TOTAL_BYTES {
            return Err(format!(
                "The model test conversation exceeds {MODEL_TEST_MAX_TOTAL_BYTES} bytes"
            ));
        }
    }
    if messages.last().map(|message| message.role.as_str()) != Some("user") {
        return Err("The last model test message must be from the user".to_owned());
    }
    Ok(())
}

/// Pay-per-token providers report an exhausted wallet as HTTP 429 rather than
/// 402, where the retry advice of the rate-limit summary is wrong: waiting
/// never helps an empty account. Classify from the error body's stable fields
/// without ever echoing its text into the summary the console shows.
pub(crate) fn model_test_error_is_exhausted_balance(body: &Value) -> bool {
    let error = body.get("error").unwrap_or(body);
    if error.get("type").and_then(Value::as_str) == Some("insufficient_quota") {
        return true;
    }
    if ["code", "internal_code"].iter().any(|field| {
        error
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|code| {
                code.eq_ignore_ascii_case("credit_balance_exhausted")
                    || code.eq_ignore_ascii_case("platform_balance_insufficient")
            })
    }) {
        return true;
    }
    error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            let lowered = message.to_lowercase();
            lowered.contains("insufficient balance")
                || lowered.contains("balance is insufficient")
                || message.contains("余额不足")
        })
}

pub(crate) fn model_test_http_error(status: u16, body: &Value) -> String {
    let summary = if status == 429 && model_test_error_is_exhausted_balance(body) {
        "The Provider account has no available balance"
    } else {
        match status {
            400 => "The model rejected the request. Check the prompt and model limits",
            401 | 403 => "Provider authentication failed. Check this Provider credential",
            402 => "The Provider account has no available balance",
            404 => "The selected model or endpoint is unavailable",
            408 | 504 => "The model request timed out",
            409 => "The Provider rejected the current request state",
            429 => "The Provider rate limit is active. Try again later",
            500..=599 => "The Provider is temporarily unavailable",
            _ => "The model request failed",
        }
    };
    format!("{summary} (HTTP {status})")
}

pub(crate) fn model_test_assistant_content(body: &Value) -> Option<String> {
    let content = body.pointer("/choices/0/message/content")?;
    if let Some(text) = content.as_str().filter(|text| !text.trim().is_empty()) {
        return Some(text.to_owned());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!parts.trim().is_empty()).then_some(parts)
}

pub(crate) fn extract_model_test_reply(status: u16, body: &str) -> Result<String, String> {
    if body.len() > MODEL_TEST_MAX_STREAM_BYTES {
        return Err("The model response exceeded the wire response limit".to_owned());
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|_| format!("The model returned invalid JSON (HTTP {status})"))?;
    if !(200..300).contains(&status) {
        return Err(model_test_http_error(status, &value));
    }
    let content = model_test_assistant_content(&value)
        .ok_or_else(|| "The model returned no assistant text".to_owned())?;
    if content.len() > MODEL_TEST_MAX_RESPONSE_BYTES {
        return Err("The model output exceeded the response limit".to_owned());
    }
    Ok(content)
}

#[tauri::command]
pub(crate) async fn test_model_chat_stream(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
    stream_state: State<'_, ModelTestStreamState>,
    upstream: String,
    model: String,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    run_model_test_chat(
        app,
        state.inner(),
        stream_state.inner(),
        upstream,
        model,
        messages,
        request_id,
    )
    .await
}

pub(crate) async fn run_model_test_chat<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    stream_state: &ModelTestStreamState,
    upstream: String,
    model: String,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    validate_model_test_messages(&messages)?;
    validate_model_test_request_id(&request_id)?;
    let upstream = upstream.trim().to_owned();
    let model = model.trim().to_owned();
    let mut config = {
        let inner = state.0.lock().unwrap();
        let config = inner.materialize()?;
        let provider = config
            .upstreams
            .get(&upstream)
            .ok_or_else(|| format!("Provider `{upstream}` is no longer configured"))?;
        if !provider
            .models
            .iter()
            .any(|candidate| candidate.model == model)
        {
            return Err(format!(
                "Model `{model}` is no longer configured for Provider `{upstream}`"
            ));
        }
        config
    };

    let target = UpstreamModel::new(
        UpstreamRef::new(upstream.clone())
            .map_err(|error| format!("Provider name is invalid: {error}"))?,
        model.clone(),
    );
    config.routing = Some(HostRoutingConfig {
        mode: HostRoutingMode::Direct,
        direct_target: Some(target),
    });
    config.router.local_only = false;
    config.router.allow_cloud_fallback = false;

    let request_context =
        RequestContext::detached(Duration::from_secs(120), Duration::from_secs(120))
            .with_upstream_response_limit(MODEL_TEST_MAX_STREAM_BYTES as u64);
    let registry = Arc::clone(&stream_state.0);
    {
        let mut streams = registry.lock().unwrap();
        streams.register(&request_id, request_context.token())?;
    }
    let registration = ModelTestStreamRegistration {
        registry,
        request_id: request_id.clone(),
    };

    let provider_runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        let _registration = registration;
        let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
        let gateway = Gateway::new_with_provider_runtime(&config, recorder, provider_runtime)?;
        let body = serde_json::to_vec(&json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "max_tokens": 1024
        }))
        .map_err(|_| "Failed to encode the model test request".to_owned())?;
        let started = Instant::now();
        let mut json_response = None;
        let mut decoder = ModelTestSseDecoder::default();
        let mut content = String::new();
        let mut first_token_ms = None;
        let mut output_budget = ModelTestOutputBudget::default();
        let mut stream_error = None;
        gateway.chat_scoped(
            &request_context,
            None,
            None,
            "POST",
            "/v1/chat/completions",
            &[("content-type".to_owned(), "application/json".to_owned())],
            &body,
            &mut |reply| {
                if request_context.is_cancelled() {
                    return false;
                }
                match reply {
                    Reply::BeginJson(reply) => {
                        json_response = Some((reply.status, reply.body));
                    }
                    Reply::BeginStream => {}
                    Reply::Chunk(chunk) => match output_budget
                        .accept_wire(chunk.len())
                        .and_then(|()| decoder.push(chunk.as_bytes()))
                        .and_then(|frames| {
                            emit_model_test_frames(
                                &app,
                                &request_id,
                                started,
                                frames,
                                &mut content,
                                &mut first_token_ms,
                                &mut output_budget,
                            )
                        }) {
                        Ok(()) => {}
                        Err(error) => {
                            stream_error = Some(error);
                            request_context.cancel();
                            return false;
                        }
                    },
                }
                true
            },
        );
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(error) = stream_error {
            return Err(error);
        }
        if let Some(reason) = request_context.cancel_reason() {
            return Err(match reason {
                CancelReason::Deadline => "The model request timed out".to_owned(),
                CancelReason::ClientDisconnect | CancelReason::ServerDrain => {
                    "Model test cancelled".to_owned()
                }
            });
        }
        if let Some((status, body)) = json_response {
            let content = extract_model_test_reply(status, &body)?;
            return Ok(ModelTestReply {
                content,
                first_token_ms: latency_ms,
                latency_ms,
            });
        }
        let frames = decoder.finish()?;
        emit_model_test_frames(
            &app,
            &request_id,
            started,
            frames,
            &mut content,
            &mut first_token_ms,
            &mut output_budget,
        )?;
        if content.trim().is_empty() {
            return Err("The model returned no assistant text".to_owned());
        }
        Ok(ModelTestReply {
            content,
            first_token_ms: first_token_ms.unwrap_or(latency_ms),
            latency_ms,
        })
    })
    .await
    .map_err(|error| format!("Model test task stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub(crate) fn cancel_model_test_chat(
    stream_state: State<'_, ModelTestStreamState>,
    request_id: String,
) -> Result<(), String> {
    validate_model_test_request_id(&request_id)?;
    let mut streams = stream_state.0.lock().unwrap();
    streams.cancel(request_id);
    Ok(())
}
