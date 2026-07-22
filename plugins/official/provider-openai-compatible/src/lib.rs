//! The official OpenAI-compatible provider adapter.
//!
//! Southbound only: the Canonical IR in, one provider's HTTP dialect out, and
//! back. Everything here is protocol translation — routing, budget, billing
//! and the credential itself stay in the host, which is what the sandbox this
//! runs in enforces.
//!
//! The logic is deliberately the same as the native reference adapter in
//! `token-station-conformance`'s own tests. The fixture pack in `fixtures/`
//! pins both to identical outputs, so they cannot drift apart silently: a
//! divergence fails one suite or the other.

use std::sync::Mutex;

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "provider-adapter-v1",
});

use exports::token_station::adapter::provider_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Map, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    Auth, ChatRequest, ChatResponse, Choice, Content, ErrorCode, ErrorEnvelope, Extensions,
    FinishReason, HttpMethod, HttpRequestDescriptor, HttpResponseParts, Message, ProviderApi,
    ProviderConfig, Role, SafeHeaders, StreamChunk, StreamEvent, ToolCall, Usage,
};

/// The unparsed tail and finish reason of the stream this instance is holding.
///
/// Instance state on purpose: the host instantiates one component per stream,
/// so this state only ever sees one provider's body.
#[derive(Default)]
struct ProviderStreamState {
    tail: String,
    pending_finish: PendingFinish,
}

/// Tracks finish arrival separately from its canonical mapping. Compatible
/// providers may add finish strings we do not know yet; those still close the
/// stream with `Done { finish_reason: None }`.
#[derive(Default)]
struct PendingFinish {
    seen: bool,
    reason: Option<FinishReason>,
    done_emitted: bool,
}

impl PendingFinish {
    fn record(&mut self, raw: &str) {
        self.seen = true;
        self.reason = finish_reason(Some(raw));
        self.done_emitted = false;
    }

    fn take_done(&mut self) -> Option<StreamEvent> {
        if !self.seen {
            return None;
        }
        self.seen = false;
        self.done_emitted = true;
        Some(StreamEvent::Done {
            finish_reason: self.reason.take(),
        })
    }

    fn finish_marker(&mut self) -> Option<StreamEvent> {
        let done = self.take_done().or_else(|| {
            (!self.done_emitted).then_some(StreamEvent::Done {
                finish_reason: None,
            })
        });
        self.seen = false;
        self.reason = None;
        self.done_emitted = false;
        done
    }
}

static STREAM_STATE: Mutex<ProviderStreamState> = Mutex::new(ProviderStreamState {
    tail: String::new(),
    pending_finish: PendingFinish {
        seen: false,
        reason: None,
        done_emitted: false,
    },
});

struct OpenAiCompatible;

// -- error plumbing -------------------------------------------------------------

/// The error channel carries a `protocol::ErrorEnvelope`, serialized.
fn fail(envelope: &ErrorEnvelope) -> String {
    serde_json::to_string(envelope).unwrap_or_else(|_| {
        r#"{"code":"internal","http_status":500,"message":"unserializable error"}"#.to_owned()
    })
}

fn internal(detail: impl std::fmt::Display) -> String {
    fail(&ErrorEnvelope::new(
        ErrorCode::Internal,
        500,
        detail.to_string(),
    ))
}

fn provider_protocol_error(message: &'static str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

fn protocol_error(message: &'static str) -> String {
    fail(&provider_protocol_error(message))
}

fn parse_input<T: for<'de> serde::Deserialize<'de>>(input: &str) -> Result<T, String> {
    serde_json::from_str(input).map_err(internal)
}

fn to_output<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(internal)
}

// -- translation ----------------------------------------------------------------

fn finish_reason(raw: Option<&str>) -> Option<FinishReason> {
    match raw? {
        "stop" => Some(FinishReason::Stop),
        "length" => Some(FinishReason::Length),
        "tool_calls" => Some(FinishReason::ToolCalls),
        "content_filter" => Some(FinishReason::ContentFilter),
        _ => None,
    }
}

fn index_of(value: &Value) -> u32 {
    u32::try_from(value.as_u64().unwrap_or(0)).unwrap_or(0)
}

/// A `Message` in the OpenAI request dialect.
fn message_to_openai(message: &Message) -> Value {
    let mut out = Map::new();
    out.insert("role".to_owned(), json!(message.role));
    if let Some(content) = &message.content {
        out.insert(
            "content".to_owned(),
            match content {
                Content::Text(text) => json!(text),
                Content::Parts(parts) => json!(parts),
            },
        );
    }
    if !message.tool_calls.is_empty() {
        let calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments},
                })
            })
            .collect();
        out.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    if let Some(id) = &message.tool_call_id {
        out.insert("tool_call_id".to_owned(), json!(id));
    }
    if let Some(name) = &message.name {
        out.insert("name".to_owned(), json!(name));
    }
    Value::Object(out)
}

fn body_of(request: &ChatRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(request.model));
    body.insert(
        "messages".to_owned(),
        Value::Array(request.messages.iter().map(message_to_openai).collect()),
    );
    if request.stream {
        body.insert("stream".to_owned(), json!(true));
        body.insert("stream_options".to_owned(), json!({"include_usage": true}));
    }
    if let Some(temperature) = request.sampling.temperature {
        body.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(top_p) = request.sampling.top_p {
        body.insert("top_p".to_owned(), json!(top_p));
    }
    if let Some(max_tokens) = request.sampling.max_output_tokens {
        body.insert("max_tokens".to_owned(), json!(max_tokens));
    }
    if !request.sampling.stop.is_empty() {
        body.insert("stop".to_owned(), json!(request.sampling.stop));
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }
    Value::Object(body)
}

fn usage_of(raw: &Value) -> Usage {
    Usage {
        input_tokens: raw["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: raw["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: raw["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .or_else(|| raw["prompt_cache_hit_tokens"].as_u64())
            .unwrap_or(0),
        cache_write_tokens: raw["prompt_tokens_details"]["cache_write_tokens"]
            .as_u64()
            .unwrap_or(0),
        reasoning_tokens: raw["completion_tokens_details"]["reasoning_tokens"]
            .as_u64()
            .unwrap_or(0),
    }
}

/// One complete SSE frame's worth of events. A finish reason is held until
/// cumulative usage arrives (or the provider emits `[DONE]`) so downstream
/// adapters never close their stream before the final accounting event.
fn events_of_frame(
    payload: &str,
    pending_finish: &mut PendingFinish,
) -> Result<Vec<StreamEvent>, String> {
    let raw: Value = serde_json::from_str(payload)
        .map_err(|_| protocol_error("the upstream emitted invalid SSE JSON"))?;
    if raw.get("error").is_some_and(|error| !error.is_null()) {
        return Err(protocol_error(
            "the upstream embedded an error in a successful SSE response",
        ));
    }
    let mut events = Vec::new();

    let usage = match raw.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage @ Value::Object(_)) => Some(usage),
        Some(_) => {
            return Err(protocol_error(
                "the upstream SSE event has an invalid usage field",
            ));
        }
    };
    let choices = match raw.get("choices") {
        Some(Value::Array(choices)) => Some(choices),
        None if usage.is_some() => None,
        _ => {
            return Err(protocol_error(
                "the upstream SSE event has an invalid choices field",
            ));
        }
    };
    if choices.is_some_and(Vec::is_empty) && usage.is_none() {
        return Err(protocol_error("the upstream SSE event contains no choices"));
    }
    for choice in choices.into_iter().flatten() {
        let index = index_of(&choice["index"]);
        let delta = &choice["delta"];

        if let Some(text) = delta["content"].as_str() {
            events.push(StreamEvent::Delta {
                index,
                content: text.to_owned(),
            });
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            events.push(StreamEvent::ToolCallDelta {
                // The tool call's own index, not the choice's: a single choice
                // may stream several calls at once.
                index: index_of(&call["index"]),
                id: call["id"].as_str().map(str::to_owned),
                name: call["function"]["name"].as_str().map(str::to_owned),
                arguments_delta: call["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            pending_finish.record(reason);
        }
    }

    if let Some(usage) = usage {
        events.push(StreamEvent::Usage {
            usage: usage_of(usage),
        });
        if let Some(done) = pending_finish.take_done() {
            events.push(done);
        }
    }
    Ok(events)
}

fn sse_frame_boundary(buffer: &str) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (_, Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, None) => None,
    }
}

impl Guest for OpenAiCompatible {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "provider-openai-compatible".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Provider,
            api_version: "provider-adapter-v1".to_owned(),
        }
    }

    fn healthcheck() -> AdapterHealth {
        AdapterHealth {
            status: HealthStatus::Ready,
            detail: None,
        }
    }

    fn model_capabilities(provider_config: String) -> Result<String, String> {
        let config: ProviderConfig = parse_input(&provider_config)?;
        // No network, so the upstream's own catalog is unreachable. What its
        // operator declared is all there is.
        to_output(&config.models)
    }

    fn build_http_request(chat_request: String, provider_config: String) -> Result<String, String> {
        let request: ChatRequest = parse_input(&chat_request)?;
        let config: ProviderConfig = parse_input(&provider_config)?;

        let mut descriptor = HttpRequestDescriptor::new(
            HttpMethod::Post,
            config.base_url.resolve(ProviderApi::ChatCompletions),
        );
        descriptor.headers =
            SafeHeaders::try_new([("content-type", "application/json")]).map_err(internal)?;
        descriptor.body = Some(body_of(&request));
        // The host holds the value; this names the slot and the dialect.
        descriptor.auth = config.auth.clone().map(Auth::bearer);
        to_output(&descriptor)
    }

    fn parse_response(response_parts: String) -> Result<String, String> {
        let parts: HttpResponseParts = parse_input(&response_parts)?;
        let raw: Value = serde_json::from_str(&parts.body)
            .map_err(|_| protocol_error("the upstream returned invalid JSON in a 2xx response"))?;
        if raw.get("error").is_some_and(|error| !error.is_null()) {
            return Err(protocol_error(
                "the upstream embedded an error in a successful response",
            ));
        }
        let raw_choices = raw["choices"]
            .as_array()
            .ok_or_else(|| protocol_error("the upstream 2xx response has no choices array"))?;
        if raw_choices.is_empty() {
            return Err(protocol_error(
                "the upstream 2xx response contains no choices",
            ));
        }

        let mut choices = Vec::new();
        for choice in raw_choices {
            let message = &choice["message"];
            let tool_calls = message["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|call| ToolCall {
                    id: call["id"].as_str().unwrap_or_default().to_owned(),
                    name: call["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: call["function"]["arguments"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                })
                .collect();

            choices.push(Choice {
                index: index_of(&choice["index"]),
                message: Message {
                    role: Role::Assistant,
                    content: message["content"]
                        .as_str()
                        .map(|text| Content::Text(text.to_owned())),
                    tool_calls,
                    tool_call_id: None,
                    name: None,
                    extensions: Extensions::new(),
                },
                finish_reason: finish_reason(choice["finish_reason"].as_str()),
            });
        }

        let usage = usage_of(&raw["usage"]);
        to_output(&ChatResponse {
            id: raw["id"].as_str().unwrap_or_default().to_owned(),
            model: raw["model"].as_str().unwrap_or_default().to_owned(),
            choices,
            usage,
            extensions: Extensions::new(),
        })
    }

    fn parse_stream_chunk(chunk: String) -> Result<String, String> {
        let chunk: StreamChunk = parse_input(&chunk)?;

        let mut state = STREAM_STATE.lock().expect("single-threaded guest");
        if chunk.data.is_empty() {
            let events = state
                .pending_finish
                .take_done()
                .into_iter()
                .collect::<Vec<_>>();
            return to_output(&events);
        }
        state.tail.push_str(&chunk.data);

        let mut events = Vec::new();
        while let Some((end, separator)) = sse_frame_boundary(&state.tail) {
            let frame = state.tail[..end].replace("\r\n", "\n");
            state.tail.drain(..end + separator);

            let mut event_name = None;
            let mut data_lines = Vec::new();
            for line in frame.lines() {
                if line.starts_with(':') {
                    continue;
                }
                if let Some(value) = line.strip_prefix("event:") {
                    event_name = Some(value.strip_prefix(' ').unwrap_or(value));
                }
                if let Some(value) = line.strip_prefix("data:") {
                    data_lines.push(value.strip_prefix(' ').unwrap_or(value));
                }
            }
            if event_name == Some("error") {
                events.push(StreamEvent::Error {
                    error: provider_protocol_error("the upstream emitted an SSE error event"),
                });
                continue;
            }
            if data_lines.is_empty() {
                continue;
            }
            let payload = data_lines.join("\n");
            if payload == "[DONE]" {
                if let Some(done) = state.pending_finish.finish_marker() {
                    events.push(done);
                }
                continue;
            }
            let ProviderStreamState { pending_finish, .. } = &mut *state;
            events.extend(events_of_frame(&payload, pending_finish)?);
        }
        to_output(&events)
    }

    fn map_provider_error(response_parts: String) -> Result<String, String> {
        let parts: HttpResponseParts = parse_input(&response_parts)?;
        let raw: Value = serde_json::from_str(&parts.body).unwrap_or(Value::Null);

        let provider_code = raw["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let code = if provider_code == "content_policy_violation" {
            ErrorCode::ContentPolicy
        } else if provider_code.contains("context_length")
            || provider_code.contains("maximum_context")
        {
            ErrorCode::ContextLength
        } else {
            match parts.status {
                400 | 404 | 422 => ErrorCode::InvalidRequest,
                401 | 403 => ErrorCode::Auth,
                402 => ErrorCode::PaymentRequired,
                408 => ErrorCode::Timeout,
                429 => ErrorCode::RateLimit,
                529 => ErrorCode::Capacity,
                500 | 502 | 503 | 504 => ErrorCode::UpstreamUnavailable,
                _ => ErrorCode::Internal,
            }
        };

        let message = match code {
            ErrorCode::InvalidRequest => "the upstream refused the request as malformed",
            ErrorCode::Auth => "the upstream rejected the credential",
            ErrorCode::PaymentRequired => {
                "the upstream requires payment or the account is out of funds"
            }
            ErrorCode::RateLimit => "the upstream rate limited this request",
            ErrorCode::ContentPolicy => "the upstream refused on content-policy grounds",
            ErrorCode::ContextLength => "the request exceeds the model's context window",
            ErrorCode::Timeout => "the upstream did not answer in time",
            ErrorCode::UpstreamUnavailable => "the upstream is unavailable",
            ErrorCode::TransportTruncated => "the upstream connection dropped mid-response",
            ErrorCode::ProviderProtocolError => "the upstream answered with an invalid body",
            ErrorCode::Capacity | ErrorCode::Capability | ErrorCode::Internal => {
                "the upstream failed"
            }
        };

        let mut envelope = ErrorEnvelope::new(code, parts.status, message);
        envelope.provider_message = raw["error"]["message"]
            .as_str()
            .filter(|message| message.chars().count() <= 256)
            .map(str::to_owned);
        envelope.retry_after_ms = parts
            .headers
            .get("retry-after")
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| seconds.saturating_mul(1000));
        to_output(&envelope)
    }
}

export!(OpenAiCompatible);
