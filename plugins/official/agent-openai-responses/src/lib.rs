//! OpenAI Responses northbound adapter, scoped to the Codex M4 path.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, FinishReason, HintKind, ImageUrl, Message, ResponseFormat, Role,
    Sampling, StreamEvent, ToolCall, ToolChoice, ToolDef, Usage,
};

struct ResponsesClient;

const LOCAL_SHELL_TOOL_NAME: &str = "__token_station_responses_local_shell";
const CONTINUATION_KEY_EXTENSION: &str = "token_station_private_continuation_key";
const CONTINUATION_SCOPE_EXTENSION: &str = "token_station_continuation_scope";
const TRANSIENT_INSTRUCTIONS_EXTENSION: &str = "responses_transient_instructions";
const TOOL_NAMESPACES_EXTENSION: &str = "responses_tool_namespaces";
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_NAMESPACE_NAME_BYTES: usize = 64;
const CONTINUATION_TTL_MS: u64 = 30 * 60 * 1_000;
const PENDING_CONTINUATION_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_CONTINUATION_ENTRIES: usize = 64;
const MAX_PENDING_CONTINUATIONS: usize = 64;
const MAX_CONTINUATION_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTINUATION_TOTAL_BYTES: usize = 16 * 1024 * 1024;

const DIRECT_REQUEST_FIELDS: &[&str] = &[
    "model",
    "instructions",
    "input",
    "tools",
    "max_output_tokens",
    "temperature",
    "top_p",
    "stream",
    "previous_response_id",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning",
    "text",
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

fn response_format_of(body: &Value) -> Result<Option<ResponseFormat>, String> {
    let Some(text) = body.get("text").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let text = text
        .as_object()
        .ok_or_else(|| invalid("text must be an object"))?;
    let Some(format) = text.get("format").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let format = format
        .as_object()
        .ok_or_else(|| invalid("text.format must be an object"))?;
    let kind = format
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("text.format declares no string type"))?;

    match kind {
        "text" => Ok(Some(ResponseFormat::Text)),
        "json_object" => Ok(Some(ResponseFormat::JsonObject)),
        "json_schema" => {
            let schema = format
                .get("schema")
                .filter(|value| value.is_object())
                .ok_or_else(|| invalid("text.format json_schema declares no schema object"))?;
            let mut normalized = serde_json::Map::new();
            for key in ["name", "description", "strict"] {
                if let Some(value) = format.get(key) {
                    normalized.insert(key.to_owned(), value.clone());
                }
            }
            normalized.insert("schema".to_owned(), schema.clone());
            Ok(Some(ResponseFormat::JsonSchema {
                json_schema: Value::Object(normalized),
            }))
        }
        kind => Err(capability(format!(
            "unsupported Responses text.format type {kind}"
        ))),
    }
}

fn validate_semantic_options(body: &Value) -> Result<(), String> {
    if let Some(previous) = body
        .get("previous_response_id")
        .filter(|value| !value.is_null())
    {
        if !previous
            .as_str()
            .is_some_and(|value| !value.is_empty() && value.len() <= 256)
        {
            return Err(invalid(
                "previous_response_id must be a non-empty string of at most 256 bytes",
            ));
        }
    }

    match body.get("tool_choice") {
        None | Some(Value::Null) => {}
        Some(Value::String(choice)) if matches!(choice.as_str(), "auto" | "none" | "required") => {}
        Some(Value::String(choice)) => {
            return Err(capability(format!(
                "Responses tool_choice {choice} cannot be preserved by Canonical IR"
            )));
        }
        Some(Value::Object(choice))
            if choice.get("type").and_then(Value::as_str) == Some("function")
                && choice.get("name").is_some_and(Value::is_string) => {}
        Some(Value::Object(_)) => return Err(invalid("forced tool_choice is malformed")),
        Some(_) => return Err(invalid("tool_choice must be a string or object")),
    }

    // Both settings are preserved: the boolean rides through to the provider
    // request verbatim (see `normalize_inbound` → Canonical IR extensions), so
    // `parallel_tool_calls=false` is honored rather than refused. Only a
    // non-boolean is a malformed request.
    match body.get("parallel_tool_calls") {
        None | Some(Value::Null | Value::Bool(_)) => {}
        Some(_) => return Err(invalid("parallel_tool_calls must be a boolean")),
    }

    // `reasoning.effort` maps onto the OpenAI-compatible `reasoning_effort`
    // request parameter (see `normalize_inbound` → Canonical IR extensions →
    // provider render). It rides through as a string; the provider validates
    // and, per its own docs, remaps unsupported levels. Other reasoning keys
    // (e.g. `summary`) have no chat-completions equivalent and are dropped.
    match body.get("reasoning") {
        None | Some(Value::Null) => {}
        Some(Value::Object(reasoning)) => {
            if let Some(effort) = reasoning.get("effort").filter(|value| !value.is_null()) {
                if !effort.is_string() {
                    return Err(invalid("reasoning.effort must be a string"));
                }
            }
        }
        Some(_) => return Err(invalid("reasoning must be an object")),
    }

    Ok(())
}

fn tool_choice_of(value: Option<&Value>) -> Result<Option<ToolChoice>, String> {
    match value.filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(Value::String(choice)) if choice == "auto" => Ok(Some(ToolChoice::Auto)),
        Some(Value::String(choice)) if choice == "none" => Ok(Some(ToolChoice::None)),
        Some(Value::String(choice)) if choice == "required" => Ok(Some(ToolChoice::Required)),
        Some(Value::Object(choice))
            if choice.get("type").and_then(Value::as_str) == Some("function") =>
        {
            let name = choice
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("forced function tool_choice declares no name"))?;
            let name = match choice.get("namespace").filter(|value| !value.is_null()) {
                Some(namespace) => flattened_namespace_tool_name(
                    namespace
                        .as_str()
                        .ok_or_else(|| invalid("forced tool_choice namespace must be a string"))?,
                    name,
                )?,
                None => name.to_owned(),
            };
            Ok(Some(ToolChoice::Other(json!({
                "type": "function",
                "function": {"name": name}
            }))))
        }
        Some(Value::Object(_)) => Err(invalid("forced tool_choice is malformed")),
        Some(_) => Err(invalid("tool_choice must be a string or object")),
    }
}

fn valid_tool_component(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn flattened_namespace_tool_name(namespace: &str, name: &str) -> Result<String, String> {
    if !valid_tool_component(namespace, MAX_NAMESPACE_NAME_BYTES) {
        return Err(invalid(
            "Responses namespace tool declares an invalid namespace",
        ));
    }
    if !valid_tool_component(name, MAX_TOOL_NAME_BYTES) {
        return Err(invalid(
            "Responses namespace tool declares an invalid function name",
        ));
    }
    let flattened = if namespace.ends_with("__") {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}__{name}")
    };
    if flattened.len() > MAX_TOOL_NAME_BYTES {
        return Err(capability(
            "Responses namespace and function name exceed the provider tool-name limit",
        ));
    }
    Ok(flattened)
}

#[derive(Clone)]
struct FlattenedNamespaceTool {
    flattened_name: String,
    namespace: String,
    name: String,
    description: Option<String>,
    parameters: Value,
    strict: Option<bool>,
}

fn flattened_namespace_tools(tool: &Value) -> Result<Vec<FlattenedNamespaceTool>, String> {
    let namespace = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("namespace tool declares no name"))?;
    let namespace_description = match tool.get("description").filter(|value| !value.is_null()) {
        Some(description) => Some(
            description
                .as_str()
                .ok_or_else(|| invalid("namespace tool description must be a string"))?,
        ),
        None => None,
    };
    let tools = tool
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("namespace tool declares no tools array"))?;
    tools
        .iter()
        .map(|nested| {
            if nested.get("type").and_then(Value::as_str) != Some("function") {
                return Err(capability(
                    "Responses namespace contains a non-function client tool",
                ));
            }
            let name = nested
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("namespace function declares no name"))?;
            let flattened_name = flattened_namespace_tool_name(namespace, name)?;
            let nested_description = match nested
                .get("description")
                .filter(|value| !value.is_null())
            {
                Some(description) => Some(
                    description
                        .as_str()
                        .ok_or_else(|| invalid("namespace function description must be a string"))?
                        .to_owned(),
                ),
                None => None,
            };
            let description = match (namespace_description, nested_description) {
                (Some(namespace), Some(function)) => Some(format!("{namespace}\n\n{function}")),
                (Some(namespace), None) => Some(namespace.to_owned()),
                (None, description) => description,
            };
            let strict = match nested.get("strict").filter(|value| !value.is_null()) {
                Some(strict) => Some(
                    strict
                        .as_bool()
                        .ok_or_else(|| invalid("namespace function strict must be a boolean"))?,
                ),
                None => None,
            };
            Ok(FlattenedNamespaceTool {
                flattened_name,
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                description,
                parameters: nested
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                strict,
            })
        })
        .collect()
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
    let name = match item.get("namespace").filter(|value| !value.is_null()) {
        Some(namespace) => flattened_namespace_tool_name(
            namespace
                .as_str()
                .ok_or_else(|| invalid("function_call namespace must be a string"))?,
            name,
        )?,
        None => name.to_owned(),
    };
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("function_call declares no arguments"))?;
    Ok(Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
            id: call_id.to_owned(),
            name,
            arguments: arguments.to_owned(),
        }],
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    })
}

/// A replayed `custom_tool_call` from a prior turn. Codex sends the custom
/// tool's raw string in `input`; wrap it back into the `{ input: <string> }`
/// arguments the flattened function tool was declared with, so the assistant
/// turn is byte-consistent with how the model would have produced it.
fn custom_tool_call_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("custom_tool_call declares no call_id"))?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("custom_tool_call declares no name"))?;
    let input = item.get("input").cloned().unwrap_or_else(|| json!(""));
    let arguments = json!({ CUSTOM_TOOL_INPUT_FIELD: input }).to_string();
    Ok(Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
            id: call_id.to_owned(),
            name: name.to_owned(),
            arguments,
        }],
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    })
}

/// A replayed `custom_tool_call_output` or `tool_search_output` from a prior
/// turn: the tool's result, keyed by `call_id`, becomes a Canonical tool
/// message exactly like a `function_call_output`.
fn tool_result_item(item: &Value, kind: &str) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{kind} declares no call_id")))?;
    let content = content_value(
        item.get("output").unwrap_or(&Value::Null),
        "tool result output",
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

/// A replayed `tool_search_call` from a prior turn. Its arguments object is
/// serialized back into the flattened `tool_search` function's arguments string.
fn tool_search_call_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("tool_search_call declares no call_id"))?;
    let arguments = match item.get("arguments") {
        Some(value) => serde_json::to_string(value).map_err(internal)?,
        None => "{}".to_owned(),
    };
    Ok(Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
            id: call_id.to_owned(),
            name: TOOL_SEARCH_PROXY_NAME.to_owned(),
            arguments,
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

fn local_shell_call_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("local_shell_call declares no call_id"))?;
    let action = item
        .get("action")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("local_shell_call declares no action object"))?;
    Ok(Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
            id: call_id.to_owned(),
            name: LOCAL_SHELL_TOOL_NAME.to_owned(),
            arguments: json!({"action": action}).to_string(),
        }],
        tool_call_id: None,
        name: None,
        extensions: Extensions::new(),
    })
}

fn local_shell_output_item(item: &Value) -> Result<Message, String> {
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("local_shell_call_output declares no id"))?;
    let output = item
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("local_shell_call_output declares no string output"))?;
    Ok(Message {
        role: Role::Tool,
        content: Some(Content::Text(output.to_owned())),
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id.to_owned()),
        name: Some(LOCAL_SHELL_TOOL_NAME.to_owned()),
        extensions: Extensions::new(),
    })
}

fn reasoning_item(item: &Value) -> Result<Message, String> {
    let mut parts = Vec::new();
    if let Some(summary) = item.get("summary").filter(|value| !value.is_null()) {
        let summary = summary
            .as_array()
            .ok_or_else(|| invalid("reasoning summary must be an array"))?;
        for part in summary {
            match part.get("type").and_then(Value::as_str) {
                Some("summary_text") => parts.push(ContentPart::Thinking {
                    thinking: part
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("reasoning summary_text declares no text"))?
                        .to_owned(),
                    signature: None,
                }),
                Some(kind) => {
                    return Err(capability(format!(
                        "unsupported Responses reasoning summary item {kind}"
                    )));
                }
                None => return Err(invalid("reasoning summary item declares no type")),
            }
        }
    }
    let mut extensions = Extensions::new();
    if let Some(id) = item.get("id").filter(|value| !value.is_null()) {
        let id = id
            .as_str()
            .ok_or_else(|| invalid("reasoning id must be a string"))?;
        extensions.insert("responses_reasoning_id".to_owned(), json!(id));
    }
    if let Some(encrypted) = item
        .get("encrypted_content")
        .filter(|value| !value.is_null())
    {
        let encrypted = encrypted
            .as_str()
            .ok_or_else(|| invalid("reasoning encrypted_content must be a string"))?;
        extensions.insert(
            "responses_reasoning_encrypted_content".to_owned(),
            json!(encrypted),
        );
    }
    Ok(Message {
        role: Role::Assistant,
        content: (!parts.is_empty()).then_some(Content::Parts(parts)),
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        extensions,
    })
}

fn merge_content(target: &mut Option<Content>, incoming: Option<Content>) {
    let Some(incoming) = incoming else {
        return;
    };
    let Some(existing) = target.take() else {
        *target = Some(incoming);
        return;
    };

    *target = Some(match (existing, incoming) {
        (Content::Text(mut left), Content::Text(right)) => {
            if !left.is_empty() && !right.is_empty() {
                left.push('\n');
            }
            left.push_str(&right);
            Content::Text(left)
        }
        (Content::Parts(mut left), Content::Parts(right)) => {
            left.extend(right);
            Content::Parts(left)
        }
        (Content::Text(left), Content::Parts(mut right)) => {
            right.insert(0, ContentPart::Text { text: left });
            Content::Parts(right)
        }
        (Content::Parts(mut left), Content::Text(right)) => {
            left.push(ContentPart::Text { text: right });
            Content::Parts(left)
        }
    });
}

fn coalesce_assistant_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut coalesced: Vec<Message> = Vec::with_capacity(messages.len());
    for mut message in messages {
        if message.role == Role::Assistant
            && coalesced
                .last()
                .is_some_and(|previous| previous.role == Role::Assistant)
        {
            let previous = coalesced
                .last_mut()
                .expect("the preceding assistant message was just checked");
            merge_content(&mut previous.content, message.content.take());
            previous.tool_calls.append(&mut message.tool_calls);
            previous.extensions.append(&mut message.extensions);
        } else {
            coalesced.push(message);
        }
    }
    coalesced
}

fn input_messages(input: &Value) -> Result<Vec<Message>, String> {
    match input {
        Value::String(text) => Ok(vec![Message::text(Role::User, text)]),
        Value::Array(items) => {
            let messages = items
                .iter()
                .map(|item| match item.get("type").and_then(Value::as_str) {
                    Some("message") | None if item.get("role").is_some() => message_item(item),
                    Some("function_call") => function_call_item(item),
                    Some("function_call_output") => function_output_item(item),
                    Some("custom_tool_call") => custom_tool_call_item(item),
                    Some("custom_tool_call_output") => {
                        tool_result_item(item, "custom_tool_call_output")
                    }
                    Some("tool_search_call") => tool_search_call_item(item),
                    Some("tool_search_output") => tool_result_item(item, "tool_search_output"),
                    Some("local_shell_call") => local_shell_call_item(item),
                    Some("local_shell_call_output") => local_shell_output_item(item),
                    Some("reasoning") => reasoning_item(item),
                    Some(kind) => Err(capability(format!(
                        "unsupported Responses input item {kind}"
                    ))),
                    None => Err(invalid("input item declares no type")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(coalesce_assistant_messages(messages))
        }
        _ => Err(invalid("input must be a string or an array of input items")),
    }
}

/// Joins a namespace and a child tool into a flat, deterministic function name.
/// Kept in sync with CC Switch's `<namespace>__<child>` scheme so the intent is
/// legible; response-side restoration (flat name → `{name, namespace}`) is a
/// host/context follow-up (see the design doc), so today the round trip arrives
/// flat.
const NAMESPACE_SEPARATOR: &str = "__";

/// Custom (freeform-grammar) Codex tools carry a raw string `input`, not a JSON
/// schema. To route them through the Canonical function-tool path we wrap that
/// string in a single required `input` property — kept in sync with CC Switch's
/// `CUSTOM_TOOL_INPUT_FIELD` — and unwrap it again when restoring the call so
/// the round trip is exact.
const CUSTOM_TOOL_INPUT_FIELD: &str = "input";
const CUSTOM_TOOL_INPUT_DESCRIPTION: &str = "Raw string input for the original custom tool. Preserve formatting exactly and follow the original tool definition embedded in the description.";
const CUSTOM_TOOL_PRESERVED_METADATA_HEADING: &str = "Original tool definition:";

/// The fixed proxy name Codex's `tool_search` built-in is translated to (kept in
/// sync with CC Switch's `TOOL_SEARCH_PROXY_NAME`), so the response side can
/// recognize and restore it to a `tool_search_call`.
const TOOL_SEARCH_PROXY_NAME: &str = "tool_search";

/// The original Codex tool kind behind a flattened Canonical function name.
/// Derived at render time from the request's own tool declarations (threaded
/// into the render context as `inbound_tools`), so the response side can restore
/// the item type Codex expects. Mirrors CC Switch's `CodexToolContext`, which
/// likewise rebuilds the map from the request rather than threading state.
enum RestoredTool {
    Custom,
    /// The `tool_search` built-in, proxied as a `tool_search` function and
    /// restored to a `tool_search_call` (client-executed).
    ToolSearch,
    /// A `namespace` child, flattened on the way in to `<namespace>__<child>`.
    /// Restored to a `function_call` bearing the bare child `name` plus a
    /// `namespace` field, which is how Codex matches it against its own
    /// namespaced registry. (`local_shell` is deliberately absent: like CC
    /// Switch, an unrepresentable built-in degrades to a plain function rather
    /// than an invented `local_shell_call` restoration.)
    Namespace {
        namespace: String,
        child: String,
    },
}

/// The `query`/`limit` schema Codex's `tool_search` built-in is translated into,
/// mirroring CC Switch's `add_tool_search_tool`.
fn tool_search_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for tools or connectors to load."
            },
            "limit": {
                "type": "integer",
                "description": "Maximum number of tool groups to return."
            }
        },
        "required": ["query"]
    })
}

/// Parse tool-call `arguments` into the object a `tool_search_call` carries: the
/// parsed object, `{}` when empty, or `{ "query": <raw> }` when unparseable.
/// Mirrors CC Switch `parse_tool_arguments_object`.
fn parse_tool_arguments_object(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "query": arguments }))
}

/// The flat, deterministic function name a `namespace` child is lifted to.
/// Shared by `tools_of` (request), [`restore_map`] (response) and
/// [`function_call_item`] (replayed history) so all three derive the exact same
/// name — the consistency CC Switch gets from deriving both directions from the
/// same request tools.
fn flatten_namespace_name(namespace: &str, child: &str) -> String {
    format!("{namespace}{NAMESPACE_SEPARATOR}{child}")
}

/// The wrapped `{ "input": "<string>" }` parameter schema every custom tool is
/// translated into. Fixed shape so the model always emits an `input` string we
/// can unwrap on the way back.
fn custom_tool_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            CUSTOM_TOOL_INPUT_FIELD: {
                "type": "string",
                "description": CUSTOM_TOOL_INPUT_DESCRIPTION
            }
        },
        "required": [CUSTOM_TOOL_INPUT_FIELD]
    })
}

/// A custom tool has no schema slot for its grammar/`format`, so its whole
/// original definition is embedded in the flattened function's description —
/// the model follows it to produce the raw `input`. Mirrors CC Switch's
/// `responses_custom_tool_description`.
fn custom_tool_description(tool: &Value) -> String {
    format!(
        "{CUSTOM_TOOL_PRESERVED_METADATA_HEADING}\n```json\n{}\n```",
        serde_json::to_string(tool).unwrap_or_default()
    )
}

/// The flattened function name for a custom tool: its declared `name`, or the
/// bare `custom` type when it declares none. Kept identical between `tools_of`
/// (request) and [`restore_map`] (response) so the two stay consistent.
fn custom_tool_name(tool: &Value) -> String {
    tool.get("name")
        .and_then(Value::as_str)
        .map_or_else(|| "custom".to_owned(), str::to_owned)
}

/// Build the flat-name → original-kind restore map from the request's tool
/// declarations. Only kinds that need response-side restoration are recorded;
/// everything else round-trips as a plain `function_call` (map miss).
fn restore_map(inbound_tools: &Value) -> BTreeMap<String, RestoredTool> {
    let mut map = BTreeMap::new();
    for tool in inbound_tools.as_array().into_iter().flatten() {
        match tool.get("type").and_then(Value::as_str) {
            Some("custom") => {
                map.insert(custom_tool_name(tool), RestoredTool::Custom);
            }
            Some("tool_search") => {
                map.insert(TOOL_SEARCH_PROXY_NAME.to_owned(), RestoredTool::ToolSearch);
            }
            // Mirror `tools_of`'s namespace flattening exactly: every named
            // child is lifted, so every flat name maps back to its child.
            Some("namespace") => {
                let Some(namespace) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                for child in tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(child) = child.get("name").and_then(Value::as_str) {
                        map.insert(
                            flatten_namespace_name(namespace, child),
                            RestoredTool::Namespace {
                                namespace: namespace.to_owned(),
                                child: child.to_owned(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    map
}

/// Unwrap the raw custom-tool `input` string from the `{ "input": ... }` chat
/// arguments the model produced. Mirrors CC Switch
/// `custom_tool_input_from_chat_arguments`.
fn custom_tool_input_from_arguments(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!("");
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(mut obj)) => obj
            .remove(CUSTOM_TOOL_INPUT_FIELD)
            .unwrap_or_else(|| json!(arguments)),
        _ => json!(arguments),
    }
}

/// Render one Canonical tool call as the Responses output item Codex expects:
/// a `custom_tool_call` when the request declared it as a custom tool, else a
/// plain `function_call`.
/// The Responses output item for one tool call, restored to the shape Codex
/// expects. `status` and `arguments` are parameters so the streaming path can
/// reuse it for both the `in_progress` `output_item.added` (empty arguments)
/// and the `completed` `output_item.done`.
fn restored_tool_item(
    call_id: &str,
    name: &str,
    arguments: &str,
    status: &str,
    restore: &BTreeMap<String, RestoredTool>,
) -> Value {
    match restore.get(name) {
        Some(RestoredTool::Custom) => json!({
            "id": format!("ctc_{call_id}"),
            "type": "custom_tool_call",
            "status": status,
            "call_id": call_id,
            "name": name,
            "input": custom_tool_input_from_arguments(arguments)
        }),
        // `tool_search` is client-executed and carries its arguments as a parsed
        // object; it has neither an item id nor a name on the wire.
        Some(RestoredTool::ToolSearch) => json!({
            "type": "tool_search_call",
            "status": status,
            "call_id": call_id,
            "execution": "client",
            "arguments": parse_tool_arguments_object(arguments)
        }),
        // A namespace child comes back as a plain function_call bearing the bare
        // child name plus a `namespace` field, so Codex can match it against its
        // namespaced tool registry.
        Some(RestoredTool::Namespace { namespace, child }) => json!({
            "type": "function_call",
            "id": format!("fc_{call_id}"),
            "status": status,
            "call_id": call_id,
            "name": child,
            "namespace": namespace,
            "arguments": arguments
        }),
        None => json!({
            "type": "function_call",
            "id": format!("fc_{call_id}"),
            "status": status,
            "call_id": call_id,
            "name": name,
            "arguments": arguments
        }),
    }
}

fn restore_tool_call_item(call: &ToolCall, restore: &BTreeMap<String, RestoredTool>) -> Value {
    restored_tool_item(&call.id, &call.name, &call.arguments, "completed", restore)
}

/// Whether a tool name was declared as a custom tool — its streamed input rides
/// the `custom_tool_call_input` SSE family instead of `function_call_arguments`.
fn is_custom_restore(name: &str, restore: &BTreeMap<String, RestoredTool>) -> bool {
    matches!(restore.get(name), Some(RestoredTool::Custom))
}

/// The Responses output-item id for a tool call: `ctc_` for a restored custom
/// tool, `fc_` otherwise. Must match the `id` [`restored_tool_item`] emits.
fn tool_item_id(call_id: &str, name: &str, restore: &BTreeMap<String, RestoredTool>) -> String {
    if is_custom_restore(name, restore) {
        format!("ctc_{call_id}")
    } else {
        format!("fc_{call_id}")
    }
}

fn push_tool(
    tools: &mut Vec<ToolDef>,
    seen: &mut BTreeSet<String>,
    name: String,
    description: Option<String>,
    parameters: Value,
) -> Result<(), String> {
    // A collision after flattening would silently drop one tool. Fail loudly and
    // ask the caller to rename, exactly as CC Switch does — never overwrite.
    if !seen.insert(name.clone()) {
        return Err(invalid(format!(
            "tool name `{name}` collides after namespace flattening; rename one of the tools"
        )));
    }
    tools.push(ToolDef {
        name,
        description,
        parameters,
    });
    Ok(())
}

fn tools_of(value: &Value) -> Result<Vec<ToolDef>, String> {
    let mut definitions = Vec::new();
    let mut names = BTreeSet::new();
    for tool in value.as_array().into_iter().flatten() {
        let kind = tool
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("tool declares no type"))?;
        if kind == "local_shell" {
            let definition = ToolDef {
                name: LOCAL_SHELL_TOOL_NAME.to_owned(),
                description: Some(
                    "Execute one argv command in the Codex client's local shell.".to_owned(),
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "object",
                            "properties": {
                                "type": {"const": "exec"},
                                "command": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                    "minItems": 1
                                },
                                "env": {
                                    "type": "object",
                                    "additionalProperties": {"type": "string"}
                                },
                                "timeout_ms": {"type": "integer", "minimum": 1},
                                "user": {"type": "string"},
                                "working_directory": {"type": "string"}
                            },
                            "required": ["type", "command"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            };
            if !names.insert(definition.name.clone()) {
                return Err(invalid("Responses tools contain a duplicate provider name"));
            }
            definitions.push(definition);
            continue;
        }
        if kind == "namespace" {
            for flattened in flattened_namespace_tools(tool)? {
                if !names.insert(flattened.flattened_name.clone()) {
                    return Err(invalid(
                        "Responses namespace tools collide after provider flattening",
                    ));
                }
                definitions.push(ToolDef {
                    name: flattened.flattened_name,
                    description: flattened.description,
                    parameters: flattened.parameters,
                });
            }
            continue;
        }
        if kind == "custom" {
            push_tool(
                &mut definitions,
                &mut names,
                custom_tool_name(tool),
                Some(custom_tool_description(tool)),
                custom_tool_parameters(),
            )?;
            continue;
        }
        if kind == "tool_search" {
            push_tool(
                &mut definitions,
                &mut names,
                TOOL_SEARCH_PROXY_NAME.to_owned(),
                Some(
                    "Search and load Codex tools, plugins, connectors, and MCP namespaces for the current task."
                        .to_owned(),
                ),
                tool_search_parameters(),
            )?;
            continue;
        }
        if kind == "web_search"
            && tool.get("external_web_access").and_then(Value::as_bool) == Some(false)
        {
            continue;
        }
        if kind != "function" {
            return Err(capability(format!(
                "Responses provider-hosted tool `{kind}` requires a native Responses provider; the translated route cannot execute it"
            )));
        }
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("function tool declares no name"))?
            .to_owned();
        if !names.insert(name.clone()) {
            return Err(invalid("Responses tools contain a duplicate provider name"));
        }
        definitions.push(ToolDef {
            name,
            description: tool
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            parameters: tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
        });
    }
    Ok(definitions)
}

fn tool_extensions(value: &Value) -> Result<Extensions, String> {
    let mut strict = serde_json::Map::new();
    let mut namespaces = serde_json::Map::new();
    let mut disabled_provider_tools = Vec::new();
    for tool in value.as_array().into_iter().flatten() {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("function tool declares no name"))?;
                if let Some(value) = tool.get("strict").filter(|value| !value.is_null()) {
                    let value = value
                        .as_bool()
                        .ok_or_else(|| invalid("function tool strict must be a boolean"))?;
                    strict.insert(name.to_owned(), json!(value));
                }
            }
            Some("namespace") => {
                for flattened in flattened_namespace_tools(tool)? {
                    if namespaces
                        .insert(
                            flattened.flattened_name.clone(),
                            json!({
                                "namespace": flattened.namespace,
                                "name": flattened.name
                            }),
                        )
                        .is_some()
                    {
                        return Err(invalid(
                            "Responses namespace tools collide after provider flattening",
                        ));
                    }
                    if let Some(value) = flattened.strict {
                        strict.insert(flattened.flattened_name, json!(value));
                    }
                }
            }
            Some("web_search")
                if tool.get("external_web_access").and_then(Value::as_bool) == Some(false) =>
            {
                disabled_provider_tools.push(tool.clone());
            }
            _ => {}
        }
    }
    let mut extensions = Extensions::new();
    if !strict.is_empty() {
        extensions.insert("responses_tool_strict".to_owned(), Value::Object(strict));
    }
    if !namespaces.is_empty() {
        extensions.insert(
            TOOL_NAMESPACES_EXTENSION.to_owned(),
            Value::Object(namespaces),
        );
    }
    if !disabled_provider_tools.is_empty() {
        extensions.insert(
            "responses_disabled_provider_tools".to_owned(),
            Value::Array(disabled_provider_tools),
        );
    }
    Ok(extensions)
}

fn request_extensions(body: &Value) -> Extensions {
    body.as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !DIRECT_REQUEST_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[derive(Clone)]
struct ContinuationHistory {
    messages: Vec<Message>,
    created_at_ms: u64,
    sequence: u64,
    bytes: usize,
}

struct PendingContinuation {
    scope: String,
    messages: Vec<Message>,
    created_at_ms: u64,
    sequence: u64,
    bytes: usize,
}

#[derive(Default)]
struct ContinuationStore {
    history: BTreeMap<(String, String), ContinuationHistory>,
    pending: BTreeMap<String, PendingContinuation>,
    next_sequence: u64,
    total_bytes: usize,
}

impl ContinuationStore {
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn allocate_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        self.next_sequence
    }

    fn prune(&mut self, now_ms: u64) {
        let expired_history: Vec<_> = self
            .history
            .iter()
            .filter(|(_, entry)| now_ms.saturating_sub(entry.created_at_ms) >= CONTINUATION_TTL_MS)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired_history {
            if let Some(entry) = self.history.remove(&key) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
        let expired_pending: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, entry)| {
                now_ms.saturating_sub(entry.created_at_ms) >= PENDING_CONTINUATION_TTL_MS
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired_pending {
            if let Some(entry) = self.pending.remove(&key) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
    }

    fn evict_oldest(&mut self) -> bool {
        let oldest_history = self
            .history
            .iter()
            .map(|(key, entry)| (entry.sequence, key.clone()))
            .min_by_key(|(sequence, _)| *sequence);
        let oldest_pending = self
            .pending
            .iter()
            .map(|(key, entry)| (entry.sequence, key.clone()))
            .min_by_key(|(sequence, _)| *sequence);
        if oldest_history.is_none() && oldest_pending.is_none() {
            return false;
        }
        match (oldest_history, oldest_pending) {
            (None, None) => unreachable!("empty store returned above"),
            (Some((_, key)), None) => {
                if let Some(entry) = self.history.remove(&key) {
                    self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
                }
            }
            (None, Some((_, key))) => {
                if let Some(entry) = self.pending.remove(&key) {
                    self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
                }
            }
            (Some((history_sequence, history_key)), Some((pending_sequence, pending_key))) => {
                if history_sequence <= pending_sequence {
                    if let Some(entry) = self.history.remove(&history_key) {
                        self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
                    }
                } else if let Some(entry) = self.pending.remove(&pending_key) {
                    self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
                }
            }
        }
        true
    }

    fn history(&mut self, scope: &str, response_id: &str) -> Result<Vec<Message>, String> {
        self.prune(Self::now_ms());
        self.history
            .get(&(scope.to_owned(), response_id.to_owned()))
            .map(|entry| entry.messages.clone())
            .ok_or_else(|| {
                invalid(
                    "continuation_expired: previous_response_id is unknown, expired, or belongs to another Agent scope",
                )
            })
    }

    fn begin(&mut self, key: String, scope: String, messages: Vec<Message>) -> Result<(), String> {
        let now_ms = Self::now_ms();
        self.prune(now_ms);
        if self.pending.contains_key(&key) {
            return Err(invalid("continuation request key is already in flight"));
        }
        let bytes = serde_json::to_vec(&messages).map_err(internal)?.len();
        if bytes > MAX_CONTINUATION_ENTRY_BYTES {
            return Err(invalid(
                "continuation request history exceeds the in-memory entry limit",
            ));
        }
        while self.pending.len() >= MAX_PENDING_CONTINUATIONS
            || self.total_bytes.saturating_add(bytes) > MAX_CONTINUATION_TOTAL_BYTES
        {
            if !self.evict_oldest() {
                break;
            }
        }
        if self.total_bytes.saturating_add(bytes) > MAX_CONTINUATION_TOTAL_BYTES {
            return Err(invalid(
                "continuation request history exceeds the in-memory store limit",
            ));
        }
        let sequence = self.allocate_sequence();
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.pending.insert(
            key,
            PendingContinuation {
                scope,
                messages,
                created_at_ms: now_ms,
                sequence,
                bytes,
            },
        );
        Ok(())
    }

    fn abandon(&mut self, continuation_key: &str) {
        if let Some(entry) = self.pending.remove(continuation_key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
    }

    fn complete(
        &mut self,
        continuation_key: &str,
        response_id: &str,
        assistant_messages: impl IntoIterator<Item = Message>,
    ) {
        let Some(mut pending) = self.pending.remove(continuation_key) else {
            return;
        };
        self.total_bytes = self.total_bytes.saturating_sub(pending.bytes);
        pending.messages.retain(|message| {
            message
                .extensions
                .get(TRANSIENT_INSTRUCTIONS_EXTENSION)
                .and_then(Value::as_bool)
                != Some(true)
        });
        pending.messages.extend(assistant_messages);
        let Ok(encoded) = serde_json::to_vec(&pending.messages) else {
            return;
        };
        if encoded.len() > MAX_CONTINUATION_ENTRY_BYTES {
            return;
        }
        let now_ms = Self::now_ms();
        self.prune(now_ms);
        let history_key = (pending.scope, response_id.to_owned());
        if let Some(previous) = self.history.remove(&history_key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        while self.history.len() >= MAX_CONTINUATION_ENTRIES
            || self.total_bytes.saturating_add(encoded.len()) > MAX_CONTINUATION_TOTAL_BYTES
        {
            if !self.evict_oldest() {
                break;
            }
        }
        if self.total_bytes.saturating_add(encoded.len()) > MAX_CONTINUATION_TOTAL_BYTES {
            return;
        }
        let sequence = self.allocate_sequence();
        self.total_bytes = self.total_bytes.saturating_add(encoded.len());
        self.history.insert(
            history_key,
            ContinuationHistory {
                messages: pending.messages,
                created_at_ms: now_ms,
                sequence,
                bytes: encoded.len(),
            },
        );
    }
}

thread_local! {
    static CONTINUATIONS: RefCell<ContinuationStore> =
        RefCell::new(ContinuationStore::default());
}

fn continuation_scope(envelope: &AgentRequestEnvelope) -> Result<String, String> {
    let scope = envelope
        .extensions
        .get(CONTINUATION_SCOPE_EXTENSION)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}:{}",
                envelope.agent_tool.as_deref().unwrap_or("unknown-agent"),
                envelope.principal.subject
            )
        });
    if scope.is_empty() || scope.len() > 256 {
        return Err(invalid("continuation scope is invalid"));
    }
    Ok(scope)
}

fn continuation_request_key(envelope: &AgentRequestEnvelope) -> Result<Option<String>, String> {
    let Some(key) = envelope
        .extensions
        .get(CONTINUATION_KEY_EXTENSION)
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    if key.is_empty()
        || key.len() > 256
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(invalid("continuation request key is invalid"));
    }
    Ok(Some(key.to_owned()))
}

fn continuation_key(context: &Value) -> Option<&str> {
    context
        .get(CONTINUATION_KEY_EXTENSION)
        .and_then(Value::as_str)
}

struct RenderedContent {
    message: Vec<Value>,
    reasoning_summary: Vec<Value>,
    encrypted_reasoning: Option<String>,
}

fn rendered_content(content: Option<&Content>) -> Result<RenderedContent, String> {
    let mut rendered = RenderedContent {
        message: Vec::new(),
        reasoning_summary: Vec::new(),
        encrypted_reasoning: None,
    };
    match content {
        None => {}
        Some(Content::Text(text)) => rendered.message.push(json!({
            "type": "output_text",
            "text": text
        })),
        Some(Content::Parts(parts)) => {
            for part in parts {
                match part {
                    ContentPart::Text { text } => rendered.message.push(json!({
                    "type": "output_text",
                    "text": text
                    })),
                    ContentPart::ImageUrl { .. } => {
                        return Err(capability(
                            "Responses assistant image output is not represented by the Canonical IR",
                        ));
                    }
                    ContentPart::Thinking {
                        thinking,
                        signature,
                    } => {
                        rendered.reasoning_summary.push(json!({
                            "type": "summary_text",
                            "text": thinking
                        }));
                        if let Some(signature) = signature {
                            rendered.encrypted_reasoning = Some(signature.clone());
                        }
                    }
                    ContentPart::RedactedThinking { data } => {
                        rendered.encrypted_reasoning = Some(data.clone());
                    }
                    // 0.3.0: unmodeled parts survive verbatim instead of
                    // silently dropping content.
                    ContentPart::Unknown(value) => rendered.message.push(value.clone()),
                }
            }
        }
    }
    Ok(rendered)
}

fn response_tool_identity(
    context: &Value,
    provider_name: &str,
) -> Result<(String, Option<String>), String> {
    let Some(identity) = context
        .get(TOOL_NAMESPACES_EXTENSION)
        .and_then(Value::as_object)
        .and_then(|namespaces| namespaces.get(provider_name))
    else {
        return Ok((provider_name.to_owned(), None));
    };
    let namespace = identity
        .get("namespace")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("namespace render context declares no namespace"))?;
    let name = identity
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("namespace render context declares no function name"))?;
    if flattened_namespace_tool_name(namespace, name)? != provider_name {
        return Err(invalid(
            "namespace render context does not match the provider tool name",
        ));
    }
    Ok((name.to_owned(), Some(namespace.to_owned())))
}

fn response_function_call_item(
    id: String,
    status: &str,
    call_id: &str,
    provider_name: &str,
    arguments: &str,
    context: &Value,
) -> Result<Value, String> {
    let (name, namespace) = response_tool_identity(context, provider_name)?;
    Ok(response_function_call_item_with_identity(
        id,
        status,
        call_id,
        &name,
        namespace.as_deref(),
        arguments,
    ))
}

fn response_function_call_item_with_identity(
    id: String,
    status: &str,
    call_id: &str,
    name: &str,
    namespace: Option<&str>,
    arguments: &str,
) -> Value {
    let mut item = json!({
        "type": "function_call",
        "id": id,
        "status": status,
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    });
    if let Some(namespace) = namespace {
        item["namespace"] = json!(namespace);
    }
    item
}

fn response_output(
    response: &ChatResponse,
    response_id: &str,
    context: &Value,
) -> Result<Vec<Value>, String> {
    let restore = restore_map(context.get("inbound_tools").unwrap_or(&Value::Null));
    let mut output = Vec::new();
    for choice in &response.choices {
        let content = rendered_content(choice.message.content.as_ref())?;
        if !content.reasoning_summary.is_empty() || content.encrypted_reasoning.is_some() {
            output.push(json!({
                "type": "reasoning",
                "id": format!("rs_{response_id}_{}", choice.index),
                "status": "completed",
                "summary": content.reasoning_summary,
                "encrypted_content": content.encrypted_reasoning
            }));
        }
        if !content.message.is_empty() {
            output.push(json!({
                "type": "message",
                "id": format!("msg_{response_id}_{}", choice.index),
                "status": "completed",
                "role": "assistant",
                "content": content.message
            }));
        }
        for call in &choice.message.tool_calls {
            if call.name == LOCAL_SHELL_TOOL_NAME {
                let arguments: Value = serde_json::from_str(&call.arguments).map_err(|error| {
                    invalid(format!(
                        "local shell tool returned invalid arguments: {error}"
                    ))
                })?;
                let action = arguments
                    .get("action")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| invalid("local shell tool returned no action object"))?;
                output.push(json!({
                    "type": "local_shell_call",
                    "id": format!("ls_{}", call.id),
                    "status": "completed",
                    "call_id": call.id,
                    "action": action
                }));
            } else if restore.contains_key(&call.name) {
                output.push(restore_tool_call_item(call, &restore));
            } else {
                output.push(response_function_call_item(
                    format!("fc_{}", call.id),
                    "completed",
                    &call.id,
                    &call.name,
                    &call.arguments,
                    context,
                )?);
            }
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
struct TextStream {
    output_index: u32,
    content: String,
}

#[derive(Clone)]
struct ToolStream {
    output_index: u32,
    call_id: String,
    response_name: String,
    namespace: Option<String>,
    name: String,
    arguments: String,
    /// The `output_item` id (`ctc_` for a restored custom tool, `fc_` otherwise),
    /// fixed when the item is first announced so every later delta/done event
    /// references the same id.
    item_id: String,
}

#[derive(Clone)]
struct ReasoningStream {
    output_index: u32,
    content: String,
    encrypted_content: String,
}

struct StreamState {
    response_id: String,
    model: String,
    continuation_key: Option<String>,
    text: BTreeMap<u32, TextStream>,
    tools: BTreeMap<u32, ToolStream>,
    reasoning: BTreeMap<u32, ReasoningStream>,
    next_output_index: u32,
    usage: Usage,
    pending_finish_reason: Option<FinishReason>,
    /// Flat-name → original Codex tool kind, built once from the request's tools
    /// so streamed tool calls restore to the same shapes as the non-streaming
    /// path. Empty when the caller declared no restorable tools.
    restore: BTreeMap<String, RestoredTool>,
}

impl StreamState {
    fn new(
        response_id: String,
        model: String,
        continuation_key: Option<String>,
        restore: BTreeMap<String, RestoredTool>,
    ) -> Self {
        Self {
            response_id,
            model,
            continuation_key,
            text: BTreeMap::new(),
            tools: BTreeMap::new(),
            reasoning: BTreeMap::new(),
            next_output_index: 0,
            usage: Usage::default(),
            pending_finish_reason: None,
            restore,
        }
    }

    fn allocate_output_index(&mut self) -> Result<u32, String> {
        let output_index = self.next_output_index;
        self.next_output_index = output_index
            .checked_add(1)
            .ok_or_else(|| internal("Responses stream produced too many output items"))?;
        Ok(output_index)
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
    continuation_key: Option<&str>,
    inbound_tools: &Value,
) -> Result<(&'a mut StreamState, bool), String> {
    let created = !states.contains_key(stream_id);
    if created {
        states.insert(
            stream_id.to_owned(),
            StreamState::new(
                response_id.to_owned(),
                model.to_owned(),
                continuation_key.map(str::to_owned),
                // The restore map is built once, here, rather than on every event.
                restore_map(inbound_tools),
            ),
        );
    }
    let context_changed = states.get(stream_id).is_some_and(|state| {
        state.response_id != response_id
            || state.model != model
            || state.continuation_key.as_deref() != continuation_key
    });
    if context_changed {
        states.remove(stream_id);
        return Err(invalid(
            "stream render context changed response_id or model mid-stream",
        ));
    }
    let state = states.get_mut(stream_id).expect("state inserted");
    Ok((state, created))
}

fn stream_assistant_messages(state: &StreamState) -> Vec<Message> {
    let mut indices = std::collections::BTreeSet::new();
    indices.extend(state.text.keys().copied());
    indices.extend(state.tools.keys().copied());
    indices.extend(state.reasoning.keys().copied());
    indices
        .into_iter()
        .map(|index| {
            let mut parts = Vec::new();
            if let Some(reasoning) = state.reasoning.get(&index) {
                parts.push(ContentPart::Thinking {
                    thinking: reasoning.content.clone(),
                    signature: (!reasoning.encrypted_content.is_empty())
                        .then(|| reasoning.encrypted_content.clone()),
                });
            }
            if let Some(text) = state.text.get(&index) {
                parts.push(ContentPart::Text {
                    text: text.content.clone(),
                });
            }
            Message {
                role: Role::Assistant,
                content: (!parts.is_empty()).then_some(Content::Parts(parts)),
                tool_calls: state
                    .tools
                    .get(&index)
                    .map(|call| {
                        vec![ToolCall {
                            id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        }]
                    })
                    .unwrap_or_default(),
                tool_call_id: None,
                name: None,
                extensions: Extensions::new(),
            }
        })
        .collect()
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
    // Each entry: (output_index, done item, optional custom input to emit on the
    // `custom_tool_call_input` family just before the item's `output_item.done`).
    let mut completed: Vec<(u32, Value, Option<(String, String)>)> =
        Vec::with_capacity(state.text.len() + state.tools.len() + state.reasoning.len());
    for (index, reasoning) in state.reasoning {
        let item_id = format!("rs_{}_{}", state.response_id, index);
        rendered.push_str(&sse(
            "response.reasoning_summary_text.done",
            json!({
                "type": "response.reasoning_summary_text.done",
                "item_id": item_id,
                "output_index": reasoning.output_index,
                "summary_index": 0,
                "text": reasoning.content
            }),
        )?);
        rendered.push_str(&sse(
            "response.reasoning_summary_part.done",
            json!({
                "type": "response.reasoning_summary_part.done",
                "item_id": item_id,
                "output_index": reasoning.output_index,
                "summary_index": 0,
                "part": {
                    "type": "summary_text",
                    "text": reasoning.content
                }
            }),
        )?);
        let item = json!({
            "type": "reasoning",
            "id": item_id,
            "status": "completed",
            "summary": [{
                "type": "summary_text",
                "text": reasoning.content
            }],
            "encrypted_content": if reasoning.encrypted_content.is_empty() {
                Value::Null
            } else {
                json!(reasoning.encrypted_content)
            }
        });
        completed.push((reasoning.output_index, item, None));
    }
    for (index, text) in state.text {
        let item = json!({
            "type": "message",
            "id": format!("msg_{}_{}", state.response_id, index),
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text.content}]
        });
        completed.push((text.output_index, item, None));
    }
    for (_, call) in state.tools {
        let item = if call.name == LOCAL_SHELL_TOOL_NAME {
            let arguments: Value = serde_json::from_str(&call.arguments).map_err(|error| {
                invalid(format!(
                    "local shell stream returned invalid arguments: {error}"
                ))
            })?;
            let action = arguments
                .get("action")
                .filter(|value| value.is_object())
                .ok_or_else(|| invalid("local shell stream returned no action object"))?;
            let item = json!({
                "type": "local_shell_call",
                "id": format!("ls_{}", call.call_id),
                "status": "completed",
                "call_id": call.call_id,
                "action": action
            });
            rendered.push_str(&sse(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": call.output_index,
                    "item": item
                }),
            )?);
            item
        } else if state.restore.contains_key(&call.name) {
            restored_tool_item(
                &call.call_id,
                &call.name,
                &call.arguments,
                "completed",
                &state.restore,
            )
        } else {
            response_function_call_item_with_identity(
                format!("fc_{}", call.call_id),
                "completed",
                &call.call_id,
                &call.response_name,
                call.namespace.as_deref(),
                &call.arguments,
            )
        };
        // A custom tool buffered its `{ input }` arguments; unwrap them now for
        // the input delta/done events its client consumes.
        let custom_input = is_custom_restore(&call.name, &state.restore).then(|| {
            (
                call.item_id.clone(),
                custom_tool_input_from_arguments(&call.arguments)
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        });
        completed.push((call.output_index, item, custom_input));
    }
    completed.sort_by_key(|(output_index, _, _)| *output_index);

    let mut output = Vec::with_capacity(completed.len());
    for (output_index, item, custom_input) in completed {
        if let Some((item_id, input)) = custom_input {
            if !input.is_empty() {
                rendered.push_str(&sse(
                    "response.custom_tool_call_input.delta",
                    json!({
                        "type": "response.custom_tool_call_input.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": input
                    }),
                )?);
            }
            rendered.push_str(&sse(
                "response.custom_tool_call_input.done",
                json!({
                    "type": "response.custom_tool_call_input.done",
                    "item_id": item_id,
                    "output_index": output_index,
                    "input": input
                }),
            )?);
        }
        rendered.push_str(&sse(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item.clone()
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
        ErrorCode::PaymentRequired => "insufficient_quota",
        ErrorCode::RateLimit => "rate_limit_exceeded",
        ErrorCode::Capacity => "server_overloaded",
        ErrorCode::Capability => "unsupported_capability",
        ErrorCode::ContextLength => "context_length_exceeded",
        ErrorCode::ContentPolicy => "invalid_prompt",
        ErrorCode::UpstreamUnavailable | ErrorCode::TransportTruncated => "server_error",
        ErrorCode::ProviderProtocolError => "upstream_protocol_error",
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
        validate_semantic_options(body)?;
        let response_format = response_format_of(body)?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("request declares no model"))?
            .to_owned();
        let scope = continuation_scope(&envelope)?;
        let mut messages = Vec::new();
        if let Some(instructions) = body.get("instructions").filter(|value| !value.is_null()) {
            let instructions = instructions
                .as_str()
                .ok_or_else(|| invalid("instructions must be a string"))?;
            let mut message = Message::text(Role::System, instructions);
            message
                .extensions
                .insert(TRANSIENT_INSTRUCTIONS_EXTENSION.to_owned(), json!(true));
            messages.push(message);
        }
        if let Some(previous_response_id) = body.get("previous_response_id").and_then(Value::as_str)
        {
            let history = CONTINUATIONS.with(|continuations| {
                continuations
                    .borrow_mut()
                    .history(&scope, previous_response_id)
            })?;
            messages.extend(history);
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
        let tools = tools_of(body.get("tools").unwrap_or(&Value::Null))?;
        let tool_choice = tool_choice_of(body.get("tool_choice"))?;
        let mut extensions = request_extensions(body);
        extensions.extend(tool_extensions(body.get("tools").unwrap_or(&Value::Null))?);
        // `parallel_tool_calls` has no first-class Canonical IR field; it rides
        // the extensions passthrough so the outbound provider request preserves
        // it verbatim (OpenAI-compatible providers accept it alongside tools).
        if let Some(parallel) = body.get("parallel_tool_calls").and_then(Value::as_bool) {
            extensions.insert("parallel_tool_calls".to_owned(), json!(parallel));
        }
        // `reasoning.effort` → `reasoning_effort`, carried through extensions and
        // rendered by the provider when the model does not declare it out.
        if let Some(effort) = body
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(Value::as_str)
        {
            extensions.insert("reasoning_effort".to_owned(), json!(effort));
        }
        if let Some(summary) = body
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("summary"))
            .filter(|value| !value.is_null())
        {
            extensions.insert("responses_reasoning_summary".to_owned(), summary.clone());
        }
        if let Some(continuation_key) = continuation_request_key(&envelope)? {
            CONTINUATIONS.with(|continuations| {
                continuations
                    .borrow_mut()
                    .begin(continuation_key.clone(), scope, messages.clone())
            })?;
            extensions.insert(
                CONTINUATION_KEY_EXTENSION.to_owned(),
                json!(continuation_key),
            );
        }
        to_output(&ChatRequest {
            model,
            messages,
            tools,
            response_format,
            tool_choice,
            sampling,
            stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
            extensions,
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
            .find_map(|choice| choice.finish_reason.clone());
        let output = response_output(&response, &response_id, &context)?;
        let rendered = to_output(&response_object(
            &response_id,
            &model,
            output,
            response.usage,
            finish_reason,
        ))?;
        if let Some(key) = continuation_key(&context) {
            CONTINUATIONS.with(|continuations| {
                continuations.borrow_mut().complete(
                    key,
                    &response_id,
                    response.choices.iter().map(|choice| choice.message.clone()),
                );
            });
        }
        Ok(rendered)
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
            if let Some(key) = continuation_key(&context) {
                CONTINUATIONS.with(|continuations| {
                    continuations.borrow_mut().abandon(key);
                });
            }
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

        let inbound_tools = context.get("inbound_tools").cloned().unwrap_or(Value::Null);
        let data = STREAMS.with(|streams| {
            let mut states = streams.borrow_mut();
            let (state, created) = ensure_stream(
                &mut states,
                stream_id,
                response_id,
                model,
                continuation_key(&context),
                &inbound_tools,
            )?;
            let mut rendered = if created {
                started_event(state)?
            } else {
                String::new()
            };
            match event {
                StreamEvent::ThinkingDelta {
                    index,
                    thinking_delta,
                } => {
                    let item_id = format!("rs_{}_{}", state.response_id, index);
                    if !state.reasoning.contains_key(&index) {
                        let output_index = state.allocate_output_index()?;
                        rendered.push_str(&sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "type": "reasoning",
                                    "id": item_id,
                                    "status": "in_progress",
                                    "summary": []
                                }
                            }),
                        )?);
                        rendered.push_str(&sse(
                            "response.reasoning_summary_part.added",
                            json!({
                                "type": "response.reasoning_summary_part.added",
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "part": {
                                    "type": "summary_text",
                                    "text": ""
                                }
                            }),
                        )?);
                        state.reasoning.insert(
                            index,
                            ReasoningStream {
                                output_index,
                                content: String::new(),
                                encrypted_content: String::new(),
                            },
                        );
                    }
                    let reasoning = state.reasoning.get_mut(&index).expect("reasoning inserted");
                    reasoning.content.push_str(&thinking_delta);
                    rendered.push_str(&sse(
                        "response.reasoning_summary_text.delta",
                        json!({
                            "type": "response.reasoning_summary_text.delta",
                            "item_id": item_id,
                            "output_index": reasoning.output_index,
                            "summary_index": 0,
                            "delta": thinking_delta
                        }),
                    )?);
                }
                StreamEvent::ThinkingSignatureDelta {
                    index,
                    signature_delta,
                } => {
                    if !state.reasoning.contains_key(&index) {
                        let output_index = state.allocate_output_index()?;
                        state.reasoning.insert(
                            index,
                            ReasoningStream {
                                output_index,
                                content: String::new(),
                                encrypted_content: String::new(),
                            },
                        );
                    }
                    state
                        .reasoning
                        .get_mut(&index)
                        .expect("reasoning inserted")
                        .encrypted_content
                        .push_str(&signature_delta);
                }
                StreamEvent::Delta { index, content } => {
                    let item_id = format!("msg_{}_{}", state.response_id, index);
                    if !state.text.contains_key(&index) {
                        let output_index = state.allocate_output_index()?;
                        rendered.push_str(&sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
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
                                "output_index": output_index,
                                "content_index": 0,
                                "part": {
                                    "type": "output_text",
                                    "text": "",
                                    "annotations": []
                                }
                            }),
                        )?);
                        state.text.insert(
                            index,
                            TextStream {
                                output_index,
                                content: String::new(),
                            },
                        );
                    }
                    let text = state.text.get_mut(&index).expect("text inserted");
                    text.content.push_str(&content);
                    rendered.push_str(&sse(
                        "response.output_text.delta",
                        json!({
                            "type": "response.output_text.delta",
                            "item_id": item_id,
                            "output_index": text.output_index,
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
                    if !state.tools.contains_key(&index) {
                        let call_id = id.clone().ok_or_else(|| {
                            invalid("first tool-call stream fragment declares no id")
                        })?;
                        let call_name = name.clone().ok_or_else(|| {
                            invalid("first tool-call stream fragment declares no name")
                        })?;
                        let (response_name, namespace) =
                            response_tool_identity(&context, &call_name)?;
                        let output_index = state.allocate_output_index()?;
                        // Announce the item in its restored shape (custom_tool_call
                        // / namespaced function_call / function_call) with its final
                        // id, so every later delta/done event lines up.
                        let item_id = tool_item_id(&call_id, &call_name, &state.restore);
                        let added_item = restored_tool_item(
                            &call_id,
                            &call_name,
                            "",
                            "in_progress",
                            &state.restore,
                        );
                        let call = ToolStream {
                            output_index,
                            call_id,
                            response_name,
                            namespace,
                            name: call_name,
                            arguments: String::new(),
                            item_id,
                        };
                        if call.name != LOCAL_SHELL_TOOL_NAME {
                            let item = if state.restore.contains_key(&call.name) {
                                added_item
                            } else {
                                response_function_call_item_with_identity(
                                    format!("fc_{}", call.call_id),
                                    "in_progress",
                                    &call.call_id,
                                    &call.response_name,
                                    call.namespace.as_deref(),
                                    "",
                                )
                            };
                            rendered.push_str(&sse(
                                "response.output_item.added",
                                json!({
                                    "type": "response.output_item.added",
                                    "output_index": output_index,
                                    "item": item
                                }),
                            )?);
                        }
                        state.tools.insert(index, call);
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
                    let item_id = call.item_id.clone();
                    let output_index = call.output_index;
                    let call_name = call.name.clone();
                    // A custom tool's arguments arrive as `{ "input": … }` JSON
                    // fragments that cannot be unwrapped incrementally, so its
                    // input is buffered and emitted once at `stream_done` on the
                    // `custom_tool_call_input` family. Everything else streams its
                    // arguments delta as usual.
                    if call_name != LOCAL_SHELL_TOOL_NAME
                        && !is_custom_restore(&call_name, &state.restore)
                    {
                        rendered.push_str(&sse(
                            "response.function_call_arguments.delta",
                            json!({
                                "type": "response.function_call_arguments.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "delta": arguments_delta
                            }),
                        )?);
                    }
                }
                StreamEvent::Usage { usage } => {
                    state.usage = usage;
                }
                StreamEvent::Finish {
                    finish_reason,
                    stop_sequence: _,
                } => {
                    state.pending_finish_reason = finish_reason;
                }
                StreamEvent::Done {
                    finish_reason,
                    // Responses SSE has no stop-sequence slot to render into.
                    stop_sequence: _,
                } => {
                    let mut state = states.remove(stream_id).expect("state inserted");
                    let finish_reason = finish_reason.or(state.pending_finish_reason.take());
                    if let Some(key) = state.continuation_key.as_deref() {
                        let assistant_messages = stream_assistant_messages(&state);
                        CONTINUATIONS.with(|continuations| {
                            continuations.borrow_mut().complete(
                                key,
                                &state.response_id,
                                assistant_messages,
                            );
                        });
                    }
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
        let code = error
            .extensions
            .get("client_error_code")
            .and_then(Value::as_str)
            .unwrap_or_else(|| error_code(error.code));
        to_output(&json!({
            "error": {
                "type": "error",
                "code": code,
                "message": error.message
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use token_station_protocol::{Choice, ToolCall};

    fn responses_envelope(body: Value) -> String {
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        let request_number = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        serde_json::to_string(&AgentRequestEnvelope {
            protocol: "openai-responses".to_owned(),
            agent_tool: Some("codex".to_owned()),
            headers: Default::default(),
            principal: token_station_protocol::Principal {
                subject: "local".to_owned(),
                tenant: None,
            },
            hints: Vec::new(),
            body,
            extensions: [
                (
                    "token_station_continuation_scope".to_owned(),
                    json!("codex:local"),
                ),
                (
                    "token_station_private_continuation_key".to_owned(),
                    json!(format!("test-request-{request_number}")),
                ),
            ]
            .into_iter()
            .collect(),
        })
        .expect("Responses envelope serializes")
    }

    #[test]
    fn previous_response_id_replays_bounded_in_memory_history() {
        let first: ChatRequest = serde_json::from_str(
            &<ResponsesClient as Guest>::normalize_inbound(responses_envelope(json!({
                "model": "deepseek-chat",
                "input": "first turn",
                "store": false
            })))
            .expect("first turn normalizes"),
        )
        .expect("canonical request parses");
        let continuation_key = first.extensions["token_station_private_continuation_key"]
            .as_str()
            .expect("normalization assigns an opaque continuation key")
            .to_owned();
        let response = ChatResponse {
            id: "resp_first".to_owned(),
            model: "deepseek-chat".to_owned(),
            choices: vec![Choice {
                index: 0,
                message: Message::text(Role::Assistant, "first answer"),
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            }],
            usage: Usage::default(),
            extensions: Extensions::new(),
        };
        <ResponsesClient as Guest>::render_response(
            serde_json::to_string(&response).expect("response serializes"),
            json!({
                "response_id": "resp_first",
                "model": "deepseek-chat",
                "token_station_private_continuation_key": continuation_key
            })
            .to_string(),
        )
        .expect("first response renders and becomes continuation history");

        let second: ChatRequest = serde_json::from_str(
            &<ResponsesClient as Guest>::normalize_inbound(responses_envelope(json!({
                "model": "deepseek-chat",
                "previous_response_id": "resp_first",
                "input": "second turn",
                "store": false
            })))
            .expect("known previous_response_id normalizes"),
        )
        .expect("second canonical request parses");

        assert_eq!(second.messages.len(), 3);
        assert_eq!(second.messages[0], Message::text(Role::User, "first turn"));
        assert_eq!(
            second.messages[1],
            Message::text(Role::Assistant, "first answer")
        );
        assert_eq!(second.messages[2], Message::text(Role::User, "second turn"));
    }

    #[test]
    fn unknown_previous_response_id_fails_explicitly() {
        let error = <ResponsesClient as Guest>::normalize_inbound(responses_envelope(json!({
            "model": "deepseek-chat",
            "previous_response_id": "resp_missing",
            "input": "continue"
        })))
        .expect_err("an unknown response id must never be treated as empty history");

        assert!(error.contains("continuation_expired"), "{error}");
        assert!(error.contains("previous_response_id"), "{error}");
    }

    #[test]
    fn reasoning_items_preserve_summary_and_opaque_continuation_data() {
        let messages = input_messages(&json!([
            {
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{"type": "summary_text", "text": "Inspect the file first."}],
                "encrypted_content": "opaque-reasoning-ticket"
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "read_file",
                "arguments": "{\"path\":\"hello.py\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "print('hello')"
            }
        ]))
        .expect("Codex reasoning and tool history normalize");

        let assistant = &messages[0];
        assert!(matches!(
            assistant.content.as_ref(),
            Some(Content::Parts(parts))
                if matches!(
                    parts.as_slice(),
                    [ContentPart::Thinking { thinking, .. }]
                        if thinking == "Inspect the file first."
                )
        ));
        assert_eq!(
            assistant.extensions["responses_reasoning_id"],
            json!("rs_1")
        );
        assert_eq!(
            assistant.extensions["responses_reasoning_encrypted_content"],
            json!("opaque-reasoning-ticket")
        );
        assert_eq!(assistant.tool_calls[0].id, "call_1");
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn local_shell_tools_translate_to_provider_functions_and_back() {
        let tools = tools_of(&json!([{"type": "local_shell"}]))
            .expect("local shell has a canonical provider mapping");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "__token_station_responses_local_shell");

        let response = ChatResponse {
            id: "resp_1".to_owned(),
            model: "deepseek".to_owned(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "call_shell_1".to_owned(),
                        name: "__token_station_responses_local_shell".to_owned(),
                        arguments: json!({
                            "action": {
                                "type": "exec",
                                "command": ["python", "-V"],
                                "timeout_ms": 10000
                            }
                        })
                        .to_string(),
                    }],
                    tool_call_id: None,
                    name: None,
                    extensions: Extensions::new(),
                },
                finish_reason: Some(FinishReason::ToolCalls),
                stop_sequence: None,
            }],
            usage: Usage::default(),
            extensions: Extensions::new(),
        };

        let output = response_output(&response, "resp_1", &json!({}))
            .expect("provider tool call renders to Responses");
        assert_eq!(output[0]["type"], json!("local_shell_call"));
        assert_eq!(output[0]["call_id"], json!("call_shell_1"));
        assert_eq!(output[0]["action"]["command"][0], json!("python"));
    }

    #[test]
    fn local_shell_history_keeps_call_and_result_correlation() {
        let messages = input_messages(&json!([
            {
                "type": "local_shell_call",
                "call_id": "call_shell_1",
                "action": {
                    "type": "exec",
                    "command": ["python", "-V"],
                    "timeout_ms": 10000
                }
            },
            {
                "type": "local_shell_call_output",
                "id": "call_shell_1",
                "output": "Python 3.13"
            }
        ]))
        .expect("local shell history normalizes");

        assert_eq!(messages[0].tool_calls[0].id, "call_shell_1");
        assert_eq!(messages[0].tool_calls[0].name, LOCAL_SHELL_TOOL_NAME);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_shell_1"));
        assert_eq!(messages[1].name.as_deref(), Some(LOCAL_SHELL_TOOL_NAME));
    }

    #[test]
    fn namespace_tools_flatten_without_losing_the_reverse_identity() {
        let request: ChatRequest = serde_json::from_str(
            &<ResponsesClient as Guest>::normalize_inbound(responses_envelope(json!({
                "model": "deepseek-chat",
                "input": [
                    {"type": "message", "role": "user", "content": "spawn one worker"},
                    {
                        "type": "function_call",
                        "call_id": "call_spawn_1",
                        "namespace": "multi_agent_v1",
                        "name": "spawn_agent",
                        "arguments": "{\"task\":\"inspect\"}"
                    },
                    {
                        "type": "function_call_output",
                        "call_id": "call_spawn_1",
                        "output": "worker complete"
                    }
                ],
                "tools": [{
                    "type": "namespace",
                    "name": "multi_agent_v1",
                    "description": "Manage isolated workers.",
                    "tools": [{
                        "type": "function",
                        "name": "spawn_agent",
                        "description": "Spawn one worker.",
                        "strict": true,
                        "parameters": {
                            "type": "object",
                            "properties": {"task": {"type": "string"}},
                            "required": ["task"],
                            "additionalProperties": false
                        }
                    }]
                }]
            })))
            .expect("Codex namespace tools normalize"),
        )
        .expect("canonical request parses");

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "multi_agent_v1__spawn_agent");
        assert_eq!(
            request.messages[1].tool_calls[0].name,
            "multi_agent_v1__spawn_agent"
        );
        assert_eq!(
            request.extensions["responses_tool_strict"]["multi_agent_v1__spawn_agent"],
            json!(true)
        );
        assert_eq!(
            request.extensions["responses_tool_namespaces"]["multi_agent_v1__spawn_agent"],
            json!({"namespace":"multi_agent_v1","name":"spawn_agent"})
        );
    }

    #[test]
    fn typed_semantic_options_and_tool_metadata_survive_normalization() {
        assert_eq!(
            tool_choice_of(Some(&json!("required"))).expect("required is typed"),
            Some(ToolChoice::Required)
        );
        assert_eq!(
            tool_choice_of(Some(&json!({"type":"function","name":"read_file"})))
                .expect("forced function is preserved"),
            Some(ToolChoice::Other(
                json!({"type":"function","function":{"name":"read_file"}})
            ))
        );
        assert_eq!(
            response_format_of(&json!({
                "text": {
                    "format": {
                        "type": "json_schema",
                        "name": "answer",
                        "strict": true,
                        "schema": {"type":"object"}
                    }
                }
            }))
            .expect("structured output is typed"),
            Some(ResponseFormat::JsonSchema {
                json_schema: json!({
                    "name":"answer",
                    "strict":true,
                    "schema":{"type":"object"}
                })
            })
        );

        let metadata = tool_extensions(&json!([
            {"type":"function","name":"read_file","strict":true},
            {"type":"web_search","external_web_access":false}
        ]))
        .expect("tool metadata is valid");
        assert_eq!(metadata["responses_tool_strict"]["read_file"], json!(true));
        assert_eq!(
            metadata["responses_disabled_provider_tools"][0]["type"],
            json!("web_search")
        );
    }

    #[test]
    fn response_reasoning_is_a_separate_responses_output_item() {
        let response = ChatResponse {
            id: "resp_reasoning".to_owned(),
            model: "deepseek-reasoner".to_owned(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Some(Content::Parts(vec![
                        ContentPart::Thinking {
                            thinking: "Inspect first.".to_owned(),
                            signature: None,
                        },
                        ContentPart::Text {
                            text: "Done.".to_owned(),
                        },
                    ])),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    extensions: Extensions::new(),
                },
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            }],
            usage: Usage::default(),
            extensions: Extensions::new(),
        };

        let output = response_output(&response, "resp_reasoning", &json!({}))
            .expect("reasoning has a Responses representation");
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(
            output[0]["summary"][0],
            json!({"type":"summary_text","text":"Inspect first."})
        );
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["content"][0]["text"], json!("Done."));
    }

    #[test]
    fn streamed_reasoning_has_delta_done_and_terminal_response_events() {
        let context = json!({
            "stream_id":"reasoning-unit-stream",
            "response_id":"resp_stream_reasoning",
            "model":"deepseek-reasoner"
        })
        .to_string();
        let delta = <ResponsesClient as Guest>::render_stream_event(
            serde_json::to_string(&StreamEvent::ThinkingDelta {
                index: 0,
                thinking_delta: "Inspect first.".to_owned(),
            })
            .expect("event serializes"),
            context.clone(),
        )
        .expect("reasoning delta renders");
        assert!(delta.contains("response.reasoning_summary_text.delta"));

        let done = <ResponsesClient as Guest>::render_stream_event(
            serde_json::to_string(&StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            })
            .expect("event serializes"),
            context,
        )
        .expect("terminal event renders");
        assert!(done.contains("response.reasoning_summary_text.done"));
        assert!(done.contains("response.output_item.done"));
        assert!(done.contains("response.completed"));
    }
}

export!(ResponsesClient);
