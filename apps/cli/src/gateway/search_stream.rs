//! Incremental Responses-to-Anthropic search output with terminal citation reconciliation.

use super::web_search::{emit_content, from_responses, search_action_input, search_id, sse};
use super::{ErrorCode, ErrorEnvelope, MAX_UPSTREAM_BODY, Reply, Value};

use crate::sse::SseFrameDecoder;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

type TextKey = (usize, usize);

fn invalid(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

pub(super) fn limit_stream_output(
    bytes: &mut u64,
    emit: &mut dyn FnMut(Reply) -> bool,
    operation: impl FnOnce(&mut dyn FnMut(Reply) -> bool) -> Result<bool, ErrorEnvelope>,
) -> Result<bool, ErrorEnvelope> {
    let mut exceeded = false;
    let result = operation(&mut |reply| {
        if let Reply::Chunk(text) = &reply {
            let next = bytes.saturating_add(text.len() as u64);
            if next > MAX_UPSTREAM_BODY {
                exceeded = true;
                return false;
            }
            *bytes = next;
        }
        emit(reply)
    });
    if exceeded {
        Err(invalid(
            "Converted search stream exceeds the output size limit.",
        ))
    } else {
        result
    }
}

pub(super) struct ResponsesSearchStream<'a> {
    request: &'a Value,
    name: &'a str,
    max_uses: Option<u64>,
    decoder: SseFrameDecoder,
    id: Option<String>,
    items: BTreeMap<usize, Value>,
    text: BTreeMap<TextKey, String>,
    annotations: BTreeMap<TextKey, Vec<Value>>,
    uses: BTreeMap<usize, Value>,
    next_block: usize,
    open_text: Option<(TextKey, usize)>,
    bytes: usize,
    output_bytes: u64,
    pub(super) started: bool,
    complete: bool,
}

impl<'a> ResponsesSearchStream<'a> {
    pub(super) fn new(request: &'a Value, name: &'a str, max_uses: Option<u64>) -> Self {
        Self {
            request,
            name,
            max_uses,
            decoder: SseFrameDecoder::default(),
            id: None,
            items: BTreeMap::new(),
            text: BTreeMap::new(),
            annotations: BTreeMap::new(),
            uses: BTreeMap::new(),
            next_block: 0,
            open_text: None,
            bytes: 0,
            output_bytes: 0,
            started: false,
            complete: false,
        }
    }

    fn start(&mut self, id: &str, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
        if self.started {
            return true;
        }
        self.id = Some(id.to_owned());
        self.started = true;
        emit(Reply::BeginStream)
            && emit(Reply::Chunk(sse(
                "message_start",
                &json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant","model":self.request["model"],"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}),
            )))
    }

    fn close_text(&mut self, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
        self.open_text.take().is_none_or(|(_, index)| {
            emit(Reply::Chunk(sse(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":index}),
            )))
        })
    }

    fn block(&mut self, block: &Value, emit: &mut dyn FnMut(Reply) -> bool) -> bool {
        if !self.close_text(emit) {
            return false;
        }
        let index = self.next_block;
        self.next_block += 1;
        emit_content(index, block, emit)
    }

    fn permitted(&self, index: usize) -> bool {
        self.max_uses.is_none_or(|limit| {
            self.items
                .range(..=index)
                .filter(|(_, item)| item["type"] == "web_search_call")
                .count() as u64
                <= limit
        })
    }

    fn delta(
        &mut self,
        key: TextKey,
        text: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        if !self.started {
            return Err(invalid("Search text arrived before response.created."));
        }
        if !self.permitted(key.0) {
            return Ok(true);
        }
        if self.open_text.is_none_or(|(current, _)| current != key) {
            if !self.close_text(emit) {
                return Ok(false);
            }
            let index = self.next_block;
            self.next_block += 1;
            self.open_text = Some((key, index));
            if !emit(Reply::Chunk(sse(
                "content_block_start",
                &json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
            ))) {
                return Ok(false);
            }
        }
        self.text.entry(key).or_default().push_str(text);
        let index = self.open_text.unwrap().1;
        Ok(emit(Reply::Chunk(sse(
            "content_block_delta",
            &json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
        ))))
    }

    pub(super) fn push(
        &mut self,
        chunk: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        let mut bytes = self.output_bytes;
        let result = limit_stream_output(&mut bytes, emit, |emit| self.push_inner(chunk, emit));
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
            return Err(invalid("Search stream exceeds the conversion size limit."));
        }
        for frame in self.decoder.push(chunk.as_bytes())? {
            let data = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" || self.complete {
                continue;
            }
            let event: Value = serde_json::from_str(&data)
                .map_err(|_| invalid("Search stream contains invalid JSON."))?;
            if !self.event(&event, emit)? {
                return Ok(false);
            }
            let entries = self.items.len()
                + self.text.len()
                + self.annotations.values().map(Vec::len).sum::<usize>();
            if entries > 4096 || self.next_block > 4096 {
                return Err(invalid("Search stream has too many output items."));
            }
        }
        Ok(true)
    }

    fn index(event: &Value, field: &str) -> Result<usize, ErrorEnvelope> {
        event[field]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value < 4096)
            .ok_or_else(|| invalid("Search stream has an invalid item index."))
    }

    #[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
    fn event(
        &mut self,
        event: &Value,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        match event["type"].as_str() {
            Some("response.created") => {
                super::web_search::check_response_id(&event["response"]["id"])?;
                let id = event["response"]["id"]
                    .as_str()
                    .ok_or_else(|| invalid("Search response ID is missing."))?;
                if self.id.as_deref().is_some_and(|previous| previous != id) {
                    return Err(invalid("Search stream changed its response ID."));
                }
                Ok(self.start(id, emit))
            }
            Some("response.output_item.added" | "response.output_item.done") => {
                let index = Self::index(event, "output_index")?;
                let item = &event["item"];
                self.items.insert(index, item.clone());
                for (ci, block) in item["content"].as_array().into_iter().flatten().enumerate() {
                    if let Some(annotations) = block["annotations"].as_array() {
                        self.annotations
                            .entry((index, ci))
                            .or_default()
                            .extend(annotations.clone());
                    }
                }
                if event["type"] == "response.output_item.done"
                    && item["type"] == "web_search_call"
                    && !self.uses.contains_key(&index)
                    && self.permitted(index)
                {
                    let id = self
                        .id
                        .as_deref()
                        .ok_or_else(|| invalid("Search call arrived before response.created."))?;
                    let count = self
                        .items
                        .range(..=index)
                        .filter(|(_, item)| item["type"] == "web_search_call")
                        .count() as u64;
                    let block = json!({"type":"server_tool_use","id":search_id(id, count),"name":self.name,"input":search_action_input(item)});
                    self.uses.insert(index, block.clone());
                    return Ok(self.block(&block, emit));
                }
                Ok(true)
            }
            Some("response.output_text.delta" | "response.refusal.delta") => {
                let key = (
                    Self::index(event, "output_index")?,
                    Self::index(event, "content_index")?,
                );
                let text = event["delta"]
                    .as_str()
                    .ok_or_else(|| invalid("Search text delta is invalid."))?;
                self.delta(key, text, emit)
            }
            Some("response.output_text.annotation.added") => {
                let key = (
                    Self::index(event, "output_index")?,
                    Self::index(event, "content_index")?,
                );
                self.annotations
                    .entry(key)
                    .or_default()
                    .push(event["annotation"].clone());
                Ok(true)
            }
            Some("response.completed" | "response.incomplete") => {
                self.terminal(event["response"].clone(), emit)
            }
            Some("response.failed" | "error") => Err(invalid("The native search stream failed.")),
            _ => Ok(true),
        }
    }

    #[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
    fn terminal(
        &mut self,
        mut document: Value,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        super::web_search::check_response_id(&document["id"])?;
        let id = document["id"]
            .as_str()
            .ok_or_else(|| invalid("Terminal search response ID is missing."))?
            .to_owned();
        if self.id.as_deref().is_some_and(|previous| previous != id) {
            return Err(invalid("Terminal search response ID does not match."));
        }
        let output = document["output"]
            .as_array_mut()
            .ok_or_else(|| invalid("Terminal search output is missing."))?;
        for (&oi, observed) in &self.items {
            let item = output
                .get_mut(oi)
                .ok_or_else(|| invalid("Terminal search output omitted an observed item."))?;
            if item["type"] != observed["type"]
                || (item["id"].is_string()
                    && observed["id"].is_string()
                    && item["id"] != observed["id"])
            {
                return Err(invalid("Terminal search item identity changed."));
            }
            if item["type"] == "web_search_call" {
                if item.get("action").is_none() {
                    item["action"] = json!({});
                }
                if !item["action"].is_object() {
                    return Err(invalid("Terminal search action must be an object."));
                }
                for field in ["type", "query", "queries", "url", "pattern", "sources"] {
                    if item["action"].get(field).is_none()
                        && let Some(value) = observed["action"].get(field)
                    {
                        item["action"][field] = value.clone();
                    }
                }
            }
        }
        for (&(oi, ci), annotations) in &self.annotations {
            let block = output
                .get_mut(oi)
                .and_then(|item| item.get_mut("content"))
                .and_then(Value::as_array_mut)
                .and_then(|content| content.get_mut(ci))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| invalid("Search annotation refers to an invalid text block."))?;
            let target = block
                .entry("annotations")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| invalid("Terminal search annotations are invalid."))?;
            for annotation in annotations {
                if !target.contains(annotation) {
                    target.push(annotation.clone());
                }
            }
        }
        let message = from_responses(
            &document,
            self.name,
            self.request["model"].as_str().unwrap_or("auto"),
            self.max_uses,
            self.request,
        )?;
        let content = message["content"].as_array().unwrap();
        let mut cursor = 0;
        let mut pending = Vec::new();
        let mut seen_text = BTreeSet::new();
        let mut seen_uses = BTreeSet::new();
        for (oi, item) in document["output"].as_array().unwrap().iter().enumerate() {
            if cursor >= content.len() {
                break;
            }
            match item["type"].as_str() {
                Some("web_search_call") => {
                    if let Some(previous) = self.uses.get(&oi) {
                        if previous != &content[cursor] {
                            return Err(invalid(
                                "Terminal search action changed after it was emitted.",
                            ));
                        }
                        seen_uses.insert(oi);
                    } else {
                        pending.push(content[cursor].clone());
                    }
                    pending.push(content[cursor + 1].clone());
                    cursor += 2;
                }
                Some("message") => {
                    for ci in 0..item["content"].as_array().unwrap().len() {
                        let mut block = content[cursor].clone();
                        if let Some(previous) = self.text.get(&(oi, ci)) {
                            let text = block["text"].as_str().unwrap_or("");
                            let remaining = text.strip_prefix(previous).ok_or_else(|| {
                                invalid("Terminal search text contradicts emitted text.")
                            })?;
                            block["text"] = json!(remaining);
                            seen_text.insert((oi, ci));
                        }
                        if block["text"].as_str().is_some_and(|text| !text.is_empty()) {
                            pending.push(block);
                        }
                        cursor += 1;
                    }
                }
                Some("function_call") => {
                    pending.push(content[cursor].clone());
                    cursor += 1;
                }
                _ => {}
            }
        }
        if seen_text.len() != self.text.len() || seen_uses.len() != self.uses.len() {
            return Err(invalid("Terminal search output omitted emitted content."));
        }
        if !self.start(&id, emit) || !self.close_text(emit) {
            return Ok(false);
        }
        for block in pending {
            if !self.block(&block, emit) {
                return Ok(false);
            }
        }
        if !emit(Reply::Chunk(sse(
            "message_delta",
            &json!({"type":"message_delta","delta":{"stop_reason":message["stop_reason"],"stop_sequence":null},"usage":message["usage"]}),
        ))) {
            return Ok(false);
        }
        self.complete = true;
        Ok(emit(Reply::Chunk(sse(
            "message_stop",
            &json!({"type":"message_stop"}),
        ))))
    }

    pub(super) fn finish(&mut self) -> Result<(), ErrorEnvelope> {
        std::mem::take(&mut self.decoder).finish()?;
        if !self.complete {
            return Err(ErrorEnvelope::new(
                ErrorCode::TransportTruncated,
                502,
                "Search stream ended before a terminal response.",
            ));
        }
        Ok(())
    }
}

pub(super) fn emit_failure(
    anthropic: bool,
    error: &ErrorEnvelope,
    emit: &mut dyn FnMut(Reply) -> bool,
) {
    let body = if anthropic {
        json!({"type":"error","error":{"type":"api_error","message":error.message}})
    } else {
        json!({"type":"error","code":"provider_protocol_error","message":error.message,"param":null})
    };
    emit(Reply::Chunk(sse("error", &body)));
}

pub(super) enum SearchStream<'a> {
    IntoAnthropic(ResponsesSearchStream<'a>),
    IntoResponses(super::reverse_search_stream::AnthropicSearchStream<'a>),
}

impl<'a> SearchStream<'a> {
    pub(super) fn new(
        anthropic: bool,
        request: &'a Value,
        name: &'a str,
        max_uses: Option<u64>,
    ) -> Self {
        if anthropic {
            Self::IntoAnthropic(ResponsesSearchStream::new(request, name, max_uses))
        } else {
            Self::IntoResponses(super::reverse_search_stream::AnthropicSearchStream::new(
                request,
            ))
        }
    }
    pub(super) fn started(&self) -> bool {
        match self {
            Self::IntoAnthropic(stream) => stream.started,
            Self::IntoResponses(stream) => stream.started,
        }
    }
    pub(super) fn push(
        &mut self,
        chunk: &str,
        emit: &mut dyn FnMut(Reply) -> bool,
    ) -> Result<bool, ErrorEnvelope> {
        match self {
            Self::IntoAnthropic(stream) => stream.push(chunk, emit),
            Self::IntoResponses(stream) => stream.push(chunk, emit),
        }
    }
    pub(super) fn finish(&mut self) -> Result<(), ErrorEnvelope> {
        match self {
            Self::IntoAnthropic(stream) => stream.finish(),
            Self::IntoResponses(stream) => stream.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: &Value) -> String {
        format!("data: {value}\n\n")
    }
    fn request() -> Value {
        json!({"model":"client-model","tools":[{"type":"web_search_20250305","name":"web_search"}]})
    }
    fn document() -> Value {
        json!({"id":"resp_test","status":"completed","output":[
            {"type":"web_search_call","id":"ws_one","status":"completed","action":{"type":"search","queries":["Rust","Cargo"],"sources":[{"url":"https://docs.rs/"}]}},
            {"type":"web_search_call","id":"ws_two","status":"failed","action":{"type":"open_page","url":"https://docs.rs/"}},
            {"type":"message","content":[{"type":"output_text","text":"Rust 文档","annotations":[]}]}
        ],"usage":{"input_tokens":10,"output_tokens":4}})
    }
    fn chunks(replies: &[Reply]) -> String {
        replies
            .iter()
            .filter_map(|reply| {
                if let Reply::Chunk(text) = reply {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn rejects_terminal_annotations_on_non_object_content_without_panicking() {
        let malformed = [
            json!(42),
            json!(false),
            json!("invalid"),
            json!([]),
            Value::Null,
        ];
        for output in malformed.iter().flat_map(|block| {
            [
                json!([{"type":"message","content":[block]}]),
                json!([block]),
            ]
        }) {
            let request = request();
            let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
            let mut replies = Vec::new();
            for event in [
                json!({"type":"response.created","response":{"id":"resp_test"}}),
                json!({"type":"response.output_text.annotation.added","output_index":0,"content_index":0,"annotation":{"type":"url_citation","url":"https://docs.rs/","title":"Docs"}}),
            ] {
                bridge
                    .push(&frame(&event), &mut |reply| {
                        replies.push(reply);
                        true
                    })
                    .unwrap();
            }
            let error = bridge.push(
                &frame(&json!({"type":"response.completed","response":{"id":"resp_test","status":"completed","output":output}})),
                &mut |reply| {
                    replies.push(reply);
                    true
                },
            ).expect_err("invalid terminal content must produce a protocol error");
            assert_eq!(error.code, ErrorCode::ProviderProtocolError);
            assert!(!chunks(&replies).contains("message_stop"));
            assert!(bridge.finish().is_err());
        }
    }

    #[test]
    fn converted_output_is_bounded_independently_of_input() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let mut emitted = 0_u64;
        let mut emit = |reply| {
            if let Reply::Chunk(text) = reply {
                emitted += text.len() as u64;
            }
            true
        };
        let start = frame(&json!({"type":"response.created","response":{"id":"r"}}));
        bridge.push(&start, &mut emit).unwrap();
        let delta = frame(
            &json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"x"}),
        );
        let mut received = start.len();
        let mut failure = None;
        for _ in 0..300_000 {
            received += delta.len();
            if let Err(error) = bridge.push(&delta, &mut emit) {
                failure = Some(error);
                break;
            }
        }
        let error = failure.expect("converted output must have a separate size limit");
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert!(error.message.contains("output size limit"));
        assert!((received as u64) < MAX_UPSTREAM_BODY);
        assert!(emitted <= MAX_UPSTREAM_BODY);
        assert!(bridge.finish().is_err());
    }

    #[test]
    fn duplicate_call_ids_do_not_complete_the_stream() {
        let mut request = request();
        request["tools"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"lookup","input_schema":{"type":"object"}}));
        request["tool_choice"] = json!({"type":"auto"});
        let client =
            json!({"type":"function_call","call_id":"same","name":"lookup","arguments":"{}"});
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let mut replies = Vec::new();
        bridge
            .push(
                &frame(&json!({"type":"response.created","response":{"id":"resp_test"}})),
                &mut |r| {
                    replies.push(r);
                    true
                },
            )
            .unwrap();
        let error = bridge.push(
            &frame(&json!({"type":"response.completed","response":{"id":"resp_test","status":"completed","output":[client, client]}})),
            &mut |r| { replies.push(r); true },
        ).unwrap_err();
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert!(!chunks(&replies).contains("message_stop"));
        assert!(bridge.finish().is_err());
    }

    #[test]
    fn streams_text_early_and_preserves_late_citations_and_failed_calls() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", Some(3));
        let mut replies = Vec::new();
        let doc = document();
        for event in [
            json!({"type":"response.created","response":{"id":"resp_test"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":doc["output"][0]}),
            json!({"type":"response.output_item.done","output_index":1,"item":doc["output"][1]}),
            json!({"type":"response.output_text.delta","output_index":2,"content_index":0,"delta":"Rust 文档"}),
        ] {
            assert!(
                bridge
                    .push(&frame(&event), &mut |r| {
                        replies.push(r);
                        true
                    })
                    .unwrap()
            );
        }
        assert!(chunks(&replies).contains("Rust 文档"));
        assert!(!chunks(&replies).contains("message_stop"));
        let annotation = json!({"type":"response.output_text.annotation.added","output_index":2,"content_index":0,"annotation":{"type":"url_citation","url":"https://docs.rs/","title":"Rust docs"}});
        bridge
            .push(&frame(&annotation), &mut |r| {
                replies.push(r);
                true
            })
            .unwrap();
        bridge
            .push(
                &frame(&json!({"type":"response.completed","response":doc})),
                &mut |r| {
                    replies.push(r);
                    true
                },
            )
            .unwrap();
        bridge.finish().unwrap();
        let output = chunks(&replies);
        assert_eq!(output.matches("Rust 文档").count(), 1);
        assert!(output.contains("Rust docs"));
        assert!(output.contains("unavailable"));
        assert!(output.contains("queries"));
        assert_eq!(output.matches("event: message_stop").count(), 1);
    }

    #[test]
    fn terminal_snapshot_keeps_previously_observed_action_and_sources() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let mut replies = Vec::new();
        let mut doc = document();
        let observed = doc["output"][0].clone();
        doc["output"][0].as_object_mut().unwrap().remove("action");
        for event in [
            json!({"type":"response.created","response":{"id":"resp_test"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":observed}),
            json!({"type":"response.completed","response":doc}),
        ] {
            bridge
                .push(&frame(&event), &mut |reply| {
                    replies.push(reply);
                    true
                })
                .unwrap();
        }
        bridge.finish().unwrap();
        let wire = chunks(&replies);
        assert!(wire.contains("https://docs.rs/"));
        assert!(wire.contains("Cargo"));
        assert!(wire.contains("message_stop"));
    }

    #[test]
    fn malformed_terminal_action_returns_an_error() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let mut doc = document();
        bridge
            .push(
                &frame(&json!({"type":"response.created","response":{"id":"resp_test"}})),
                &mut |_| true,
            )
            .unwrap();
        bridge.push(&frame(&json!({"type":"response.output_item.done","output_index":0,"item":doc["output"][0]})), &mut |_| true).unwrap();
        doc["output"][0]["action"] = json!("malformed");
        assert!(
            bridge
                .push(
                    &frame(&json!({"type":"response.completed","response":doc})),
                    &mut |_| true
                )
                .is_err()
        );
    }

    #[test]
    fn truncated_or_failed_stream_never_reports_success() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        bridge
            .push(
                &frame(&json!({"type":"response.created","response":{"id":"r"}})),
                &mut |_| true,
            )
            .unwrap();
        assert!(bridge.finish().is_err());
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        assert!(
            bridge
                .push(
                    &frame(&json!({"type":"response.failed","response":{"status":"failed"}})),
                    &mut |_| true
                )
                .is_err()
        );
    }

    #[test]
    fn cancellation_stops_emission_and_fragmented_frames_keep_unicode() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        assert!(
            !bridge
                .push(
                    &frame(&json!({"type":"response.created","response":{"id":"r"}})),
                    &mut |_| false
                )
                .unwrap()
        );
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let wire = frame(&json!({"type":"response.created","response":{"id":"resp_test"}}))
            + &frame(&json!({"type":"response.completed","response":document()}));
        let mut output = Vec::new();
        for ch in wire.chars() {
            bridge
                .push(&ch.to_string(), &mut |r| {
                    output.push(r);
                    true
                })
                .unwrap();
        }
        bridge.finish().unwrap();
        assert!(chunks(&output).contains("Rust 文档"));
    }

    #[test]
    fn contradictory_terminal_text_is_rejected_without_success() {
        let request = request();
        let mut bridge = ResponsesSearchStream::new(&request, "web_search", None);
        let mut output = Vec::new();
        for event in [
            json!({"type":"response.created","response":{"id":"resp_test"}}),
            json!({"type":"response.output_text.delta","output_index":2,"content_index":0,"delta":"Contradiction"}),
        ] {
            bridge
                .push(&frame(&event), &mut |r| {
                    output.push(r);
                    true
                })
                .unwrap();
        }
        assert!(
            bridge
                .push(
                    &frame(&json!({"type":"response.completed","response":document()})),
                    &mut |r| {
                        output.push(r);
                        true
                    }
                )
                .is_err()
        );
        assert!(!chunks(&output).contains("message_stop"));
    }
}
