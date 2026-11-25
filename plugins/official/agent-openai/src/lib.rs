//! The official OpenAI-compatible inbound agent adapter.
//!
//! Northbound only: an agent tool's OpenAI-dialect request in, the Canonical
//! IR out, and Canonical responses rendered back into the dialect. It never
//! sees a credential — the envelope's headers arrive with credential values
//! already redacted, and the `agent-adapter-v1` world cannot even name one.
//!
//! The mapping is South's north codec (`south-north-codec`), the one the
//! server uses for this wire. This adapter keeps only what the codec leaves to
//! a host: the Cursor body shim, the strict `response_format` check, the
//! required model, the per-stream render state, the `[DONE]` sentinel and the
//! error frame. The fixture pack in `fixtures/` pins the rendered output.

use std::cell::RefCell;
use std::collections::BTreeMap;

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Value};
use south_north_codec::{
    chat_request_from_openai_chat, openai_chat_response, ChatStreamContext, OpenAiChatStream,
    ResponseContext,
};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatResponse, ErrorCode, ErrorEnvelope, HintKind,
    ResponseFormat, StreamEvent,
};

struct OpenAiClient;

/// The headers this adapter reads a routing hint out of.
const HINT_HEADERS: &[(&str, HintKind)] = &[
    ("x-agent-step", HintKind::StepType),
    ("x-agent-task", HintKind::TaskType),
    ("x-agent-preference", HintKind::Preference),
];

/// The most streams this instance renders at once. The host admits no more.
const MAX_STREAMS: usize = 1024;

// -- error plumbing -------------------------------------------------------------

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

fn invalid(detail: &str) -> String {
    fail(&ErrorEnvelope::new(ErrorCode::InvalidRequest, 400, detail))
}

fn capability(detail: impl Into<String>) -> String {
    fail(&ErrorEnvelope::new(ErrorCode::Capability, 400, detail))
}

fn parse_input<T: for<'de> serde::Deserialize<'de>>(input: &str) -> Result<T, String> {
    serde_json::from_str(input).map_err(internal)
}

fn to_output<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(internal)
}

// -- host shims around the codec --------------------------------------------------

fn response_format(body: &Value) -> Result<Option<ResponseFormat>, String> {
    let Some(raw) = body.get("response_format").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let format = raw
        .as_object()
        .ok_or_else(|| invalid("response_format must be an object"))?;
    let kind = format
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("response_format declares no string type"))?;
    let allowed: &[&str] = match kind {
        "text" | "json_object" => &["type"],
        "json_schema" => &["type", "json_schema"],
        _ => {
            return Err(capability(format!(
                "unsupported Chat Completions response_format type {kind}"
            )))
        }
    };
    if format.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(capability("response_format contains unsupported fields"));
    }
    let result = match kind {
        "text" => ResponseFormat::Text,
        "json_object" => ResponseFormat::JsonObject,
        "json_schema" => {
            let schema = format
                .get("json_schema")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("response_format.json_schema must be an object"))?;
            if schema
                .keys()
                .any(|key| !["name", "description", "schema", "strict"].contains(&key.as_str()))
            {
                return Err(capability(
                    "response_format.json_schema contains unsupported fields",
                ));
            }
            if schema
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| {
                    name.is_empty()
                        || name.len() > 64
                        || !name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
            {
                return Err(invalid(
                    "JSON schema format requires a valid name of at most 64 characters",
                ));
            }
            if schema
                .get("schema")
                .is_none_or(|schema| !schema.is_object())
            {
                return Err(invalid("JSON schema format requires an object schema"));
            }
            if schema
                .get("description")
                .is_some_and(|value| !value.is_string())
            {
                return Err(invalid("JSON schema description must be a string"));
            }
            if schema
                .get("strict")
                .is_some_and(|value| !value.is_boolean() && !value.is_null())
            {
                return Err(invalid("JSON schema strict must be a boolean or null"));
            }
            ResponseFormat::JsonSchema {
                json_schema: Value::Object(schema.clone()),
            }
        }
        _ => unreachable!(),
    };
    Ok(Some(result))
}

/// Cursor Agent has been observed sending a Responses-shaped body to the
/// `/chat/completions` URL. Keep this conversion scoped to the Cursor agent
/// identity so ordinary OpenAI-compatible clients remain strict.
fn normalize_cursor_body(body: &Value) -> Result<Value, String> {
    let Some(object) = body.as_object() else {
        return Err(invalid("request body must be an object"));
    };
    if object.get("messages").is_some() {
        return Ok(body.clone());
    }
    let Some(input) = object.get("input").and_then(Value::as_array) else {
        return Ok(body.clone());
    };
    let mut normalized = object.clone();
    let mut messages = Vec::new();
    for item in input {
        let role = item
            .get("role")
            .and_then(Value::as_str)
            .or_else(|| {
                item.get("type")
                    .and_then(Value::as_str)
                    .filter(|kind| *kind == "message")
                    .map(|_| "user")
            })
            .ok_or_else(|| invalid("Cursor input item declares no message role"))?;
        let content = item.get("content").cloned().unwrap_or(Value::Null);
        let content = match content {
            Value::Array(parts) => {
                let text = parts
                    .iter()
                    .filter_map(|part| {
                        part.get("text")
                            .and_then(Value::as_str)
                            .or_else(|| part.get("input_text").and_then(Value::as_str))
                    })
                    .collect::<Vec<_>>()
                    .join("");
                Value::String(text)
            }
            other => other,
        };
        messages.push(json!({"role": role, "content": content}));
    }
    // Responses fields with a Chat equivalent. Before, the system prompt, the
    // output cap and the reasoning level were dropped here.
    match normalized.remove("instructions") {
        Some(Value::String(text)) if !text.is_empty() => {
            messages.insert(0, json!({"role": "system", "content": text}));
        }
        None | Some(Value::Null | Value::String(_)) => {}
        Some(_) => return Err(invalid("Cursor instructions must be a string")),
    }
    if let Some(cap) = normalized.remove("max_output_tokens") {
        if !normalized.contains_key("max_completion_tokens")
            && !normalized.contains_key("max_tokens")
        {
            normalized.insert("max_completion_tokens".to_owned(), cap);
        }
    }
    if let Some(effort) = normalized
        .remove("reasoning")
        .and_then(|reasoning| reasoning.get("effort").filter(|e| e.is_string()).cloned())
    {
        normalized.entry("reasoning_effort").or_insert(effort);
    }
    normalized.remove("input");
    normalized.remove("previous_response_id");
    normalized.remove("store");
    normalized.remove("include");
    normalized.remove("truncation");
    normalized.insert("messages".to_owned(), Value::Array(messages));
    if let Some(Value::Array(tools)) = normalized.get_mut("tools") {
        for tool in tools {
            if tool.get("type").and_then(Value::as_str) == Some("function")
                && tool.get("function").is_none()
            {
                let mut function = tool.clone();
                if let Some(object) = function.as_object_mut() {
                    object.remove("type");
                    let mut wrapped = serde_json::Map::new();
                    wrapped.insert("type".to_owned(), json!("function"));
                    wrapped.insert("function".to_owned(), Value::Object(object.clone()));
                    *tool = Value::Object(wrapped);
                }
            }
        }
    }
    Ok(Value::Object(normalized))
}

/// Drops `reasoning_content` from assistant turns before the codec reads them.
///
/// The codec turns it into a leading thinking block. The host passes an
/// unmarked thinking block to every upstream, so an Anthropic upstream would
/// get it without a signature, and an assistant turn with only reasoning and
/// tool calls would carry `content: []` to an OpenAI-compatible one. Until that
/// path is verified, the adapter keeps the earlier behavior: a client's
/// reasoning text is not replayed (design record, P3.1 R7).
fn without_reasoning_replay(body: &mut Value) {
    let messages = body.get_mut("messages").and_then(Value::as_array_mut);
    for message in messages.into_iter().flatten() {
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            if let Some(object) = message.as_object_mut() {
                object.remove("reasoning_content");
            }
        }
    }
}

/// The codec refuses tool arguments that are not JSON. This adapter has always
/// passed them on, and some clients repair them, so it renders a valid
/// placeholder and puts the original text back (design record, P3.1 S4).
fn render_keeping_tool_arguments(
    mut response: ChatResponse,
    context: &ResponseContext,
) -> Result<Value, String> {
    let mut kept = Vec::new();
    for (choice_index, choice) in response.choices.iter_mut().enumerate() {
        for (call_index, call) in choice.message.tool_calls.iter_mut().enumerate() {
            if serde_json::from_str::<serde::de::IgnoredAny>(&call.arguments).is_err() {
                let original = std::mem::replace(&mut call.arguments, "{}".to_owned());
                kept.push((choice_index, call_index, original));
            }
        }
    }
    let mut rendered = openai_chat_response(&response, context).map_err(internal)?;
    for (choice_index, call_index, original) in kept {
        let pointer =
            format!("/choices/{choice_index}/message/tool_calls/{call_index}/function/arguments");
        let slot = rendered
            .pointer_mut(&pointer)
            .ok_or_else(|| internal("rendered tool call is missing its arguments"))?;
        *slot = Value::String(original);
    }
    Ok(rendered)
}

/// One stream's codec state and the identity it was opened with.
struct StreamState {
    response_id: String,
    model: String,
    codec: OpenAiChatStream,
    finish_seen: bool,
}

thread_local! {
    static STREAMS: RefCell<BTreeMap<String, StreamState>> =
        const { RefCell::new(BTreeMap::new()) };
}

fn context_field<'a>(context: &'a Value, name: &str) -> Result<&'a str, String> {
    context
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(&format!("stream render context declares no {name}")))
}

/// Renders one event of a stream through its codec state. Returns the SSE text
/// and whether the stream ended.
fn render_in_stream(state: &mut StreamState, event: StreamEvent) -> Result<(String, bool), String> {
    let mut events = Vec::with_capacity(2);
    match &event {
        StreamEvent::Finish { .. } => state.finish_seen = true,
        // Every current producer sends Finish before Done. A Done that carries
        // a reason alone still closes with a finish chunk, as it always did.
        StreamEvent::Done {
            finish_reason: Some(reason),
            stop_sequence,
        } if !state.finish_seen => events.push(StreamEvent::Finish {
            finish_reason: Some(reason.clone()),
            stop_sequence: stop_sequence.clone(),
        }),
        _ => {}
    }
    let done = matches!(event, StreamEvent::Done { .. });
    events.push(event);
    let frames = state.codec.frames(&events).map_err(internal)?;
    let mut data = String::new();
    for frame in frames {
        data.push_str("data: ");
        data.push_str(&serde_json::to_string(&frame.data).map_err(internal)?);
        data.push_str("\n\n");
    }
    if done {
        data.push_str("data: [DONE]\n\n");
    }
    Ok((data, done))
}

impl Guest for OpenAiClient {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "agent-openai".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Agent,
            api_version: "agent-adapter-v1".to_owned(),
        }
    }

    fn healthcheck() -> AdapterHealth {
        AdapterHealth {
            status: HealthStatus::Ready,
            detail: None,
        }
    }

    fn supported_agent_protocols(
    ) -> Vec<exports::token_station::adapter::agent_adapter::AgentProtocolCapability> {
        vec![
            exports::token_station::adapter::agent_adapter::AgentProtocolCapability {
                protocol: "openai-chat-completions".to_owned(),
                agent_tools: vec![
                    "opencode".to_owned(),
                    "openclaw".to_owned(),
                    "nous-hermes-agent".to_owned(),
                    "generic-openai-sdk".to_owned(),
                    "cursor".to_owned(),
                    "continue".to_owned(),
                    "workbuddy".to_owned(),
                    "grok-build".to_owned(),
                    "kimi-code".to_owned(),
                    "deepseek-harness".to_owned(),
                ],
            },
        ]
    }

    fn match_inbound(
        request_head: String,
    ) -> exports::token_station::adapter::agent_adapter::MatchResult {
        // `{ method, path, headers }`; this adapter owns the OpenAI paths.
        let head: Value = serde_json::from_str(&request_head).unwrap_or(Value::Null);
        let path = head["path"].as_str().unwrap_or_default();
        let matched = path.ends_with("/chat/completions");

        exports::token_station::adapter::agent_adapter::MatchResult {
            matched,
            protocol: matched.then(|| "openai-chat-completions".to_owned()),
        }
    }

    fn normalize_inbound(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse_input(&envelope)?;
        let mut body = if envelope.agent_tool.as_deref() == Some("cursor") {
            normalize_cursor_body(&envelope.body)?
        } else {
            envelope.body.clone()
        };
        // The codec accepts a missing model as an empty one. A request must
        // name the model it wants routed.
        if body.get("model").and_then(Value::as_str).is_none() {
            return Err(invalid("request declares no model"));
        }
        let response_format = response_format(&body)?;
        without_reasoning_replay(&mut body);
        let mut request =
            chat_request_from_openai_chat(&body).map_err(|error| invalid(&error.to_string()))?;
        // The strict check above wins over the codec's lenient parse.
        request.response_format = response_format;
        to_output(&request)
    }

    fn extract_agent_hint(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse_input(&envelope)?;

        // A hint is routing *input*. Naming a provider here would not make the
        // router honour it, so there is nothing to guard against.
        let hints: Vec<AgentHint> = HINT_HEADERS
            .iter()
            .filter_map(|(header, kind)| {
                envelope
                    .headers
                    .value(header)
                    .map(|value| AgentHint::new(*kind, value))
            })
            .collect();
        to_output(&hints)
    }

    fn render_response(response: String, context: String) -> Result<String, String> {
        let response: ChatResponse = parse_input(&response)?;
        let context: Value = serde_json::from_str(&context).unwrap_or(Value::Null);
        let fallback_id = context
            .get("response_id")
            .and_then(Value::as_str)
            .unwrap_or("token-station")
            .to_owned();
        let rendered = render_keeping_tool_arguments(
            response,
            &ResponseContext {
                created: 0,
                fallback_id,
            },
        )?;
        to_output(&rendered)
    }

    fn render_stream_event(event: String, context: String) -> Result<String, String> {
        let event: StreamEvent = parse_input(&event)?;
        let context: Value = parse_input(&context)?;
        let response_id = context_field(&context, "response_id")?;
        let model = context_field(&context, "model")?;
        // The host names every stream. A context without a name still renders,
        // as it did when this adapter kept no state: its state is keyed on the
        // response id instead.
        let stream_key = match context.get("stream_id").and_then(Value::as_str) {
            Some(id) => format!("stream:{id}"),
            None => format!("response:{response_id}"),
        };
        let stream_id = stream_key.as_str();

        // The codec renders nothing for an error. The client gets this wire's
        // error frame, and the stream ends.
        if let StreamEvent::Error { error } = &event {
            STREAMS.with(|streams| streams.borrow_mut().remove(stream_id));
            let rendered = Self::map_inbound_error(to_output(error)?, String::new())?;
            return to_output(&json!({ "data": format!("data: {rendered}\n\n") }));
        }

        STREAMS.with(|streams| {
            let mut streams = streams.borrow_mut();
            if !streams.contains_key(stream_id) && streams.len() >= MAX_STREAMS {
                return Err(capability("too many open streams"));
            }
            let state = streams
                .entry(stream_id.to_owned())
                .or_insert_with(|| StreamState {
                    response_id: response_id.to_owned(),
                    model: model.to_owned(),
                    codec: OpenAiChatStream::new(ChatStreamContext {
                        id: response_id.to_owned(),
                        model: model.to_owned(),
                        created: 0,
                    }),
                    finish_seen: false,
                });
            let result = if state.response_id != response_id || state.model != model {
                Err(invalid(
                    "stream render context changed response_id or model mid-stream",
                ))
            } else {
                render_in_stream(state, event)
            };
            match result {
                Ok((data, ended)) => {
                    if ended {
                        streams.remove(stream_id);
                    }
                    to_output(&json!({ "data": data }))
                }
                Err(error) => {
                    streams.remove(stream_id);
                    Err(error)
                }
            }
        })
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse_input(&error)?;
        let code = error
            .extensions
            .get("client_error_code")
            .cloned()
            .unwrap_or(serde_json::to_value(error.code).map_err(internal)?);
        to_output(&json!({
            "error": {
                "message": error.message,
                "type": code,
                "code": code,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use token_station_protocol::{
        ChatRequest, Content, ContentPart, Extensions, HeaderDigest, Principal,
    };

    fn envelope(agent_tool: &str, body: Value) -> String {
        serde_json::to_string(&AgentRequestEnvelope {
            protocol: "openai-chat-completions".to_owned(),
            agent_tool: Some(agent_tool.to_owned()),
            headers: HeaderDigest::default(),
            principal: Principal {
                subject: "local".to_owned(),
                tenant: None,
            },
            hints: Vec::new(),
            body,
            extensions: Extensions::new(),
        })
        .unwrap()
    }

    fn normalize(agent_tool: &str, body: Value) -> Result<ChatRequest, String> {
        OpenAiClient::normalize_inbound(envelope(agent_tool, body))
            .map(|request| serde_json::from_str(&request).unwrap())
    }

    /// Renders `events` as one stream and returns the SSE text.
    fn stream(stream_id: &str, events: &[Value]) -> String {
        let context = json!({
            "stream_id": stream_id,
            "response_id": "chatcmpl-token-station-7",
            "model": "routed-model"
        });
        events
            .iter()
            .map(|event| {
                let rendered =
                    OpenAiClient::render_stream_event(event.to_string(), context.to_string())
                        .expect("canonical stream event renders");
                let rendered: Value = serde_json::from_str(&rendered).unwrap();
                rendered["data"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    fn chunks(data: &str) -> Vec<Value> {
        data.split("\n\n")
            .filter_map(|frame| frame.strip_prefix("data: "))
            .filter(|frame| *frame != "[DONE]")
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect()
    }

    #[test]
    fn structured_formats_preserve_constraints_and_reject_invalid_fields() {
        let schema = json!({"name":"session_title","description":"A title","strict":true,"schema":{"type":"object","properties":{"title":{"type":"string","maxLength":80}},"required":["title"],"additionalProperties":false,"$defs":{"bound":{"type":"integer","maximum":12}}}});
        for format in [
            json!({"type":"text"}),
            json!({"type":"json_object"}),
            json!({"type":"json_schema","json_schema":schema}),
        ] {
            assert_eq!(
                serde_json::to_value(response_format(&json!({"response_format":format})).unwrap())
                    .unwrap(),
                format
            );
        }
        assert_eq!(response_format(&json!({})).unwrap(), None);
        assert_eq!(
            response_format(&json!({"response_format":null})).unwrap(),
            None
        );
        for format in [
            json!({}),
            json!("json_object"),
            json!({"type":3}),
            json!({"type":"future"}),
            json!({"type":"text","strict":true}),
            json!({"type":"json_object","json_schema":{}}),
            json!({"type":"json_schema"}),
            json!({"type":"json_schema","json_schema":[]}),
            json!({"type":"json_schema","json_schema":{"schema":{}}}),
            json!({"type":"json_schema","json_schema":{"name":"bad name","schema":{}}}),
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":false}}),
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{},"strict":"true"}}),
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{},"description":3}}),
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{},"future":true}}),
        ] {
            assert!(
                response_format(&json!({"response_format":format})).is_err(),
                "{format}"
            );
        }
        // A format the strict check refuses never reaches the codec.
        let refused = normalize(
            "opencode",
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"response_format":{"type":"grammar"}}),
        )
        .unwrap_err();
        assert!(refused.contains("unsupported Chat Completions response_format type grammar"));
    }

    #[test]
    fn opencode_logical_model_reaches_canonical_request_unchanged() {
        for model in ["auto", "fast", "balanced", "power"] {
            let request = normalize(
                "opencode",
                json!({"model": model, "messages": [{"role": "user", "content": "hello"}]}),
            )
            .expect("OpenCode request normalizes");
            assert_eq!(request.model, model);
        }
    }

    #[test]
    fn a_request_without_a_model_is_refused() {
        let error = normalize(
            "opencode",
            json!({"messages":[{"role":"user","content":"hi"}]}),
        )
        .unwrap_err();
        let error: ErrorEnvelope = serde_json::from_str(&error).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "request declares no model");
    }

    #[test]
    fn codec_mapping_reaches_the_canonical_request() {
        let request = normalize(
            "generic-openai-sdk",
            json!({
                "model": "m",
                "messages": [
                    {"role": "developer", "content": "be brief"},
                    {"role": "user", "content": "hi"}
                ],
                "stop": "END",
                "max_tokens": 100,
                "max_completion_tokens": 200,
                "seed": 7
            }),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request.messages[0].role).unwrap(),
            json!("system")
        );
        assert_eq!(request.sampling.stop, ["END"]);
        assert_eq!(request.sampling.max_output_tokens, Some(200));
        assert_eq!(request.extensions.get("seed"), Some(&json!(7)));

        let error = normalize(
            "opencode",
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":-1}),
        )
        .unwrap_err();
        let error: ErrorEnvelope = serde_json::from_str(&error).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("max_tokens"), "{}", error.message);
    }

    #[test]
    fn client_reasoning_text_is_not_replayed_upstream() {
        let request = normalize(
            "opencode",
            json!({"model": "deepseek-reasoner", "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": null, "reasoning_content": "call the tool",
                 "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
            ]}),
        )
        .unwrap();
        assert_eq!(request.messages[1].content, None);
        assert_eq!(request.messages[1].tool_calls.len(), 1);
    }

    #[test]
    fn cursor_responses_shape_is_lowered_to_chat_messages_and_function_tools() {
        let body = json!({
            "model": "tokenstation-auto",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "read marker"}]
            }],
            "tools": [{
                "type": "function",
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object"}
            }],
            "previous_response_id": "resp_old",
            "store": true
        });
        let normalized = normalize_cursor_body(&body).expect("Cursor body lowers");
        assert_eq!(normalized["messages"][0]["role"], json!("user"));
        assert_eq!(normalized["messages"][0]["content"], json!("read marker"));
        assert_eq!(
            normalized["tools"][0]["function"]["name"],
            json!("read_file")
        );
        assert!(normalized.get("input").is_none());
        assert!(normalized.get("previous_response_id").is_none());
        assert!(normalized.get("store").is_none());
    }

    #[test]
    fn cursor_instructions_output_cap_and_reasoning_reach_the_request() {
        let request = normalize(
            "cursor",
            json!({
                "model": "gpt-5.5",
                "instructions": "You are Cursor.",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "reasoning": {"effort": "medium"},
                "max_output_tokens": 2048
            }),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request.messages[0].role).unwrap(),
            json!("system")
        );
        assert_eq!(
            request.messages[0].content,
            Some(Content::Text("You are Cursor.".to_owned()))
        );
        assert_eq!(request.sampling.max_output_tokens, Some(2048));
        assert_eq!(
            serde_json::to_value(&request.reasoning.effort).unwrap(),
            json!("medium")
        );
        for key in ["instructions", "max_output_tokens", "reasoning"] {
            assert!(!request.extensions.contains_key(key), "{key}");
        }
        // An explicit Chat cap or level is not overwritten.
        let explicit = normalize_cursor_body(&json!({
            "model": "m", "input": [], "max_output_tokens": 1, "max_tokens": 9,
            "reasoning": {"effort": "low"}, "reasoning_effort": "high"
        }))
        .unwrap();
        assert_eq!(explicit["max_tokens"], json!(9));
        assert!(explicit.get("max_completion_tokens").is_none());
        assert_eq!(explicit["reasoning_effort"], json!("high"));
    }

    #[test]
    fn non_stream_usage_writes_only_reported_buckets() {
        let response = json!({
            "id": "chatcmpl-usage",
            "model": "routed-model",
            "choices": [],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_read_tokens": 60,
                "cache_write_tokens": 10,
                "reasoning_tokens": 5
            }
        });
        let rendered = OpenAiClient::render_response(response.to_string(), String::new())
            .expect("canonical response renders");
        let rendered: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            rendered["usage"],
            json!({
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": {"cached_tokens": 60},
                "completion_tokens_details": {"reasoning_tokens": 5}
            })
        );
    }

    #[test]
    fn reasoning_only_content_renders_null_text_and_reasoning() {
        let response = ChatResponse {
            id: "chatcmpl-r".to_owned(),
            model: "m".to_owned(),
            choices: vec![serde_json::from_value(json!({
                "index": 0,
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "private reasoning"}]},
                "finish_reason": "stop"
            }))
            .unwrap()],
            usage: Default::default(),
            extensions: Extensions::new(),
        };
        let rendered =
            OpenAiClient::render_response(serde_json::to_string(&response).unwrap(), String::new())
                .unwrap();
        let rendered: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(rendered["choices"][0]["message"]["content"], Value::Null);
        assert_eq!(
            rendered["choices"][0]["message"]["reasoning_content"],
            json!("private reasoning")
        );
        let _ = ContentPart::Thinking {
            thinking: String::new(),
            signature: None,
        };
    }

    #[test]
    fn tool_arguments_that_are_not_json_pass_through() {
        let response = json!({
            "id": "chatcmpl-args",
            "model": "m",
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_1", "name": "f", "arguments": "{not json"},
                    {"id": "call_2", "name": "g", "arguments": "{\"ok\":true}"},
                    {"id": "call_3", "name": "h", "arguments": ""}
                ]}}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });
        let rendered = OpenAiClient::render_response(response.to_string(), String::new()).unwrap();
        let rendered: Value = serde_json::from_str(&rendered).unwrap();
        let calls = &rendered["choices"][0]["message"]["tool_calls"];
        assert_eq!(calls[0]["function"]["arguments"], json!("{not json"));
        assert_eq!(calls[1]["function"]["arguments"], json!("{\"ok\":true}"));
        assert_eq!(calls[2]["function"]["arguments"], json!(""));
    }

    #[test]
    fn stream_chunks_carry_identity_role_and_wire_key_order() {
        let data = stream(
            "stream-identity",
            &[
                json!({"type": "thinking_delta", "index": 0, "block_index": 2, "thinking_delta": "checking"}),
                json!({"type": "delta", "index": 0, "content": "hi"}),
                json!({"type": "done", "finish_reason": "stop"}),
            ],
        );
        assert!(
            data.starts_with(
                "data: {\"id\":\"chatcmpl-token-station-7\",\"object\":\"chat.completion.chunk\",\
                 \"created\":0,\"model\":\"routed-model\",\"choices\":"
            ),
            "{data}"
        );
        let chunks = chunks(&data);
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], json!("assistant"));
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["reasoning_content"],
            json!("checking")
        );
        assert!(chunks[1]["choices"][0]["delta"].get("role").is_none());
        assert!(data.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn finish_and_usage_close_the_stream_in_one_chunk_before_done() {
        let data = stream(
            "stream-order",
            &[
                json!({"type": "delta", "index": 0, "content": "x"}),
                json!({"type": "finish", "finish_reason": "stop", "stop_sequence": null}),
                json!({"type": "usage", "usage": {"input_tokens": 42, "output_tokens": 7,
                    "cache_read_tokens": 3, "cache_write_tokens": 1, "reasoning_tokens": 0}}),
                json!({"type": "done", "finish_reason": "stop", "stop_sequence": null}),
            ],
        );
        let chunks = chunks(&data);
        assert_eq!(chunks.len(), 2, "{data}");
        let terminal = &chunks[1];
        assert_eq!(terminal["choices"][0]["finish_reason"], json!("stop"));
        assert_eq!(
            terminal["usage"],
            json!({"prompt_tokens": 42, "completion_tokens": 7, "total_tokens": 49,
                "prompt_tokens_details": {"cached_tokens": 3}})
        );
        let finish = data.find("\"finish_reason\":\"stop\"").unwrap();
        let usage = data.find("\"prompt_tokens\":42").unwrap();
        let done = data.find("data: [DONE]").unwrap();
        assert!(finish < usage && usage < done);
    }

    #[test]
    fn usage_without_a_finish_rides_its_own_chunk() {
        let data = stream(
            "stream-usage-only",
            &[
                json!({"type": "usage", "usage": {"input_tokens": 5, "output_tokens": 1}}),
                json!({"type": "done", "finish_reason": null}),
            ],
        );
        let chunks = chunks(&data);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"], json!([]));
        assert_eq!(chunks[0]["usage"]["total_tokens"], json!(6));
    }

    #[test]
    fn a_done_with_a_reason_and_no_finish_still_closes_with_a_finish_chunk() {
        let data = stream(
            "stream-done-only",
            &[
                json!({"type": "delta", "index": 0, "content": "hi"}),
                json!({"type": "done", "finish_reason": "length"}),
            ],
        );
        let chunks = chunks(&data);
        assert_eq!(
            chunks.last().unwrap()["choices"][0]["finish_reason"],
            json!("length")
        );
    }

    #[test]
    fn redacted_thinking_renders_nothing() {
        let data = stream(
            "stream-redacted",
            &[
                json!({"type": "redacted_thinking", "index": 0, "block_index": 2, "data": "opaque"}),
                json!({"type": "delta", "index": 0, "content": "answer"}),
            ],
        );
        assert!(!data.contains("opaque"));
        assert!(data.contains("\"content\":\"answer\""));
    }

    #[test]
    fn an_error_renders_the_wire_error_frame_and_ends_the_stream() {
        let data = stream(
            "stream-error",
            &[
                json!({"type": "delta", "index": 0, "content": "x"}),
                json!({"type": "error", "error": {"code": "upstream_unavailable",
                    "http_status": 502, "message": "upstream went away"}}),
            ],
        );
        assert!(data.ends_with(
            "data: {\"error\":{\"message\":\"upstream went away\",\"type\":\"upstream_unavailable\",\"code\":\"upstream_unavailable\"}}\n\n"
        ));
        STREAMS.with(|streams| assert!(!streams.borrow().contains_key("stream:stream-error")));
    }

    #[test]
    fn stream_state_is_per_stream_and_freed_at_done() {
        let first = json!({"stream_id": "a", "response_id": "chatcmpl-a", "model": "m"});
        let second = json!({"stream_id": "b", "response_id": "chatcmpl-b", "model": "m"});
        let delta = json!({"type": "delta", "index": 0, "content": "x"}).to_string();
        let done = json!({"type": "done", "finish_reason": "stop"}).to_string();
        for context in [&first, &second] {
            let rendered =
                OpenAiClient::render_stream_event(delta.clone(), context.to_string()).unwrap();
            // Each stream's first delta carries the role: the state is not shared.
            assert!(rendered.contains("assistant"), "{rendered}");
        }
        OpenAiClient::render_stream_event(done.clone(), first.to_string()).unwrap();
        STREAMS.with(|streams| {
            assert!(!streams.borrow().contains_key("stream:a"));
            assert!(streams.borrow().contains_key("stream:b"));
        });
        let changed = json!({"stream_id": "b", "response_id": "chatcmpl-other", "model": "m"});
        let error = OpenAiClient::render_stream_event(delta, changed.to_string()).unwrap_err();
        assert!(error.contains("changed response_id or model"), "{error}");
        STREAMS.with(|streams| assert!(!streams.borrow().contains_key("stream:b")));
        // A context without a stream name is keyed on its response id.
        let anonymous = json!({"response_id": "chatcmpl-anon", "model": "m"});
        OpenAiClient::render_stream_event(done, anonymous.to_string()).unwrap();
        let missing = json!({"stream_id": "c", "model": "m"});
        let delta = json!({"type": "delta", "index": 0, "content": "x"}).to_string();
        assert!(
            OpenAiClient::render_stream_event(delta, missing.to_string())
                .unwrap_err()
                .contains("declares no response_id")
        );
    }
}

export!(OpenAiClient);
