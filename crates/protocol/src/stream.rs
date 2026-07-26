use serde::{Deserialize, Serialize};

use crate::{ErrorEnvelope, FinishReason, Usage};

/// One raw fragment of a provider's streaming body, before parsing.
///
/// The host reads it off the socket and hands it to a `provider-adapter`, which
/// must turn it into zero or more [`StreamEvent`]s without buffering the whole
/// body. A fragment is not guaranteed to be a complete SSE frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamChunk {
    pub data: String,
}

/// A normalized streaming event.
///
/// Internally tagged so a stream stays readable in logs and fixtures. Variants
/// carry no [`crate::Extensions`]: an unknown field inside a variant is ignored
/// rather than preserved, because a stream event is consumed once and never
/// re-serialized back to the peer that sent it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Incremental assistant text for the choice at `index`.
    Delta { index: u32, content: String },
    /// Incremental tool-call arguments. `id` and `name` arrive on the first
    /// fragment of a call and are absent afterwards, matching how providers
    /// stream them.
    ToolCallDelta {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        arguments_delta: String,
    },
    /// Token accounting. Most providers emit this once, just before `done` —
    /// but a stream may legitimately carry it **more than once** (Anthropic
    /// reports input-side buckets in `message_start` and the final output
    /// count in `message_delta`). Consumers fold repeated reports with
    /// [`crate::Usage::absorb`]: field-wise, last-nonzero-wins, so a zero in
    /// a later report never erases an earlier nonzero bucket.
    Usage { usage: Usage },
    /// Incremental reasoning text for the choice at `index` (Anthropic
    /// `thinking_delta`; OpenAI-compatible `reasoning_content` deltas).
    ThinkingDelta { index: u32, thinking_delta: String },
    /// The signature fragment closing a thinking block (Anthropic
    /// `signature_delta`). Arrives after the block's text deltas.
    ThinkingSignatureDelta { index: u32, signature_delta: String },
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
        /// The stop sequence that fired (see [`crate::Choice::stop_sequence`];
        /// Anthropic reports it alongside `stop_reason` in `message_delta`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_sequence: Option<String>,
    },
    /// A mid-stream failure. The host stops the exchange when it sees this; an
    /// adapter must not keep emitting events afterwards.
    Error { error: ErrorEnvelope },
}

/// How one streaming exchange actually ended.
///
/// The reliability keystone: before this existed, a stream that broke after a
/// half-sentence still returned `()` and the caller recorded HTTP 200. Only
/// [`StreamOutcome::Complete`] is allowed to settle as success; every other
/// variant is a distinct, non-lying terminal state that health, billing, the
/// request record and the client all read from one `settle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamOutcome {
    /// Upstream closed cleanly after a terminal event (`[DONE]` / finish reason
    /// / `message_stop`). The only outcome that may be recorded as success.
    Complete,
    /// Already committed to the stream (`BeginStream` emitted, some output sent)
    /// when the upstream broke, errored, or produced an unparseable frame — the
    /// answer is incomplete. Never a success; never transparently replayed.
    FailedAfterPartial,
    /// Failed before a single output byte cleared the checkpoint. No content
    /// reached the client, so a candidate may still be tried within budget.
    FailedBeforeOutput,
    /// The client hung up or a drain deadline fired: a deliberate cancellation,
    /// not the upstream's fault. Not counted against upstream health.
    ClientCancelled,
}

#[cfg(test)]
mod tests {
    use super::StreamEvent;
    use crate::{ErrorCode, ErrorEnvelope, FinishReason, Usage};

    #[test]
    fn delta_is_internally_tagged() {
        let event = StreamEvent::Delta {
            index: 0,
            content: "hel".to_owned(),
        };
        let json = serde_json::to_string(&event).expect("serializable event");

        assert_eq!(json, r#"{"type":"delta","index":0,"content":"hel"}"#);
    }

    #[test]
    fn tool_call_delta_omits_absent_identity_on_continuation_fragments() {
        let event = StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: r#"ing"}"#.to_owned(),
        };
        let json = serde_json::to_value(&event).expect("serializable event");

        assert!(json.get("id").is_none());
        assert!(json.get("name").is_none());
    }

    #[test]
    fn every_variant_round_trips() {
        let events = vec![
            StreamEvent::Delta {
                index: 0,
                content: "hi".to_owned(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".to_owned()),
                name: Some("get_weather".to_owned()),
                arguments_delta: "{\"city".to_owned(),
            },
            StreamEvent::Usage {
                usage: Usage {
                    input_tokens: 3,
                    output_tokens: 1,
                    ..Usage::default()
                },
            },
            StreamEvent::ThinkingDelta {
                index: 0,
                thinking_delta: "hmm ".to_owned(),
            },
            StreamEvent::ThinkingSignatureDelta {
                index: 0,
                signature_delta: "EqQBCg".to_owned(),
            },
            StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            },
            StreamEvent::Done {
                finish_reason: Some(FinishReason::StopSequence),
                stop_sequence: Some("\n\nHuman:".to_owned()),
            },
            StreamEvent::Done {
                finish_reason: Some(FinishReason::Other("weird_reason".to_owned())),
                stop_sequence: None,
            },
            StreamEvent::Error {
                error: ErrorEnvelope::new(ErrorCode::RateLimit, 429, "slow down"),
            },
        ];

        let encoded = serde_json::to_string(&events).expect("serializable events");
        let decoded: Vec<StreamEvent> = serde_json::from_str(&encoded).expect("valid events");

        assert_eq!(decoded, events);
    }

    #[test]
    fn unknown_event_type_is_a_version_mismatch() {
        let parsed: Result<StreamEvent, _> = serde_json::from_str(r#"{"type":"audio_delta"}"#);

        assert!(parsed.is_err());
    }
}
