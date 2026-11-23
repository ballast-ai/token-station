//! Anthropic search streams rendered as Responses events without buffering answer deltas.

use super::reverse_search::{from_anthropic, message_id, response_id};
use super::web_search::sse;
use super::{ErrorCode, ErrorEnvelope, MAX_UPSTREAM_BODY, Reply, Value};

use crate::sse::SseFrameDecoder;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

pub(super) struct AnthropicSearchStream<'a> {
    request: &'a Value,
    decoder: SseFrameDecoder,
    message: Value,
    arguments: BTreeMap<usize, String>,
    closed: BTreeSet<usize>,
    outputs: BTreeMap<usize, usize>,
    sequence: usize,
    bytes: usize,
    output_bytes: u64,
    pub(super) started: bool,
    complete: bool,
}

impl<'a> AnthropicSearchStream<'a> {
    pub(super) fn new(request: &'a Value) -> Self {
        Self {
            request,
            decoder: SseFrameDecoder::default(),
            message: Value::Null,
            arguments: BTreeMap::new(),
            closed: BTreeSet::new(),
            outputs: BTreeMap::new(),
            sequence: 0,
            bytes: 0,
            output_bytes: 0,
            started: false,
            complete: false,
        }
    }

    fn emit(&mut self, kind: &str, mut body: Value, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
        body["type"] = json!(kind);
        body["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        emit(Reply::Chunk(sse(kind, &body)))
    }

    fn text_delta(&mut self, ci: usize, text: &str, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
        self.emit("response.output_text.delta", json!({"output_index":self.outputs[&ci],"item_id":message_id(&self.message, ci),"content_index":0,"delta":text}), emit)
    }

    pub(super) fn push(
        &mut self,
        chunk: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        let mut bytes = self.output_bytes;
        let result = super::search_stream::limit_stream_output(&mut bytes, emit, |emit| {
            self.push_inner(chunk, emit)
        });
        self.output_bytes = bytes;
        if result.is_err() {
            self.complete = false;
        }
        result
    }

    fn push_inner(
        &mut self,
        chunk: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        self.bytes = self.bytes.saturating_add(chunk.len());
        if self.bytes as u64 > MAX_UPSTREAM_BODY {
            return Err(invalid("Native search stream exceeds the size limit."));
        }
        for frame in self.decoder.push(chunk.as_bytes())? {
            let data = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || self.complete {
                continue;
            }
            let event: Value = serde_json::from_str(&data)
                .map_err(|_| invalid("Native search stream contains invalid JSON."))?;
            if !self.event(&event, emit)? {
                return Ok(false);
            }
            let entries: usize = self.message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|block| {
                    1 + block["citations"].as_array().map_or(0, Vec::len)
                        + block["content"].as_array().map_or(0, Vec::len)
                })
                .sum();
            if entries > 4096 {
                return Err(invalid("Native search stream has too many output items."));
            }
        }
        Ok(true)
    }

    fn index(event: &Value) -> Result<usize, ErrorEnvelope> {
        event["index"]
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < 4096)
            .ok_or_else(|| invalid("Native search content index is invalid."))
    }

    #[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
    fn event(
        &mut self,
        event: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        match event["type"].as_str() {
            Some("message_start") => {
                super::web_search::check_response_id(&event["message"]["id"])?;
                if self.started
                    || event["message"]["id"].as_str().is_none()
                    || event["message"]["type"] != "message"
                {
                    return Err(invalid("Native search message_start is invalid."));
                }
                self.message = event["message"].clone();
                if self.message["content"]
                    .as_array()
                    .is_none_or(|content| !content.is_empty())
                {
                    return Err(invalid(
                        "Native search message_start must have empty content.",
                    ));
                }
                self.started = true;
                if !emit(Reply::BeginStream) {
                    return Ok(false);
                }
                Ok(self.emit("response.created", json!({"response":{"id":response_id(&self.message),"object":"response","model":self.request["model"],"status":"in_progress","output":[],"usage":null}}), emit))
            }
            Some("content_block_start") => {
                if !self.started {
                    return Err(invalid(
                        "Native search content arrived before message_start.",
                    ));
                }
                let ci = Self::index(event)?;
                let block = &event["content_block"];
                if matches!(block["type"].as_str(), Some("server_tool_use" | "tool_use"))
                    && !super::web_search::valid_call_id(&block["id"])
                {
                    return Err(invalid("Native search call ID is invalid."));
                }
                if ci != self.message["content"].as_array().unwrap().len() {
                    return Err(invalid("Native search content indexes are not sequential."));
                }
                self.message["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(block.clone());
                match block["type"].as_str() {
                    Some("server_tool_use") => {
                        if block["name"] != "web_search"
                            || block["id"].as_str().is_none_or(str::is_empty)
                        {
                            return Err(invalid(
                                "Native search returned an unsupported server tool.",
                            ));
                        }
                        let oi = self.outputs.len();
                        self.outputs.insert(ci, oi);
                        if !self.emit("response.output_item.added", json!({"output_index":oi,"item":{"type":"web_search_call","id":block["id"],"status":"in_progress","action":{"type":"search"}}}), emit) { return Ok(false); }
                        Ok(self.emit(
                            "response.web_search_call.in_progress",
                            json!({"output_index":oi,"item_id":block["id"]}),
                            emit,
                        ))
                    }
                    Some("text") => {
                        let oi = self.outputs.len();
                        self.outputs.insert(ci, oi);
                        let id = message_id(&self.message, ci);
                        if !self.emit("response.output_item.added", json!({"output_index":oi,"item":{"type":"message","id":id,"role":"assistant","status":"in_progress","content":[]}}), emit)
                            || !self.emit("response.content_part.added", json!({"output_index":oi,"item_id":id,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}), emit) { return Ok(false); }
                        let text = block["text"]
                            .as_str()
                            .ok_or_else(|| invalid("Native search text is invalid."))?;
                        Ok(text.is_empty() || self.text_delta(ci, text, emit))
                    }
                    Some("tool_use") => {
                        self.outputs.insert(ci, self.outputs.len());
                        Ok(true)
                    }
                    Some("web_search_tool_result") => Ok(true),
                    _ => Err(invalid(
                        "Native search returned an unsupported content block.",
                    )),
                }
            }
            Some("content_block_delta") => {
                let ci = Self::index(event)?;
                if self.closed.contains(&ci) {
                    return Err(invalid("Native search changed a closed content block."));
                }
                let block = self.message["content"]
                    .as_array_mut()
                    .and_then(|blocks| blocks.get_mut(ci))
                    .ok_or_else(|| invalid("Native search delta refers to an unknown block."))?;
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") if block["type"] == "text" => {
                        let text = delta["text"]
                            .as_str()
                            .ok_or_else(|| invalid("Native search text delta is invalid."))?;
                        let Value::String(current) = &mut block["text"] else {
                            return Err(invalid("Native search text is invalid."));
                        };
                        current.push_str(text);
                        Ok(self.text_delta(ci, text, emit))
                    }
                    Some("input_json_delta")
                        if matches!(
                            block["type"].as_str(),
                            Some("server_tool_use" | "tool_use")
                        ) =>
                    {
                        let text = delta["partial_json"]
                            .as_str()
                            .ok_or_else(|| invalid("Native search tool input delta is invalid."))?;
                        self.arguments.entry(ci).or_default().push_str(text);
                        Ok(true)
                    }
                    Some("citations_delta") if block["type"] == "text" => {
                        if block.get("citations").is_none() {
                            block["citations"] = json!([]);
                        }
                        block["citations"]
                            .as_array_mut()
                            .ok_or_else(|| invalid("Native citations are invalid."))?
                            .push(delta["citation"].clone());
                        Ok(true)
                    }
                    _ => Err(invalid(
                        "Native search returned an unsupported content delta.",
                    )),
                }
            }
            Some("content_block_stop") => {
                let ci = Self::index(event)?;
                if self.message["content"]
                    .as_array()
                    .is_none_or(|content| ci >= content.len())
                    || !self.closed.insert(ci)
                {
                    return Err(invalid("Native search content stop is invalid."));
                }
                if let Some(arguments) = self.arguments.remove(&ci) {
                    self.message["content"][ci]["input"] = serde_json::from_str(&arguments)
                        .map_err(|_| invalid("Native search tool arguments are invalid JSON."))?;
                }
                Ok(true)
            }
            Some("message_delta") => {
                if !self.started {
                    return Err(invalid(
                        "Native search message delta arrived before message_start.",
                    ));
                }
                if let Some(reason) = event["delta"].get("stop_reason") {
                    self.message["stop_reason"] = reason.clone();
                }
                if let Some(usage) = event["usage"].as_object() {
                    if !self.message["usage"].is_object() {
                        self.message["usage"] = json!({});
                    }
                    self.message["usage"]
                        .as_object_mut()
                        .unwrap()
                        .extend(usage.clone());
                }
                Ok(true)
            }
            Some("message_stop") => self.terminal(emit),
            Some("error") => Err(invalid("The native Anthropic search stream failed.")),
            Some("ping") => Ok(true),
            _ => Err(invalid(
                "Native search returned an unsupported stream event.",
            )),
        }
    }

    #[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
    fn terminal(&mut self, emit: &mut dyn FnMut(Reply) -> bool) -> Result<bool, ErrorEnvelope> {
        if !self.started
            || self.message["content"]
                .as_array()
                .is_none_or(|content| content.len() != self.closed.len())
        {
            return Err(invalid(
                "Native search stopped with unfinished content blocks.",
            ));
        }
        let response = from_anthropic(&self.message, self.request)?;
        let output = response["output"].as_array().unwrap();
        if output.len() != self.outputs.len() {
            return Err(invalid(
                "Native search output count changed during conversion.",
            ));
        }
        for (oi, item) in output.iter().enumerate() {
            match item["type"].as_str() {
                Some("message") => {
                    let part = &item["content"][0];
                    for (ai, annotation) in
                        part["annotations"].as_array().unwrap().iter().enumerate()
                    {
                        if !self.emit("response.output_text.annotation.added", json!({"output_index":oi,"item_id":item["id"],"content_index":0,"annotation_index":ai,"annotation":annotation}), emit) { return Ok(false); }
                    }
                    if !self.emit("response.output_text.done", json!({"output_index":oi,"item_id":item["id"],"content_index":0,"text":part["text"]}), emit)
                        || !self.emit("response.content_part.done", json!({"output_index":oi,"item_id":item["id"],"content_index":0,"part":part}), emit) { return Ok(false); }
                }
                Some("web_search_call") if item["status"] == "completed" => {
                    if !self.emit(
                        "response.web_search_call.completed",
                        json!({"output_index":oi,"item_id":item["id"]}),
                        emit,
                    ) {
                        return Ok(false);
                    }
                }
                Some("function_call") => {
                    let mut pending = item.clone();
                    pending["arguments"] = json!("");
                    pending["status"] = json!("in_progress");
                    if !self.emit("response.output_item.added", json!({"output_index":oi,"item":pending}), emit)
                        || !self.emit("response.function_call_arguments.delta", json!({"output_index":oi,"item_id":item["id"],"delta":item["arguments"]}), emit)
                        || !self.emit("response.function_call_arguments.done", json!({"output_index":oi,"item_id":item["id"],"arguments":item["arguments"]}), emit) { return Ok(false); }
                }
                _ => {}
            }
            if !self.emit(
                "response.output_item.done",
                json!({"output_index":oi,"item":item}),
                emit,
            ) {
                return Ok(false);
            }
        }
        self.complete = true;
        Ok(self.emit(
            if response["status"] == "incomplete" {
                "response.incomplete"
            } else {
                "response.completed"
            },
            json!({"response":response}),
            emit,
        ))
    }

    pub(super) fn finish(&mut self) -> Result<(), ErrorEnvelope> {
        std::mem::take(&mut self.decoder).finish()?;
        if !self.complete {
            return Err(ErrorEnvelope::new(
                ErrorCode::TransportTruncated,
                502,
                "Native search stream ended before message_stop.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(event: &Value) -> String {
        format!("data: {event}\n\n")
    }
    fn request() -> Value {
        json!({"model":"auto","tools":[{"type":"web_search"}]})
    }
    fn wire() -> Vec<Value> {
        vec![
            json!({"type":"message_start","message":{"id":"msg_test","type":"message","content":[],"usage":{"input_tokens":2,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"s1","name":"web_search","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"Rust\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"s1","content":[{"type":"web_search_result","url":"https://docs.rs/","title":"Docs"}]}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Rust 文档"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"citations_delta","citation":{"type":"web_search_result_location","url":"https://docs.rs/","title":"Docs"}}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
            json!({"type":"message_stop"}),
        ]
    }
    #[test]
    fn oversized_message_id_is_rejected_before_any_output() {
        let request = request();
        let mut bridge = AnthropicSearchStream::new(&request);
        let mut replies = 0;
        let event = json!({"type":"message_start","message":{"id":"x".repeat(1025),"type":"message","content":[]}});
        let error = bridge
            .push(&frame(&event), &mut |_| {
                replies += 1;
                true
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert_eq!(replies, 0);
        assert!(!bridge.started);
    }

    #[test]
    fn repeated_item_ids_cannot_exceed_the_converted_stream_budget() {
        let request = request();
        let mut bridge = AnthropicSearchStream::new(&request);
        let mut emitted = 0_u64;
        let mut emit = |reply| {
            if let Reply::Chunk(text) = reply {
                emitted += text.len() as u64;
            }
            true
        };
        let start = frame(
            &json!({"type":"message_start","message":{"id":"x".repeat(1024),"type":"message","content":[]}}),
        );
        let block = frame(
            &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        );
        bridge.push(&start, &mut emit).unwrap();
        bridge.push(&block, &mut emit).unwrap();
        let delta = frame(
            &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}),
        );
        let mut received = start.len() + block.len();
        let mut failure = None;
        for _ in 0..40_000 {
            received += delta.len();
            if let Err(error) = bridge.push(&delta, &mut emit) {
                failure = Some(error);
                break;
            }
        }
        let error = failure.expect("small input must not amplify beyond the output budget");
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert!((received as u64) < MAX_UPSTREAM_BODY);
        assert!(emitted > MAX_UPSTREAM_BODY / 2);
        assert!(emitted <= MAX_UPSTREAM_BODY);
        assert!(bridge.finish().is_err());
    }

    #[test]
    fn duplicate_call_ids_do_not_complete_the_stream() {
        let mut request = request();
        request["tools"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"function","name":"lookup","parameters":{"type":"object"}}));
        let mut bridge = AnthropicSearchStream::new(&request);
        let mut chunks = Vec::new();
        let mut emit = |reply| {
            if let Reply::Chunk(text) = reply {
                chunks.push(text);
            }
            true
        };
        bridge.push(&frame(&wire()[0]), &mut emit).unwrap();
        for index in 0..2 {
            bridge.push(&frame(&json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":"same","name":"lookup","input":{"x":index}}})), &mut emit).unwrap();
            bridge
                .push(
                    &frame(&json!({"type":"content_block_stop","index":index})),
                    &mut emit,
                )
                .unwrap();
        }
        bridge.push(&frame(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}})), &mut emit).unwrap();
        let error = bridge
            .push(&frame(&json!({"type":"message_stop"})), &mut emit)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert!(!chunks.join("").contains("response.completed"));
        assert!(bridge.finish().is_err());
    }

    #[test]
    fn reverse_stream_emits_early_text_and_terminal_sources_with_usage() {
        let request = request();
        let mut bridge = AnthropicSearchStream::new(&request);
        let mut chunks = Vec::new();
        for (index, event) in wire().into_iter().enumerate() {
            bridge
                .push(&frame(&event), &mut |reply| {
                    if let Reply::Chunk(text) = reply {
                        chunks.push(text);
                    }
                    true
                })
                .unwrap();
            if index == 7 {
                assert!(chunks.join("").contains("Rust 文档"));
                assert!(!chunks.join("").contains("response.completed"));
            }
        }
        bridge.finish().unwrap();
        let events: Vec<Value> = chunks
            .iter()
            .map(|chunk| {
                serde_json::from_str(
                    chunk
                        .lines()
                        .find_map(|line| line.strip_prefix("data: "))
                        .unwrap(),
                )
                .unwrap()
            })
            .collect();
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], index);
        }
        let terminal = events.last().unwrap();
        assert_eq!(terminal["type"], "response.completed");
        assert_eq!(terminal["response"]["usage"]["total_tokens"], 5);
        assert_eq!(terminal["response"]["output"][0]["status"], "completed");
        assert_eq!(
            terminal["response"]["output"][1]["content"][0]["annotations"][0]["url"],
            "https://docs.rs/"
        );
    }
    #[test]
    fn reverse_stream_rejects_truncation_failure_and_malformed_tool_json() {
        let request = request();
        let mut bridge = AnthropicSearchStream::new(&request);
        bridge
            .push(&frame(&wire()[0].clone()), &mut |_| true)
            .unwrap();
        assert!(bridge.finish().is_err());
        assert!(
            bridge
                .push(
                    &frame(&json!({"type":"error","error":{"type":"api_error"}})),
                    &mut |_| true
                )
                .is_err()
        );
        let mut bridge = AnthropicSearchStream::new(&request);
        for event in wire().into_iter().take(2) {
            bridge.push(&frame(&event), &mut |_| true).unwrap();
        }
        bridge.push(&frame(&json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}})), &mut |_| true).unwrap();
        assert!(
            bridge
                .push(
                    &frame(&json!({"type":"content_block_stop","index":0})),
                    &mut |_| true
                )
                .is_err()
        );
    }
    #[test]
    fn reverse_stream_cancellation_stops_before_terminal() {
        let request = request();
        let mut bridge = AnthropicSearchStream::new(&request);
        assert!(
            !bridge
                .push(&frame(&wire()[0].clone()), &mut |_| false)
                .unwrap()
        );
    }
}
