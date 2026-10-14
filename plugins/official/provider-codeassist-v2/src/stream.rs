use super::*;
#[derive(Default)]
pub(crate) struct Parser {
    tail: Vec<u8>,
    calls: u32,
    model: String,
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
            if let Some(call) = part.get("functionCall") {
                let name = call["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| protocol("Code Assist returned an unnamed tool."))?;
                if self.model.is_empty() {
                    return Err(protocol("Code Assist tool stream has no model identity."));
                }
                let id = call["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("call_{}_{}", self.calls, name));
                events.push(StreamEvent::ToolCallDelta {
                    index: self.calls,
                    id: Some(signed_id(
                        &id,
                        part["thoughtSignature"].as_str().unwrap_or_default(),
                        &self.model,
                    )),
                    name: Some(name.into()),
                    arguments_delta: call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string(),
                });
                self.calls += 1;
            } else if part["thought"].as_bool() == Some(true) {
                if let Some(text) = part["text"].as_str() {
                    events.push(StreamEvent::ThinkingDelta {
                        index: 0,
                        thinking_delta: text.into(),
                    });
                }
                if let Some(sig) = part["thoughtSignature"].as_str() {
                    events.push(StreamEvent::ThinkingSignatureDelta {
                        index: 0,
                        signature_delta: sig.into(),
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
        if let Some(meta) = r.get("usageMetadata") {
            let n = |s: &str| meta[s].as_u64().unwrap_or(0);
            let mut usage = Usage {
                input_tokens: n("promptTokenCount"),
                output_tokens: n("candidatesTokenCount"),
                cache_read_tokens: n("cachedContentTokenCount"),
                reasoning_tokens: n("thoughtsTokenCount"),
                ..Usage::default()
            };
            adjust_usage(&mut usage);
            events.push(StreamEvent::Usage { usage });
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
            if !self.saw_frame && self.tail.is_empty() {
                return Ok(Vec::new());
            }
            if !self.tail.is_empty() {
                return Err(protocol("Code Assist stream ended inside an SSE frame."));
            }
            if let Some(finish) = self.finish.take() {
                self.terminal = true;
                return Ok(vec![
                    StreamEvent::Finish {
                        finish_reason: Some(finish),
                        stop_sequence: None,
                    },
                    StreamEvent::Done {
                        finish_reason: None,
                        stop_sequence: None,
                    },
                ]);
            }
            return Err(protocol(
                "Code Assist stream ended without a finish reason.",
            ));
        }
        if self.tail.len() + chunk.len() > LIMIT {
            return Err(protocol("Code Assist SSE frame exceeds its size limit."));
        }
        self.tail.extend_from_slice(chunk);
        let mut events = Vec::new();
        loop {
            let a = self
                .tail
                .windows(2)
                .position(|x| x == b"\n\n")
                .map(|p| (p, 2));
            let b = self
                .tail
                .windows(4)
                .position(|x| x == b"\r\n\r\n")
                .map(|p| (p, 4));
            let boundary = match (a, b) {
                (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                (a, None) => a,
                (None, b) => b,
            };
            let Some((at, sep)) = boundary else { break };
            let frame = self.tail.drain(..at + sep).collect::<Vec<_>>();
            let text = std::str::from_utf8(&frame[..at])
                .map_err(|_| protocol("Code Assist SSE is not UTF-8."))?;
            let data = text
                .lines()
                .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let v = serde_json::from_str(&data)
                .map_err(|_| protocol("Code Assist SSE contains invalid JSON."))?;
            events.extend(self.frame(v)?);
            if self.failed {
                self.tail.clear();
                break;
            }
        }
        Ok(events)
    }
}
