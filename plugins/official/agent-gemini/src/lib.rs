//! Official Google Gemini `generateContent` northbound adapter.
//!
//! Gemini puts the requested model and streaming mode in the URL rather than
//! the JSON body. The host records that redacted transport path in the request
//! envelope's extension map; credentials never cross the plugin boundary.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::BTreeMap;

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Map, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, FinishReason, Message, Role, Sampling, StreamEvent, ToolCall,
    ToolDef,
};

struct GeminiClient;

fn fail(error: &ErrorEnvelope) -> String {
    serde_json::to_string(error).unwrap_or_else(|_| {
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

fn invalid(detail: impl Into<String>) -> String {
    fail(&ErrorEnvelope::new(ErrorCode::InvalidRequest, 400, detail))
}

fn capability(detail: impl Into<String>) -> String {
    fail(&ErrorEnvelope::new(ErrorCode::Capability, 400, detail))
}

fn parse<T: for<'de> serde::Deserialize<'de>>(input: &str) -> Result<T, String> {
    serde_json::from_str(input).map_err(internal)
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(internal)
}

fn transport_path(envelope: &AgentRequestEnvelope) -> Result<&str, String> {
    envelope
        .extensions
        .get("transport_path")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Gemini request envelope declares no transport_path"))
}

fn route(path: &str) -> Option<(&str, bool)> {
    let path = path.split('?').next()?.trim_end_matches('/');
    let marker = "/models/";
    let model_start = path.rfind(marker)? + marker.len();
    let tail = &path[model_start..];
    let (model, stream) = tail
        .strip_suffix(":generateContent")
        .map(|model| (model, false))
        .or_else(|| {
            tail.strip_suffix(":streamGenerateContent")
                .map(|model| (model, true))
        })?;
    (!model.is_empty()
        && model.len() <= 256
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte)))
    .then_some((model, stream))
}

fn text_parts(parts: &Value, context: &str) -> Result<Option<Content>, String> {
    let parts = parts
        .as_array()
        .ok_or_else(|| invalid(format!("{context}.parts must be an array")))?;
    let mut content = Vec::new();
    for part in parts {
        let object = part
            .as_object()
            .ok_or_else(|| invalid(format!("{context}.parts entries must be objects")))?;
        if let Some(text) = object.get("text") {
            let text = text
                .as_str()
                .ok_or_else(|| invalid(format!("{context} text part must be a string")))?;
            content.push(ContentPart::Text {
                text: text.to_owned(),
            });
        } else if object.contains_key("inlineData")
            || object.contains_key("fileData")
            || object.contains_key("videoMetadata")
        {
            return Err(capability(
                "Gemini multimodal parts are not supported by this adapter",
            ));
        } else if !object.contains_key("functionCall") && !object.contains_key("functionResponse") {
            return Err(capability("unsupported Gemini content part"));
        }
    }
    Ok((!content.is_empty()).then_some(Content::Parts(content)))
}

fn parse_system(body: &Value) -> Result<Option<Message>, String> {
    let Some(system) = body.get("systemInstruction") else {
        return Ok(None);
    };
    let content = text_parts(
        system
            .get("parts")
            .ok_or_else(|| invalid("systemInstruction declares no parts"))?,
        "systemInstruction",
    )?;
    Ok(content.map(|content| Message {
        role: Role::System,
        content: Some(content),
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    }))
}

fn parse_messages(body: &Value) -> Result<Vec<Message>, String> {
    let contents = body
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Gemini request declares no contents array"))?;
    let mut messages = Vec::new();
    if let Some(system) = parse_system(body)? {
        messages.push(system);
    }
    for (content_index, item) in contents.iter().enumerate() {
        let role = match item.get("role").and_then(Value::as_str).unwrap_or("user") {
            "user" => Role::User,
            "model" => Role::Assistant,
            other => return Err(invalid(format!("unsupported Gemini role `{other}`"))),
        };
        let parts = item
            .get("parts")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("Gemini content declares no parts array"))?;
        let content = text_parts(&Value::Array(parts.clone()), "contents")?;
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();
        for (part_index, part) in parts.iter().enumerate() {
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("functionCall declares no name"))?;
                let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(ToolCall {
                    id: format!("gemini-{content_index}-{part_index}"),
                    name: name.to_owned(),
                    arguments: encode(&args)?,
                });
            }
            if let Some(result) = part.get("functionResponse") {
                let name = result
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("functionResponse declares no name"))?;
                let response = result.get("response").cloned().unwrap_or(Value::Null);
                tool_results.push(Message {
                    role: Role::Tool,
                    content: Some(Content::Text(encode(&response)?)),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(format!("gemini-{content_index}-{part_index}")),
                    name: Some(name.to_owned()),
                    extensions: Extensions::new(),
                });
            }
        }
        if content.is_some() || !tool_calls.is_empty() {
            messages.push(Message {
                role,
                content,
                tool_calls,
                tool_call_id: None,
                name: None,
                extensions: Extensions::new(),
            });
        }
        messages.extend(tool_results);
    }
    Ok(messages)
}

fn parse_tools(body: &Value) -> Result<Vec<ToolDef>, String> {
    let Some(tools) = body.get("tools") else {
        return Ok(Vec::new());
    };
    let tools = tools
        .as_array()
        .ok_or_else(|| invalid("tools must be an array"))?;
    let mut definitions = Vec::new();
    for group in tools {
        let declarations = group
            .get("functionDeclarations")
            .and_then(Value::as_array)
            .ok_or_else(|| capability("only Gemini functionDeclarations tools are supported"))?;
        for declaration in declarations {
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("function declaration declares no name"))?;
            definitions.push(ToolDef {
                name: name.to_owned(),
                description: declaration
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parameters: declaration
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
            });
        }
    }
    Ok(definitions)
}

fn string_array(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(format!("{field} must contain only strings")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("{field} must be an array"))),
    }
}

fn finish_reason(reason: Option<FinishReason>) -> Option<&'static str> {
    match reason {
        Some(FinishReason::Stop) => Some("STOP"),
        Some(FinishReason::Length) => Some("MAX_TOKENS"),
        Some(FinishReason::ToolCalls) => Some("STOP"),
        Some(FinishReason::ContentFilter) => Some("SAFETY"),
        None => None,
    }
}

fn response_parts(message: &Message) -> Result<Vec<Value>, String> {
    let mut parts = Vec::new();
    match &message.content {
        None => {}
        Some(Content::Text(text)) => parts.push(json!({"text": text})),
        Some(Content::Parts(items)) => {
            for item in items {
                match item {
                    ContentPart::Text { text } => parts.push(json!({"text": text})),
                    ContentPart::ImageUrl { .. } => {
                        return Err(capability("Gemini response cannot render image output"))
                    }
                }
            }
        }
    }
    for call in &message.tool_calls {
        let args: Value = serde_json::from_str(&call.arguments)
            .map_err(|_| invalid("tool call arguments are not complete JSON"))?;
        parts.push(json!({"functionCall": {"name": call.name, "args": args}}));
    }
    Ok(parts)
}

fn stream_id(context: &Value) -> String {
    context
        .get("stream_id")
        .and_then(Value::as_str)
        .unwrap_or("gemini-stream")
        .to_owned()
}

#[derive(Default)]
struct PendingCall {
    name: Option<String>,
    arguments: String,
}

thread_local! {
    static STREAM_CALLS: RefCell<BTreeMap<String, BTreeMap<u32, PendingCall>>> =
        const { RefCell::new(BTreeMap::new()) };
}

fn sse(value: &Value) -> Result<String, String> {
    Ok(format!("data: {}\n\n", encode(value)?))
}

impl Guest for GeminiClient {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "agent-gemini".to_owned(),
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
                protocol: "google-gemini-generate-content".to_owned(),
                agent_tools: vec![
                    "gemini-cli".to_owned(),
                    "generic-google-genai-sdk".to_owned(),
                ],
            },
        ]
    }

    fn match_inbound(
        request_head: String,
    ) -> exports::token_station::adapter::agent_adapter::MatchResult {
        let head: Value = serde_json::from_str(&request_head).unwrap_or(Value::Null);
        let matched = head
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| method.eq_ignore_ascii_case("POST"))
            && head
                .get("path")
                .and_then(Value::as_str)
                .and_then(route)
                .is_some();
        exports::token_station::adapter::agent_adapter::MatchResult {
            matched,
            protocol: matched.then(|| "google-gemini-generate-content".to_owned()),
        }
    }

    fn normalize_inbound(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse(&envelope)?;
        let (model, stream) = route(transport_path(&envelope)?)
            .ok_or_else(|| invalid("transport_path is not a Gemini generateContent route"))?;
        let generation = envelope
            .body
            .get("generationConfig")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let max_output_tokens = generation
            .get("maxOutputTokens")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| invalid("maxOutputTokens must be an unsigned 32-bit integer"))
            })
            .transpose()?;
        encode(&ChatRequest {
            model: model.to_owned(),
            messages: parse_messages(&envelope.body)?,
            tools: parse_tools(&envelope.body)?,
            response_format: None,
            sampling: Sampling {
                temperature: generation.get("temperature").and_then(Value::as_f64),
                top_p: generation.get("topP").and_then(Value::as_f64),
                max_output_tokens,
                stop: string_array(generation.get("stopSequences"), "stopSequences")?,
            },
            stream,
            extensions: Extensions::new(),
        })
    }

    fn extract_agent_hint(_envelope: String) -> Result<String, String> {
        encode(&Vec::<AgentHint>::new())
    }

    fn render_response(response: String, _context: String) -> Result<String, String> {
        let response: ChatResponse = parse(&response)?;
        let candidates = response
            .choices
            .iter()
            .map(|choice| {
                Ok(json!({
                    "index": choice.index,
                    "content": {"role": "model", "parts": response_parts(&choice.message)?},
                    "finishReason": finish_reason(choice.finish_reason),
                }))
            })
            .collect::<Result<Vec<Value>, String>>()?;
        encode(&json!({
            "candidates": candidates,
            "usageMetadata": {
                "promptTokenCount": response.usage.input_tokens,
                "candidatesTokenCount": response.usage.output_tokens,
                "totalTokenCount": response.usage.total(),
                "cachedContentTokenCount": response.usage.cache_read_tokens,
            },
            "modelVersion": response.model,
            "responseId": response.id,
        }))
    }

    fn render_stream_event(event: String, context: String) -> Result<String, String> {
        let event: StreamEvent = parse(&event)?;
        let context: Value = parse(&context)?;
        let id = stream_id(&context);
        let model = context
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let data = match event {
            StreamEvent::Delta { index, content } => sse(&json!({
                "candidates": [{"index": index, "content": {"role": "model", "parts": [{"text": content}]}}],
                "modelVersion": model,
            }))?,
            StreamEvent::ToolCallDelta {
                index,
                name,
                arguments_delta,
                ..
            } => {
                STREAM_CALLS.with(|streams| {
                    let mut streams = streams.borrow_mut();
                    let pending = streams
                        .entry(id.clone())
                        .or_default()
                        .entry(index)
                        .or_default();
                    if name.is_some() {
                        pending.name = name;
                    }
                    pending.arguments.push_str(&arguments_delta);
                });
                String::new()
            }
            StreamEvent::Usage { usage } => sse(&json!({
                "candidates": [],
                "usageMetadata": {
                    "promptTokenCount": usage.input_tokens,
                    "candidatesTokenCount": usage.output_tokens,
                    "totalTokenCount": usage.total(),
                    "cachedContentTokenCount": usage.cache_read_tokens,
                },
                "modelVersion": model,
            }))?,
            StreamEvent::Done {
                finish_reason: reason,
            } => {
                let calls = STREAM_CALLS.with(|streams| streams.borrow_mut().remove(&id));
                let mut parts = Vec::new();
                if let Some(calls) = calls {
                    for (_, call) in calls {
                        let name = call
                            .name
                            .ok_or_else(|| invalid("streamed tool call declares no name"))?;
                        let args: Value = serde_json::from_str(&call.arguments).map_err(|_| {
                            invalid("streamed tool arguments are not complete JSON")
                        })?;
                        parts.push(json!({"functionCall": {"name": name, "args": args}}));
                    }
                }
                sse(&json!({
                    "candidates": [{
                        "index": 0,
                        "content": {"role": "model", "parts": parts},
                        "finishReason": finish_reason(reason),
                    }],
                    "modelVersion": model,
                }))?
            }
            StreamEvent::Error { error } => {
                STREAM_CALLS.with(|streams| streams.borrow_mut().remove(&id));
                let rendered = Self::map_inbound_error(encode(&error)?, String::new())?;
                format!("data: {rendered}\n\n")
            }
        };
        encode(&json!({"data": data}))
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse(&error)?;
        let status = match error.code {
            ErrorCode::InvalidRequest => "INVALID_ARGUMENT",
            ErrorCode::Auth => "UNAUTHENTICATED",
            ErrorCode::PaymentRequired => "FAILED_PRECONDITION",
            ErrorCode::RateLimit => "RESOURCE_EXHAUSTED",
            ErrorCode::Capacity | ErrorCode::UpstreamUnavailable => "UNAVAILABLE",
            ErrorCode::Capability => "FAILED_PRECONDITION",
            ErrorCode::ContentPolicy => "PERMISSION_DENIED",
            ErrorCode::ContextLength => "OUT_OF_RANGE",
            ErrorCode::Timeout => "DEADLINE_EXCEEDED",
            ErrorCode::TransportTruncated
            | ErrorCode::ProviderProtocolError
            | ErrorCode::Internal => "INTERNAL",
        };
        let mut body = Map::new();
        body.insert("code".to_owned(), json!(error.http_status));
        body.insert("message".to_owned(), json!(error.message));
        body.insert("status".to_owned(), json!(status));
        encode(&json!({"error": body}))
    }
}

export!(GeminiClient);
