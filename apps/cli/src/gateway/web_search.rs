//! Adapt hosted search through the existing provider route.
#[allow(clippy::wildcard_imports)]
use super::*;
use std::fmt::Write as _;

fn search_tool(tool: &Value) -> bool {
    tool.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.starts_with("web_search_"))
}

fn declares_search(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(search_tool))
}

fn unsupported(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}

fn protocol_error(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

fn text_content(value: &Value) -> Result<String, ErrorEnvelope> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let blocks = value
        .as_array()
        .ok_or_else(|| unsupported("Web Search requires text content"))?;
    let mut text = Vec::new();
    for block in blocks {
        if block["type"] != "text" {
            return Err(unsupported(
                "The Responses search bridge requires text-only search context",
            ));
        }
        text.push(
            block["text"]
                .as_str()
                .ok_or_else(|| unsupported("Web Search text is invalid"))?,
        );
    }
    Ok(text.join("\n"))
}

// The direct search subrequest is text-only. Reject other content instead of
// silently discarding images, documents, thinking signatures, or tool history.
#[allow(clippy::too_many_lines)]
fn to_responses(body: &Value) -> Result<(Value, String), ErrorEnvelope> {
    let tools = body["tools"]
        .as_array()
        .ok_or_else(|| unsupported("Web Search tools are missing"))?;
    let hosted: Vec<_> = tools.iter().filter(|tool| search_tool(tool)).collect();
    if hosted.len() != 1 {
        return Err(unsupported(
            "The Responses search bridge requires one hosted search tool",
        ));
    }
    let tool = hosted[0];
    let kind = tool["type"].as_str().unwrap_or_default();
    if !matches!(
        kind,
        "web_search_20250305" | "web_search_20260209" | "web_search_20260318"
    ) {
        return Err(unsupported(
            "This Web Search version is not supported by the Responses bridge",
        ));
    }
    if tool
        .get("allowed_callers")
        .is_some_and(|v| v != &json!(["direct"]))
        || (kind != "web_search_20250305" && tool.get("allowed_callers").is_none())
    {
        return Err(unsupported(
            "The Responses search bridge requires direct tool calls",
        ));
    }
    if tool.get("response_inclusion").is_some()
        || tool.get("blocked_domains").is_some_and(|v| v != &json!([]))
    {
        return Err(unsupported(
            "The Responses search bridge cannot enforce blocked_domains or response_inclusion",
        ));
    }
    let name = tool["name"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| unsupported("Web Search name is missing"))?
        .to_owned();
    let mut search = json!({"type":"web_search"});
    if let Some(domains) = tool.get("allowed_domains") {
        if !domains
            .as_array()
            .is_some_and(|a| a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty())))
        {
            return Err(unsupported(
                "Web Search allowed_domains must contain domain names",
            ));
        }
        search["filters"] = json!({"allowed_domains":domains});
    }
    if let Some(location) = tool.get("user_location") {
        if !location.is_object() {
            return Err(unsupported("Web Search user_location must be an object"));
        }
        search["user_location"] = location.clone();
    }
    let mut output_tools = vec![search];
    for tool in tools.iter().filter(|tool| !search_tool(tool)) {
        if !matches!(
            tool.get("type").and_then(Value::as_str),
            None | Some("custom")
        ) {
            return Err(unsupported(
                "The Responses search bridge cannot execute another hosted tool",
            ));
        }
        let schema = tool
            .get("input_schema")
            .filter(|v| v.is_object())
            .ok_or_else(|| unsupported("Client tool input_schema is missing"))?;
        output_tools.push(json!({"type":"function", "name":tool["name"], "description":tool.get("description").unwrap_or(&json!("")), "parameters":schema}));
    }
    let mut input = Vec::new();
    if let Some(system) = body.get("system") {
        input.push(json!({"role":"system","content":text_content(system)?}));
    }
    for message in body["messages"]
        .as_array()
        .ok_or_else(|| unsupported("Web Search messages are missing"))?
    {
        let role = message["role"]
            .as_str()
            .ok_or_else(|| unsupported("Web Search message role is missing"))?;
        if !matches!(role, "user" | "assistant") {
            return Err(unsupported("Web Search message role is invalid"));
        }
        append_message(&mut input, message, role)?;
    }
    let mut result = json!({"model":body["model"],"input":input,"tools":output_tools,"stream":false,"store":false,
        "include":["web_search_call.action.sources"]});
    if let Some(limit) = tool.get("max_uses") {
        let limit = limit
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| unsupported("Web Search max_uses must be a positive integer"))?;
        result["max_tool_calls"] = json!(limit);
    }
    if let Some(limit) = body.get("max_tokens") {
        result["max_output_tokens"] = limit.clone();
    }
    if let Some(choice) = body.get("tool_choice") {
        result["tool_choice"] = match choice["type"].as_str() {
            Some("auto") => json!("auto"),
            Some("any") => json!("required"),
            Some("none") => json!("none"),
            Some("tool") if choice["name"] == name => json!({"type":"web_search"}),
            Some("tool") => json!({"type":"function","name":choice["name"]}),
            _ => return Err(unsupported("Web Search tool_choice is invalid")),
        };
        if let Some(disabled) = choice.get("disable_parallel_tool_use") {
            let disabled = disabled
                .as_bool()
                .ok_or_else(|| unsupported("disable_parallel_tool_use must be a boolean"))?;
            result["parallel_tool_calls"] = json!(!disabled);
        }
    }
    Ok((result, name))
}

// Replay hosted results as readable context. Responses call IDs belong to the
// upstream session and cannot be reconstructed from Anthropic opaque IDs.
fn append_message(
    input: &mut Vec<Value>,
    message: &Value,
    role: &str,
) -> Result<(), ErrorEnvelope> {
    let content = &message["content"];
    if let Some(text) = content.as_str() {
        input.push(json!({"role":role,"content":text}));
        return Ok(());
    }
    for block in content
        .as_array()
        .ok_or_else(|| unsupported("Web Search message content is invalid"))?
    {
        let item = match block["type"].as_str() {
            Some("text") => json!({"role":role,"content":text_content(&json!([block]))?}),
            Some("tool_use") if role == "assistant" => {
                json!({"type":"function_call","call_id":block["id"],"name":block["name"],"arguments":block["input"].to_string()})
            }
            Some("tool_result") if role == "user" => {
                json!({"type":"function_call_output","call_id":block["tool_use_id"],"output":text_content(&block["content"])?})
            }
            Some("server_tool_use") if role == "assistant" => {
                json!({"role":"assistant","content":format!("Previous hosted search: {}",block["input"])})
            }
            Some("web_search_tool_result") if role == "assistant" => {
                let text = if let Some(results) = block["content"].as_array() {
                    results
                        .iter()
                        .map(|v| {
                            format!(
                                "{}: {}",
                                v["title"].as_str().unwrap_or("Search result"),
                                v["url"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    format!(
                        "Previous search error: {}",
                        block["content"]["error_code"].as_str().unwrap_or("unknown")
                    )
                };
                json!({"role":"assistant","content":text})
            }
            _ => {
                return Err(unsupported(
                    "The Responses search bridge requires text, client tools, or search history. Use an Anthropic-native backend for other content.",
                ));
            }
        };
        input.push(item);
    }
    Ok(())
}

fn sources(values: &Value) -> Vec<Value> {
    values.as_array().into_iter().flatten().filter_map(|source| {
        let url = source["url"].as_str()?;
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "https" | "http") { return None; }
        Some(json!({"type":"web_search_result","url":url,"title":source["title"].as_str().unwrap_or(url),"encrypted_content":"","page_age":null}))
    }).collect()
}

#[allow(clippy::too_many_lines)]
fn from_responses(
    body: &Value,
    name: &str,
    model: &str,
    max_uses: Option<u64>,
    request: &Value,
) -> Result<Value, ErrorEnvelope> {
    if !matches!(body["status"].as_str(), Some("completed" | "incomplete")) {
        return Err(protocol_error(
            "The search backend did not complete a valid Responses request",
        ));
    }
    let output = body["output"]
        .as_array()
        .ok_or_else(|| protocol_error("The search backend response has no output array"))?;
    // Bound object overhead as well as bytes before constructing the reply.
    let entries = output.iter().fold(output.len(), |total, item| {
        let blocks = item["content"].as_array();
        let annotations = blocks.into_iter().flatten().fold(0_usize, |count, block| {
            count.saturating_add(block["annotations"].as_array().map_or(0, Vec::len))
        });
        total
            .saturating_add(blocks.map_or(0, Vec::len))
            .saturating_add(annotations)
            .saturating_add(item["action"]["sources"].as_array().map_or(0, Vec::len))
    });
    if entries > 4096 {
        return Err(protocol_error(
            "The search backend returned too many output items",
        ));
    }
    let last_search = output
        .iter()
        .rposition(|item| item["type"] == "web_search_call");
    let mut content = Vec::new();
    let mut count = 0_u64;
    let mut has_function = false;
    let mut citations = Vec::new();
    for item in output {
        for block in item["content"].as_array().into_iter().flatten() {
            citations.extend(sources(&block["annotations"]));
        }
    }
    for (index, item) in output.iter().enumerate() {
        match item["type"].as_str() {
            Some("web_search_call") => {
                count += 1;
                let response_id: String = body["id"]
                    .as_str()
                    .unwrap_or("search")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .take(100)
                    .collect();
                let id = format!("srvtoolu_{response_id}_{count}");
                content.push(json!({"type":"server_tool_use","id":id,"name":name,
                    "input":{"query":item["action"]["query"].as_str().unwrap_or("")}}));
                let result = if max_uses.is_some_and(|limit| count > limit) {
                    json!({"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"})
                } else if item["status"] != "completed" {
                    json!({"type":"web_search_tool_result_error","error_code":"unavailable"})
                } else {
                    let mut found = sources(&item["action"]["sources"]);
                    // Some compatible backends expose sources only as final citations.
                    // Assign them only to the final search when per-call sources are absent.
                    if found.is_empty() && last_search == Some(index) {
                        found = std::mem::take(&mut citations);
                    }
                    json!(found)
                };
                content.push(
                    json!({"type":"web_search_tool_result","tool_use_id":id,"content":result}),
                );
                if max_uses.is_some_and(|limit| count > limit) {
                    break;
                }
            }
            Some("message") => {
                for block in item["content"]
                    .as_array()
                    .ok_or_else(|| protocol_error("The search backend message has no content"))?
                {
                    match block["type"].as_str() {
                        Some("output_text") => {
                            let mut text = block["text"].as_str().ok_or_else(|| protocol_error("The search backend text is invalid"))?.to_owned();
                            for citation in sources(&block["annotations"]) {
                                let url = citation["url"].as_str().unwrap_or_default();
                                if !text.contains(url) { let _ = write!(text, "\n{}: {url}",citation["title"].as_str().unwrap_or(url)); }
                            }
                            content.push(json!({"type":"text","text":text}));
                        }
                        Some("refusal") => content.push(json!({"type":"text","text":block["refusal"].as_str().unwrap_or("Search was refused.")})),
                        _ => return Err(protocol_error("The search backend returned an unsupported message block")),
                    }
                }
            }
            Some("function_call") => {
                let function_name = item["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| protocol_error("The search backend function name is invalid"))?;
                let declared = request["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|tool| {
                        !search_tool(tool)
                            && tool["name"] == function_name
                            && tool.get("input_schema").is_some_and(Value::is_object)
                            && tool
                                .get("allowed_callers")
                                .is_none_or(|callers| callers == &json!(["direct"]))
                    });
                let choice = &request["tool_choice"];
                if !declared
                    || choice["type"] == "none"
                    || (choice["type"] == "tool" && choice["name"] != function_name)
                {
                    return Err(protocol_error(
                        "The search backend returned an unauthorized client tool call",
                    ));
                }
                if item["call_id"].as_str().is_none_or(str::is_empty) {
                    return Err(protocol_error(
                        "The search backend function call ID is invalid",
                    ));
                }
                let input: Value = serde_json::from_str(item["arguments"].as_str().unwrap_or(""))
                    .map_err(|_| {
                    protocol_error("The search backend returned invalid function arguments")
                })?;
                if !input.is_object() {
                    return Err(protocol_error(
                        "The search backend function arguments must be an object",
                    ));
                }
                content.push(json!({"type":"tool_use","id":item["call_id"],"name":item["name"],"input":input}));
                has_function = true;
            }
            Some("reasoning") => {}
            _ => {
                return Err(protocol_error(
                    "The search backend returned an unsupported output item",
                ));
            }
        }
    }
    if !content.iter().any(|block| {
        block["type"] != "text"
            || block["text"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty())
    }) {
        return Err(protocol_error(
            "The search backend returned no usable output",
        ));
    }
    // Count serialized bytes without allocating a second copy of the response.
    let mut budget = ResponseBudget {
        remaining: MAX_UPSTREAM_BODY,
    };
    serde_json::to_writer(&mut budget, &content)
        .map_err(|_| protocol_error("The converted search response exceeds the size limit"))?;
    let input = body["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let cached = body["usage"]["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    Ok(
        json!({"id":body["id"].as_str().unwrap_or("msg_search"),"type":"message","role":"assistant","model":model,
        "content":content,"stop_reason":if has_function {"tool_use"} else if body["status"] == "incomplete" {"max_tokens"} else {"end_turn"},"stop_sequence":null,
        "usage":{"input_tokens":input.saturating_sub(cached),"cache_read_input_tokens":cached,"output_tokens":body["usage"]["output_tokens"].as_u64().unwrap_or(0),"server_tool_use":{"web_search_requests":count}}}),
    )
}

struct ResponseBudget {
    remaining: u64,
}

impl std::io::Write for ResponseBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = bytes.len() as u64;
        if length > self.remaining {
            return Err(std::io::Error::other("search response size limit"));
        }
        self.remaining -= length;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sse(event: &str, body: &Value) -> String {
    format!("event: {event}\ndata: {body}\n\n")
}

fn emit_message(message: &Value, stream: bool, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
    if !stream {
        return emit(Reply::BeginJson(JsonReply {
            status: 200,
            body: message.to_string(),
        }));
    }
    // Do not clone the buffered content merely to clear it for message_start.
    let mut start = Value::Object(
        message
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.as_str() != "content")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    start["usage"]["output_tokens"] = json!(0);
    if !emit(Reply::BeginStream)
        || !emit(Reply::Chunk(sse(
            "message_start",
            &json!({"type":"message_start","message":start}),
        )))
    {
        return false;
    }
    for (index, block) in message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let mut start = block.clone();
        let delta = match block["type"].as_str() {
            Some("text") => {
                start["text"] = json!("");
                Some(json!({"type":"text_delta","text":block["text"]}))
            }
            Some("tool_use" | "server_tool_use") => {
                start["input"] = json!({});
                Some(json!({"type":"input_json_delta","partial_json":block["input"].to_string()}))
            }
            _ => None,
        };
        if !emit(Reply::Chunk(sse(
            "content_block_start",
            &json!({"type":"content_block_start","index":index,"content_block":start}),
        ))) {
            return false;
        }
        if let Some(delta) = delta
            && !emit(Reply::Chunk(sse(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":index,"delta":delta}),
            )))
        {
            return false;
        }
        if !emit(Reply::Chunk(sse(
            "content_block_stop",
            &json!({"type":"content_block_stop","index":index}),
        ))) {
            return false;
        }
    }
    emit(Reply::Chunk(sse(
        "message_delta",
        &json!({"type":"message_delta","delta":{"stop_reason":message["stop_reason"],"stop_sequence":null},"usage":message["usage"]}),
    ))) && emit(Reply::Chunk(sse(
        "message_stop",
        &json!({"type":"message_stop"}),
    )))
}

impl Gateway {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn try_web_search(
        &self,
        ctx: &RequestContext,
        agent: &LoadedAgent,
        router: &Router,
        headers: &[(String, String)],
        body: &[u8],
        routing_model: Option<&str>,
        emit: &mut dyn FnMut(Reply) -> bool,
        record: &mut RequestRecord,
    ) -> Result<Option<(UpstreamModel, StreamOutcome)>, ErrorEnvelope> {
        if body.len() > MAX_INBOUND_BODY {
            return Ok(None);
        }
        let Ok(original) = serde_json::from_slice::<Value>(body) else {
            return Ok(None);
        };
        if !declares_search(&original) {
            return Ok(None);
        }
        let search_router = router;
        if let Some(served) = self.try_anthropic_passthrough(
            ctx,
            agent,
            search_router,
            headers,
            body,
            routing_model,
            emit,
            record,
        )? {
            return Ok(Some(served));
        }
        // Inspect the configured route before filtering candidates. A search
        // request must not silently select an unrelated native provider.
        let model = routing_model
            .or_else(|| original["model"].as_str())
            .unwrap_or("auto");
        let mut probe = ChatRequest::new(model, Vec::new());
        probe.tools.push(ToolDef {
            name: "web_search_probe".to_owned(),
            description: None,
            parameters: json!({}),
        });
        let (now, session) = Self::quota_preamble(search_router, || format!("web-search:{model}"));
        let candidates = self.candidates(std::time::Instant::now(), now);
        let native_responses = self
            .route_with_mode(search_router, &probe, &[], &candidates, &session)
            .ok()
            .and_then(|decision| self.upstreams.get(decision.chosen.upstream.as_str()))
            .is_some_and(|upstream| upstream.dialect == ApiDialect::ResponsesNative);
        if !native_responses {
            let error =
                unsupported("The selected provider route does not support native web_search.");
            record_conversion(
                record,
                ConversionStage::InboundNormalize,
                "anthropic-messages",
                CANONICAL_CHAT_PROTOCOL,
                false,
                Some(error.code),
            );
            annotate_conversion_failure(record, &error);
            return Err(error);
        }
        let (mut forwarded, name) = to_responses(&original)?;
        forwarded["model"] = json!(model);
        let stream = original["stream"].as_bool().unwrap_or(false);
        let max_uses = forwarded["max_tool_calls"].as_u64();
        let mut answer = None;
        let result = self.try_responses_passthrough(
            ctx,
            agent,
            search_router,
            headers,
            forwarded.to_string().as_bytes(),
            &mut |reply| {
                if let Reply::BeginJson(json) = reply {
                    answer = Some(json);
                    true
                } else {
                    false
                }
            },
            record,
        )?;
        let Some((target, outcome)) = result else {
            return Err(unsupported(
                "The selected provider route cannot execute native Web Search.",
            ));
        };
        record.stream = stream;
        if matches!(outcome, StreamOutcome::ClientCancelled) {
            Self::emit_cancelled(emit);
            return Ok(Some((target, outcome)));
        }
        let Some(answer) = answer else {
            return Ok(Some((target, outcome)));
        };
        if answer.status >= 400 {
            let error = ErrorEnvelope::new(
                if matches!(answer.status, 401 | 403) {
                    ErrorCode::Auth
                } else if answer.status == 429 {
                    ErrorCode::RateLimit
                } else if answer.status >= 500 {
                    ErrorCode::UpstreamUnavailable
                } else {
                    ErrorCode::InvalidRequest
                },
                answer.status,
                "The search backend rejected the request. Check native Web Search support, credentials, and limits.",
            );
            record.status = error.http_status;
            record.error_code = Some(error.code);
            self.settle(record, &target, StreamOutcome::FailedBeforeOutput);
            return Err(error);
        }
        let message = serde_json::from_str::<Value>(&answer.body)
            .map_err(|_| protocol_error("The search backend returned invalid JSON"))
            .and_then(|parsed| {
                from_responses(
                    &parsed,
                    &name,
                    original["model"].as_str().unwrap_or(model),
                    max_uses,
                    &original,
                )
            })
            .inspect_err(|error| {
                // The upstream already consumed tokens even when wire conversion fails.
                record.status = error.http_status;
                record.error_code = Some(error.code);
                self.settle(record, &target, StreamOutcome::FailedBeforeOutput);
                record_conversion(
                    record,
                    ConversionStage::OutboundRender,
                    "openai-responses",
                    "anthropic-messages",
                    false,
                    Some(error.code),
                );
            })?;
        record_conversion(
            record,
            ConversionStage::OutboundRender,
            "openai-responses",
            "anthropic-messages",
            true,
            None,
        );
        let outcome = if emit_message(&message, stream, emit) {
            StreamOutcome::Complete
        } else {
            StreamOutcome::ClientCancelled
        };
        Ok(Some((target, outcome)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        json!({"model":"any-chat-model", "max_tokens":512, "stream":true,
            "messages":[{"role":"user","content":"Find current news"}],
            "tools":[{"type":"web_search_20250305","name":"web_search","max_uses":2,
                "allowed_domains":["example.com"]}],
            "tool_choice":{"type":"tool","name":"web_search"}})
    }

    #[test]
    fn hosted_search_preserves_limits_filters_and_forced_choice() {
        let (body, name) = to_responses(&request()).unwrap();
        assert_eq!(name, "web_search");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tool_calls"], 2);
        assert_eq!(
            body["tools"][0]["filters"]["allowed_domains"],
            json!(["example.com"])
        );
        assert_eq!(body["tool_choice"], json!({"type":"web_search"}));
        assert_eq!(body["include"], json!(["web_search_call.action.sources"]));
    }

    #[test]
    fn unsupported_search_constraints_fail_before_network_io() {
        for (field, value) in [
            ("blocked_domains", json!(["example.com"])),
            ("max_uses", json!(0)),
            ("allowed_callers", json!(["code_execution"])),
        ] {
            let mut body = request();
            body["tools"][0][field] = value;
            assert!(to_responses(&body).is_err(), "{field}");
        }
    }

    #[test]
    fn a_custom_function_named_web_search_is_not_hosted_search() {
        let body = json!({"tools":[{"name":"web_search","input_schema":{"type":"object"}}]});
        assert!(!declares_search(&body));
    }

    #[test]
    fn search_call_sources_and_citations_return_to_claude() {
        let result = from_responses(&json!({"id":"resp_1","status":"completed","output":[
            {"type":"web_search_call","id":"ws_1","status":"completed","action":{"query":"news","sources":[{"url":"https://example.com","title":"News"}]}},
            {"type":"message","content":[{"type":"output_text","text":"Today's news","annotations":[{"type":"url_citation","url":"https://example.com","title":"News"}]}]}
        ],"usage":{"input_tokens":10,"output_tokens":20}}), "web_search", "main-model", Some(2), &request()).unwrap();
        assert_eq!(result["content"][0]["type"], "server_tool_use");
        assert_eq!(
            result["content"][1]["tool_use_id"],
            result["content"][0]["id"]
        );
        assert_eq!(
            result["content"][1]["content"][0]["url"],
            "https://example.com"
        );
        assert!(
            result["content"][2]["text"]
                .as_str()
                .unwrap()
                .contains("https://example.com")
        );
        assert_eq!(result["usage"]["server_tool_use"]["web_search_requests"], 1);
        assert!(
            from_responses(
                &json!({"status":"failed","output":[]}),
                "web_search",
                "model",
                None,
                &request()
            )
            .is_err()
        );
    }

    #[test]
    fn identical_search_calls_receive_final_citations_only_once() {
        let call = json!({"type":"web_search_call","id":"same","status":"completed","action":{"query":"news"}});
        let mut output = vec![call; 1000];
        output.push(json!({"type":"message","content":[{"type":"output_text","text":"news","annotations":[{"url":"https://example.com","title":"x".repeat(100_000)}]}]}));
        let body = json!({"status":"completed","output":output});
        let result = from_responses(&body, "web_search", "model", None, &request()).unwrap();
        let assigned = result["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|block| {
                block["type"] == "web_search_tool_result"
                    && block["content"]
                        .as_array()
                        .is_some_and(|sources| !sources.is_empty())
            })
            .count();
        assert_eq!(assigned, 1);
        assert!(result.to_string().len() < 1_000_000);
    }

    #[test]
    fn client_calls_require_declaration_and_matching_tool_choice() {
        let body = json!({"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}]});
        let mut original = request();
        assert!(from_responses(&body, "web_search", "model", None, &original).is_err());
        original["tools"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"lookup","input_schema":{"type":"object"}}));
        // The original forced hosted-search choice still forbids this client tool.
        assert!(from_responses(&body, "web_search", "model", None, &original).is_err());
        original["tool_choice"] = json!({"type":"none"});
        assert!(from_responses(&body, "web_search", "model", None, &original).is_err());
        for choice in [
            json!({"type":"auto"}),
            json!({"type":"tool","name":"lookup"}),
        ] {
            original["tool_choice"] = choice;
            let result = from_responses(&body, "web_search", "model", None, &original).unwrap();
            assert_eq!(result["content"][0]["name"], "lookup");
            assert_eq!(result["stop_reason"], "tool_use");
        }
    }

    #[test]
    fn empty_or_reasoning_only_success_is_rejected() {
        for output in [
            json!([]),
            json!([{"type":"reasoning"}]),
            json!([{"type":"message","content":[{"type":"output_text","text":" "}]}]),
        ] {
            let body = json!({"status":"completed","output":output});
            assert!(from_responses(&body, "web_search", "model", None, &request()).is_err());
        }
        let refusal = json!({"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"Search is unavailable."}]}]});
        assert!(from_responses(&refusal, "web_search", "model", None, &request()).is_ok());
    }

    #[test]
    fn converted_output_has_byte_and_entry_limits() {
        let body = json!({"status":"completed","output":vec![json!({"type":"reasoning"});4097]});
        assert!(from_responses(&body, "web_search", "model", None, &request()).is_err());
        let mut budget = ResponseBudget { remaining: 8 };
        assert!(serde_json::to_writer(&mut budget, &json!("0123456789")).is_err());
    }
}
