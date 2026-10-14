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
    ToolChoice, ToolDef,
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
    let mut pending_calls: Vec<(String, String, bool)> = Vec::new();
    let mut generated_call_id = 0_u64;
    if let Some(system) = parse_system(body)? {
        messages.push(system);
    }
    for item in contents {
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
        for part in parts {
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("functionCall declares no name"))?;
                let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                let id = match call.get("id") {
                    None | Some(Value::Null) => {
                        let id = format!("gemini-call-{generated_call_id}");
                        generated_call_id = generated_call_id
                            .checked_add(1)
                            .ok_or_else(|| invalid("too many Gemini function calls"))?;
                        id
                    }
                    Some(Value::String(id)) if !id.is_empty() => id.clone(),
                    Some(Value::String(_)) => {
                        return Err(invalid("functionCall id must not be empty"));
                    }
                    Some(_) => return Err(invalid("functionCall id must be a string")),
                };
                if pending_calls.iter().any(|(existing, _, _)| existing == &id) {
                    return Err(invalid("functionCall id is duplicated"));
                }
                pending_calls.push((id.clone(), name.to_owned(), false));
                tool_calls.push(ToolCall {
                    id,
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
                let explicit_id = match result.get("id") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(id)) if !id.is_empty() => Some(id.as_str()),
                    Some(Value::String(_)) => {
                        return Err(invalid("functionResponse id must not be empty"));
                    }
                    Some(_) => return Err(invalid("functionResponse id must be a string")),
                };
                let pending_index = if let Some(id) = explicit_id {
                    pending_calls
                        .iter()
                        .position(|(candidate, _, answered)| candidate == id && !answered)
                        .ok_or_else(|| {
                            invalid("functionResponse references an unknown or completed call id")
                        })?
                } else {
                    pending_calls
                        .iter()
                        .position(|(_, call_name, answered)| call_name == name && !answered)
                        .ok_or_else(|| {
                            invalid(
                                "functionResponse has no matching unanswered functionCall by name",
                            )
                        })?
                };
                let (call_id, call_name, answered) = &mut pending_calls[pending_index];
                if call_name != name {
                    return Err(invalid(
                        "functionResponse name does not match its functionCall id",
                    ));
                }
                *answered = true;
                tool_results.push(Message {
                    role: Role::Tool,
                    content: Some(Content::Text(encode(&response)?)),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call_id.clone()),
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
        if group.get("functionDeclarations").is_some()
            && group
                .as_object()
                .is_some_and(|fields| fields.keys().any(|key| key != "functionDeclarations"))
        {
            return Err(capability("Gemini function declarations cannot share a group with hosted tools or other constraints"));
        }
        let declarations = match group.get("functionDeclarations").and_then(Value::as_array) {
            Some(declarations) => declarations,
            None => {
                // Gemini hosted tools — Google Search grounding, URL Context,
                // Code Execution — arrive as their own group keys
                // ({"google_search": {}}, {"url_context": {}},
                // {"code_execution": {}}) and are executed by Google's servers,
                // not the router. Canonical IR only carries functionDeclarations.
                // Name the hosted tool and fail closed rather than emitting a
                // generic "unsupported": the failure is then actionable (native
                // Gemini route, or disable the tool on the Agent).
                let named = group
                    .as_object()
                    .map(|group| {
                        group
                            .keys()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|keys| !keys.is_empty())
                    .unwrap_or_else(|| "unknown".to_owned());
                return Err(capability(format!(
                    "Gemini hosted tools ({named}) are executed by Google's servers and have no \
                     Canonical IR representation; only functionDeclarations are supported. Route \
                     to a native Gemini upstream or disable the tool on the Agent."
                )));
            }
        };
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
                parameters: function_parameters(declaration)?,
            });
        }
    }
    Ok(definitions)
}

fn function_parameters(declaration: &Value) -> Result<Value, String> {
    match (
        declaration.get("parameters"),
        declaration.get("parametersJsonSchema"),
    ) {
        (Some(_), Some(_)) => Err(invalid(
            "function declaration must not combine parameters and parametersJsonSchema",
        )),
        (Some(schema), None) if schema.is_object() => {
            // Older clients also put standard JSON Schema in parameters. Keep
            // that schema intact unless it contains legacy-only vocabulary.
            if uses_legacy_schema(schema) {
                legacy_schema(schema, 0)
            } else {
                Ok(schema.clone())
            }
        }
        (None, Some(schema)) if schema.is_object() => Ok(schema.clone()),
        (None, None) => Ok(json!({"type":"object"})),
        _ => Err(invalid(
            "function declaration parameters must be a schema object",
        )),
    }
}

fn uses_legacy_schema(schema: &Value) -> bool {
    let Some(fields) = schema.as_object() else {
        return false;
    };
    if fields.contains_key("nullable")
        || fields.contains_key("propertyOrdering")
        || fields
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                !matches!(
                    kind,
                    "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
                )
            })
        || [
            "minItems",
            "maxItems",
            "minLength",
            "maxLength",
            "minProperties",
            "maxProperties",
        ]
        .iter()
        .any(|key| fields.get(*key).is_some_and(Value::is_string))
    {
        return true;
    }
    for key in ["properties", "$defs", "definitions", "dependentSchemas"] {
        if fields
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|children| children.values().any(uses_legacy_schema))
        {
            return true;
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if fields.get(key).is_some_and(uses_legacy_schema) {
            return true;
        }
    }
    ["anyOf", "allOf", "oneOf", "prefixItems"]
        .iter()
        .any(|key| {
            fields
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(|children| children.iter().any(uses_legacy_schema))
        })
}

/// Convert only modeled legacy OpenAPI semantics. JSON Schema declarations
/// use a separate field and remain unchanged.
fn legacy_schema(schema: &Value, depth: u8) -> Result<Value, String> {
    if depth >= 64 {
        return Err(invalid(
            "function schema nesting exceeds the supported depth",
        ));
    }
    let fields = schema
        .as_object()
        .ok_or_else(|| invalid("legacy function schema must be an object"))?;
    let mut result = Map::new();
    for (key, value) in fields {
        let converted = match key.as_str() {
            "type" => {
                let kind = value
                    .as_str()
                    .ok_or_else(|| invalid("legacy schema type must be a string"))?;
                let kind = match kind {
                    "OBJECT" | "object" => "object",
                    "ARRAY" | "array" => "array",
                    "STRING" | "string" => "string",
                    "INTEGER" | "integer" => "integer",
                    "NUMBER" | "number" => "number",
                    "BOOLEAN" | "boolean" => "boolean",
                    "NULL" | "null" => "null",
                    _ => return Err(capability("legacy schema type is not supported")),
                };
                json!(kind)
            }
            "properties" => {
                let properties = value
                    .as_object()
                    .ok_or_else(|| invalid("schema properties must be an object"))?;
                let mut mapped = Map::new();
                for (name, child) in properties {
                    mapped.insert(name.clone(), legacy_schema(child, depth + 1)?);
                }
                Value::Object(mapped)
            }
            "items" => legacy_schema(value, depth + 1)?,
            "additionalProperties" if value.is_boolean() => value.clone(),
            "additionalProperties" => legacy_schema(value, depth + 1)?,
            "anyOf" => {
                let children = value
                    .as_array()
                    .filter(|items| !items.is_empty())
                    .ok_or_else(|| invalid("schema anyOf must be a nonempty array"))?;
                Value::Array(
                    children
                        .iter()
                        .map(|child| legacy_schema(child, depth + 1))
                        .collect::<Result<_, _>>()?,
                )
            }
            "minItems" | "maxItems" | "minLength" | "maxLength" | "minProperties"
            | "maxProperties" => {
                let number = value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|number| number.parse::<u64>().ok()))
                    .ok_or_else(|| invalid("schema size limits must be nonnegative integers"))?;
                json!(number)
            }
            "minimum" | "maximum" if value.is_number() => value.clone(),
            "minimum" | "maximum" => return Err(invalid("schema numeric bounds must be numbers")),
            "title" | "description" | "format" | "pattern" if value.is_string() => value.clone(),
            "title" | "description" | "format" | "pattern" => {
                return Err(invalid("schema text fields must be strings"))
            }
            "required" => {
                string_array(Some(value), "schema required")?;
                value.clone()
            }
            "enum" if value.is_array() => value.clone(),
            "enum" => return Err(invalid("schema enum must be an array")),
            "default" | "example" => value.clone(),
            "nullable" if value.is_boolean() => continue,
            "nullable" => return Err(invalid("schema nullable must be a boolean")),
            _ => {
                return Err(capability(
                    "legacy schema contains an unsupported constraint",
                ))
            }
        };
        result.insert(key.clone(), converted);
    }
    let schema = Value::Object(result);
    // A union preserves enum and nested constraints for non-null values.
    Ok(if fields.get("nullable") == Some(&Value::Bool(true)) {
        json!({"anyOf":[schema,{"type":"null"}]})
    } else {
        schema
    })
}

fn function_choice(body: &Value, tools: &mut Vec<ToolDef>) -> Result<Option<ToolChoice>, String> {
    let Some(config) = body.get("toolConfig").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let config = config
        .as_object()
        .ok_or_else(|| invalid("toolConfig must be an object"))?;
    if config.keys().any(|key| key != "functionCallingConfig") {
        return Err(capability("toolConfig contains an unsupported constraint"));
    }
    let Some(calling) = config.get("functionCallingConfig") else {
        return Ok(None);
    };
    let calling = calling
        .as_object()
        .ok_or_else(|| invalid("functionCallingConfig must be an object"))?;
    if calling
        .keys()
        .any(|key| !matches!(key.as_str(), "mode" | "allowedFunctionNames"))
    {
        return Err(capability(
            "functionCallingConfig contains an unsupported constraint",
        ));
    }
    let mode = match calling.get("mode") {
        None => "AUTO",
        Some(Value::String(mode)) => mode.as_str(),
        _ => return Err(invalid("functionCallingConfig mode must be a string")),
    };
    if let Some(allowed) = calling.get("allowedFunctionNames") {
        if mode != "ANY" {
            return Err(invalid("allowedFunctionNames requires ANY mode"));
        }
        let names = string_array(Some(allowed), "allowedFunctionNames")?;
        let unique: std::collections::BTreeSet<_> = names.iter().map(String::as_str).collect();
        if unique.is_empty()
            || unique.len() != names.len()
            || unique
                .iter()
                .any(|name| !tools.iter().any(|tool| tool.name == *name))
        {
            return Err(invalid(
                "allowedFunctionNames must contain unique declared function names",
            ));
        }
        // Restrict executable declarations only. Historical calls still retain
        // their names and IDs in parse_messages, even when outside this subset.
        tools.retain(|tool| unique.contains(tool.name.as_str()));
    }
    match mode {
        "AUTO" | "MODE_UNSPECIFIED" => Ok(Some(ToolChoice::Auto)),
        "NONE" => Ok(Some(ToolChoice::None)),
        "ANY" if !tools.is_empty() => Ok(Some(ToolChoice::Required)),
        "ANY" => Err(invalid("ANY mode requires at least one declared function")),
        _ => Err(capability("functionCallingConfig mode is not supported")),
    }
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

fn finish_reason(reason: Option<FinishReason>) -> Option<String> {
    match reason {
        Some(FinishReason::Stop) => Some("STOP".to_owned()),
        Some(FinishReason::Length) => Some("MAX_TOKENS".to_owned()),
        Some(FinishReason::ToolCalls) => Some("STOP".to_owned()),
        Some(FinishReason::ContentFilter) => Some("SAFETY".to_owned()),
        // Gemini reports a fired stop sequence as a plain STOP.
        Some(FinishReason::StopSequence) => Some("STOP".to_owned()),
        // 0.3.0: unknown reasons survive verbatim instead of vanishing.
        Some(FinishReason::Other(other)) => Some(other),
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
                    // 0.3.0: reasoning renders as a Gemini thought part; the
                    // signature rides its `thoughtSignature` slot.
                    ContentPart::Thinking {
                        thinking,
                        signature,
                    } => {
                        let mut part = json!({"text": thinking, "thought": true});
                        if let Some(signature) = signature {
                            part["thoughtSignature"] = json!(signature);
                        }
                        parts.push(part);
                    }
                    // Opaque encrypted reasoning has no Gemini wire slot;
                    // there is nothing renderable inside it.
                    ContentPart::RedactedThinking { .. } => {}
                    // 0.3.0: unmodeled parts survive verbatim instead of
                    // silently dropping content.
                    ContentPart::Unknown(value) => parts.push(value.clone()),
                }
            }
        }
    }
    for call in &message.tool_calls {
        let args: Value = serde_json::from_str(&call.arguments)
            .map_err(|_| invalid("tool call arguments are not complete JSON"))?;
        let mut function = json!({"name": call.name, "args": args});
        if !call.id.is_empty() {
            function["id"] = json!(call.id);
        }
        parts.push(json!({"functionCall": function}));
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
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

thread_local! {
    static STREAM_CALLS: RefCell<BTreeMap<String, BTreeMap<u32, PendingCall>>> =
        const { RefCell::new(BTreeMap::new()) };
    static STREAM_FINISHES: RefCell<BTreeMap<String, Option<FinishReason>>> =
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
        if generation.get("responseSchema").is_some()
            || generation.get("responseJsonSchema").is_some()
        {
            return Err(capability(
                "Gemini response schemas are not supported by this adapter",
            ));
        }
        if let Some(mime) = generation.get("responseMimeType") {
            let mime = mime
                .as_str()
                .ok_or_else(|| invalid("responseMimeType must be a string"))?;
            if mime != "text/plain" {
                return Err(capability(
                    "Gemini structured response MIME types are not supported by this adapter",
                ));
            }
        }
        let max_output_tokens = generation
            .get("maxOutputTokens")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| invalid("maxOutputTokens must be an unsigned 32-bit integer"))
            })
            .transpose()?;
        let mut tools = parse_tools(&envelope.body)?;
        let tool_choice = function_choice(&envelope.body, &mut tools)?;
        encode(&ChatRequest {
            model: model.to_owned(),
            messages: parse_messages(&envelope.body)?,
            tools,
            response_format: None,
            tool_choice,
            sampling: Sampling {
                temperature: generation.get("temperature").and_then(Value::as_f64),
                top_p: generation.get("topP").and_then(Value::as_f64),
                max_output_tokens,
                stop: string_array(generation.get("stopSequences"), "stopSequences")?,
            },
            stream,
            host_values: token_station_protocol::ComponentValues::new(),
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
                    "finishReason": finish_reason(choice.finish_reason.clone()),
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
            // 0.3.0: reasoning deltas stream as Gemini thought parts; a
            // signature fragment has no streaming wire slot and renders
            // nothing. This wire ignores `block_index` while preserving
            // `index` as the candidate choice index.
            StreamEvent::ThinkingDelta {
                index,
                block_index: _,
                thinking_delta,
            } => sse(&json!({
                "candidates": [{"index": index, "content": {"role": "model", "parts": [{"text": thinking_delta, "thought": true}]}}],
                "modelVersion": model,
            }))?,
            StreamEvent::ThinkingSignatureDelta { .. } => String::new(),
            StreamEvent::RedactedThinking { .. } => {
                return Err(invalid("gemini cannot render redacted thinking"));
            }
            StreamEvent::Delta { index, content } => sse(&json!({
                "candidates": [{"index": index, "content": {"role": "model", "parts": [{"text": content}]}}],
                "modelVersion": model,
            }))?,
            StreamEvent::ToolCallDelta {
                index,
                id: call_id,
                name,
                arguments_delta,
            } => {
                STREAM_CALLS.with(|streams| {
                    let mut streams = streams.borrow_mut();
                    let pending = streams
                        .entry(id.clone())
                        .or_default()
                        .entry(index)
                        .or_default();
                    if let Some(call_id) = call_id.filter(|value| !value.is_empty()) {
                        if pending.id.as_ref().is_some_and(|id| id != &call_id) {
                            return Err(invalid("streamed function call changed its ID"));
                        }
                        pending.id = Some(call_id);
                    }
                    if let Some(name) = name {
                        if pending
                            .name
                            .as_ref()
                            .is_some_and(|current| current != &name)
                        {
                            return Err(invalid("streamed function call changed its name"));
                        }
                        pending.name = Some(name);
                    }
                    pending.arguments.push_str(&arguments_delta);
                    Ok(())
                })?;
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
            StreamEvent::Finish {
                finish_reason,
                stop_sequence: _,
            } => {
                STREAM_FINISHES.with(|finishes| {
                    finishes.borrow_mut().insert(id.clone(), finish_reason);
                });
                String::new()
            }
            StreamEvent::Done {
                finish_reason: mut reason,
                // Gemini candidates carry no stop-sequence report slot.
                stop_sequence: _,
            } => {
                if reason.is_none() {
                    reason = STREAM_FINISHES
                        .with(|finishes| finishes.borrow_mut().remove(&id).flatten());
                } else {
                    STREAM_FINISHES.with(|finishes| {
                        finishes.borrow_mut().remove(&id);
                    });
                }
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
                        let mut function = json!({"name": name, "args": args});
                        if let Some(id) = call.id {
                            function["id"] = json!(id);
                        }
                        parts.push(json!({"functionCall": function}));
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
                STREAM_FINISHES.with(|finishes| {
                    finishes.borrow_mut().remove(&id);
                });
                let rendered = Self::map_inbound_error(encode(&error)?, String::new())?;
                format!("data: {rendered}\n\n")
            }
        };
        encode(&json!({"data": data}))
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse(&error)?;
        let status = if error
            .extensions
            .get("client_error_code")
            .and_then(Value::as_str)
            == Some("route_not_configured")
        {
            "FAILED_PRECONDITION"
        } else {
            match error.code {
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
            }
        };
        let mut body = Map::new();
        body.insert("code".to_owned(), json!(error.http_status));
        body.insert("message".to_owned(), json!(error.message));
        body.insert("status".to_owned(), json!(status));
        encode(&json!({"error": body}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(body: Value) -> Result<ChatRequest, String> {
        GeminiClient::normalize_inbound(json!({
            "protocol":"google-gemini-generate-content", "principal":{"subject":"fixture"}, "headers":{"names":[],"values":{}},
            "transport_path":"/v1beta/models/test:generateContent", "body":body
        }).to_string()).map(|encoded| serde_json::from_str(&encoded).unwrap())
    }

    fn tool_body() -> Value {
        json!({"contents":[{"role":"user","parts":[{"text":"test"}]}],"tools":[{"functionDeclarations":[
            {"name":"read","parametersJsonSchema":{"type":"object"}},
            {"name":"write","parametersJsonSchema":{"type":"object"}},
            {"name":"delete","parametersJsonSchema":{"type":"object"}}
        ]}]})
    }

    #[test]
    fn structured_response_constraints_are_rejected_before_translation() {
        for generation in [
            json!({"responseSchema":{"type":"OBJECT"}}),
            json!({"responseJsonSchema":{"type":"object"}}),
            json!({"responseMimeType":"application/json"}),
            json!({"responseMimeType":"text/x.enum"}),
            json!({"responseMimeType":"text/plain","responseJsonSchema":{"type":"string"}}),
        ] {
            let mut body = tool_body();
            body["generationConfig"] = generation;
            let error: ErrorEnvelope =
                serde_json::from_str(&normalized(body).unwrap_err()).unwrap();
            assert_eq!(error.code, ErrorCode::Capability);
            assert_eq!(error.http_status, 400);
        }
        let mut body = tool_body();
        body["generationConfig"] = json!({"responseMimeType":"text/plain"});
        let request = normalized(body).unwrap();
        assert_eq!(request.tools.len(), 3);
        assert_eq!(request.tools[0].parameters, json!({"type":"object"}));
    }

    #[test]
    fn buffered_tool_calls_preserve_available_ids() {
        let message = Message {
            role: Role::Assistant,
            content: None,
            tool_calls: vec![
                ToolCall {
                    id: "call-one".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "call-two".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            ],
            tool_call_id: None,
            name: None,
            extensions: Extensions::new(),
        };
        let parts = response_parts(&message).unwrap();
        assert_eq!(parts[0]["functionCall"]["id"], "call-one");
        assert_eq!(parts[1]["functionCall"]["id"], "call-two");
    }

    #[test]
    fn streaming_tool_ids_survive_interleaved_fragments() {
        let context = json!({"stream_id":"ids-regression","model":"test"}).to_string();
        for event in [
            json!({"type":"tool_call_delta","index":0,"id":"call-one","name":"read","arguments_delta":"{\"n\":"}),
            json!({"type":"tool_call_delta","index":1,"id":"call-two","name":"read","arguments_delta":"{\"n\":"}),
            json!({"type":"tool_call_delta","index":1,"arguments_delta":"2}"}),
            json!({"type":"tool_call_delta","index":0,"arguments_delta":"1}"}),
        ] {
            GeminiClient::render_stream_event(event.to_string(), context.clone()).unwrap();
        }
        let output = GeminiClient::render_stream_event(
            json!({"type":"done","finish_reason":"tool_calls"}).to_string(),
            context,
        )
        .unwrap();
        let rendered: Value = serde_json::from_str(&output).unwrap();
        let body: Value = serde_json::from_str(
            rendered["data"]
                .as_str()
                .unwrap()
                .trim()
                .strip_prefix("data: ")
                .unwrap(),
        )
        .unwrap();
        let parts = &body["candidates"][0]["content"]["parts"];
        assert_eq!(
            parts[0]["functionCall"],
            json!({"id":"call-one","name":"read","args":{"n":1}})
        );
        assert_eq!(
            parts[1]["functionCall"],
            json!({"id":"call-two","name":"read","args":{"n":2}})
        );
    }

    #[test]
    fn legacy_schema_converts_types_limits_and_nullable_without_changing_enum_values() {
        let schema = function_parameters(&json!({"parameters":{
            "type":"OBJECT","properties":{
                "values":{"type":"ARRAY","minItems":"1","items":{"type":"INTEGER","minimum":0}},
                "mode":{"type":"STRING","enum":["FAST","SAFE"],"nullable":true},
                "choice":{"anyOf":[{"type":"BOOLEAN"},{"type":"NUMBER"}]}
            },"required":["values"],"additionalProperties":false
        }}))
        .unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["values"]["minItems"], 1);
        assert_eq!(schema["properties"]["values"]["items"]["type"], "integer");
        assert_eq!(
            schema["properties"]["mode"]["anyOf"][0]["enum"],
            json!(["FAST", "SAFE"])
        );
        assert_eq!(
            schema["properties"]["mode"]["anyOf"][1],
            json!({"type":"null"})
        );
        assert_eq!(
            schema["properties"]["choice"]["anyOf"][0]["type"],
            "boolean"
        );
        assert_eq!(schema["additionalProperties"], false);
        let native = json!({"type":"object","$defs":{"leaf":{"type":["string","null"]}},"properties":{"x":{"$ref":"#/$defs/leaf"}}});
        assert_eq!(
            function_parameters(&json!({"parametersJsonSchema":native})).unwrap(),
            native
        );
        assert_eq!(
            function_parameters(&json!({"parameters":native})).unwrap(),
            native
        );
    }

    #[test]
    fn unsupported_or_malformed_legacy_schema_is_not_forwarded_as_json_schema() {
        for schema in [
            json!({"type":"TYPE_UNSPECIFIED"}),
            json!({"type":"OBJECT","propertyOrdering":["x"]}),
            json!({"type":"ARRAY","items":false}),
            json!({"type":"STRING","nullable":"yes"}),
            json!({"type":"ARRAY","maxItems":"-1"}),
            json!({"type":"OBJECT","properties":{"x":{"type":"UNKNOWN"}}}),
        ] {
            let error = function_parameters(&json!({"parameters":schema}))
                .expect_err("unsupported legacy schema");
            assert_eq!(
                serde_json::from_str::<ErrorEnvelope>(&error)
                    .unwrap()
                    .http_status,
                400
            );
        }
    }

    #[test]
    fn function_config_maps_modes_and_restricts_single_or_multiple_allowed_names() {
        for (mode, expected) in [("NONE", "none"), ("AUTO", "auto"), ("ANY", "required")] {
            let mut body = tool_body();
            body["toolConfig"] = json!({"functionCallingConfig":{"mode":mode}});
            let request = normalized(body).unwrap();
            assert_eq!(serde_json::to_value(request.tool_choice).unwrap(), expected);
        }
        for names in [json!(["read"]), json!(["read", "write"])] {
            let mut body = tool_body();
            body["toolConfig"] =
                json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":names}});
            body["contents"].as_array_mut().unwrap().extend([
                json!({"role":"model","parts":[{"functionCall":{"id":"old-call","name":"delete","args":{}}}]}),
                json!({"role":"user","parts":[{"functionResponse":{"id":"old-call","name":"delete","response":{"ok":true}}}]})
            ]);
            let request = normalized(body).unwrap();
            assert_eq!(
                request
                    .tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
                names
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<Vec<_>>()
            );
            assert_eq!(request.messages[1].tool_calls[0].name, "delete");
            assert_eq!(
                request.messages[2].tool_call_id.as_deref(),
                Some("old-call")
            );
        }
    }

    #[test]
    fn unsupported_function_constraints_fail_before_normalization() {
        for config in [
            json!({"functionCallingConfig":{"mode":"VALIDATED"}}),
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["missing"]}}),
            json!({"functionCallingConfig":{"mode":"AUTO","allowedFunctionNames":["read"]}}),
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[]}}),
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["read","read"]}}),
            json!({"functionCallingConfig":{"mode":"ANY","futureConstraint":true}}),
            json!({"retrievalConfig":{}}),
        ] {
            let mut body = tool_body();
            body["toolConfig"] = config;
            let error = normalized(body).expect_err("constraint must not disappear");
            assert_eq!(
                serde_json::from_str::<ErrorEnvelope>(&error)
                    .unwrap()
                    .http_status,
                400
            );
        }
    }

    #[test]
    fn mixed_hosted_and_function_tool_groups_fail_closed() {
        let mut body = tool_body();
        body["tools"][0]["google_search"] = json!({});
        let error = normalized(body).expect_err("hosted semantics must not disappear");
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&error)
                .unwrap()
                .http_status,
            400
        );
    }

    #[test]
    fn function_responses_keep_their_explicit_call_id() {
        let messages = parse_messages(&json!({
            "contents": [
                {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "id": "call-read-1",
                            "name": "read_file",
                            "args": {"path": "hello.py"}
                        }
                    }]
                },
                {
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "id": "call-read-1",
                            "name": "read_file",
                            "response": {"content": "print('hello')"}
                        }
                    }]
                }
            ]
        }))
        .expect("Gemini tool history normalizes");

        assert_eq!(messages[0].tool_calls[0].id, "call-read-1");
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call-read-1"));
    }

    #[test]
    fn idless_function_responses_match_the_prior_unanswered_call_by_name() {
        let messages = parse_messages(&json!({
            "contents": [
                {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": {"path": "hello.py"}
                        }
                    }]
                },
                {
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": "read_file",
                            "response": {"content": "print('hello')"}
                        }
                    }]
                }
            ]
        }))
        .expect("Gemini idless tool history normalizes");

        assert_eq!(
            messages[1].tool_call_id.as_deref(),
            Some(messages[0].tool_calls[0].id.as_str())
        );
    }

    #[test]
    fn redacted_thinking_is_rejected_instead_of_dropped() {
        let event = json!({
            "type": "redacted_thinking",
            "index": 0,
            "block_index": 2,
            "data": "供应商原值+/=opaque"
        });
        let context = json!({
            "stream_id": "stream-redacted",
            "model": "gemini-test"
        });

        let error = GeminiClient::render_stream_event(event.to_string(), context.to_string())
            .expect_err("Gemini has no redacted-thinking wire slot");
        let error: ErrorEnvelope = serde_json::from_str(&error).expect("canonical error");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "gemini cannot render redacted thinking");
    }
}

export!(GeminiClient);
