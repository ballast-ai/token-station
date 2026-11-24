use super::*;
use south_component_conformance::sse_split::{SseSplitErrorV1, SseSplitterV1};

#[derive(Default)]
struct CallState {
    index: u32,
    id: String,
    name: String,
    arguments: String,
    closed: bool,
}

#[derive(Default)]
pub(super) struct ResponsesStream {
    splitter: SseSplitterV1,
    received_data: bool,
    calls: BTreeMap<String, CallState>,
    indexes: BTreeMap<u32, String>,
    text: BTreeMap<(u32, u32), String>,
    complete: bool,
    failure: Option<ErrorEnvelope>,
    pub id: String,
    pub model: String,
}

fn number(value: &Value) -> ComponentResultV1<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| protocol("An upstream output index is missing or invalid"))
}

impl ResponsesStream {
    fn call_key(&self, event: &Value) -> ComponentResultV1<String> {
        let id = event
            .get("item_id")
            .map(|value| {
                let id = value
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| protocol("A tool argument event has an invalid item ID"))?;
                if !self.calls.contains_key(id) {
                    return Err(protocol("A tool argument event names an unknown item ID"));
                }
                Ok(id)
            })
            .transpose()?;
        let indexed_id = event
            .get("output_index")
            .map(|value| {
                self.indexes
                    .get(&number(value)?)
                    .map(String::as_str)
                    .ok_or_else(|| protocol("A tool argument event names an unknown output index"))
            })
            .transpose()?;
        match (id, indexed_id) {
            (Some(id), Some(indexed)) if id != indexed => Err(protocol(
                "The tool item ID and output index identify different calls",
            )),
            (Some(id), _) | (_, Some(id)) => Ok(id.to_owned()),
            (None, None) => Err(protocol("A tool argument event has no tool identity")),
        }
    }

    fn start_call(
        &mut self,
        item: &Value,
        index: u32,
        events: &mut Vec<StreamEvent>,
    ) -> ComponentResultV1<()> {
        if item["namespace"]
            .as_str()
            .is_some_and(|s| s != TOOL_NAMESPACE)
        {
            return Err(protocol("The upstream called an unknown tool namespace"));
        }
        let id = item["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| protocol("A tool call has no item ID"))?;
        let call_id = item["call_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| protocol("A tool call has no call ID"))?;
        let name = item["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| protocol("A tool call has no name"))?;
        if let Some(old) = self.calls.get(id) {
            if old.id != call_id
                || old.name != name
                || self.indexes.get(&index).is_none_or(|key| key != id)
            {
                return Err(protocol("The upstream changed a tool call identity"));
            }
            return Ok(());
        }
        if self.calls.len() >= 1024
            || self.calls.values().any(|call| call.id == call_id)
            || self.indexes.contains_key(&index)
        {
            return Err(protocol(
                "The upstream repeated or exceeded tool identities",
            ));
        }
        let tool_index = self.calls.len() as u32;
        self.calls.insert(
            id.to_owned(),
            CallState {
                index: tool_index,
                id: call_id.to_owned(),
                name: name.to_owned(),
                ..CallState::default()
            },
        );
        self.indexes.insert(index, id.to_owned());
        events.push(StreamEvent::ToolCallDelta {
            index: tool_index,
            id: Some(call_id.to_owned()),
            name: Some(name.to_owned()),
            signature: None,
            arguments_delta: String::new(),
        });
        Ok(())
    }

    fn arguments(
        &mut self,
        key: &str,
        value: &str,
        done: bool,
        events: &mut Vec<StreamEvent>,
    ) -> ComponentResultV1<()> {
        let call = self
            .calls
            .get_mut(key)
            .ok_or_else(|| protocol("Unknown upstream tool item"))?;
        let delta = if done {
            if call.closed {
                if call.arguments != value {
                    return Err(protocol("The upstream changed completed tool arguments"));
                }
                return Ok(());
            }
            value
                .strip_prefix(&call.arguments)
                .ok_or_else(|| protocol("The upstream changed streamed tool arguments"))?
        } else {
            if call.closed {
                return Err(protocol("Arguments arrived after the tool call completed"));
            }
            value
        };
        if call.arguments.len().saturating_add(delta.len()) > MAX_VALUE_BYTES {
            return Err(protocol("Tool arguments exceeded the adapter limit"));
        }
        if !delta.is_empty() {
            call.arguments.push_str(delta);
            events.push(StreamEvent::ToolCallDelta {
                index: call.index,
                id: None,
                name: None,
                signature: None,
                arguments_delta: delta.to_owned(),
            });
        }
        if done {
            let value: Value = serde_json::from_str(&call.arguments)
                .map_err(|_| protocol("Completed tool arguments are not valid JSON"))?;
            if !value.is_object() {
                return Err(protocol("Completed function arguments must be an object"));
            }
            call.closed = true;
        }
        Ok(())
    }

    fn text(
        &mut self,
        key: (u32, u32),
        text: &str,
        done: bool,
        events: &mut Vec<StreamEvent>,
    ) -> ComponentResultV1<()> {
        let old = self.text.entry(key).or_default();
        let delta = if done {
            text.strip_prefix(old.as_str())
                .ok_or_else(|| protocol("The upstream changed streamed text"))?
        } else {
            text
        };
        if old.len().saturating_add(delta.len()) > MAX_VALUE_BYTES {
            return Err(protocol("Response text exceeded the adapter limit"));
        }
        if !delta.is_empty() {
            old.push_str(delta);
            events.push(StreamEvent::Delta {
                index: 0,
                content: delta.to_owned(),
            });
        }
        Ok(())
    }

    fn item_done(
        &mut self,
        item: &Value,
        index: u32,
        events: &mut Vec<StreamEvent>,
    ) -> ComponentResultV1<()> {
        match item["type"].as_str() {
            Some("function_call") => {
                self.start_call(item, index, events)?;
                let args = item["arguments"]
                    .as_str()
                    .ok_or_else(|| protocol("A completed tool call has no arguments"))?;
                self.arguments(item["id"].as_str().unwrap_or_default(), args, true, events)?;
            }
            Some("message") => {
                let parts = item["content"]
                    .as_array()
                    .ok_or_else(|| protocol("An output message has no content"))?;
                for (i, part) in parts.iter().enumerate() {
                    match part["type"].as_str() {
                        Some("output_text") => self.text(
                            (index, i as u32),
                            part["text"]
                                .as_str()
                                .ok_or_else(|| protocol("An output text block has no text"))?,
                            true,
                            events,
                        )?,
                        Some("refusal") => {
                            return Err(ErrorEnvelope::new(
                                ErrorCode::ContentPolicy,
                                400,
                                "The provider refused the request.",
                            ))
                        }
                        _ => {
                            return Err(protocol(
                                "The provider returned an unsupported output block",
                            ))
                        }
                    }
                }
            }
            // Reasoning is private model state. The pinned South stream ABI
            // cannot carry opaque Responses items. Do not request or fabricate
            // that state and do not mislabel it as an Anthropic signature.
            Some("reasoning") => {
                if item.get("encrypted_content").is_some_and(|v| !v.is_null()) {
                    return Err(ErrorEnvelope::new(
                        ErrorCode::Capability,
                        400,
                        "Copilot cannot preserve opaque Responses reasoning state",
                    ));
                }
            }
            _ => return Err(protocol("The provider returned an unsupported output item")),
        }
        Ok(())
    }

    fn event(&mut self, v: Value, events: &mut Vec<StreamEvent>) -> ComponentResultV1<()> {
        let kind = v["type"]
            .as_str()
            .ok_or_else(|| protocol("An upstream event has no type"))?;
        if self.complete {
            // Keep completion terminal even when the endpoint emits a trailing
            // duplicate completion. Never produce duplicate tools or usage.
            return Ok(());
        }
        match kind {
            "response.created" | "response.in_progress" => {
                if let Some(id) = v["response"]["id"].as_str() {
                    self.id = id.to_owned();
                }
                if let Some(model) = v["response"]["model"].as_str() {
                    self.model = model.to_owned();
                }
            }
            "response.output_text.delta" => {
                let key = (
                    v["output_index"].as_u64().unwrap_or(0) as u32,
                    v["content_index"].as_u64().unwrap_or(0) as u32,
                );
                self.text(
                    key,
                    v["delta"]
                        .as_str()
                        .ok_or_else(|| protocol("Text delta is missing"))?,
                    false,
                    events,
                )?;
            }
            "response.output_text.done" => {
                let key = (
                    v["output_index"].as_u64().unwrap_or(0) as u32,
                    v["content_index"].as_u64().unwrap_or(0) as u32,
                );
                self.text(
                    key,
                    v["text"]
                        .as_str()
                        .ok_or_else(|| protocol("Completed text is missing"))?,
                    true,
                    events,
                )?;
            }
            "response.output_item.added" => match v["item"]["type"].as_str() {
                Some("function_call") => {
                    self.start_call(&v["item"], number(&v["output_index"])?, events)?
                }
                Some("message") => {}
                Some("reasoning") => {
                    if v["item"]
                        .get("encrypted_content")
                        .is_some_and(|v| !v.is_null())
                    {
                        return Err(ErrorEnvelope::new(
                            ErrorCode::Capability,
                            400,
                            "Copilot cannot preserve opaque Responses reasoning state",
                        ));
                    }
                }
                _ => return Err(protocol("The provider started an unsupported output item")),
            },
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let key = self.call_key(&v)?;
                let done = kind.ends_with(".done");
                let field = if done { "arguments" } else { "delta" };
                self.arguments(
                    &key,
                    v[field]
                        .as_str()
                        .ok_or_else(|| protocol("Tool argument content is missing"))?,
                    done,
                    events,
                )?;
            }
            "response.output_item.done" => {
                self.item_done(&v["item"], number(&v["output_index"])?, events)?
            }
            "response.refusal.delta" | "response.refusal.done" => {
                return Err(ErrorEnvelope::new(
                    ErrorCode::ContentPolicy,
                    400,
                    "The provider refused the request.",
                ))
            }
            "error" => {
                return Err(reasoning_rejection(&v["error"])
                    .unwrap_or_else(|| protocol("The ChatGPT plan response failed")))
            }
            "response.failed" => return Err(protocol("The ChatGPT plan response failed")),
            "response.incomplete" => {
                return Err(ErrorEnvelope::new(
                    ErrorCode::TransportTruncated,
                    502,
                    "The ChatGPT plan response is incomplete",
                ))
            }
            "response.completed" => {
                let response = &v["response"];
                if response["status"] != "completed"
                    || response.get("error").is_some_and(|e| !e.is_null())
                {
                    return Err(protocol(
                        "The completion event does not confirm successful completion",
                    ));
                }
                if let Some(id) = response["id"].as_str() {
                    self.id = id.to_owned();
                }
                if let Some(model) = response["model"].as_str() {
                    self.model = model.to_owned();
                }
                if let Some(items) = response["output"].as_array() {
                    for (index, item) in items.iter().enumerate() {
                        self.item_done(item, index as u32, events)?;
                    }
                }
                if self.calls.values().any(|call| !call.closed) {
                    return Err(protocol(
                        "The response completed with unfinished tool arguments",
                    ));
                }
                let reason = if self.calls.is_empty() {
                    FinishReason::Stop
                } else {
                    FinishReason::ToolCalls
                };
                events.push(StreamEvent::Finish {
                    finish_reason: Some(reason),
                    stop_sequence: None,
                });
                events.push(StreamEvent::Usage {
                    usage: usage_of(response.get("usage"))?,
                });
                events.push(StreamEvent::Done {
                    finish_reason: None,
                    stop_sequence: None,
                });
                self.complete = true;
            }
            // Reasoning summaries are display hints, not replayable reasoning
            // state. Encrypted items fail through the output-item checks above.
            "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.content_part.added"
            | "response.content_part.done"
            | "ping" => {}
            _ => {
                return Err(protocol(
                    "The provider returned an unsupported response event",
                ))
            }
        }
        Ok(())
    }
}

impl ResponsesStream {
    /// One dispatched SSE event's data.
    fn frame(&mut self, data: &str, events: &mut Vec<StreamEvent>) -> ComponentResultV1<()> {
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            if self.complete {
                return Ok(());
            }
            return Err(protocol(
                "The upstream sent DONE without response.completed",
            ));
        }
        let v = serde_json::from_str(data)
            .map_err(|_| protocol("An SSE frame contains invalid JSON"))?;
        self.event(v, events)
    }
}

impl StreamParserV1 for ResponsesStream {
    fn finish(&mut self) -> ComponentResultV1<Vec<StreamEvent>> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.complete {
            return Err(ErrorEnvelope::new(
                ErrorCode::TransportTruncated,
                502,
                "The stream ended before response.completed",
            ));
        }
        if !self.splitter.is_idle() {
            return Err(protocol("The stream ended inside an SSE frame"));
        }
        Ok(Vec::new())
    }

    fn parse_chunk(&mut self, chunk: &[u8]) -> ComponentResultV1<Vec<StreamEvent>> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if chunk.is_empty() {
            // South's byte-split conformance sends an empty prefix. It is not
            // EOF. The explicit native finish still rejects an empty stream.
            return if self.received_data {
                self.finish()
            } else {
                Ok(Vec::new())
            };
        }
        self.received_data = true;
        // The SSE standard's line ends (LF, CR, CRLF), as the host decoder cuts
        // them. Frames completed before a refusal are judged first.
        let pushed = self.splitter.push(chunk);
        let mut events = Vec::new();
        for frame in pushed.frames {
            if frame.data.len() > MAX_VALUE_BYTES {
                return Err(protocol("An SSE fragment exceeded the adapter limit"));
            }
            if let Err(error) = self.frame(&frame.data, &mut events) {
                self.failure = Some(error.clone());
                events.push(StreamEvent::Error { error });
                return Ok(events);
            }
        }
        match pushed.error {
            Some(SseSplitErrorV1::TooLarge) => {
                Err(protocol("An SSE fragment exceeded the adapter limit"))
            }
            Some(SseSplitErrorV1::NotUtf8) => {
                let error = protocol("An SSE frame contains invalid UTF-8");
                self.failure = Some(error.clone());
                events.push(StreamEvent::Error { error });
                Ok(events)
            }
            None => Ok(events),
        }
    }
}

/// The terminal `response.usage`, under South 0.85.1's Responses rule
/// (`reference_openai_responses::usage_of`, south #131): a completed response
/// without exact usage is an error, never a zero.
fn usage_of(usage: Option<&Value>) -> ComponentResultV1<Usage> {
    let Some(usage) = usage.filter(|usage| usage.is_object()) else {
        return Err(protocol("Copilot response carries no usage object"));
    };
    let required = |key: &str| {
        usage[key]
            .as_u64()
            .ok_or_else(|| protocol("Copilot usage lacks a valid token count"))
    };
    let details = |key: &str| match usage.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(details @ Value::Object(_)) => Ok(Some(details)),
        Some(_) => Err(protocol("Copilot usage has invalid token details")),
    };
    let optional = |details: Option<&Value>, key: &str| match details.and_then(|d| d.get(key)) {
        None | Some(Value::Null) => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| protocol("Copilot usage has an invalid token count")),
    };
    let input = required("input_tokens")?;
    let output = required("output_tokens")?;
    let total = required("total_tokens")?;
    if input.checked_add(output) != Some(total) {
        return Err(protocol(
            "Copilot total_tokens does not equal input_tokens plus output_tokens",
        ));
    }
    let input_details = details("input_tokens_details")?;
    let cached = optional(input_details, "cached_tokens")?;
    let cache_write = optional(input_details, "cache_write_tokens")?;
    if cached
        .checked_add(cache_write)
        .is_none_or(|cache| cache > input)
    {
        return Err(protocol(
            "Copilot cached and cache-write counts exceed input_tokens",
        ));
    }
    let reasoning = optional(details("output_tokens_details")?, "reasoning_tokens")?;
    if reasoning > output {
        return Err(protocol("Copilot reasoning_tokens exceeds output_tokens"));
    }
    Ok(Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cached,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        ..Usage::default()
    })
}
