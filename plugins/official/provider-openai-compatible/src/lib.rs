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
    Auth, ChatRequest, ChatResponse, Choice, Content, ContentPart, ErrorCode, ErrorEnvelope,
    Extensions, FinishReason, HttpMethod, HttpRequestDescriptor, HttpResponseParts, Message,
    ProviderConfig, Role, SafeHeaders, StreamChunk, StreamEvent, ToolCall, Usage,
};

/// The unparsed tail of the stream this instance is holding.
///
/// Instance state on purpose: the host instantiates one component per stream,
/// so this buffer only ever sees one provider's body.
static STREAM_TAIL: Mutex<String> = Mutex::new(String::new());

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
        // 0.3.0: unknown reasons survive verbatim instead of vanishing.
        other => Some(FinishReason::Other(other.to_owned())),
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
    if let Some(tool_choice) = &request.tool_choice {
        body.insert("tool_choice".to_owned(), json!(tool_choice));
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

/// One complete SSE frame's worth of events.
fn events_of_frame(payload: &str) -> Result<Vec<StreamEvent>, String> {
    let raw: Value = serde_json::from_str(payload).map_err(internal)?;
    let mut events = Vec::new();

    for choice in raw["choices"].as_array().into_iter().flatten() {
        let index = index_of(&choice["index"]);
        let delta = &choice["delta"];

        if let Some(thinking) = delta["reasoning_content"].as_str() {
            events.push(StreamEvent::ThinkingDelta {
                index,
                thinking_delta: thinking.to_owned(),
            });
        }
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
            events.push(StreamEvent::Done {
                finish_reason: finish_reason(Some(reason)),
                stop_sequence: None,
            });
        }
    }

    if let Some(usage) = raw.get("usage").filter(|usage| !usage.is_null()) {
        events.push(StreamEvent::Usage {
            usage: Usage {
                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                ..Usage::default()
            },
        });
    }
    Ok(events)
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
            format!("{}/chat/completions", config.base_url),
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
        let raw: Value = serde_json::from_str(&parts.body).map_err(internal)?;

        let mut choices = Vec::new();
        for choice in raw["choices"].as_array().into_iter().flatten() {
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
                // openai chat wire has no stop-sequence report slot.
                stop_sequence: None,
                message: Message {
                    role: Role::Assistant,
                    // 0.3.0: a reasoning report lifts content into parts form
                    // with the thinking block first (mirrors anthropic block
                    // order); plain responses keep the bare-string shape.
                    content: match (
                        message["reasoning_content"]
                            .as_str()
                            .filter(|s| !s.is_empty()),
                        message["content"].as_str(),
                    ) {
                        (Some(thinking), text) => Some(Content::Parts({
                            let mut parts = vec![ContentPart::Thinking {
                                thinking: thinking.to_owned(),
                                signature: None,
                            }];
                            if let Some(text) = text.filter(|t| !t.is_empty()) {
                                parts.push(ContentPart::Text {
                                    text: text.to_owned(),
                                });
                            }
                            parts
                        })),
                        (None, text) => text.map(|t| Content::Text(t.to_owned())),
                    },
                    tool_calls,
                    tool_call_id: None,
                    name: None,
                    extensions: Extensions::new(),
                },
                finish_reason: finish_reason(choice["finish_reason"].as_str()),
            });
        }

        let usage = &raw["usage"];
        to_output(&ChatResponse {
            id: raw["id"].as_str().unwrap_or_default().to_owned(),
            model: raw["model"].as_str().unwrap_or_default().to_owned(),
            choices,
            usage: Usage {
                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                cache_read_tokens: usage["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or(0),
                cache_write_tokens: 0,
                reasoning_tokens: usage["completion_tokens_details"]["reasoning_tokens"]
                    .as_u64()
                    .unwrap_or(0),
                ..Usage::default()
            },
            extensions: Extensions::new(),
        })
    }

    fn parse_stream_chunk(chunk: String) -> Result<String, String> {
        let chunk: StreamChunk = parse_input(&chunk)?;

        let mut tail = STREAM_TAIL.lock().expect("single-threaded guest");
        tail.push_str(&chunk.data);

        let mut events = Vec::new();
        while let Some(end) = tail.find("\n\n") {
            let frame = tail[..end].to_owned();
            tail.drain(..end + 2);

            let Some(payload) = frame.strip_prefix("data: ") else {
                continue;
            };
            if payload == "[DONE]" {
                continue;
            }
            events.extend(events_of_frame(payload)?);
        }
        to_output(&events)
    }

    fn map_provider_error(response_parts: String) -> Result<String, String> {
        let parts: HttpResponseParts = parse_input(&response_parts)?;
        let raw: Value = serde_json::from_str(&parts.body).unwrap_or(Value::Null);

        let code = if raw["error"]["code"].as_str() == Some("content_policy_violation") {
            ErrorCode::ContentPolicy
        } else {
            match parts.status {
                400 | 404 | 422 => ErrorCode::InvalidRequest,
                401 | 403 => ErrorCode::Auth,
                408 => ErrorCode::Timeout,
                429 => ErrorCode::RateLimit,
                500 | 502 | 503 | 504 => ErrorCode::UpstreamUnavailable,
                _ => ErrorCode::Internal,
            }
        };

        let message = match code {
            ErrorCode::InvalidRequest => "the upstream refused the request as malformed",
            ErrorCode::Auth => "the upstream rejected the credential",
            ErrorCode::RateLimit => "the upstream rate limited this request",
            ErrorCode::ContentPolicy => "the upstream refused on content-policy grounds",
            ErrorCode::Timeout => "the upstream did not answer in time",
            ErrorCode::UpstreamUnavailable => "the upstream is unavailable",
            ErrorCode::Capacity | ErrorCode::Capability | ErrorCode::Internal => {
                "the upstream failed"
            }
        };

        let mut envelope = ErrorEnvelope::new(code, parts.status, message);
        // The upstream's own words, never its body: a body may echo the request.
        envelope.provider_message = raw["error"]["message"].as_str().map(str::to_owned);
        envelope.retry_after_ms = parts
            .headers
            .get("retry-after")
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| seconds * 1000);
        to_output(&envelope)
    }
}

export!(OpenAiCompatible);
