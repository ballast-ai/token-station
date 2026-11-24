use super::*;
use south_component_conformance::sse_split::{SseSplitErrorV1, SseSplitterV1};
#[derive(Default)]
pub(crate) struct Parser {
    splitter: SseSplitterV1,
    calls: u32,
    model: String,
    thoughts: Vec<Value>,
    thought_bytes: usize,
    finish: Option<FinishReason>,
    terminal: bool,
    failed: bool,
    saw_frame: bool,
}
impl Parser {
    fn frame(&mut self, v: Value) -> ComponentResultV1<Vec<StreamEvent>> {
        self.saw_frame = true;
        if v.get("error").is_some() {
            self.failed = true;
            let p = HttpResponseParts {
                status: v["error"]["code"]
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .unwrap_or(502),
                body: v.to_string(),
                headers: Default::default(),
                extensions: Default::default(),
            };
            return Ok(vec![StreamEvent::Error {
                error: CodeAssist.map_provider_error(&p)?,
            }]);
        }
        let r = v
            .get("response")
            .ok_or_else(|| protocol("Code Assist stream envelope is missing."))?;
        if r["modelVersion"].is_string() {
            self.model = r["modelVersion"].as_str().unwrap_or_default().into();
        }
        let c = &r["candidates"][0];
        let mut events = Vec::new();
        for part in c["content"]["parts"].as_array().into_iter().flatten() {
            validate_output_part(part)?;
            if let Some(call) = part.get("functionCall") {
                let name = call["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| protocol("Code Assist returned an unnamed tool."))?;
                if self.model.is_empty() {
                    return Err(protocol("Code Assist tool stream has no model identity."));
                }
                let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
                if !arguments.is_object() {
                    return Err(protocol(
                        "Google tool arguments must contain a JSON object.",
                    ));
                }
                events.push(StreamEvent::ToolCallDelta {
                    index: self.calls,
                    id: Some(signed_id(
                        call["id"].as_str(),
                        part["thoughtSignature"].as_str().unwrap_or_default(),
                        &self.model,
                    )?),
                    name: Some(name.into()),
                    arguments_delta: arguments.to_string(),
                    // The id carries the thought signature.
                    signature: None,
                });
                self.calls += 1;
            } else if part["thought"].as_bool() == Some(true) {
                validate_thought_part(part)?;
                self.thought_bytes = self.thought_bytes.saturating_add(part.to_string().len());
                if self.thought_bytes > LIMIT {
                    return Err(protocol("Code Assist thinking exceeds its size limit."));
                }
                self.thoughts.push(part.clone());
                if let Some(text) = part["text"].as_str() {
                    events.push(StreamEvent::ThinkingDelta {
                        index: 0,
                        block_index: 0,
                        thinking_delta: text.into(),
                    });
                }
            } else if let Some(text) = part["text"].as_str() {
                // Google does not require non-function text signatures for replay.
                // The stock stream IR has no visible-text metadata event. Emit the
                // answer as text; required function-call signatures remain in IDs.
                if part
                    .get("thoughtSignature")
                    .is_some_and(|signature| !signature.is_string())
                {
                    return Err(protocol(
                        "Code Assist returned an invalid optional text signature.",
                    ));
                }
                if !text.is_empty() {
                    events.push(StreamEvent::Delta {
                        index: 0,
                        content: text.into(),
                    });
                }
            } else {
                return Err(capability(
                    "Code Assist returned an unsupported content part.",
                ));
            }
        }
        if let Some(reason) = c["finishReason"].as_str() {
            self.finish = Some(match reason {
                "STOP" if self.calls > 0 => FinishReason::ToolCalls,
                "STOP" => FinishReason::Stop,
                "MAX_TOKENS" => FinishReason::Length,
                "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" => FinishReason::ContentFilter,
                other => FinishReason::Other(other.into()),
            });
        }
        // Gemini streams running counts. Only the chunk that finishes the
        // candidate carries the turn's usage; earlier counts are progress
        // (South `provider.stream.intermediate-counts-are-progress-not-usage`).
        if c["finishReason"].is_string() {
            if let Some(meta) = r.get("usageMetadata") {
                events.push(StreamEvent::Usage {
                    usage: usage_of(meta)?,
                });
            }
        }
        Ok(events)
    }
}
impl StreamParserV1 for Parser {
    fn parse_chunk(&mut self, chunk: &[u8]) -> ComponentResultV1<Vec<StreamEvent>> {
        if self.failed || self.terminal {
            return Ok(Vec::new());
        }
        if chunk.is_empty() {
            if !self.saw_frame && self.splitter.is_idle() {
                return Ok(Vec::new());
            }
            if !self.splitter.is_idle() {
                return Err(protocol("Code Assist stream ended inside an SSE frame."));
            }
            if let Some(finish) = self.finish.take() {
                let mut events = Vec::new();
                if !self.thoughts.is_empty() {
                    events.push(StreamEvent::ThinkingSignatureDelta {
                        index: 0,
                        block_index: 0,
                        signature_delta: thinking_signature(&self.thoughts, &self.model)?,
                    });
                }
                events.extend([
                    StreamEvent::Finish {
                        finish_reason: Some(finish),
                        stop_sequence: None,
                    },
                    StreamEvent::Done {
                        finish_reason: None,
                        stop_sequence: None,
                    },
                ]);
                self.terminal = true;
                return Ok(events);
            }
            return Err(protocol(
                "Code Assist stream ended without a finish reason.",
            ));
        }
        // The SSE standard's line ends (LF, CR, CRLF), as the host decoder cuts
        // them. Frames completed before a refusal are judged first.
        let pushed = self.splitter.push(chunk);
        let mut events = Vec::new();
        for frame in pushed.frames {
            if frame.data.len() > LIMIT {
                return Err(protocol("Code Assist SSE frame exceeds its size limit."));
            }
            if frame.data.is_empty() {
                continue;
            }
            let v = serde_json::from_str(&frame.data)
                .map_err(|_| protocol("Code Assist SSE contains invalid JSON."))?;
            events.extend(self.frame(v)?);
            if self.failed {
                return Ok(events);
            }
        }
        match pushed.error {
            Some(SseSplitErrorV1::NotUtf8) => Err(protocol("Code Assist SSE is not UTF-8.")),
            Some(SseSplitErrorV1::TooLarge) => {
                Err(protocol("Code Assist SSE frame exceeds its size limit."))
            }
            None => Ok(events),
        }
    }
}
