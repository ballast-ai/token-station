//! The official Anthropic Messages inbound agent adapter.
//!
//! This component is northbound and provider-neutral. It translates Anthropic
//! Messages requests into the Canonical IR and renders Canonical responses back
//! to Anthropic JSON/SSE. Provider selection and credentials remain in the host.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Map, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, FinishReason, HintKind, ImageUrl, Message, Role, Sampling,
    StreamEvent, ToolCall, ToolChoice, ToolDef, Usage,
};

struct AnthropicClient;

const KNOWN_REQUEST_FIELDS: &[&str] = &[
    "model",
    "messages",
    "system",
    "tools",
    "max_tokens",
    "stop_sequences",
    "temperature",
    "top_p",
    "stream",
    "thinking",
    "tool_choice",
];

// These are intentionally adapter-local extensions rather than new Canonical
// IR fields. Thinking configuration and signatures are Anthropic-specific and
// must remain opaque unless a downstream provider explicitly supports them.
const ANTHROPIC_THINKING_EXTENSION: &str = "anthropic_thinking";
const ANTHROPIC_THINKING_BLOCKS_EXTENSION: &str = "anthropic_thinking_blocks";

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

fn validate_thinking(body: &Value) -> Result<(), String> {
    match body.get("thinking") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(config)) => match config.get("type").and_then(Value::as_str) {
            // There is no provider-side consumer for this adapter-local
            // extension yet. Accepting enabled/adaptive thinking would claim
            // a capability while silently dropping its semantics upstream.
            Some("disabled") => Ok(()),
            Some(kind) => Err(capability(format!(
                "Anthropic thinking type {kind} is not supported by the configured provider"
            ))),
            None => Err(invalid("thinking declares no string type")),
        },
        Some(_) => Err(invalid("thinking must be an object")),
    }
}

fn validate_tool_choice(body: &Value) -> Result<(), String> {
    let Some(choice) = body.get("tool_choice").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let choice = choice
        .as_object()
        .ok_or_else(|| invalid("tool_choice must be an object"))?;
    let kind = choice
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("tool_choice declares no string type"))?;

    if choice
        .get("disable_parallel_tool_use")
        .is_some_and(|value| value == &Value::Bool(true))
    {
        return Err(capability(
            "Anthropic tool_choice cannot disable parallel tool use through Canonical IR",
        ));
    }
    if choice
        .get("disable_parallel_tool_use")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(invalid(
            "tool_choice.disable_parallel_tool_use must be a boolean",
        ));
    }

    match kind {
        "auto" => Ok(()),
        _ => Err(capability(format!(
            "Anthropic tool_choice type {kind} cannot be preserved by Canonical IR"
        ))),
    }
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

fn parse_image(block: &Value) -> Result<ContentPart, String> {
    let source = block
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("image block declares no source"))?;
    let url = match source.get("type").and_then(Value::as_str) {
        Some("url") => source
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("URL image source declares no url"))?
            .to_owned(),
        Some("base64") => {
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("base64 image source declares no media_type"))?;
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("base64 image source declares no data"))?;
            format!("data:{media_type};base64,{data}")
        }
        Some(kind) => {
            return Err(capability(format!(
                "unsupported image source type `{kind}`"
            )))
        }
        None => return Err(invalid("image source declares no type")),
    };

    Ok(ContentPart::ImageUrl {
        image_url: ImageUrl { url, detail: None },
    })
}

fn parse_plain_block(block: &Value) -> Result<ContentPart, String> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => Ok(ContentPart::Text {
            text: block
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("text block declares no text"))?
                .to_owned(),
        }),
        Some("image") => parse_image(block),
        Some("thinking" | "redacted_thinking") => Err(capability(
            "Anthropic thinking blocks require an approved Canonical IR extension",
        )),
        Some(kind) => Err(capability(format!(
            "unsupported Anthropic content block `{kind}`"
        ))),
        None => Err(invalid("content block declares no type")),
    }
}

fn content_from_parts(parts: Vec<ContentPart>) -> Option<Content> {
    (!parts.is_empty()).then_some(Content::Parts(parts))
}

fn system_message(system: &Value) -> Result<Option<Message>, String> {
    match system {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(Message::text(Role::System, text))),
        Value::Array(blocks) => {
            let parts = blocks
                .iter()
                .map(parse_plain_block)
                .collect::<Result<Vec<_>, _>>()?;
            if parts
                .iter()
                .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
            {
                return Err(invalid("Anthropic system blocks may only contain text"));
            }
            Ok(content_from_parts(parts).map(|content| Message {
                role: Role::System,
                content: Some(content),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                extensions: Extensions::new(),
            }))
        }
        _ => Err(invalid(
            "system must be a string or an array of text blocks",
        )),
    }
}

fn tool_call(block: &Value) -> Result<ToolCall, String> {
    let id = block
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("tool_use block declares no id"))?;
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("tool_use block declares no name"))?;
    let input = block
        .get("input")
        .ok_or_else(|| invalid("tool_use block declares no input"))?;

    Ok(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::to_string(input).map_err(internal)?,
    })
}

fn tool_result_content(block: &Value) -> Result<Option<Content>, String> {
    match block.get("content") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(Content::Text(text.clone()))),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(parse_plain_block)
            .collect::<Result<Vec<_>, _>>()
            .map(content_from_parts),
        Some(_) => Err(invalid(
            "tool_result content must be a string or an array of content blocks",
        )),
    }
}

fn push_user_parts(messages: &mut Vec<Message>, parts: &mut Vec<ContentPart>) {
    if let Some(content) = content_from_parts(std::mem::take(parts)) {
        messages.push(Message {
            role: Role::User,
            content: Some(content),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            extensions: Extensions::new(),
        });
    }
}

fn parse_user_blocks(blocks: &[Value]) -> Result<Vec<Message>, String> {
    let mut messages = Vec::new();
    let mut parts = Vec::new();

    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                push_user_parts(&mut messages, &mut parts);
                let tool_call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("tool_result block declares no tool_use_id"))?;
                let mut extensions = Extensions::new();
                if let Some(is_error) = block.get("is_error").and_then(Value::as_bool) {
                    extensions.insert("anthropic_tool_result_is_error".to_owned(), json!(is_error));
                }
                messages.push(Message {
                    role: Role::Tool,
                    content: tool_result_content(block)?,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(tool_call_id.to_owned()),
                    name: None,
                    extensions,
                });
            }
            Some("tool_use") => {
                return Err(invalid("user messages cannot contain tool_use blocks"));
            }
            _ => parts.push(parse_plain_block(block)?),
        }
    }
    push_user_parts(&mut messages, &mut parts);
    Ok(messages)
}

fn parse_assistant_blocks(blocks: &[Value]) -> Result<Message, String> {
    let mut parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut thinking_blocks = Vec::new();

    for (index, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_calls.push(tool_call(block)?),
            Some("thinking" | "redacted_thinking") => thinking_blocks.push(json!({
                "index": index,
                "block": block,
            })),
            Some("tool_result") => {
                return Err(invalid(
                    "assistant messages cannot contain tool_result blocks",
                ));
            }
            Some("image") => {
                return Err(invalid("assistant messages cannot contain image blocks"));
            }
            _ => parts.push(parse_plain_block(block)?),
        }
    }

    let mut extensions = Extensions::new();
    if !thinking_blocks.is_empty() {
        extensions.insert(
            ANTHROPIC_THINKING_BLOCKS_EXTENSION.to_owned(),
            Value::Array(thinking_blocks),
        );
    }

    Ok(Message {
        role: Role::Assistant,
        content: content_from_parts(parts),
        tool_calls,
        tool_call_id: None,
        name: None,
        extensions,
    })
}

fn parse_messages(body: &Value) -> Result<Vec<Message>, String> {
    let raw_messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("request declares no messages array"))?;
    let mut messages = Vec::new();

    if let Some(system) = system_message(body.get("system").unwrap_or(&Value::Null))? {
        messages.push(system);
    }

    for raw in raw_messages {
        let role = raw
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("message declares no role"))?;
        match (role, raw.get("content")) {
            ("system", Some(content)) => {
                let message = system_message(content)?
                    .ok_or_else(|| invalid("system message declares no content"))?;
                messages.push(message);
            }
            ("user", Some(Value::String(text))) => {
                messages.push(Message::text(Role::User, text));
            }
            ("assistant", Some(Value::String(text))) => {
                messages.push(Message::text(Role::Assistant, text));
            }
            ("user", Some(Value::Array(blocks))) => {
                messages.extend(parse_user_blocks(blocks)?);
            }
            ("assistant", Some(Value::Array(blocks))) => {
                messages.push(parse_assistant_blocks(blocks)?);
            }
            ("system" | "user" | "assistant", _) => {
                return Err(invalid("message content must be a string or block array"));
            }
            _ => return Err(invalid(format!("unknown Anthropic message role `{role}`"))),
        }
    }
    Ok(messages)
}

fn parse_tools(body: &Value) -> Result<Vec<ToolDef>, String> {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|tool| {
            Ok(ToolDef {
                name: tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("tool declares no name"))?
                    .to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parameters: tool
                    .get("input_schema")
                    .cloned()
                    .ok_or_else(|| invalid("tool declares no input_schema"))?,
            })
        })
        .collect()
}

fn request_extensions(body: &Value) -> Extensions {
    let mut extensions: Extensions = body
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !KNOWN_REQUEST_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(thinking) = body.get("thinking").filter(|value| !value.is_null()) {
        extensions.insert(ANTHROPIC_THINKING_EXTENSION.to_owned(), thinking.clone());
    }
    extensions
}

fn finish_reason_to_anthropic(reason: Option<FinishReason>) -> Value {
    match reason {
        Some(FinishReason::Stop) => json!("end_turn"),
        Some(FinishReason::Length) => json!("max_tokens"),
        Some(FinishReason::ToolCalls) => json!("tool_use"),
        Some(FinishReason::ContentFilter) => json!("refusal"),
        Some(FinishReason::StopSequence) => json!("stop_sequence"),
        // 0.3.0: unknown reasons survive verbatim instead of vanishing.
        Some(FinishReason::Other(other)) => json!(other),
        None => Value::Null,
    }
}

fn response_blocks(message: &Message) -> Result<Vec<Value>, String> {
    let mut blocks = Vec::new();
    match message.content.as_ref() {
        Some(Content::Text(text)) => blocks.push(json!({"type": "text", "text": text})),
        Some(Content::Parts(parts)) => {
            for part in parts {
                match part {
                    ContentPart::Text { text } => {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                    ContentPart::ImageUrl { .. } => {
                        return Err(capability(
                            "Anthropic assistant responses cannot contain image blocks",
                        ));
                    }
                    ContentPart::Thinking {
                        thinking,
                        signature,
                    } => {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": signature,
                        }));
                    }
                    ContentPart::RedactedThinking { data } => {
                        blocks.push(json!({"type": "redacted_thinking", "data": data}));
                    }
                    // 0.3.0: unmodeled parts survive verbatim instead of
                    // silently dropping content.
                    ContentPart::Unknown(value) => blocks.push(value.clone()),
                }
            }
        }
        None => {}
    }
    for call in &message.tool_calls {
        let input = serde_json::from_str::<Value>(&call.arguments).map_err(|_| {
            invalid(format!(
                "tool call `{}` arguments are not complete JSON",
                call.id
            ))
        })?;
        if !input.is_object() {
            return Err(invalid(format!(
                "tool call `{}` arguments must encode a JSON object",
                call.id
            )));
        }
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": input,
        }));
    }
    Ok(blocks)
}

fn anthropic_error_type(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Auth | ErrorCode::PaymentRequired => "authentication_error",
        ErrorCode::RateLimit => "rate_limit_error",
        ErrorCode::Capacity => "overloaded_error",
        ErrorCode::InvalidRequest
        | ErrorCode::Capability
        | ErrorCode::ContentPolicy
        | ErrorCode::ContextLength => "invalid_request_error",
        ErrorCode::UpstreamUnavailable
        | ErrorCode::TransportTruncated
        | ErrorCode::ProviderProtocolError
        | ErrorCode::Timeout
        | ErrorCode::Internal => "api_error",
    }
}

const MAX_ACTIVE_STREAMS: usize = 1024;
const MAX_BLOCKS_PER_STREAM: u32 = 256;

#[derive(Debug)]
struct ToolBlock {
    content_index: u32,
    id: String,
    name: String,
}

#[derive(Debug)]
struct StreamState {
    response_id: String,
    model: String,
    usage: Usage,
    started: bool,
    next_block_index: u32,
    text_blocks: BTreeMap<u32, u32>,
    thinking_blocks: BTreeMap<u32, u32>,
    tool_blocks: BTreeMap<u32, ToolBlock>,
    open_blocks: BTreeSet<u32>,
}

impl StreamState {
    fn new(response_id: &str, model: &str, input_tokens: u64) -> Self {
        Self {
            response_id: response_id.to_owned(),
            model: model.to_owned(),
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
            started: false,
            next_block_index: 0,
            text_blocks: BTreeMap::new(),
            thinking_blocks: BTreeMap::new(),
            tool_blocks: BTreeMap::new(),
            open_blocks: BTreeSet::new(),
        }
    }

    fn allocate_block(&mut self) -> Result<u32, String> {
        if self.next_block_index >= MAX_BLOCKS_PER_STREAM {
            return Err(capability(format!(
                "stream exceeds the {MAX_BLOCKS_PER_STREAM}-block safety limit"
            )));
        }
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.open_blocks.insert(index);
        Ok(index)
    }

    /// The anthropic content-block index for choice `index`'s thinking
    /// block, opening it on first use.
    fn thinking_block(&mut self, index: u32, rendered: &mut String) -> Result<u32, String> {
        if let Some(block_index) = self.thinking_blocks.get(&index) {
            return Ok(*block_index);
        }
        let block_index = self.allocate_block()?;
        self.thinking_blocks.insert(index, block_index);
        rendered.push_str(&sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        )?);
        Ok(block_index)
    }

    fn ensure_started(&mut self, rendered: &mut String) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        rendered.push_str(&sse(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": self.response_id,
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": self.model,
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": self.usage.input_tokens, "output_tokens": 0}
                }
            }),
        )?);
        self.started = true;
        Ok(())
    }
}

fn stream_usage(usage: Usage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cache_read_tokens,
        "cache_creation_input_tokens": usage.cache_write_tokens,
    })
}

thread_local! {
    static STREAM_STATES: RefCell<BTreeMap<String, StreamState>> = const {
        RefCell::new(BTreeMap::new())
    };
}

fn stream_context(context: &Value) -> Result<(&str, &str, &str, u64), String> {
    let stream_id = context
        .get("stream_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("stream render context declares no stream_id"))?;
    let response_id = context
        .get("response_id")
        .or_else(|| context.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("msg_token_station");
    let model = context
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let input_tokens = context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok((stream_id, response_id, model, input_tokens))
}

fn sse(event: &str, data: Value) -> Result<String, String> {
    Ok(format!("event: {event}\ndata: {}\n\n", to_output(&data)?))
}

impl Guest for AnthropicClient {
    fn metadata() -> AdapterMetadata {
        AdapterMetadata {
            name: "agent-anthropic".to_owned(),
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
                protocol: "anthropic-messages".to_owned(),
                agent_tools: vec![
                    "claude-code".to_owned(),
                    "claude-desktop".to_owned(),
                    "generic-anthropic-sdk".to_owned(),
                ],
            },
        ]
    }

    fn match_inbound(
        request_head: String,
    ) -> exports::token_station::adapter::agent_adapter::MatchResult {
        let head: Value = serde_json::from_str(&request_head).unwrap_or(Value::Null);
        let method = head
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let path = head
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        let matched = method.eq_ignore_ascii_case("POST") && path.ends_with("/v1/messages");

        exports::token_station::adapter::agent_adapter::MatchResult {
            matched,
            protocol: matched.then(|| "anthropic-messages".to_owned()),
        }
    }

    fn normalize_inbound(envelope: String) -> Result<String, String> {
        let envelope: AgentRequestEnvelope = parse_input(&envelope)?;
        let body = &envelope.body;
        validate_thinking(body)?;
        validate_tool_choice(body)?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("request declares no model"))?
            .to_owned();
        let max_output_tokens = as_u32(
            body.get("max_tokens")
                .ok_or_else(|| invalid("request declares no max_tokens"))?,
            "max_tokens",
        )?;
        let stop = match body.get("stop_sequences") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("stop_sequences must contain only strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(invalid("stop_sequences must be an array")),
        };

        to_output(&ChatRequest {
            model,
            messages: parse_messages(body)?,
            tools: parse_tools(body)?,
            response_format: None,
            // `validate_tool_choice` already refused everything but `auto`.
            tool_choice: body
                .get("tool_choice")
                .filter(|value| !value.is_null())
                .map(|_| ToolChoice::Auto),
            sampling: Sampling {
                temperature: body.get("temperature").and_then(Value::as_f64),
                top_p: body.get("top_p").and_then(Value::as_f64),
                max_output_tokens: Some(max_output_tokens),
                stop,
            },
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

    fn render_response(response: String, _context: String) -> Result<String, String> {
        let response: ChatResponse = parse_input(&response)?;
        if response.choices.len() != 1 {
            return Err(capability(
                "Anthropic Messages requires exactly one response choice",
            ));
        }
        let choice = &response.choices[0];
        let mut usage = Map::new();
        usage.insert(
            "input_tokens".to_owned(),
            json!(response.usage.input_tokens),
        );
        usage.insert(
            "output_tokens".to_owned(),
            json!(response.usage.output_tokens),
        );
        if response.usage.cache_read_tokens > 0 {
            usage.insert(
                "cache_read_input_tokens".to_owned(),
                json!(response.usage.cache_read_tokens),
            );
        }
        if response.usage.cache_write_tokens > 0 {
            usage.insert(
                "cache_creation_input_tokens".to_owned(),
                json!(response.usage.cache_write_tokens),
            );
        }

        to_output(&json!({
            "id": response.id,
            "type": "message",
            "role": "assistant",
            "content": response_blocks(&choice.message)?,
            "model": response.model,
            "stop_reason": finish_reason_to_anthropic(choice.finish_reason.clone()),
            "stop_sequence": null,
            "usage": Value::Object(usage),
        }))
    }

    fn render_stream_event(event: String, context: String) -> Result<String, String> {
        let event: StreamEvent = parse_input(&event)?;
        let context: Value = parse_input(&context)?;
        let (stream_id, response_id, model, input_tokens) = stream_context(&context)?;
        let data = STREAM_STATES.with(|states| {
            let mut states = states.borrow_mut();

            if let StreamEvent::Error { error } = &event {
                states.remove(stream_id);
                return sse(
                    "error",
                    json!({
                        "type": "error",
                        "error": {
                            "type": anthropic_error_type(error.code),
                            "message": error.message
                        }
                    }),
                );
            }

            if !states.contains_key(stream_id) {
                if states.len() >= MAX_ACTIVE_STREAMS {
                    return Err(capability(format!(
                        "adapter reached the {MAX_ACTIVE_STREAMS}-stream safety limit"
                    )));
                }
                states.insert(
                    stream_id.to_owned(),
                    StreamState::new(response_id, model, input_tokens),
                );
            }

            if let Some(state) = states.get(stream_id) {
                if state.response_id != response_id || state.model != model {
                    states.remove(stream_id);
                    return Err(invalid(
                        "stream render context changed response_id or model mid-stream",
                    ));
                }
            }

            let result = match event {
                StreamEvent::Delta { index, content } => {
                    let state = states.get_mut(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;
                    let block_index = match state.text_blocks.get(&index) {
                        Some(block_index) => *block_index,
                        None => {
                            let block_index = state.allocate_block()?;
                            state.text_blocks.insert(index, block_index);
                            rendered.push_str(&sse(
                                "content_block_start",
                                json!({
                                    "type": "content_block_start",
                                    "index": block_index,
                                    "content_block": {"type": "text", "text": ""}
                                }),
                            )?);
                            block_index
                        }
                    };
                    rendered.push_str(&sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": {"type": "text_delta", "text": content}
                        }),
                    )?);
                    Ok(rendered)
                }
                // 0.3.0: reasoning streams as native Anthropic thinking
                // blocks, one per choice index, closed at `Done` with the
                // other open blocks.
                StreamEvent::ThinkingDelta {
                    index,
                    thinking_delta,
                } => {
                    let state = states.get_mut(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;
                    let block_index = state.thinking_block(index, &mut rendered)?;
                    rendered.push_str(&sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": {"type": "thinking_delta", "thinking": thinking_delta}
                        }),
                    )?);
                    Ok(rendered)
                }
                StreamEvent::ThinkingSignatureDelta {
                    index,
                    signature_delta,
                } => {
                    let state = states.get_mut(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;
                    let block_index = state.thinking_block(index, &mut rendered)?;
                    rendered.push_str(&sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": {"type": "signature_delta", "signature": signature_delta}
                        }),
                    )?);
                    Ok(rendered)
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    let state = states.get_mut(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;

                    if !state.tool_blocks.contains_key(&index) {
                        let first_id = id
                            .as_deref()
                            .ok_or_else(|| {
                                invalid("first tool-call stream fragment declares no id")
                            })?
                            .to_owned();
                        let first_name = name
                            .as_deref()
                            .ok_or_else(|| {
                                invalid("first tool-call stream fragment declares no name")
                            })?
                            .to_owned();
                        let content_index = state.allocate_block()?;
                        rendered.push_str(&sse(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": content_index,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": first_id,
                                    "name": first_name,
                                    "input": {}
                                }
                            }),
                        )?);
                        state.tool_blocks.insert(
                            index,
                            ToolBlock {
                                content_index,
                                id: first_id,
                                name: first_name,
                            },
                        );
                    }

                    let block = state.tool_blocks.get(&index).expect("tool block inserted");
                    if id.as_deref().is_some_and(|id| id != block.id)
                        || name.as_deref().is_some_and(|name| name != block.name)
                    {
                        return Err(invalid(
                            "tool-call identity changed between stream fragments",
                        ));
                    }
                    rendered.push_str(&sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": block.content_index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": arguments_delta
                            }
                        }),
                    )?);
                    Ok(rendered)
                }
                StreamEvent::Usage { usage } => {
                    let state = states.get_mut(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;
                    state.usage = usage;
                    rendered.push_str(&sse(
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": null, "stop_sequence": null},
                            "usage": stream_usage(state.usage)
                        }),
                    )?);
                    Ok(rendered)
                }
                StreamEvent::Done {
                    finish_reason,
                    stop_sequence,
                } => {
                    let mut state = states.remove(stream_id).expect("state inserted above");
                    let mut rendered = String::new();
                    state.ensure_started(&mut rendered)?;
                    for block_index in &state.open_blocks {
                        rendered.push_str(&sse(
                            "content_block_stop",
                            json!({"type": "content_block_stop", "index": block_index}),
                        )?);
                    }
                    rendered.push_str(&sse(
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": {
                                "stop_reason": finish_reason_to_anthropic(finish_reason),
                                "stop_sequence": stop_sequence
                            },
                            "usage": stream_usage(state.usage)
                        }),
                    )?);
                    rendered.push_str(&sse("message_stop", json!({"type": "message_stop"}))?);
                    Ok(rendered)
                }
                StreamEvent::Error { .. } => unreachable!("error handled before state creation"),
            };

            if result.is_err() {
                states.remove(stream_id);
            }
            result
        })?;
        to_output(&json!({"data": data}))
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse_input(&error)?;
        to_output(&json!({
            "type": "error",
            "error": {
                "type": anthropic_error_type(error.code),
                "message": error.message,
            }
        }))
    }
}

export!(AnthropicClient);
