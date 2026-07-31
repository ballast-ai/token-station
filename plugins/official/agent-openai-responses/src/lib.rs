//! OpenAI Responses northbound adapter, scoped to the Codex M4 path.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, FinishReason, HintKind, ImageUrl, Message, Role, Sampling,
    StreamEvent, ToolCall, ToolChoice, ToolDef, Usage,
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
    "previous_response_id",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning",
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

fn validate_semantic_options(body: &Value) -> Result<(), String> {
    if body
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(capability(
            "Responses previous_response_id requires stateful response chaining that Canonical IR does not represent",
        ));
    }

    match body.get("tool_choice") {
        None | Some(Value::Null) => {}
        Some(Value::String(choice)) if choice == "auto" => {}
        Some(Value::String(choice)) => {
            return Err(capability(format!(
                "Responses tool_choice {choice} cannot be preserved by Canonical IR"
            )));
        }
        Some(Value::Object(_)) => {
            return Err(capability(
                "Responses forced tool_choice cannot be preserved by Canonical IR",
            ));
        }
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
    // A replayed namespace call carries a `namespace` field alongside the bare
    // child name (this is how `restore_tool_call_item` emitted it). Flatten it
    // back to the `<namespace>__<child>` name the tool was declared under, so
    // history lines up with the current turn's tools.
    let name = match item.get("namespace").and_then(Value::as_str) {
        Some(namespace) if !namespace.is_empty() => flatten_namespace_name(namespace, name),
        _ => name.to_owned(),
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
                    Some("reasoning") => Err(capability(
                        "Responses reasoning items require an approved Canonical IR extension",
                    )),
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
    Namespace { namespace: String, child: String },
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

/// Codex pairs `function` tools with built-ins (`local_shell`, `custom`,
/// `namespace`, `tool_search`) and hosted tools (`web_search`, `file_search`,
/// `code_interpreter`, …). Only `function` maps cleanly to Canonical IR, but
/// dropping the rest hands the model a smaller tool set than the client declared
/// (a fake success), and failing the whole request leaves Codex dead on arrival.
/// So, following CC Switch, we translate every tool to a `function` the model can
/// call: `namespace` children are lifted to top-level functions with the stable
/// flat name `<namespace>__<child>` (a collision is a hard error, never a silent
/// overwrite), and every other type becomes a function carrying its declared
/// name/description/parameters. Client-executed tools (local_shell, custom,
/// namespace) then run inside Codex exactly as before.
///
/// LIMITATION: response-side name restoration (flat name → `{name, namespace}`,
/// and `function_call` → `custom_tool_call`/`local_shell_call` items) needs the
/// host to thread a restore map from the request into the render context, since
/// `normalize_inbound` and `render_*` are separate stateless calls. Until that
/// lands, namespace tool calls come back flat. Genuinely server-hosted tools
/// (web_search, file_search, …) only truly execute on an upstream that owns
/// them; on other providers they are best-effort, at CC Switch parity.
fn tools_of(value: &Value) -> Result<Vec<ToolDef>, String> {
    let mut tools = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for tool in value.as_array().into_iter().flatten() {
        let kind = match tool.get("type").and_then(Value::as_str) {
            Some(kind) => kind,
            None => return Err(invalid("tool declares no type")),
        };
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let parameters = tool.get("parameters").cloned().unwrap_or_else(|| json!({}));
        match kind {
            "function" => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("function tool declares no name"))?
                    .to_owned();
                push_tool(&mut tools, &mut seen, name, description, parameters)?;
            }
            "namespace" => {
                let namespace = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("namespace tool declares no name"))?;
                for child in tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let child_name = child
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("namespace child tool declares no name"))?;
                    let flat = flatten_namespace_name(namespace, child_name);
                    push_tool(
                        &mut tools,
                        &mut seen,
                        flat,
                        child
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        child.get("parameters").cloned().unwrap_or_else(|| json!({})),
                    )?;
                }
            }
            // A custom (freeform-grammar) tool has no JSON schema — the model
            // returns a raw string. Wrap that in a fixed `{ input: string }`
            // schema so it rides the function-tool path; `restore_tool_call_item`
            // unwraps it back into a `custom_tool_call` on the way out.
            "custom" => {
                push_tool(
                    &mut tools,
                    &mut seen,
                    custom_tool_name(tool),
                    Some(custom_tool_description(tool)),
                    custom_tool_parameters(),
                )?;
            }
            // The `tool_search` built-in is proxied as a function with a fixed
            // query/limit schema; `restore_tool_call_item` turns the call back
            // into a `tool_search_call`.
            "tool_search" => {
                push_tool(
                    &mut tools,
                    &mut seen,
                    TOOL_SEARCH_PROXY_NAME.to_owned(),
                    Some(
                        "Search and load Codex tools, plugins, connectors, and MCP namespaces for the current task."
                            .to_owned(),
                    ),
                    tool_search_parameters(),
                )?;
            }
            // local_shell, web_search, file_search, code_interpreter, computer,
            // mcp, image_generation, … → a function tool named after its declared
            // name, or its type when it has none.
            _ => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or_else(|| kind.to_owned(), str::to_owned);
                push_tool(&mut tools, &mut seen, name, description, parameters)?;
            }
        }
    }
    Ok(tools)
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
                // 0.3.0: reasoning renders as a Responses reasoning text part;
                // its signature has no Responses wire slot.
                ContentPart::Thinking { thinking, .. } => Ok(json!({
                    "type": "reasoning_text",
                    "text": thinking
                })),
                ContentPart::RedactedThinking { .. } => Err(capability(
                    "Responses output cannot render encrypted reasoning blocks",
                )),
                // 0.3.0: unmodeled parts survive verbatim instead of silently
                // dropping content.
                ContentPart::Unknown(value) => Ok(value.clone()),
            })
            .collect(),
    }
}

fn response_output(
    response: &ChatResponse,
    response_id: &str,
    restore: &BTreeMap<String, RestoredTool>,
) -> Result<Vec<Value>, String> {
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
            output.push(restore_tool_call_item(call, restore));
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
    name: String,
    arguments: String,
    /// The `output_item` id (`ctc_` for a restored custom tool, `fc_` otherwise),
    /// fixed when the item is first announced so every later delta/done event
    /// references the same id.
    item_id: String,
}

struct StreamState {
    response_id: String,
    model: String,
    text: BTreeMap<u32, TextStream>,
    tools: BTreeMap<u32, ToolStream>,
    next_output_index: u32,
    usage: Usage,
    /// Flat-name → original Codex tool kind, built once from the request's tools
    /// so streamed tool calls restore to the same shapes as the non-streaming
    /// path. Empty when the caller declared no restorable tools.
    restore: BTreeMap<String, RestoredTool>,
}

impl StreamState {
    fn new(response_id: String, model: String, restore: BTreeMap<String, RestoredTool>) -> Self {
        Self {
            response_id,
            model,
            text: BTreeMap::new(),
            tools: BTreeMap::new(),
            next_output_index: 0,
            usage: Usage::default(),
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
    inbound_tools: &Value,
) -> Result<(&'a mut StreamState, bool), String> {
    let created = !states.contains_key(stream_id);
    if created {
        states.insert(
            stream_id.to_owned(),
            // The restore map is built once, here, rather than on every event.
            StreamState::new(
                response_id.to_owned(),
                model.to_owned(),
                restore_map(inbound_tools),
            ),
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
    // Each entry: (output_index, done item, optional custom input to emit on the
    // `custom_tool_call_input` family just before the item's `output_item.done`).
    let mut completed: Vec<(u32, Value, Option<(String, String)>)> =
        Vec::with_capacity(state.text.len() + state.tools.len());
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
        let item =
            restored_tool_item(&call.call_id, &call.name, &call.arguments, "completed", &state.restore);
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
        validate_text_format(body)?;
        validate_semantic_options(body)?;
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
        let mut extensions = request_extensions(body);
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
        to_output(&ChatRequest {
            model,
            messages,
            tools: tools_of(body.get("tools").unwrap_or(&Value::Null))?,
            response_format: None,
            // `validate_request` already refused everything but `auto`.
            tool_choice: body
                .get("tool_choice")
                .filter(|value| !value.is_null())
                .map(|_| ToolChoice::Auto),
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
        let restore = restore_map(context.get("inbound_tools").unwrap_or(&Value::Null));
        let finish_reason = response
            .choices
            .iter()
            .find_map(|choice| choice.finish_reason.clone());
        let output = response_output(&response, &response_id, &restore)?;
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

        let inbound_tools = context.get("inbound_tools").cloned().unwrap_or(Value::Null);
        let data = STREAMS.with(|streams| {
            let mut states = streams.borrow_mut();
            let (state, created) =
                ensure_stream(&mut states, stream_id, response_id, model, &inbound_tools)?;
            let mut rendered = if created {
                started_event(state)?
            } else {
                String::new()
            };
            match event {
                // 0.3.0 thinking events have no Responses rendering yet: the
                // reasoning-summary SSE family is stateful and unimplemented,
                // so reasoning deltas render nothing rather than a malformed
                // frame.
                StreamEvent::ThinkingDelta { .. } | StreamEvent::ThinkingSignatureDelta { .. } => {}
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
                        let output_index = state.allocate_output_index()?;
                        // Announce the item in its restored shape (custom_tool_call
                        // / namespaced function_call / function_call) with its final
                        // id, so every later delta/done event lines up.
                        let item_id = tool_item_id(&call_id, &call_name, &state.restore);
                        let added_item =
                            restored_tool_item(&call_id, &call_name, "", "in_progress", &state.restore);
                        let call = ToolStream {
                            output_index,
                            call_id,
                            name: call_name,
                            arguments: String::new(),
                            item_id,
                        };
                        rendered.push_str(&sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": added_item
                            }),
                        )?);
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
                    if !is_custom_restore(&call_name, &state.restore) {
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
                StreamEvent::Done {
                    finish_reason,
                    // Responses SSE has no stop-sequence slot to render into.
                    stop_sequence: _,
                } => {
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
