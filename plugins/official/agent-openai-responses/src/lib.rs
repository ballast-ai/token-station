//! OpenAI Responses northbound adapter, scoped to the Codex M4 path.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::BTreeMap;

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, FinishReason, HintKind, ImageUrl, Message, Role, Sampling,
    StreamEvent, ToolCall, ToolDef, Usage,
};

struct ResponsesClient;

const DIRECT_REQUEST_FIELDS: &[&str] = &[
    "model",
    "instructions",
    "input",
    "tools",
    "max_output_tokens",
    "temperature",
    "top_p",
    "stream",
];

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

fn invalid(detail: impl Into<String>) -> String {
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

fn as_u32(value: &Value, field: &str) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| invalid(format!("{field} must be an unsigned 32-bit integer")))
}

fn validate_text_format(body: &Value) -> Result<(), String> {
    let Some(text) = body.get("text").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let text = text
        .as_object()
        .ok_or_else(|| invalid("text must be an object"))?;
    let Some(format) = text.get("format").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let format = format
        .as_object()
        .ok_or_else(|| invalid("text.format must be an object"))?;
    let kind = format
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("text.format declares no string type"))?;

    match kind {
        "text" => Ok(()),
        "json_schema" | "json_object" => Err(capability(
            "Responses structured output requires an approved Canonical IR/provider mapping",
        )),
        kind => Err(capability(format!(
            "unsupported Responses text.format type {kind}"
        ))),
    }
}

fn image_part(block: &Value) -> Result<ContentPart, String> {
    if block.get("file_id").is_some_and(|value| !value.is_null()) {
        return Err(capability(
            "Responses file_id images require an approved Canonical IR extension",
        ));
    }
    let image_url = block
        .get("image_url")
        .and_then(|value| match value {
            Value::String(url) => Some(ImageUrl {
                url: url.clone(),
                detail: block
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            Value::Object(object) => {
                object
                    .get("url")
                    .and_then(Value::as_str)
                    .map(|url| ImageUrl {
                        url: url.to_owned(),
                        detail: object
                            .get("detail")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
            }
            _ => None,
        })
        .ok_or_else(|| invalid("input_image declares no image_url"))?;
    Ok(ContentPart::ImageUrl { image_url })
}

fn content_part(block: &Value) -> Result<ContentPart, String> {
    match block.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "text") => Ok(ContentPart::Text {
            text: block
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("text content declares no text"))?
                .to_owned(),
        }),
        Some("input_image") => image_part(block),
        Some(kind) => Err(capability(format!(
            "unsupported Responses content item {kind}"
        ))),
        None => Err(invalid("content item declares no type")),
    }
}

fn content_value(value: &Value, field: &str) -> Result<Option<Content>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(Content::Text(text.clone()))),
        Value::Array(parts) => parts
            .iter()
            .map(content_part)
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| (!parts.is_empty()).then_some(Content::Parts(parts))),
        _ => Err(invalid(format!(
            "{field} must be a string or an array of content items"
        ))),
    }
}

fn role_of(value: &Value) -> Result<Role, String> {
    match value.as_str() {
        Some("developer" | "system") => Ok(Role::System),
        Some("user") => Ok(Role::User),
        Some("assistant") => Ok(Role::Assistant),
        _ => Err(invalid("message declares no known role")),
    }
}

fn message_item(item: &Value) -> Result<Message, String> {
    Ok(Message {
        role: role_of(&item["role"])?,
        content: content_value(&item["content"], "message content")?,
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    })
}

fn function_call_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("function_call declares no call_id"))?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("function_call declares no name"))?;
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("function_call declares no arguments"))?;
    Ok(Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
            id: call_id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        }],
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    })
}

fn function_output_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("function_call_output declares no call_id"))?;
    let content = content_value(
        item.get("output").unwrap_or(&Value::Null),
        "function_call_output output",
    )?;
    Ok(Message {
        role: Role::Tool,
        content,
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id.to_owned()),
        name: None,
        extensions: Extensions::new(),
    })
}

fn input_messages(input: &Value) -> Result<Vec<Message>, String> {
    match input {
        Value::String(text) => Ok(vec![Message::text(Role::User, text)]),
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("message") | None if item.get("role").is_some() => message_item(item),
                Some("function_call") => function_call_item(item),
                Some("function_call_output") => function_output_item(item),
                Some("reasoning") => Err(capability(
                    "Responses reasoning items require an approved Canonical IR extension",
                )),
                Some(kind) => Err(capability(format!(
                    "unsupported Responses input item {kind}"
                ))),
                None => Err(invalid("input item declares no type")),
            })
            .collect(),
        _ => Err(invalid("input must be a string or an array of input items")),
    }
}

fn tools_of(value: &Value) -> Result<Vec<ToolDef>, String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| {
            let kind = tool
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("tool declares no type"))?;
            if kind != "function" {
                return Err(capability(format!(
                    "unsupported Responses tool type {kind}"
                )));
            }
            Ok(ToolDef {
                name: tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("function tool declares no name"))?
                    .to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parameters: tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
            })
        })
        .collect()
}

fn request_extensions(body: &Value) -> Extensions {
    body.as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !DIRECT_REQUEST_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn text_parts(content: Option<&Content>) -> Result<Vec<Value>, String> {
    match content {
        None => Ok(Vec::new()),
        Some(Content::Text(text)) => Ok(vec![json!({
            "type": "output_text",
            "text": text
        })]),
        Some(Content::Parts(parts)) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => Ok(json!({
                    "type": "output_text",
                    "text": text
                })),
                ContentPart::ImageUrl { .. } => Err(capability(
                    "Responses assistant image output is not represented by the Canonical IR",
                )),
            })
            .collect(),
    }
}

fn response_output(response: &ChatResponse, response_id: &str) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    for choice in &response.choices {
        let content = text_parts(choice.message.content.as_ref())?;
        if !content.is_empty() {
            output.push(json!({
                "type": "message",
                "id": format!("msg_{response_id}_{}", choice.index),
                "status": "completed",
                "role": "assistant",
                "content": content
            }));
        }
        for call in &choice.message.tool_calls {
            output.push(json!({
                "type": "function_call",
                "id": format!("fc_{}", call.id),
                "status": "completed",
                "call_id": call.id,
                "name": call.name,
                "arguments": call.arguments
            }));
        }
    }
    Ok(output)
}

fn usage_json(usage: Usage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "input_tokens_details": {
            "cached_tokens": usage.cache_read_tokens,
            "cache_write_tokens": usage.cache_write_tokens
        },
        "output_tokens": usage.output_tokens,
        "output_tokens_details": {
            "reasoning_tokens": usage.reasoning_tokens
        },
        "total_tokens": usage.total()
    })
}

fn incomplete_details(reason: Option<FinishReason>) -> Value {
    match reason {
        Some(FinishReason::Length) => json!({"reason": "max_output_tokens"}),
        Some(FinishReason::ContentFilter) => json!({"reason": "content_filter"}),
        _ => Value::Null,
    }
}

fn response_object(
    response_id: &str,
    model: &str,
    output: Vec<Value>,
    usage: Usage,
    finish_reason: Option<FinishReason>,
) -> Value {
    let incomplete = matches!(
        finish_reason,
        Some(FinishReason::Length | FinishReason::ContentFilter)
    );
    json!({
        "id": response_id,
        "object": "response",
        "created_at": 0,
        "status": if incomplete { "incomplete" } else { "completed" },
        "error": null,
        "incomplete_details": incomplete_details(finish_reason),
        "instructions": null,
        "model": model,
        "output": output,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "tools": [],
        "usage": usage_json(usage)
    })
}

fn context_identity(context: &Value, fallback_id: &str, fallback_model: &str) -> (String, String) {
    (
        context
            .get("response_id")
            .and_then(Value::as_str)
            .unwrap_or(fallback_id)
            .to_owned(),
        context
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(fallback_model)
            .to_owned(),
    )
}

fn sse(kind: &str, payload: Value) -> Result<String, String> {
    Ok(format!(
        "event: {kind}\ndata: {}\n\n",
        serde_json::to_string(&payload).map_err(internal)?
    ))
}

#[derive(Clone)]
struct ToolStream {
    call_id: String,
    name: String,
    arguments: String,
}

struct StreamState {
    response_id: String,
    model: String,
    text: BTreeMap<u32, String>,
    tools: BTreeMap<u32, ToolStream>,
    usage: Usage,
}

impl StreamState {
    fn new(response_id: String, model: String) -> Self {
        Self {
            response_id,
            model,
            text: BTreeMap::new(),
            tools: BTreeMap::new(),
            usage: Usage::default(),
        }
    }
}

thread_local! {
    static STREAMS: RefCell<BTreeMap<String, StreamState>> =
        const { RefCell::new(BTreeMap::new()) };
}

fn ensure_stream<'a>(
    states: &'a mut BTreeMap<String, StreamState>,
    stream_id: &str,
    response_id: &str,
    model: &str,
) -> Result<(&'a mut StreamState, bool), String> {
    let created = !states.contains_key(stream_id);
    if created {
        states.insert(
            stream_id.to_owned(),
            StreamState::new(response_id.to_owned(), model.to_owned()),
        );
    }
    let context_changed = states
        .get(stream_id)
        .is_some_and(|state| state.response_id != response_id || state.model != model);
    if context_changed {
        states.remove(stream_id);
        return Err(invalid(
            "stream render context changed response_id or model mid-stream",
        ));
    }
    let state = states.get_mut(stream_id).expect("state inserted");
    Ok((state, created))
}

fn started_event(state: &StreamState) -> Result<String, String> {
    sse(
        "response.created",
        json!({
            "type": "response.created",
            "response": {
                "id": state.response_id,
                "object": "response",
                "created_at": 0,
                "status": "in_progress",
                "model": state.model,
                "output": []
            }
        }),
    )
}

fn stream_done(state: StreamState, finish_reason: Option<FinishReason>) -> Result<String, String> {
    let mut rendered = String::new();
    let mut output = Vec::new();
    for (index, text) in state.text {
        let item = json!({
            "type": "message",
            "id": format!("msg_{}_{}", state.response_id, index),
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}]
        });
        rendered.push_str(&sse(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": index,
                "item": item
            }),
        )?);
        output.push(item);
    }
    for (index, call) in state.tools {
        let item = json!({
            "type": "function_call",
            "id": format!("fc_{}", call.call_id),
            "status": "completed",
            "call_id": call.call_id,
            "name": call.name,
            "arguments": call.arguments
        });
        rendered.push_str(&sse(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": index,
                "item": item
            }),
        )?);
        output.push(item);
    }
    let response = response_object(
        &state.response_id,
        &state.model,
        output,
        state.usage,
        finish_reason,
    );
    let kind = if response["status"] == "incomplete" {
        "response.incomplete"
    } else {
        "response.completed"
    };
    rendered.push_str(&sse(kind, json!({"type": kind, "response": response}))?);
    Ok(rendered)
}

fn error_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid_request",
        ErrorCode::Auth => "authentication_error",
        ErrorCode::RateLimit => "rate_limit_exceeded",
        ErrorCode::Capacity => "server_overloaded",
        ErrorCode::Capability => "unsupported_capability",
        ErrorCode::ContentPolicy => "invalid_prompt",
        ErrorCode::UpstreamUnavailable => "server_error",
        ErrorCode::Timeout => "timeout",
        ErrorCode::Internal => "internal_error",
    }
}

impl Guest for ResponsesClient {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "agent-openai-responses".to_owned(),
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
                protocol: "openai-responses".to_owned(),
                agent_tools: vec!["codex".to_owned()],
            },
        ]
    }

    fn match_inbound(
        request_head: String,
    ) -> exports::token_station::adapter::agent_adapter::MatchResult {
        let head: Value = serde_json::from_str(&request_head).unwrap_or(Value::Null);
        let method = head["method"].as_str().unwrap_or_default();
        let path = head["path"].as_str().unwrap_or_default();
        let matched = method.eq_ignore_ascii_case("POST")
            && path
                .split('?')
                .next()
                .is_some_and(|path| path == "/v1/responses");
        exports::token_station::adapter::agent_adapter::MatchResult {
            matched,
            protocol: matched.then(|| "openai-responses".to_owned()),
        }
    }

    fn normalize_inbound(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse_input(&envelope)?;
        let body = &envelope.body;
        validate_text_format(body)?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("request declares no model"))?
            .to_owned();
        let mut messages = Vec::new();
        if let Some(instructions) = body.get("instructions").filter(|value| !value.is_null()) {
            let instructions = instructions
                .as_str()
                .ok_or_else(|| invalid("instructions must be a string"))?;
            messages.push(Message::text(Role::System, instructions));
        }
        let input = body
            .get("input")
            .ok_or_else(|| invalid("request declares no input"))?;
        messages.extend(input_messages(input)?);
        let sampling = Sampling {
            temperature: body.get("temperature").and_then(Value::as_f64),
            top_p: body.get("top_p").and_then(Value::as_f64),
            max_output_tokens: body
                .get("max_output_tokens")
                .filter(|value| !value.is_null())
                .map(|value| as_u32(value, "max_output_tokens"))
                .transpose()?,
            stop: Vec::new(),
        };
        to_output(&ChatRequest {
            model,
            messages,
            tools: tools_of(body.get("tools").unwrap_or(&Value::Null))?,
            response_format: None,
            sampling,
            stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
            extensions: request_extensions(body),
        })
    }

    fn extract_agent_hint(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse_input(&envelope)?;
        let hints = envelope
            .headers
            .value("x-agent-step")
            .filter(|value| matches!(*value, "planning" | "edit" | "summarize"))
            .map(|value| vec![AgentHint::new(HintKind::StepType, value)])
            .unwrap_or_default();
        to_output(&hints)
    }

    fn render_response(response: String, context: String) -> Result<String, String> {
        let response: ChatResponse = parse_input(&response)?;
        let context: Value = parse_input(&context)?;
        let (response_id, model) = context_identity(&context, &response.id, &response.model);
        let finish_reason = response
            .choices
            .iter()
            .find_map(|choice| choice.finish_reason);
        let output = response_output(&response, &response_id)?;
        to_output(&response_object(
            &response_id,
            &model,
            output,
            response.usage,
            finish_reason,
        ))
    }

    fn render_stream_event(event: String, context: String) -> Result<String, String> {
        let event: StreamEvent = parse_input(&event)?;
        let context: Value = parse_input(&context)?;
        let stream_id = context
            .get("stream_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("stream render context declares no stream_id"))?;
        let response_id = context
            .get("response_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("stream render context declares no response_id"))?;
        let model = context
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("stream render context declares no model"))?;

        if let StreamEvent::Error { error } = event {
            STREAMS.with(|streams| {
                streams.borrow_mut().remove(stream_id);
            });
            let response = json!({
                "id": response_id,
                "object": "response",
                "created_at": 0,
                "status": "failed",
                "model": model,
                "output": [],
                "error": {
                    "type": "server_error",
                    "code": error_code(error.code),
                    "message": error.message
                }
            });
            return to_output(&json!({
                "data": sse(
                    "response.failed",
                    json!({"type": "response.failed", "response": response})
                )?
            }));
        }

        let data = STREAMS.with(|streams| {
            let mut states = streams.borrow_mut();
            let (state, created) = ensure_stream(&mut states, stream_id, response_id, model)?;
            let mut rendered = if created {
                started_event(state)?
            } else {
                String::new()
            };
            match event {
                StreamEvent::Delta { index, content } => {
                    let item_id = format!("msg_{}_{}", state.response_id, index);
                    if !state.text.contains_key(&index) {
                        rendered.push_str(&sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": index,
                                "item": {
                                    "type": "message",
                                    "id": item_id,
                                    "status": "in_progress",
                                    "role": "assistant",
                                    "content": []
                                }
                            }),
                        )?);
                        rendered.push_str(&sse(
                            "response.content_part.added",
                            json!({
                                "type": "response.content_part.added",
                                "item_id": item_id,
                                "output_index": index,
                                "content_index": 0,
                                "part": {
                                    "type": "output_text",
                                    "text": "",
                                    "annotations": []
                                }
                            }),
                        )?);
                    }
                    state.text.entry(index).or_default().push_str(&content);
                    rendered.push_str(&sse(
                        "response.output_text.delta",
                        json!({
                            "type": "response.output_text.delta",
                            "item_id": item_id,
                            "output_index": index,
                            "content_index": 0,
                            "delta": content
                        }),
                    )?);
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        state.tools.entry(index)
                    {
                        let call = ToolStream {
                            call_id: id.clone().ok_or_else(|| {
                                invalid("first tool-call stream fragment declares no id")
                            })?,
                            name: name.clone().ok_or_else(|| {
                                invalid("first tool-call stream fragment declares no name")
                            })?,
                            arguments: String::new(),
                        };
                        rendered.push_str(&sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": index,
                                "item": {
                                    "type": "function_call",
                                    "id": format!("fc_{}", call.call_id),
                                    "status": "in_progress",
                                    "call_id": call.call_id,
                                    "name": call.name,
                                    "arguments": ""
                                }
                            }),
                        )?);
                        entry.insert(call);
                    }
                    let call = state.tools.get_mut(&index).expect("tool inserted");
                    if id.as_deref().is_some_and(|id| id != call.call_id)
                        || name.as_deref().is_some_and(|name| name != call.name)
                    {
                        return Err(invalid(
                            "tool-call identity changed between stream fragments",
                        ));
                    }
                    call.arguments.push_str(&arguments_delta);
                    rendered.push_str(&sse(
                        "response.function_call_arguments.delta",
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "item_id": format!("fc_{}", call.call_id),
                            "output_index": index,
                            "delta": arguments_delta
                        }),
                    )?);
                }
                StreamEvent::Usage { usage } => {
                    state.usage = usage;
                }
                StreamEvent::Done { finish_reason } => {
                    let state = states.remove(stream_id).expect("state inserted");
                    rendered.push_str(&stream_done(state, finish_reason)?);
                }
                StreamEvent::Error { .. } => unreachable!("error handled above"),
            }
            Ok::<_, String>(rendered)
        })?;
        to_output(&json!({"data": data}))
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse_input(&error)?;
        to_output(&json!({
            "error": {
                "type": "error",
                "code": error_code(error.code),
                "message": error.message
            }
        }))
    }
}

export!(ResponsesClient);
