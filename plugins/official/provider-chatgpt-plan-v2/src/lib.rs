//! ChatGPT plan Responses translation on the South-owned v2 ABI.
//!
//! This component has no network or credential access. The Host supplies a
//! registration-bound credential through the declared slot. Sampling limits
//! that the plan endpoint does not accept are deliberately not sent.

use serde_json::{json, Value};
use south_component_conformance::{ComponentResultV1, ProviderComponentV1, StreamParserV1};
use south_provider_api::{ComponentMetadataV1, PROVIDER_WORLD};
use std::collections::{BTreeMap, BTreeSet};
use token_station_protocol::{
    Auth, ChatRequest, ChatResponse, Choice, Content, ContentPart, ErrorCode, ErrorEnvelope,
    Extensions, FinishReason, HttpMethod, HttpRequestDescriptor, HttpResponseParts, Message,
    ModelCapability, ProviderApi, ProviderConfig, Role, SafeHeaders, StreamEvent, ToolCall,
    ToolChoice, Usage,
};

mod codex;
#[cfg(target_arch = "wasm32")]
mod shell;
mod stream;

pub struct ChatGptPlan;
pub const TOOL_NAMESPACE: &str = "tokenstation_tools";
const MAX_VALUE_BYTES: usize = 4 * 1024 * 1024;

fn capability(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}

fn protocol(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

fn check_config(config: &ProviderConfig) -> ComponentResultV1<()> {
    if config.provider == "codex-plan" {
        return codex::check_config(config);
    }
    let endpoint = config.base_url.as_str();
    // Host account bindings remain fixed to OpenAI. Loopback is reserved for
    // isolated conformance tests with a synthetic store credential.
    let loopback_fixture = config.base_url.is_loopback()
        && endpoint
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/'))
            .is_some_and(|(_, path)| path == "v1");
    if config.provider != "chatgpt-plan"
        || (endpoint != "https://api.openai.com/v1" && !loopback_fixture)
    {
        return Err(capability(
            "ChatGPT plan requests require the fixed official Responses endpoint",
        ));
    }
    if config.auth.as_ref().map(|a| a.as_str()) != Some("chatgpt_access_token") {
        return Err(ErrorEnvelope::new(
            ErrorCode::Auth,
            401,
            "ChatGPT plan requests require a registered account credential",
        ));
    }
    Ok(())
}

fn content_parts(content: &Content, role: Role) -> ComponentResultV1<Vec<Value>> {
    let text_type = if role == Role::Assistant {
        "output_text"
    } else {
        "input_text"
    };
    match content {
        Content::Text(text) => Ok(vec![json!({"type":text_type,"text":text})]),
        Content::Parts(parts) => parts.iter().filter(|part| {
            // Unsigned assistant thinking is a display summary from another
            // Agent. It is not verifiable model state or executable content.
            !matches!(part, ContentPart::Thinking { signature: None, .. } if role == Role::Assistant)
        }).map(|part| match part {
            ContentPart::Text { text } => Ok(json!({"type":text_type,"text":text})),
            ContentPart::ImageUrl { image_url } if role != Role::Assistant && role != Role::System => {
                if !(image_url.url.starts_with("https://") || image_url.url.starts_with("data:image/")) {
                    return Err(capability("ChatGPT plan images require HTTPS or an inline image data URL"));
                }
                Ok(json!({"type":"input_image","image_url":image_url.url,"detail":image_url.detail.as_deref().unwrap_or("auto")}))
            }
            _ => Err(capability("ChatGPT plan cannot translate this content block without losing its meaning")),
        }).collect(),
    }
}

fn build_body(r: &ChatRequest) -> ComponentResultV1<Value> {
    if !r.sampling.stop.is_empty() {
        return Err(capability("ChatGPT plan does not support stop sequences"));
    }
    // Unknown extension fields remain forward compatible. Known semantic
    // fields that this transport cannot honor fail closed.
    for key in [
        "previous_response_id",
        "conversation",
        "additional_tools",
        "native_tools",
        "hosted_tools",
        "web_search",
        "anthropic_server_tools",
    ] {
        if r.extensions
            .get(key)
            .is_some_and(|v| !v.is_null() && v != &json!(false))
        {
            return Err(capability(
                "ChatGPT plan cannot translate the requested continuation or hosted tool",
            ));
        }
    }
    let mut input = Vec::new();
    let mut instructions = Vec::new();
    let mut calls = BTreeSet::new();
    let mut answered = BTreeSet::new();
    for message in &r.messages {
        if message.role == Role::System {
            if !message.tool_calls.is_empty() || message.tool_call_id.is_some() {
                return Err(capability("System instructions cannot contain tool calls"));
            }
            if let Some(content) = &message.content {
                for part in content_parts(content, Role::System)? {
                    instructions.push(part["text"].as_str().unwrap_or_default().to_owned());
                }
            }
            continue;
        }
        if message.role == Role::Tool {
            let id = message
                .tool_call_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| capability("A tool result requires its original call ID"))?;
            if !calls.contains(id)
                || !answered.insert(id.to_owned())
                || !message.tool_calls.is_empty()
            {
                return Err(capability(
                    "A tool result requires one preceding matching tool call",
                ));
            }
            let output = match &message.content {
                Some(Content::Text(text)) => json!(text),
                Some(content) => json!(content_parts(content, Role::Tool)?),
                None => json!(""),
            };
            input.push(json!({"type":"function_call_output","call_id":id,"output":output}));
            continue;
        }
        if message.tool_call_id.is_some()
            || (message.role != Role::Assistant && !message.tool_calls.is_empty())
        {
            return Err(capability(
                "Only an assistant message can contain tool calls",
            ));
        }
        if let Some(content) = &message.content {
            let visible = content_parts(content, message.role)?;
            if !visible.is_empty() {
                input.push(json!({"type":"message","role": if message.role == Role::Assistant {"assistant"} else {"user"},"content":visible}));
            }
        }
        for call in &message.tool_calls {
            if call.id.is_empty() || call.name.is_empty() || !calls.insert(call.id.clone()) {
                return Err(capability(
                    "Tool calls require unique IDs and nonempty names",
                ));
            }
            let _: Value = serde_json::from_str(&call.arguments)
                .map_err(|_| capability("Historical tool arguments must contain complete JSON"))?;
            input.push(json!({"type":"function_call","namespace":TOOL_NAMESPACE,"name":call.name,"call_id":call.id,"arguments":call.arguments}));
        }
    }
    if calls != answered {
        return Err(capability(
            "Each historical tool call requires its result before the next inference",
        ));
    }
    let forced = match &r.tool_choice {
        Some(ToolChoice::Other(value)) => {
            if value.as_object().is_none_or(|object| {
                object
                    .keys()
                    .any(|key| !["type", "name", "function"].contains(&key.as_str()))
            }) {
                return Err(capability(
                    "ChatGPT plan cannot preserve this tool selection constraint",
                ));
            }
            let typ = value["type"].as_str().unwrap_or_default();
            let name = match typ {
                "function" => value["function"]["name"]
                    .as_str()
                    .or_else(|| value["name"].as_str()),
                "tool" => value["name"].as_str(),
                _ => None,
            }
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                capability("ChatGPT plan supports only named function tool selection")
            })?;
            if value.get("function").is_some_and(|function| {
                function
                    .as_object()
                    .is_none_or(|object| object.keys().any(|key| key != "name"))
            }) {
                return Err(capability(
                    "ChatGPT plan cannot preserve this function selection constraint",
                ));
            }
            Some(name)
        }
        _ => None,
    };
    let mut names = BTreeSet::new();
    let mut tools = Vec::new();
    for tool in &r.tools {
        if tool.name.is_empty() || !names.insert(tool.name.as_str()) || !tool.parameters.is_object()
        {
            return Err(capability(
                "Function tools require unique names and object schemas",
            ));
        }
        if forced.is_some_and(|name| name != tool.name) {
            continue;
        }
        tools.push(json!({"type":"function","name":tool.name,"description":tool.description.as_deref().unwrap_or(""),"parameters":tool.parameters,"strict":false}));
    }
    if forced.is_some_and(|name| !names.contains(name))
        || (r.tools.is_empty() && matches!(r.tool_choice, Some(ToolChoice::Required)))
    {
        return Err(capability(
            "The required function is not available in this request",
        ));
    }
    let mut body = json!({"model":r.model,"input":input,"stream":true,"store":false});
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    if !tools.is_empty() {
        body["tools"] = json!([{"type":"namespace","name":TOOL_NAMESPACE,"description":"Tools executed by the requesting local agent.","tools":tools}]);
    }
    if let Some(choice) = &r.tool_choice {
        body["tool_choice"] = match choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Required | ToolChoice::Other(_) => json!("required"),
        };
    }
    if let Some(parallel) = r.extensions.get("parallel_tool_calls") {
        if !parallel.is_boolean() {
            return Err(capability("parallel_tool_calls must be a boolean"));
        }
        body["parallel_tool_calls"] = parallel.clone();
    }
    if let Some(effort) = r.extensions.get("reasoning_effort") {
        let effort = effort
            .as_str()
            .ok_or_else(|| capability("reasoning_effort must be a string"))?;
        body["reasoning"] = json!({"effort":effort});
    }
    if let Some(format) = &r.response_format {
        use token_station_protocol::ResponseFormat;
        body["text"] = match format {
            ResponseFormat::Text => json!({"format":{"type":"text"}}),
            ResponseFormat::JsonObject => json!({"format":{"type":"json_object"}}),
            ResponseFormat::JsonSchema { json_schema } => {
                let mut format = json_schema
                    .as_object()
                    .cloned()
                    .ok_or_else(|| capability("JSON schema format must be an object"))?;
                format.insert("type".into(), json!("json_schema"));
                json!({"format":format})
            }
        };
    }
    Ok(body)
}

// Match machine-readable fields only; upstream text can reflect private inputs.
fn reasoning_rejection(error: &Value) -> Option<ErrorEnvelope> {
    (error["code"].as_str() == Some("unsupported_value")
        && error["param"].as_str() == Some("reasoning.effort"))
    .then(|| ErrorEnvelope::new(ErrorCode::InvalidRequest, 400,
        "Unsupported value for reasoning_effort. The selected model does not support this reasoning effort."))
}

impl ProviderComponentV1 for ChatGptPlan {
    fn metadata(&self) -> ComponentMetadataV1 {
        ComponentMetadataV1 {
            name: "provider-chatgpt-plan".into(),
            version: "2.1.0".into(),
            api_version: PROVIDER_WORLD.into(),
        }
    }
    fn model_capabilities(
        &self,
        config: &ProviderConfig,
    ) -> ComponentResultV1<Vec<ModelCapability>> {
        Ok(config.models.clone())
    }
    fn build_http_request(
        &self,
        r: &ChatRequest,
        config: &ProviderConfig,
    ) -> ComponentResultV1<HttpRequestDescriptor> {
        check_config(config)?;
        let mut descriptor = HttpRequestDescriptor::new(
            HttpMethod::Post,
            config.base_url.resolve(ProviderApi::Responses),
        );
        descriptor.headers = if config.provider == "codex-plan" {
            codex::headers(config)?
        } else {
            SafeHeaders::try_new([
                ("content-type", "application/json"),
                ("accept", "text/event-stream"),
            ])
            .map_err(|_| protocol("Cannot build safe request headers"))?
        };
        descriptor.auth = config.auth.clone().map(Auth::bearer);
        let body = build_body(r)?;
        descriptor.body = Some(if config.provider == "codex-plan" {
            codex::body(body)?
        } else {
            body
        });
        Ok(descriptor)
    }
    fn parse_response(&self, parts: &HttpResponseParts) -> ComponentResultV1<ChatResponse> {
        if !(200..300).contains(&parts.status) {
            return Err(self.map_provider_error(parts)?);
        }
        // A buffered client still receives an upstream SSE response. Use the
        // identical parser, including its terminal and failure checks.
        let mut parser = stream::ResponsesStream::default();
        let mut events = parser.parse_chunk(parts.body.as_bytes())?;
        events.extend(parser.finish()?);
        let mut message = Message::text(Role::Assistant, "");
        let mut text = String::new();
        let mut tools: BTreeMap<u32, ToolCall> = BTreeMap::new();
        let mut usage = Usage::default();
        let mut reason = None;
        for event in events {
            match event {
                StreamEvent::Delta { content, .. } => text.push_str(&content),
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    let call = tools.entry(index).or_insert(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });
                    if let Some(id) = id {
                        call.id = id;
                    }
                    if let Some(name) = name {
                        call.name = name;
                    }
                    call.arguments.push_str(&arguments_delta);
                }
                StreamEvent::Usage { usage: report } => usage.absorb(report),
                StreamEvent::Finish { finish_reason, .. } => reason = finish_reason,
                StreamEvent::Error { error } => return Err(error),
                _ => {}
            }
        }
        message.content = Some(Content::Text(text));
        message.tool_calls = tools.into_values().collect();
        Ok(ChatResponse {
            id: parser.id,
            model: parser.model,
            choices: vec![Choice {
                index: 0,
                message,
                finish_reason: reason,
                stop_sequence: None,
            }],
            usage,
            extensions: Extensions::new(),
        })
    }
    fn map_provider_error(&self, parts: &HttpResponseParts) -> ComponentResultV1<ErrorEnvelope> {
        // Never copy upstream text: it can contain reflected user content or
        // credentials. Stable messages are sufficient for retry decisions.
        let raw: Value = serde_json::from_str(&parts.body).unwrap_or(Value::Null);
        let code = match raw["error"]["code"].as_str().unwrap_or_default() {
            "context_length_exceeded" => ErrorCode::ContextLength,
            "content_policy_violation" => ErrorCode::ContentPolicy,
            "insufficient_quota" | "usage_limit_reached" => ErrorCode::PaymentRequired,
            _ => match parts.status {
                401 | 403 => ErrorCode::Auth,
                402 => ErrorCode::PaymentRequired,
                429 => ErrorCode::RateLimit,
                408 => ErrorCode::Timeout,
                500..=599 => ErrorCode::UpstreamUnavailable,
                400 | 404 | 422 => ErrorCode::InvalidRequest,
                _ => ErrorCode::ProviderProtocolError,
            },
        };
        let message = match code {
            ErrorCode::Auth => "ChatGPT account authorization was rejected. Sign in again.",
            ErrorCode::PaymentRequired => "ChatGPT plan allowance is unavailable.",
            ErrorCode::RateLimit => "ChatGPT plan requests are rate limited.",
            ErrorCode::ContextLength => "The request exceeds the model context window.",
            ErrorCode::ContentPolicy => "The provider rejected the content.",
            ErrorCode::Timeout => "The ChatGPT plan request timed out.",
            ErrorCode::InvalidRequest => "The ChatGPT plan endpoint rejected the request.",
            _ => "The ChatGPT plan endpoint failed.",
        };
        let mut error = if parts.status == 400 {
            reasoning_rejection(&raw["error"])
        } else {
            None
        }
        .unwrap_or_else(|| ErrorEnvelope::new(code, parts.status, message));
        error.retry_after_ms = parts
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, value)| value.trim().parse::<u64>().ok())
            .and_then(|seconds| seconds.checked_mul(1000));
        Ok(error)
    }
    fn stream_parser(&self) -> Box<dyn StreamParserV1> {
        Box::new(stream::ResponsesStream::default())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn vendored_wit_matches_the_pinned_south_contract() {
        assert_eq!(
            include_str!("../wit/provider-adapter.wit"),
            south_provider_api::ADAPTER_WIT
        );
    }
}
