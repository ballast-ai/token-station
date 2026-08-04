//! The official OpenAI-compatible inbound agent adapter.
//!
//! Northbound only: an agent tool's OpenAI-dialect request in, the Canonical
//! IR out, and Canonical responses rendered back into the dialect. It never
//! sees a credential — the envelope's headers arrive with credential values
//! already redacted, and the `agent-adapter-v1` world cannot even name one.
//!
//! The logic is deliberately the same as the native reference adapter in
//! `token-station-conformance`'s own tests. The fixture pack in `fixtures/`
//! pins both to identical outputs, so they cannot drift apart silently.

wit_bindgen::generate!({
    path: "../../../crates/plugin-api/wit",
    world: "agent-adapter-v1",
});

use exports::token_station::adapter::agent_adapter::{AdapterHealth, AdapterMetadata, Guest};
use serde_json::{json, Map, Value};
use token_station::adapter::common::{AdapterKind, HealthStatus};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, HintKind, Message, Role, Sampling, StreamEvent, ToolCall, ToolDef,
};

struct OpenAiClient;

/// The headers this adapter reads a routing hint out of.
const HINT_HEADERS: &[(&str, HintKind)] = &[
    ("x-agent-step", HintKind::StepType),
    ("x-agent-task", HintKind::TaskType),
    ("x-agent-preference", HintKind::Preference),
];

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

// -- translation ----------------------------------------------------------------

fn tool_calls_from_openai(raw: &Value) -> Vec<ToolCall> {
    raw.as_array()
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
        .collect()
}

fn tool_calls_to_openai(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments},
                })
            })
            .collect(),
    )
}

fn content_to_json(content: Option<&Content>) -> Value {
    match content {
        Some(Content::Text(text)) => json!(text),
        Some(Content::Parts(parts)) => {
            let text = parts.iter().fold(String::new(), |mut text, part| {
                if let ContentPart::Text { text: part } = part {
                    text.push_str(part);
                }
                text
            });
            if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            }
        }
        None => Value::Null,
    }
}

fn reasoning_to_json(content: Option<&Content>) -> Option<Value> {
    let Content::Parts(parts) = content? else {
        return None;
    };
    let reasoning = parts.iter().fold(String::new(), |mut reasoning, part| {
        if let ContentPart::Thinking { thinking, .. } = part {
            reasoning.push_str(thinking);
        }
        reasoning
    });
    (!reasoning.is_empty()).then(|| json!(reasoning))
}

fn validate_response_format(body: &Value) -> Result<(), String> {
    let Some(format) = body.get("response_format").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let format = format
        .as_object()
        .ok_or_else(|| invalid("response_format must be an object"))?;
    let kind = format
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("response_format declares no string type"))?;

    match kind {
        "text" => Ok(()),
        "json_schema" | "json_object" => Err(capability(
            "Chat Completions structured output requires an approved Canonical IR/provider mapping",
        )),
        kind => Err(capability(format!(
            "unsupported Chat Completions response_format type {kind}"
        ))),
    }
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
        let body = if envelope.agent_tool.as_deref() == Some("cursor") {
            normalize_cursor_body(&envelope.body)?
        } else {
            envelope.body.clone()
        };
        validate_response_format(&body)?;

        let model = body["model"]
            .as_str()
            .ok_or_else(|| invalid("request declares no model"))?
            .to_owned();

        let mut messages = Vec::new();
        for raw in body["messages"].as_array().into_iter().flatten() {
            let role = match raw["role"].as_str() {
                Some("system") => Role::System,
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                Some("tool") => Role::Tool,
                _ => return Err(invalid("message declares no known role")),
            };
            let content = match &raw["content"] {
                Value::String(text) => Some(Content::Text(text.clone())),
                parts @ Value::Array(_) => {
                    let parsed: Vec<ContentPart> =
                        serde_json::from_value(parts.clone()).map_err(internal)?;
                    Some(Content::Parts(parsed))
                }
                _ => None,
            };
            messages.push(Message {
                role,
                content,
                tool_calls: tool_calls_from_openai(&raw["tool_calls"]),
                tool_call_id: raw["tool_call_id"].as_str().map(str::to_owned),
                name: raw["name"].as_str().map(str::to_owned),
                extensions: Extensions::new(),
            });
        }

        let tools = body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| ToolDef {
                name: tool["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                description: tool["function"]["description"].as_str().map(str::to_owned),
                parameters: tool["function"]["parameters"].clone(),
            })
            .collect();

        let mut extensions = Extensions::new();
        if let Some(effort) = body.get("reasoning_effort").and_then(Value::as_str) {
            extensions.insert("reasoning_effort".to_owned(), json!(effort));
        }
        if let Some(parallel) = body.get("parallel_tool_calls").and_then(Value::as_bool) {
            extensions.insert("parallel_tool_calls".to_owned(), json!(parallel));
        }

        to_output(&ChatRequest {
            model,
            messages,
            tools,
            response_format: None,
            tool_choice: body
                .get("tool_choice")
                .map(|v| serde_json::from_value(v.clone()).expect("ToolChoice accepts any value")),
            sampling: Sampling {
                temperature: body["temperature"].as_f64(),
                top_p: body["top_p"].as_f64(),
                max_output_tokens: body["max_tokens"]
                    .as_u64()
                    .or_else(|| body["max_completion_tokens"].as_u64())
                    .and_then(|value| u32::try_from(value).ok()),
                stop: body["stop"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect(),
            },
            stream: body["stream"].as_bool().unwrap_or(false),
            extensions,
        })
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

    fn render_response(response: String, _context: String) -> Result<String, String> {
        let response: ChatResponse = parse_input(&response)?;

        let choices: Vec<Value> = response
            .choices
            .iter()
            .map(|choice| {
                let mut message = Map::new();
                message.insert("role".to_owned(), json!(choice.message.role));
                message.insert(
                    "content".to_owned(),
                    content_to_json(choice.message.content.as_ref()),
                );
                if let Some(reasoning) = reasoning_to_json(choice.message.content.as_ref()) {
                    message.insert("reasoning_content".to_owned(), reasoning);
                }
                if !choice.message.tool_calls.is_empty() {
                    message.insert(
                        "tool_calls".to_owned(),
                        tool_calls_to_openai(&choice.message.tool_calls),
                    );
                }
                json!({
                    "index": choice.index,
                    "message": Value::Object(message),
                    "finish_reason": choice.finish_reason,
                })
            })
            .collect();

        to_output(&json!({
            "id": response.id,
            "object": "chat.completion",
            "model": response.model,
            "choices": choices,
            "usage": {
                "prompt_tokens": response.usage.input_tokens,
                "completion_tokens": response.usage.output_tokens,
                "total_tokens": response.usage.total(),
            },
        }))
    }

    fn render_stream_event(event: String, _context: String) -> Result<String, String> {
        let event: StreamEvent = parse_input(&event)?;

        // Templated rather than serialized: the entry protocol's key order is
        // part of its wire format, and a `Map` would sort it.
        let data = match &event {
            StreamEvent::Delta { index, content } => format!(
                "data: {{\"choices\":[{{\"index\":{index},\"delta\":{{\"content\":{}}}}}]}}\n\n",
                serde_json::to_string(content).map_err(internal)?
            ),
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let mut function = format!(
                    "\"arguments\":{}",
                    serde_json::to_string(arguments_delta).map_err(internal)?
                );
                if let Some(name) = name {
                    function = format!(
                        "\"name\":{},{function}",
                        serde_json::to_string(name).map_err(internal)?
                    );
                }
                let identity = match id {
                    Some(id) => {
                        format!("\"id\":{},", serde_json::to_string(id).map_err(internal)?)
                    }
                    None => String::new(),
                };
                format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":\
                     [{{\"index\":{index},{identity}\"function\":{{{function}}}}}]}}}}]}}\n\n"
                )
            }
            StreamEvent::Usage { usage } => format!(
                "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":{}}}}}\n\n",
                usage.input_tokens, usage.output_tokens
            ),
            // 0.3.0: reasoning deltas ride the openai-compat
            // `delta.reasoning_content` slot; the signature fragment has no
            // openai wire slot and renders nothing.
            StreamEvent::ThinkingDelta {
                index,
                thinking_delta,
            } => format!(
                "data: {{\"choices\":[{{\"index\":{index},\"delta\":{{\"reasoning_content\":{}}}}}]}}\n\n",
                serde_json::to_string(thinking_delta).map_err(internal)?
            ),
            StreamEvent::ThinkingSignatureDelta { .. } => String::new(),
            StreamEvent::Done {
                finish_reason,
                // openai chat SSE has no stop-sequence slot to render into.
                stop_sequence: _,
            } => {
                let reason = match finish_reason {
                    Some(reason) => serde_json::to_string(reason).map_err(internal)?,
                    None => "null".to_owned(),
                };
                format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":{reason}}}]}}\
                     \n\ndata: [DONE]\n\n"
                )
            }
            StreamEvent::Error { error } => {
                let rendered = Self::map_inbound_error(to_output(error)?, String::new())?;
                format!("data: {rendered}\n\n")
            }
        };

        to_output(&json!({ "data": data }))
    }

    fn map_inbound_error(error: String, _context: String) -> Result<String, String> {
        let error: ErrorEnvelope = parse_input(&error)?;
        let code = serde_json::to_value(error.code).map_err(internal)?;
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

    #[test]
    fn chat_completion_content_is_always_text_or_null() {
        let mixed = Content::Parts(vec![
            ContentPart::Thinking {
                thinking: "private reasoning".to_owned(),
                signature: None,
            },
            ContentPart::Text {
                text: "Hello".to_owned(),
            },
            ContentPart::Text {
                text: ", world".to_owned(),
            },
        ]);
        assert_eq!(content_to_json(Some(&mixed)), json!("Hello, world"));
        assert_eq!(
            reasoning_to_json(Some(&mixed)),
            Some(json!("private reasoning"))
        );

        let reasoning_only = Content::Parts(vec![ContentPart::Thinking {
            thinking: "private reasoning".to_owned(),
            signature: None,
        }]);
        assert_eq!(content_to_json(Some(&reasoning_only)), Value::Null);
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
}

export!(OpenAiClient);
