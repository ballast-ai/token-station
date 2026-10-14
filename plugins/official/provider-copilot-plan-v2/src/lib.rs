//! Copilot subscription transport on the South ABI. The host resolves credentials and performs I/O.
mod responses;
use responses::ResponsesCodec;
use serde_json::{json, Value};
use south_component_conformance::{
    reference::OpenAiCompatibleReferenceV1, reference_anthropic::AnthropicReferenceV1,
};
use south_component_conformance::{ComponentResultV1, ProviderComponentV1, StreamParserV1};
use south_provider_api::{ComponentMetadataV1, PROVIDER_WORLD};
use token_station_protocol::{
    Auth, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode, ErrorEnvelope,
    HttpRequestDescriptor, HttpResponseParts, ModelCapability, ProviderConfig, Role, SafeHeaders,
    StreamEvent,
};
#[cfg(target_arch = "wasm32")]
mod shell;
pub struct CopilotPlan;
fn capability(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}
fn protocol(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}
fn valid_base(base: &str) -> bool {
    matches!(
        base,
        "https://api.githubcopilot.com"
            | "https://api.individual.githubcopilot.com"
            | "https://api.business.githubcopilot.com"
            | "https://api.enterprise.githubcopilot.com"
    )
}
fn endpoint<'a>(request: &ChatRequest, config: &'a ProviderConfig) -> ComponentResultV1<&'a str> {
    if config.provider != "copilot-plan"
        || !valid_base(&config.base_url.as_str())
        || config.auth.as_ref().map(|a| a.as_str()) != Some("local_subscription_token")
    {
        return Err(capability(
            "Copilot requires a bound subscription and an official API destination",
        ));
    }
    let identity = &config.extensions;
    if !matches!(
        identity.get("copilot_source_kind").and_then(Value::as_str),
        Some("editor" | "cli")
    ) {
        return Err(capability("Copilot local credential source is invalid"));
    }
    let paths = identity
        .get("copilot_model_endpoints")
        .and_then(|models| models.get(&request.model))
        .and_then(Value::as_array)
        .ok_or_else(|| capability("The selected model is absent from the bound Copilot catalog"))?;
    // Choose the richest observed dialect. Signed Anthropic state requires Messages.
    let has_state=request.messages.iter().any(|m|matches!(&m.content,Some(Content::Parts(p)) if p.iter().any(|p|matches!(p,ContentPart::Thinking{..}|ContentPart::RedactedThinking{..}))));
    for preferred in if has_state {
        vec!["/v1/messages"]
    } else {
        vec!["/responses", "/v1/messages", "/chat/completions"]
    } {
        if paths.iter().any(|v| v.as_str() == Some(preferred))
            && check_semantics(request, preferred).is_ok()
        {
            return Ok(preferred);
        }
    }
    Err(capability(
        "Copilot cannot preserve this request on the observed model endpoints",
    ))
}
fn check_semantics(r: &ChatRequest, path: &str) -> ComponentResultV1<()> {
    if path == "/responses" && !r.sampling.stop.is_empty() {
        return Err(capability(
            "Copilot Responses does not support stop sequences",
        ));
    }
    for key in [
        "previous_response_id",
        "conversation",
        "native_tools",
        "hosted_tools",
        "additional_tools",
        "anthropic_server_tools",
        "web_search",
        "thinking",
        "anthropic_thinking",
    ] {
        if r.extensions
            .get(key)
            .is_some_and(|v| !v.is_null() && v != &json!(false))
        {
            return Err(capability(
                "Copilot cannot preserve the requested continuation or hosted tool",
            ));
        }
    }
    if path == "/v1/messages" && r.response_format.is_some() {
        return Err(capability(
            "Copilot Messages cannot preserve the requested response format",
        ));
    }
    if path == "/v1/messages"
        && [
            "reasoning_effort",
            "parallel_tool_calls",
            "responses_tool_strict",
        ]
        .iter()
        .any(|key| r.extensions.contains_key(*key))
    {
        return Err(capability(
            "Copilot Messages cannot preserve the requested reasoning or tool controls",
        ));
    }
    for message in &r.messages {
        if message
            .extensions
            .contains_key("responses_reasoning_encrypted_content")
        {
            return Err(capability(
                "Copilot cannot preserve opaque Responses reasoning state",
            ));
        }
        if path == "/v1/messages" && message.name.is_some() {
            return Err(capability(
                "Copilot Messages cannot preserve a named message",
            ));
        }
        if let Some(Content::Parts(parts)) = &message.content {
            for part in parts {
                match part {
                    ContentPart::Unknown(_) => {
                        return Err(capability(
                            "Copilot cannot preserve an unknown content block",
                        ))
                    }
                    part if path == "/v1/messages"
                        && matches!(message.role, Role::Tool | Role::System)
                        && !matches!(part, ContentPart::Text { .. }) =>
                    {
                        return Err(capability(
                            "Copilot Messages requires text system instructions and tool results",
                        ))
                    }
                    ContentPart::Thinking { .. } | ContentPart::RedactedThinking { .. }
                        if path != "/v1/messages" =>
                    {
                        return Err(capability(
                            "This Copilot endpoint cannot preserve signed thinking",
                        ))
                    }
                    ContentPart::ImageUrl { image_url }
                        if !(image_url.url.starts_with("https://")
                            || image_url.url.starts_with("data:image/")) =>
                    {
                        return Err(capability(
                            "Copilot images require HTTPS or inline image data",
                        ))
                    }
                    _ => (),
                }
            }
        }
    }
    Ok(())
}
fn prepare(
    r: &ChatRequest,
    config: &ProviderConfig,
    path: &str,
) -> ComponentResultV1<HttpRequestDescriptor> {
    let mut native = config.clone();
    native.provider = match path {
        "/v1/messages" => "anthropic",
        "/responses" => "chatgpt-plan",
        _ => "openai-compatible",
    }
    .into();
    let mut descriptor = if path == "/responses" {
        native.base_url = serde_json::from_value(json!("https://api.openai.com/v1"))
            .map_err(|_| protocol("Invalid reference endpoint"))?;
        native.auth = Some(
            serde_json::from_value(json!("chatgpt_access_token"))
                .map_err(|_| protocol("Invalid reference credential slot"))?,
        );
        let mut d = ResponsesCodec.build_http_request(r, &native)?;
        if let Some(body) = d.body.as_mut() {
            if let Some(groups) = body["tools"].as_array() {
                let mut tools = Vec::new();
                for group in groups {
                    if let Some(inner) = group["tools"].as_array() {
                        tools.extend(inner.iter().cloned());
                    }
                }
                body["tools"] = json!(tools);
            }
            if let Some(items) = body["input"].as_array_mut() {
                for item in items {
                    if let Some(o) = item.as_object_mut() {
                        o.remove("namespace");
                    }
                }
            }
            for (key, value) in [
                ("temperature", r.sampling.temperature.map(|v| json!(v))),
                ("top_p", r.sampling.top_p.map(|v| json!(v))),
                (
                    "max_output_tokens",
                    r.sampling.max_output_tokens.map(|v| json!(v)),
                ),
            ] {
                if let Some(value) = value {
                    body[key] = value;
                }
            }
        }
        d
    } else if path == "/v1/messages" {
        let mut mapped = r.clone();
        if let Some(token_station_protocol::ToolChoice::Other(choice)) = &r.tool_choice {
            let object = choice
                .as_object()
                .ok_or_else(|| capability("Copilot requires an object tool selection"))?;
            if object
                .keys()
                .any(|k| !["type", "name", "function"].contains(&k.as_str()))
            {
                return Err(capability(
                    "Copilot cannot preserve this tool selection constraint",
                ));
            }
            if !matches!(choice["type"].as_str(), Some("tool" | "function")) {
                return Err(capability(
                    "Copilot supports only named function tool selection",
                ));
            }
            if choice
                .get("function")
                .is_some_and(|f| f.as_object().is_none_or(|o| o.keys().any(|k| k != "name")))
            {
                return Err(capability(
                    "Copilot cannot preserve this function selection constraint",
                ));
            }
            let name = choice["function"]["name"]
                .as_str()
                .or_else(|| choice["name"].as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| capability("Copilot tool selection requires a name"))?;
            if !r.tools.iter().any(|t| t.name == name) {
                return Err(capability(
                    "The selected Copilot tool is absent from the request",
                ));
            }
            mapped.tool_choice = Some(token_station_protocol::ToolChoice::Other(
                json!({"type":"function","function":{"name":name}}),
            ));
        }
        AnthropicReferenceV1.build_http_request(&mapped, &native)?
    } else {
        OpenAiCompatibleReferenceV1.build_http_request(r, &native)?
    };
    if path == "/chat/completions"
        && r.extensions.get("reasoning_effort").is_some_and(|value| {
            descriptor
                .body
                .as_ref()
                .and_then(|body| body.get("reasoning_effort"))
                != Some(value)
        })
    {
        return Err(capability(
            "The Copilot model does not declare the requested reasoning effort",
        ));
    }
    descriptor.url = format!("{}{path}", config.base_url.as_str());
    descriptor.auth = config.auth.clone().map(Auth::bearer);
    let mut headers: Vec<(String, String)> = descriptor
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let cli = config.extensions["copilot_source_kind"] == "cli";
    headers.extend(
        if cli {
            vec![
                ("editor-version", "copilot/1.0.88"),
                ("copilot-integration-id", "copilot-developer-cli"),
                ("x-github-api-version", "2026-07-01"),
                ("user-agent", "copilot/1.0.88"),
            ]
        } else {
            vec![
                ("editor-version", "vscode/1.140.0"),
                ("editor-plugin-version", "copilot-chat/0.68.0"),
                ("copilot-integration-id", "vscode-chat"),
                ("x-github-api-version", "2026-01-09"),
                ("user-agent", "GitHubCopilotChat/0.68.0"),
            ]
        }
        .into_iter()
        .map(|(k, v)| (k.into(), v.into())),
    );
    let role = r.messages.last();
    let user = role.is_some_and(|m| m.role == Role::User);
    headers.push((
        "x-initiator".into(),
        if user { "user" } else { "agent" }.into(),
    ));
    headers.push(("openai-intent".into(), "conversation-panel".into()));
    if r.messages.iter().any(|m|matches!(&m.content,Some(Content::Parts(p)) if p.iter().any(|p|matches!(p,ContentPart::ImageUrl{..})))){headers.push(("copilot-vision-request".into(),"true".into()));}
    descriptor.headers =
        SafeHeaders::try_new(headers).map_err(|_| protocol("Cannot build safe Copilot headers"))?;
    Ok(descriptor)
}
impl ProviderComponentV1 for CopilotPlan {
    fn metadata(&self) -> ComponentMetadataV1 {
        ComponentMetadataV1 {
            name: "provider-copilot-plan".into(),
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
        let path = endpoint(r, config)?;
        check_semantics(r, path)?;
        prepare(r, config, path)
    }
    fn parse_response(&self, parts: &HttpResponseParts) -> ComponentResultV1<ChatResponse> {
        if !(200..300).contains(&parts.status) {
            return Err(self.map_provider_error(parts)?);
        }
        if parts.body.trim_start().starts_with("event:")
            || parts.body.trim_start().starts_with("data:")
        {
            return ResponsesCodec.parse_response(parts);
        }
        let value: Value = serde_json::from_str(&parts.body)
            .map_err(|_| protocol("Copilot returned invalid JSON"))?;
        if value["type"] == "message" {
            if value["content"].as_array().is_none_or(|blocks| {
                blocks.iter().any(|block| {
                    !matches!(
                        block["type"].as_str(),
                        Some("text" | "thinking" | "redacted_thinking" | "tool_use")
                    )
                })
            }) {
                return Err(capability(
                    "Copilot cannot preserve this buffered Messages content block",
                ));
            }
            AnthropicReferenceV1.parse_response(parts)
        } else if value.get("choices").is_some() {
            OpenAiCompatibleReferenceV1.parse_response(parts)
        } else {
            Err(protocol("Copilot returned an unknown response dialect"))
        }
    }
    fn map_provider_error(&self, parts: &HttpResponseParts) -> ComponentResultV1<ErrorEnvelope> {
        let code = match parts.status {
            401 | 403 => ErrorCode::Auth,
            402 => ErrorCode::PaymentRequired,
            429 => ErrorCode::RateLimit,
            408 => ErrorCode::Timeout,
            400 | 404 | 422 => ErrorCode::InvalidRequest,
            500..=599 => ErrorCode::UpstreamUnavailable,
            _ => ErrorCode::ProviderProtocolError,
        };
        let mut e = ErrorEnvelope::new(
            code,
            parts.status,
            match code {
                ErrorCode::Auth => "Copilot authorization was rejected. Check the local login.",
                ErrorCode::PaymentRequired => "Copilot plan allowance is unavailable.",
                ErrorCode::RateLimit => "Copilot requests are rate limited.",
                _ => "The Copilot subscription endpoint rejected the request.",
            },
        );
        e.retry_after_ms = parts
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .and_then(|v| v.checked_mul(1000));
        Ok(e)
    }
    fn stream_parser(&self) -> Box<dyn StreamParserV1> {
        Box::new(DialectStream {
            pending: Vec::new(),
            parser: None,
            done: false,
        })
    }
}
struct DialectStream {
    pending: Vec<u8>,
    parser: Option<Box<dyn StreamParserV1>>,
    done: bool,
}
impl StreamParserV1 for DialectStream {
    fn parse_chunk(&mut self, chunk: &[u8]) -> ComponentResultV1<Vec<StreamEvent>> {
        if chunk.is_empty() {
            if self.parser.is_none() && self.pending.is_empty() {
                return Ok(Vec::new());
            }
            return self.finish();
        }
        if let Some(p) = &mut self.parser {
            let events = p.parse_chunk(chunk)?;
            self.done |= events.iter().any(|e| matches!(e, StreamEvent::Done { .. }));
            return Ok(events);
        }
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > 4 * 1024 * 1024 {
            return Err(protocol("Copilot stream frame exceeds its size limit"));
        }
        let text = match std::str::from_utf8(&self.pending) {
            Ok(t) => t,
            Err(e) if e.error_len().is_none() => return Ok(Vec::new()),
            Err(_) => return Err(protocol("Copilot stream is not UTF-8")),
        };
        if !text.contains("\n\n") && !text.contains("\r\n\r\n") {
            return Ok(Vec::new());
        }
        let complete_end = text
            .rfind("\n\n")
            .map(|p| p + 2)
            .or_else(|| text.rfind("\r\n\r\n").map(|p| p + 4))
            .unwrap_or_default();
        let data = text[..complete_end]
            .lines()
            .find_map(|line| line.strip_prefix("data:").map(str::trim));
        let Some(data) = data else {
            return Ok(Vec::new());
        };
        let raw: Value = serde_json::from_str(data)
            .map_err(|_| protocol("Copilot stream has an invalid first frame"))?;
        let event = text[..complete_end]
            .lines()
            .find_map(|line| line.strip_prefix("event:").map(str::trim));
        let mut parser = if raw.get("choices").is_some() {
            OpenAiCompatibleReferenceV1.stream_parser()
        } else if raw["type"]
            .as_str()
            .is_some_and(|s| s.starts_with("response."))
        {
            ResponsesCodec.stream_parser()
        } else if event.is_some_and(|s| {
            s.starts_with("message_") || s.starts_with("content_block_") || s == "ping"
        }) || raw["type"].as_str().is_some_and(|s| {
            s.starts_with("message_") || s.starts_with("content_block_") || s == "ping"
        }) {
            Box::new(MessagesStream::new())
        } else {
            return Err(protocol("Copilot stream has an unknown dialect"));
        };
        let events = parser.parse_chunk(&self.pending)?;
        self.done |= events.iter().any(|e| matches!(e, StreamEvent::Done { .. }));
        self.pending.clear();
        self.parser = Some(parser);
        Ok(events)
    }
    fn finish(&mut self) -> ComponentResultV1<Vec<StreamEvent>> {
        let events = self
            .parser
            .as_mut()
            .ok_or_else(|| protocol("Copilot stream ended without a response"))?
            .finish()?;
        self.done |= events.iter().any(|e| matches!(e, StreamEvent::Done { .. }));
        if !self.done {
            return Err(protocol("Copilot stream ended without a terminal event"));
        }
        Ok(events)
    }
}
/// Rejects Anthropic stream state that the pinned South stream ABI cannot represent.
struct MessagesStream {
    tail: Vec<u8>,
    parser: Box<dyn StreamParserV1>,
}
impl MessagesStream {
    fn new() -> Self {
        Self {
            tail: Vec::new(),
            parser: AnthropicReferenceV1.stream_parser(),
        }
    }
}
impl StreamParserV1 for MessagesStream {
    fn parse_chunk(&mut self, chunk: &[u8]) -> ComponentResultV1<Vec<StreamEvent>> {
        if chunk.is_empty() {
            if !self.tail.is_empty() {
                return Err(protocol("Copilot Messages ended inside a stream frame"));
            }
            return self.parser.parse_chunk(chunk);
        }
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > 4 * 1024 * 1024 {
            return Err(protocol("Copilot Messages frame exceeds its size limit"));
        }
        let mut events = Vec::new();
        loop {
            let lf = self
                .tail
                .windows(2)
                .position(|v| v == b"\n\n")
                .map(|p| (p, 2));
            let crlf = self
                .tail
                .windows(4)
                .position(|v| v == b"\r\n\r\n")
                .map(|p| (p, 4));
            let boundary = match (lf, crlf) {
                (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
                (Some(a), None) | (None, Some(a)) => Some(a),
                _ => None,
            };
            let Some((end, separator)) = boundary else {
                break;
            };
            let frame = self.tail.drain(..end + separator).collect::<Vec<_>>();
            let text = std::str::from_utf8(&frame)
                .map_err(|_| protocol("Copilot Messages frame is not UTF-8"))?;
            let event = text
                .lines()
                .find_map(|line| line.strip_prefix("event:").map(str::trim));
            let data = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim))
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() {
                let raw: Value = serde_json::from_str(&data)
                    .map_err(|_| protocol("Copilot Messages frame has invalid JSON"))?;
                if event == Some("error")
                    || raw["type"] == "error"
                    || raw.get("error").is_some_and(|v| !v.is_null())
                {
                    return Err(protocol("Copilot Messages emitted an upstream error"));
                }
                if event == Some("content_block_start") || raw["type"] == "content_block_start" {
                    let block = &raw["content_block"];
                    if !matches!(
                        block["type"].as_str(),
                        Some("text" | "thinking" | "tool_use")
                    ) {
                        return Err(capability(
                            "Copilot cannot preserve this streamed Messages content block",
                        ));
                    }
                    if block["type"] == "tool_use"
                        && block.get("input").is_some_and(|v| v != &json!({}))
                    {
                        return Err(capability(
                            "Copilot cannot preserve initialized streamed tool arguments",
                        ));
                    }
                    events.extend(self.parser.parse_chunk(&frame)?);
                    if let Some(text) = block["text"].as_str().filter(|v| !v.is_empty()) {
                        events.push(StreamEvent::Delta {
                            index: 0,
                            content: text.into(),
                        });
                    }
                    if let Some(thinking) = block["thinking"].as_str().filter(|v| !v.is_empty()) {
                        events.push(StreamEvent::ThinkingDelta {
                            index: 0,
                            thinking_delta: thinking.into(),
                        });
                    }
                    if let Some(signature) = block["signature"].as_str().filter(|v| !v.is_empty()) {
                        events.push(StreamEvent::ThinkingSignatureDelta {
                            index: 0,
                            signature_delta: signature.into(),
                        });
                    }
                    continue;
                }
                if (event == Some("content_block_delta") || raw["type"] == "content_block_delta")
                    && !matches!(
                        raw["delta"]["type"].as_str(),
                        Some(
                            "text_delta"
                                | "thinking_delta"
                                | "signature_delta"
                                | "input_json_delta"
                        )
                    )
                {
                    return Err(capability(
                        "Copilot cannot preserve this streamed Messages delta",
                    ));
                }
                if (event == Some("message_start") || raw["type"] == "message_start")
                    && raw["message"]["content"]
                        .as_array()
                        .is_some_and(|v| !v.is_empty())
                {
                    return Err(capability(
                        "Copilot cannot preserve initialized Messages stream content",
                    ));
                }
            }
            events.extend(self.parser.parse_chunk(&frame)?);
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wit_matches() {
        assert_eq!(
            include_str!("../wit/provider-adapter.wit"),
            south_provider_api::ADAPTER_WIT
        );
    }
}
