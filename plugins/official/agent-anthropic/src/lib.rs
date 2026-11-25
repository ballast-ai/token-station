//! The official Anthropic Messages inbound agent adapter.
//!
//! This component is northbound and provider-neutral. It translates Anthropic
//! Messages requests into the Canonical IR and renders Canonical responses back
//! to Anthropic JSON/SSE. Provider selection and credentials remain in the host.
//!
//! The mapping is South's north codec (`south-north-codec`), the one the
//! server uses for this wire. This adapter keeps what the codec leaves to a
//! host or does not produce: the request checks it always made, the Claude
//! replay markers, the extensions the plan components read, the effort clamp,
//! the per-stream render state and its limits, and the error frame.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Value};
use south_north_codec::{
    anthropic_frames, anthropic_message_response, chat_request_from_anthropic_messages,
    AnthropicSseState, CodecError, ResponseContext,
};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, HintKind, ImageUrl, Message, Role, StreamEvent, ToolCall,
};

struct AnthropicClient;

// Top-level wire keys of typed Canonical IR fields. The codec refuses a client
// key with one of these names. This adapter has always dropped them instead,
// and keeps doing so. `host_values` is the sharp case, since only the host may
// mint them.
const RESERVED_IR_KEYS: &[&str] = &[
    "host_values",
    "parallel_tool_calls",
    "reasoning_budget_tokens",
    "reasoning_effort",
    "reasoning_mode",
    "response_format",
    "sampling",
];

// `anthropic_thinking` stays an adapter-local extension: it is the request's
// thinking *configuration*, which the plan components still read.
const ANTHROPIC_THINKING_EXTENSION: &str = "anthropic_thinking";
const TOOL_RESULT_IS_ERROR_EXTENSION: &str = "anthropic_tool_result_is_error";

const MAX_ACTIVE_STREAMS: usize = 1024;
const MAX_BLOCKS_PER_STREAM: usize = 256;

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

fn validate_response_format(body: &Value) -> Result<(), String> {
    let has_format = |value: &Value| {
        value.get("output_format").is_some()
            || value
                .get("output_config")
                .is_some_and(|config| config.get("format").is_some())
    };
    if has_format(body)
        || body
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                messages
                    .iter()
                    .any(|message| message["role"] == "system" && has_format(message))
            })
    {
        return Err(capability(
            "Anthropic structured response formats are not supported by this adapter",
        ));
    }
    Ok(())
}

fn validate_thinking(body: &Value) -> Result<(), String> {
    match body.get("thinking") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(config)) => match config.get("type").and_then(Value::as_str) {
            // Native Anthropic routes bypass this adapter and preserve the
            // object verbatim. Translated routes keep it in extensions; the
            // provider may render a compatible effort control only when its
            // target model explicitly declares that capability.
            Some("adaptive" | "enabled" | "disabled") => Ok(()),
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

    // `auto`/`any` translate losslessly to the Canonical `ToolChoice`; a forced
    // specific `tool` is accepted but degraded to `auto` on this translate path
    // (see the mapping in `normalize_inbound`). The old message blamed "Canonical
    // IR", which is false — `ToolChoice::Other` can carry any shape; the real
    // constraint is the downstream OpenAI-compatible chat provider. The native
    // Anthropic passthrough path carries `{type:tool}` + server tools intact.
    match kind {
        "auto" | "any" | "tool" => Ok(()),
        _ => Err(capability(format!(
            "Anthropic tool_choice type {kind} is not supported by the configured chat provider"
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
        cache_control: None,
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
            cache_control: None,
        }),
        Some("image") => parse_image(block),
        Some("thinking") => Ok(ContentPart::Thinking {
            thinking: block
                .get("thinking")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("thinking block declares no thinking"))?
                .to_owned(),
            signature: block
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        Some("redacted_thinking") => Ok(ContentPart::RedactedThinking {
            data: block
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("redacted_thinking block declares no data"))?
                .to_owned(),
        }),
        // Every other typed block — server-tool history (`server_tool_use`,
        // `web_search_tool_result`, `mcp_tool_use`, …), `document`,
        // `search_result`, or a type this adapter has never heard of — rides
        // through the IR verbatim as `ContentPart::Unknown`. This adapter runs
        // before routing and cannot know which renderer will serve the
        // request, so it is not the place to decide what can be expressed:
        // the Anthropic renderer writes these blocks back unchanged, and a
        // renderer for another wire refuses them by name at render time
        // (stage A′; south 0.15.0 renderer refusal). Refusing here used to
        // turn every such request into a 400 even when an Anthropic upstream
        // would have served it.
        Some(_) => Ok(ContentPart::Unknown(block.clone())),
        None => Err(invalid("content block declares no type")),
    }
}

/// The Claude signed-thinking replay markers for one assistant turn, or `None`
/// when the turn cannot carry them.
///
/// South replays signatures only for a turn that names its family and lists
/// every block in wire order (`reasoning_replay::validated_layout`). The layout
/// admits text, `redacted_thinking`, signed thinking and tool calls. A turn
/// with no reasoning needs no replay. A turn with an unsigned thinking block or
/// a block the IR does not model (`server_tool_use`, …) cannot be described,
/// so it gets no markers and the component sends it without replay, as before.
/// The host removes the markers again for a target that cannot replay.
fn replay_markers(
    parts: &[ContentPart],
    tool_calls: &[ToolCall],
    order: &[Value],
) -> Option<Extensions> {
    const MAX_BLOCKS: usize = 128;
    let reasoning = parts.iter().any(|part| {
        matches!(
            part,
            ContentPart::Thinking { .. } | ContentPart::RedactedThinking { .. }
        )
    });
    let replayable = parts.iter().all(|part| match part {
        ContentPart::Text { .. } | ContentPart::RedactedThinking { .. } => true,
        ContentPart::Thinking { signature, .. } => signature
            .as_deref()
            .is_some_and(|signature| !signature.is_empty()),
        ContentPart::ImageUrl { .. } | ContentPart::Unknown(_) => false,
    });
    let mut ids = BTreeSet::new();
    let unique_ids = tool_calls
        .iter()
        .all(|call| !call.id.is_empty() && ids.insert(call.id.as_str()));
    if !reasoning || !replayable || !unique_ids || order.len() > MAX_BLOCKS {
        return None;
    }
    let mut extensions = Extensions::new();
    extensions.insert(
        "reasoning_replay_protocol_family".to_owned(),
        json!("claude-signed-thinking"),
    );
    extensions.insert(
        "reasoning_replay_block_layout".to_owned(),
        Value::Array(order.to_vec()),
    );
    Some(extensions)
}

fn is_anthropic_server_tool(tool_type: &str) -> bool {
    [
        "web_search_",
        "web_fetch_",
        "code_execution_",
        "tool_search_",
        "mcp_",
        "advisor_",
    ]
    .iter()
    .any(|prefix| tool_type.starts_with(prefix))
}

/// Maps Anthropic `output_config.effort` onto the provider-neutral
/// `reasoning_effort` extension the OpenAI-compatible provider renders. Anthropic
/// `low`/`medium`/`high` pass through; `max` clamps to `high`, the ceiling the
/// OpenAI `reasoning_effort` wire accepts. The `xhigh` dialect CC Switch emits
/// for `max` belongs to xAI/Codex Responses upstreams, not generic
/// OpenAI-compatible providers — sending it here risks a 400, so it is not used
/// until a native Responses passthrough exists. Unknown values are dropped,
/// matching the field's optimistic passthrough, and the provider's own
/// `reasoning_effort_allowed` gate still drops it for models that opt out.
fn reasoning_effort_from_output_config(body: &Value) -> Option<&'static str> {
    let output_config = body.get("output_config").or_else(|| {
        body.get("messages")?
            .as_array()?
            .iter()
            .rev()
            .filter(|message| message["role"] == "system")
            .find_map(|message| message.get("output_config"))
    })?;
    match output_config.get("effort").and_then(Value::as_str)? {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" | "max" => Some("high"),
        _ => None,
    }
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

// -- request checks the codec does not make -------------------------------------

/// The structural refusals this adapter has always made, before the codec
/// reads the body.
fn validate_shape(body: &Value) -> Result<(), String> {
    body.get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("request declares no model"))?;
    as_u32(
        body.get("max_tokens")
            .ok_or_else(|| invalid("request declares no max_tokens"))?,
        "max_tokens",
    )?;
    match body.get("stop_sequences") {
        None => {}
        Some(Value::Array(values)) => {
            if !values.iter().all(Value::is_string) {
                return Err(invalid("stop_sequences must contain only strings"));
            }
        }
        Some(_) => return Err(invalid("stop_sequences must be an array")),
    }
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        tool.get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("tool declares no name"))?;
        let tool_type = tool.get("type").and_then(Value::as_str);
        // Server tools execute inside an Anthropic-native provider and have
        // result, citation and usage semantics that Canonical IR cannot
        // represent. Refuse them before routing instead of presenting an
        // empty-schema function with no executor.
        if let Some(kind) = tool_type.filter(|kind| is_anthropic_server_tool(kind)) {
            return Err(capability(format!(
                "Anthropic server tool `{kind}` requires an anthropic-native provider; the translated route cannot execute it"
            )));
        }
        if matches!(tool_type, None | Some("custom")) && tool.get("input_schema").is_none() {
            return Err(invalid("tool declares no input_schema"));
        }
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("request declares no messages array"))?;
    // A block without a type is malformed, not unmodelled. The codec would
    // carry it as an unknown part.
    let blocks_of = |value: &Value| value.as_array().cloned().unwrap_or_default();
    let mut all_blocks = blocks_of(&body["system"]);
    for message in messages {
        for block in blocks_of(&message["content"]) {
            if block["type"] == "tool_result" {
                all_blocks.extend(blocks_of(&block["content"]));
            }
            all_blocks.push(block);
        }
    }
    if all_blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str).is_none())
    {
        return Err(invalid("content block declares no type"));
    }
    for message in messages {
        let blocks = message.get("content").and_then(Value::as_array);
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                for block in blocks.into_iter().flatten() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_result") => {
                            return Err(invalid(
                                "assistant messages cannot contain tool_result blocks",
                            ))
                        }
                        Some("image") => {
                            return Err(invalid("assistant messages cannot contain image blocks"))
                        }
                        _ => {}
                    }
                }
            }
            Some("user")
                if blocks
                    .into_iter()
                    .flatten()
                    .any(|block| block["type"] == "tool_use") =>
            {
                return Err(invalid("user messages cannot contain tool_use blocks"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn codec_error(error: &CodecError) -> String {
    invalid(error.to_string())
}

/// Restores what the codec does not produce, matching each raw block to the
/// IR message the codec built from it. The codec keeps assistant turns and
/// tool results one to one and in order; a count that does not match leaves
/// the request as the codec built it.
fn restore_adapter_state(body: &Value, request: &mut ChatRequest) -> Result<(), String> {
    let raw_messages = body["messages"].as_array().cloned().unwrap_or_default();

    // An assistant turn with only `tool_use` blocks has no content, as before.
    // The codec gives it an empty part list, which a Chat wire sends as `[]`.
    for message in &mut request.messages {
        if message.role == Role::Assistant
            && matches!(&message.content, Some(Content::Parts(parts)) if parts.is_empty())
        {
            message.content = None;
        }
    }

    // Claude replay markers on assistant turns, in wire order.
    let raw_assistants: Vec<&Value> = raw_messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect();
    let ir_assistants: Vec<&mut Message> = request
        .messages
        .iter_mut()
        .filter(|message| message.role == Role::Assistant)
        .collect();
    if raw_assistants.len() == ir_assistants.len() {
        for (raw, message) in raw_assistants.into_iter().zip(ir_assistants) {
            let Some(blocks) = raw["content"].as_array() else {
                continue;
            };
            let parts = match &message.content {
                Some(Content::Parts(parts)) => parts.clone(),
                _ => Vec::new(),
            };
            let mut order = Vec::new();
            let mut ordinal = 0;
            for block in blocks {
                if block["type"] == "tool_use" {
                    order.push(json!({"kind": "tool_call", "call_id": block["id"]}));
                } else {
                    order.push(json!({"kind": "content", "ordinal": ordinal}));
                    ordinal += 1;
                }
            }
            if let Some(markers) = replay_markers(&parts, &message.tool_calls, &order) {
                message.extensions.extend(markers);
            }
        }
    }

    // Tool results: the error flag the plan components read, and non-text
    // blocks the codec drops (an image a tool returned stays with it).
    let raw_results: Vec<&Value> = raw_messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .collect();
    let ir_results: Vec<&mut Message> = request
        .messages
        .iter_mut()
        .filter(|message| message.role == Role::Tool)
        .collect();
    if raw_results.len() == ir_results.len() {
        for (raw, message) in raw_results.into_iter().zip(ir_results) {
            if let Some(is_error) = raw.get("is_error").and_then(Value::as_bool) {
                message
                    .extensions
                    .insert(TOOL_RESULT_IS_ERROR_EXTENSION.to_owned(), json!(is_error));
            }
            if let Some(blocks) = raw["content"].as_array() {
                if blocks.iter().any(|block| block["type"] != "text") {
                    let parts = blocks
                        .iter()
                        .map(parse_plain_block)
                        .collect::<Result<Vec<_>, _>>()?;
                    message.content = (!parts.is_empty()).then_some(Content::Parts(parts));
                }
            }
        }
    }

    // The thinking configuration, for the plan components.
    if let Some(thinking) = body.get("thinking").filter(|value| !value.is_null()) {
        request
            .extensions
            .insert(ANTHROPIC_THINKING_EXTENSION.to_owned(), thinking.clone());
    }

    // The effort clamp: `max` is `high`, an unknown level is dropped, and a
    // Claude Code system entry may carry it.
    request.reasoning.effort = reasoning_effort_from_output_config(body)
        .map(|effort| serde_json::from_value(json!(effort)).map_err(internal))
        .transpose()?;
    Ok(())
}

// -- streams ---------------------------------------------------------------------

struct StreamState {
    response_id: String,
    model: String,
    codec: AnthropicSseState,
    /// The wire block each source thinking block renders into. The codec keeps
    /// one open thinking block, so two source blocks (interleaved thinking)
    /// would share it, and their signatures with it.
    thinking_blocks: BTreeMap<u32, usize>,
}

thread_local! {
    static STREAM_STATES: RefCell<BTreeMap<String, StreamState>> = const {
        RefCell::new(BTreeMap::new())
    };
}

fn render_frames(state: &mut StreamState, event: &StreamEvent) -> Result<String, String> {
    let thinking_block = match event {
        StreamEvent::ThinkingDelta { block_index, .. }
        | StreamEvent::ThinkingSignatureDelta { block_index, .. } => Some(*block_index),
        _ => None,
    };
    if let Some(block_index) = thinking_block {
        // Point the codec at this source block's wire block, or at none so it
        // opens a new one.
        state.codec.thinking_index = state.thinking_blocks.get(&block_index).copied();
    }
    let frames = anthropic_frames(std::slice::from_ref(event), &mut state.codec);
    if let (Some(block_index), Some(index)) = (thinking_block, state.codec.thinking_index) {
        state.thinking_blocks.insert(block_index, index);
    }
    if state.codec.next_index > MAX_BLOCKS_PER_STREAM {
        return Err(capability(format!(
            "stream exceeded {MAX_BLOCKS_PER_STREAM} content blocks"
        )));
    }
    let mut rendered = String::new();
    for frame in frames {
        rendered.push_str(&sse(frame.event, frame.data)?);
    }
    Ok(rendered)
}

fn state_ended(states: &BTreeMap<String, StreamState>, stream_id: &str) -> bool {
    states
        .get(stream_id)
        .is_some_and(|state| state.codec.terminated)
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
        validate_response_format(body)?;
        validate_thinking(body)?;
        validate_tool_choice(body)?;
        validate_shape(body)?;
        let mut wire = body.clone();
        if let Some(object) = wire.as_object_mut() {
            for key in RESERVED_IR_KEYS {
                object.remove(*key);
            }
        }
        let mut request =
            chat_request_from_anthropic_messages(&wire).map_err(|e| codec_error(&e))?;
        restore_adapter_state(body, &mut request)?;
        to_output(&request)
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
        let context: Value = serde_json::from_str(&context).unwrap_or(Value::Null);
        let fallback_id = context
            .get("response_id")
            .and_then(Value::as_str)
            .unwrap_or("msg_token_station")
            .to_owned();
        let rendered = anthropic_message_response(
            &response,
            &ResponseContext {
                created: 0,
                fallback_id,
            },
        )
        .map_err(|error| match &error {
            // The Messages envelope carries one message.
            CodecError::Unrenderable { field, .. } if field == "choices" => {
                capability("Anthropic Messages requires exactly one response choice")
            }
            _ => codec_error(&error),
        })?;
        to_output(&rendered)
    }

    fn render_stream_event(event: String, context: String) -> Result<String, String> {
        let event: StreamEvent = parse_input(&event)?;
        let context: Value = parse_input(&context)?;
        let (stream_id, response_id, model, _) = stream_context(&context)?;
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
                    StreamState {
                        response_id: response_id.to_owned(),
                        model: model.to_owned(),
                        codec: AnthropicSseState::new(model, response_id),
                        thinking_blocks: BTreeMap::new(),
                    },
                );
            }
            let state = states.get_mut(stream_id).expect("state inserted above");
            let result = if state.response_id != response_id || state.model != model {
                Err(invalid(
                    "stream render context changed response_id or model mid-stream",
                ))
            } else {
                render_frames(state, &event)
            };
            if result.is_err()
                || matches!(event, StreamEvent::Done { .. })
                || state_ended(&states, stream_id)
            {
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

#[cfg(test)]
mod tests {
    use super::{AnthropicClient, Guest};
    use serde_json::{json, Value};
    use token_station_protocol::{ErrorCode, ErrorEnvelope, ReasoningEffort};

    #[test]
    fn structured_response_constraints_are_rejected_without_affecting_effort_or_tools() {
        let normalize = |body: Value| {
            AnthropicClient::normalize_inbound(
                json!({
                    "protocol":"anthropic-messages","principal":{"subject":"fixture"},
                    "headers":{"names":[],"values":{}},"body":body
                })
                .to_string(),
            )
        };
        let base = json!({"model":"fixture","max_tokens":100,"messages":[{"role":"user","content":"test"}],"tools":[{"name":"read","input_schema":{"type":"object"}}]});
        for legacy in [false, true] {
            let mut body = base.clone();
            let format = json!({"type":"json_schema","schema":{"type":"object"}});
            if legacy {
                body["output_format"] = format;
            } else {
                body["output_config"] = json!({"effort":"low","format":format});
            }
            let error: ErrorEnvelope = serde_json::from_str(&normalize(body).unwrap_err()).unwrap();
            assert_eq!(error.code, ErrorCode::Capability);
            assert_eq!(error.http_status, 400);
        }
        let mut body = base.clone();
        body["messages"].as_array_mut().unwrap().push(json!({"role":"system","content":[],"output_config":{"format":{"type":"json_schema","schema":{"type":"object"}}}}));
        assert!(normalize(body).is_err());
        let mut body = base;
        body["output_config"] = json!({"effort":"low"});
        let request: super::ChatRequest = serde_json::from_str(&normalize(body).unwrap()).unwrap();
        assert_eq!(request.reasoning.effort, Some(ReasoningEffort::Low));
        assert!(!request.extensions.contains_key("reasoning_effort"));
        assert_eq!(request.tools[0].parameters, json!({"type":"object"}));
    }

    #[test]
    fn empty_system_configuration_entries_preserve_effort_without_empty_prompt_text() {
        let request = normalized(json!({
            "model": "fixture",
            "max_tokens": 100,
            "system": [{"type":"text","text":"Keep these instructions."}],
            "messages": [
                {"role":"system","content":[{"type":"text","text":""}]},
                {"role":"user","content":"Read the file."},
                {"role":"system","content":[],"output_config":{"effort":"low"}},
                {"role":"system","content":[],"output_config":{"effort":"medium"}}
            ]
        }))
        .expect("empty configuration entries are valid");
        let messages = request["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].to_string().contains("Keep these instructions."));
        assert_eq!(request["reasoning_effort"], "medium");
    }

    #[test]
    fn top_level_effort_precedes_system_configuration_entries() {
        let request = normalized(json!({"model": "fixture", "max_tokens": 100,
            "output_config":{"effort":"high"},"messages":[
            {"role":"system","content":"A later instruction.","output_config":{"effort":"low"}},
            {"role":"user","content":"Read the file."}
        ]}))
        .unwrap();
        assert_eq!(request["reasoning_effort"], "high");
        assert_eq!(request["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn effort_max_is_clamped_and_unknown_levels_are_dropped() {
        for (effort, expected) in [("max", Some("high")), ("xhigh", None), ("low", Some("low"))] {
            let request = normalized(json!({"model": "fixture", "max_tokens": 100,
                "output_config": {"effort": effort},
                "messages": [{"role":"user","content":"q"}]}))
            .unwrap();
            assert_eq!(
                request.get("reasoning_effort").and_then(Value::as_str),
                expected,
                "{effort}"
            );
        }
    }

    #[test]
    fn rendered_response_exposes_fresh_input_and_separate_cache_buckets() {
        let response = json!({
            "id": "msg_test",
            "model": "claude-test",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {
                "input_tokens": 150,
                "output_tokens": 12,
                "cache_read_tokens": 100,
                "cache_write_tokens": 20
            }
        });

        let rendered =
            <AnthropicClient as Guest>::render_response(response.to_string(), "{}".to_owned())
                .expect("rendered response");
        let rendered: Value = serde_json::from_str(&rendered).expect("JSON response");

        assert_eq!(rendered["usage"]["input_tokens"], json!(30));
        assert_eq!(rendered["usage"]["cache_read_input_tokens"], json!(100));
        assert_eq!(rendered["usage"]["cache_creation_input_tokens"], json!(20));
    }

    #[test]
    fn buffered_stop_sequence_preserves_the_matched_value() {
        for sequence in [Some("_STOP_"), None] {
            let response = json!({"id":"fixture","model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"prefix"},"finish_reason":if sequence.is_some(){"stop_sequence"}else{"stop"},"stop_sequence":sequence}],"usage":{"input_tokens":1,"output_tokens":1}});
            let rendered =
                AnthropicClient::render_response(response.to_string(), "{}".into()).unwrap();
            let rendered: Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(rendered["stop_sequence"], json!(sequence));
            assert_eq!(
                rendered["stop_reason"],
                if sequence.is_some() {
                    "stop_sequence"
                } else {
                    "end_turn"
                }
            );
        }
    }

    #[test]
    fn redacted_thinking_renders_no_block_and_the_stream_continues() {
        let context = json!({
            "stream_id": "stream-redacted",
            "response_id": "msg-redacted",
            "model": "claude-test"
        });
        let render = |event: Value| {
            let rendered = <AnthropicClient as Guest>::render_stream_event(
                event.to_string(),
                context.to_string(),
            )
            .expect("event renders");
            serde_json::from_str::<Value>(&rendered).unwrap()["data"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let redacted = render(
            json!({"type": "redacted_thinking", "index": 0, "block_index": 2,
            "data": "供应商原值+/=opaque"}),
        );
        assert!(!redacted.contains("opaque"));
        let text = render(json!({"type": "delta", "index": 0, "content": "answer"}));
        assert!(text.contains("\"text\":\"answer\""), "{text}");
        render(json!({"type": "done", "finish_reason": "stop"}));
    }

    #[test]
    fn reasoning_source_blocks_remain_separate_within_one_choice() {
        let context = json!({
            "stream_id": "stream-multiple-thinking-blocks",
            "response_id": "msg-multiple-thinking-blocks",
            "model": "claude-test",
            "input_tokens": 1
        });
        let render = |event: Value| {
            let rendered = <AnthropicClient as Guest>::render_stream_event(
                event.to_string(),
                context.to_string(),
            )
            .expect("reasoning event renders");
            serde_json::from_str::<Value>(&rendered).expect("render result is JSON")["data"]
                .as_str()
                .expect("render result contains SSE data")
                .to_owned()
        };

        let first_thinking = render(json!({
            "type": "thinking_delta",
            "index": 0,
            "block_index": 2,
            "thinking_delta": "first thought"
        }));
        let first_signature = render(json!({
            "type": "thinking_signature_delta",
            "index": 0,
            "block_index": 2,
            "signature_delta": "first signature"
        }));
        let second_thinking = render(json!({
            "type": "thinking_delta",
            "index": 0,
            "block_index": 5,
            "thinking_delta": "second thought"
        }));
        let second_signature = render(json!({
            "type": "thinking_signature_delta",
            "index": 0,
            "block_index": 5,
            "signature_delta": "second signature"
        }));

        assert!(first_thinking.contains("event: content_block_start"));
        assert!(first_thinking.contains(r#""index":0"#));
        assert!(first_thinking.contains(r#""thinking":"first thought""#));
        assert!(first_signature.contains(r#""index":0"#));
        assert!(first_signature.contains(r#""signature":"first signature""#));
        assert!(second_thinking.contains("event: content_block_start"));
        assert!(second_thinking.contains(r#""index":1"#));
        assert!(second_thinking.contains(r#""thinking":"second thought""#));
        assert!(second_signature.contains(r#""index":1"#));
        assert!(second_signature.contains(r#""signature":"second signature""#));
    }

    fn normalized(body: Value) -> Result<Value, String> {
        AnthropicClient::normalize_inbound(
            json!({
                "protocol":"anthropic-messages","principal":{"subject":"fixture"},
                "headers":{"names":[],"values":{}},"body":body
            })
            .to_string(),
        )
        .map(|output| serde_json::from_str(&output).unwrap())
    }

    fn with_assistant(content: Value) -> Value {
        json!({"model":"fixture","max_tokens":100,"messages":[
            {"role":"user","content":"question"},
            {"role":"assistant","content":content},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"found"}]}
        ]})
    }

    #[test]
    fn thinking_configuration_is_typed_and_kept_for_plan_components() {
        let mut body = with_assistant(json!("answer"));
        body["thinking"] = json!({"type":"enabled","budget_tokens":2048});
        let request = normalized(body).unwrap();
        assert_eq!(request["reasoning_mode"], "enabled");
        assert_eq!(request["reasoning_budget_tokens"], 2048);
        assert_eq!(request["anthropic_thinking"]["budget_tokens"], 2048);

        let mut body = with_assistant(json!("answer"));
        body["thinking"] = json!({"type":"enabled","budget_tokens":"lots"});
        let error: ErrorEnvelope = serde_json::from_str(&normalized(body).unwrap_err()).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn a_signed_turn_carries_its_replay_layout_in_wire_order() {
        let request = normalized(with_assistant(json!([
            {"type":"thinking","thinking":"plan","signature":"sig-1"},
            {"type":"redacted_thinking","data":"opaque"},
            {"type":"text","text":"Looking it up."},
            {"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}
        ])))
        .unwrap();
        let assistant = &request["messages"][1];
        assert_eq!(
            assistant["reasoning_replay_protocol_family"],
            "claude-signed-thinking"
        );
        assert_eq!(
            assistant["reasoning_replay_block_layout"],
            json!([
                {"kind":"content","ordinal":0},
                {"kind":"content","ordinal":1},
                {"kind":"content","ordinal":2},
                {"kind":"tool_call","call_id":"toolu_1"}
            ])
        );
        assert_eq!(assistant["content"][0]["signature"], "sig-1");
    }

    #[test]
    fn turns_that_cannot_be_described_carry_no_replay_markers() {
        for content in [
            json!("plain answer"),
            json!([{"type":"text","text":"no reasoning"}]),
            json!([{"type":"thinking","thinking":"unsigned"},{"type":"text","text":"a"}]),
            json!([{"type":"thinking","thinking":"empty","signature":""}]),
            json!([
                {"type":"thinking","thinking":"plan","signature":"sig-1"},
                {"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}
            ]),
        ] {
            let request = normalized(with_assistant(content.clone())).unwrap();
            let assistant = &request["messages"][1];
            assert!(
                assistant.get("reasoning_replay_protocol_family").is_none()
                    && assistant.get("reasoning_replay_block_layout").is_none(),
                "{content}"
            );
        }
    }

    #[test]
    fn client_keys_named_like_typed_ir_fields_stay_out_of_extensions() {
        let mut body = with_assistant(json!("answer"));
        body["host_values"] = json!({"end_user_hash":"client-chosen"});
        body["reasoning_mode"] = json!("enabled");
        body["sampling"] = json!({"temperature": 2});
        body["metadata"] = json!({"user_id":"kept-as-data"});
        let request = normalized(body).unwrap();
        assert!(request.get("host_values").is_none());
        assert!(request.get("reasoning_mode").is_none());
        assert!(request["sampling"].get("temperature").is_none());
        // The end-user value is the host's to derive, so the client's
        // `metadata` is not carried.
        assert!(request.get("metadata").is_none());
    }

    #[test]
    fn a_tool_only_assistant_turn_has_no_content() {
        let request = normalized(with_assistant(json!([
            {"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}
        ])))
        .unwrap();
        let assistant = &request["messages"][1];
        assert!(assistant.get("content").is_none(), "{assistant}");
        assert_eq!(assistant["tool_calls"][0]["id"], "toolu_1");
    }

    #[test]
    fn tool_results_keep_their_error_flag_and_images() {
        let request = normalized(json!({"model":"fixture","max_tokens":100,"messages":[
            {"role":"user","content":"look"},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"Read","input":{}},
                {"type":"tool_use","id":"toolu_2","name":"Bash","input":{}}]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_1","content":[
                    {"type":"text","text":"image:"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]},
                {"type":"tool_result","tool_use_id":"toolu_2","is_error":true,"content":"denied"}]}
        ]}))
        .unwrap();
        let first = &request["messages"][2];
        assert_eq!(first["role"], "tool");
        assert_eq!(
            first["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
        assert!(first.get("anthropic_tool_result_is_error").is_none());
        let second = &request["messages"][3];
        assert_eq!(second["anthropic_tool_result_is_error"], true);
        assert_eq!(second["content"], "denied");
    }

    #[test]
    fn tool_results_precede_text_in_the_same_user_turn() {
        let request = normalized(json!({"model":"fixture","max_tokens":100,"messages":[
            {"role":"user","content":"read"},
            {"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{}}]},
            {"role":"user","content":[{"type":"text","text":"note"},
                {"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}
        ]}))
        .unwrap();
        assert_eq!(request["messages"][2]["role"], "tool");
        assert_eq!(request["messages"][3]["role"], "user");
    }

    #[test]
    fn the_checks_this_adapter_always_made_still_refuse() {
        for (body, message) in [
            (json!({"model":"fixture","messages":[{"role":"user","content":"q"}]}), "request declares no max_tokens"),
            (json!({"max_tokens":1,"messages":[{"role":"user","content":"q"}]}), "request declares no model"),
            (json!({"model":"fixture","max_tokens":1,"messages":[{"role":"user","content":"q"}],
                "stop_sequences":"END"}), "stop_sequences must be an array"),
            (json!({"model":"fixture","max_tokens":1,"messages":[{"role":"user","content":"q"}],
                "tools":[{"name":"t"}]}), "tool declares no input_schema"),
            (json!({"model":"fixture","max_tokens":1,"messages":[{"role":"user","content":"q"}],
                "tools":[{"type":"web_search_20250305","name":"web_search"}]}),
                "Anthropic server tool `web_search_20250305` requires an anthropic-native provider; the translated route cannot execute it"),
            (json!({"model":"fixture","max_tokens":1,"messages":[{"role":"user","content":"q"},
                {"role":"assistant","content":[{"type":"tool_result","tool_use_id":"x","content":"y"}]}]}),
                "assistant messages cannot contain tool_result blocks"),
        ] {
            let error: ErrorEnvelope = serde_json::from_str(&normalized(body).unwrap_err()).unwrap();
            assert_eq!(error.message, message);
        }
    }

    fn stream(stream_id: &str, events: &[Value]) -> String {
        let context =
            json!({"stream_id": stream_id, "response_id": "msg_ts", "model": "claude-test"});
        events
            .iter()
            .map(|event| {
                let rendered = <AnthropicClient as Guest>::render_stream_event(
                    event.to_string(),
                    context.to_string(),
                )
                .expect("event renders");
                serde_json::from_str::<Value>(&rendered).unwrap()["data"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn stream_usage_rides_the_one_terminal_message_delta() {
        let data = stream(
            "stream-usage",
            &[
                json!({"type":"usage","usage":{"input_tokens":100,"output_tokens":1,
                    "cache_read_tokens":60,"cache_write_tokens":30}}),
                json!({"type":"delta","index":0,"content":"x"}),
                json!({"type":"usage","usage":{"output_tokens":20}}),
                json!({"type":"finish","finish_reason":"stop"}),
                json!({"type":"done","finish_reason":"stop"}),
            ],
        );
        assert_eq!(data.matches("event: message_delta").count(), 1, "{data}");
        assert_eq!(data.matches("event: message_stop").count(), 1, "{data}");
        assert!(data.contains("\"input_tokens\":10"), "{data}");
        assert!(data.contains("\"cache_read_input_tokens\":60"), "{data}");
        assert!(data.contains("\"output_tokens\":20"), "{data}");
        super::STREAM_STATES.with(|states| assert!(!states.borrow().contains_key("stream-usage")));
    }

    #[test]
    fn stream_state_is_per_stream_and_a_changed_context_is_refused() {
        let first = stream(
            "stream-a",
            &[json!({"type":"delta","index":0,"content":"x"})],
        );
        let second = stream(
            "stream-b",
            &[json!({"type":"delta","index":0,"content":"y"})],
        );
        assert!(first.contains("event: message_start") && second.contains("event: message_start"));
        let changed =
            json!({"stream_id":"stream-a","response_id":"msg_other","model":"claude-test"});
        let error = <AnthropicClient as Guest>::render_stream_event(
            json!({"type":"delta","index":0,"content":"z"}).to_string(),
            changed.to_string(),
        )
        .unwrap_err();
        assert!(error.contains("changed response_id or model"), "{error}");
        super::STREAM_STATES.with(|states| {
            assert!(!states.borrow().contains_key("stream-a"));
            assert!(states.borrow().contains_key("stream-b"));
        });
    }
}

export!(AnthropicClient);
