use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Extensions, Usage};

/// Who authored a [`Message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    /// The result of a tool the assistant called, fed back into the exchange.
    Tool,
}

/// A single part of a multimodal message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    ImageUrl {
        image_url: ImageUrl,
    },
    /// A model reasoning block (Anthropic `thinking`). `signature` is the
    /// provider's verification ticket for replaying the block on a later
    /// turn — adapters must round-trip it untouched.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An encrypted reasoning block (Anthropic `redacted_thinking`): opaque,
    /// but must survive a round trip byte-for-byte at the value level.
    RedactedThinking {
        data: String,
    },
    /// Any part whose `type` this crate does not model. The raw object is
    /// preserved verbatim so translation never silently drops content
    /// (0.3.0; known tags above always win during deserialization).
    #[serde(untagged)]
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Message body: plain text, or ordered parts when the message is multimodal.
///
/// Untagged so the wire form matches what agents already send: a bare string,
/// or an array of parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// A tool the assistant asked the caller to run.
///
/// `arguments` stays a JSON string rather than a parsed value: providers stream
/// it in fragments and do not guarantee it parses until the call is complete,
/// so parsing here would force adapters to buffer and would lose the exact bytes
/// the model produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// A stable view of a tool call's `arguments`, for comparison, de-duplication
/// or a receipt — computed without ever changing what goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalArguments {
    /// The arguments parsed as JSON, re-serialized deterministically (object
    /// keys sorted, insignificant whitespace dropped). Two calls that mean the
    /// same thing produce the same string here.
    Canonical(String),
    /// The arguments are not valid JSON. The exact bytes the model produced are
    /// preserved and flagged — never silently rewritten into something with a
    /// different meaning.
    Unparseable(String),
}

impl ToolCall {
    /// Canonicalizes `arguments` for a stable representation. Valid JSON becomes
    /// a deterministic re-serialization; anything else comes back verbatim and
    /// marked [`CanonicalArguments::Unparseable`]. The wire `arguments` string is
    /// untouched — this is a view, not a mutation, so the exact bytes still reach
    /// the tool.
    #[must_use]
    pub fn canonical_arguments(&self) -> CanonicalArguments {
        match serde_json::from_str::<serde_json::Value>(&self.arguments) {
            // serde_json::Value orders object keys (no `preserve_order`), so the
            // re-serialization is canonical.
            Ok(value) => serde_json::to_string(&value).map_or_else(
                |_| CanonicalArguments::Unparseable(self.arguments.clone()),
                CanonicalArguments::Canonical,
            ),
            Err(_) => CanonicalArguments::Unparseable(self.arguments.clone()),
        }
    }
}

/// A tool the caller offers to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's parameters, passed through opaquely.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: Value },
}

/// Sampling parameters, all optional because providers disagree on defaults.
///
/// A `provider-adapter` drops what its provider does not support rather than
/// approximating it; silently mapping `top_p` onto `temperature` would make
/// routing between providers change output in ways the caller cannot see.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sampling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set on a [`Role::Tool`] message to say which call it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl Message {
    #[must_use]
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(Content::Text(text.into())),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            extensions: Extensions::new(),
        }
    }
}

/// How the caller constrains tool selection.
///
/// String forms (`"auto"`/`"none"`/`"required"`) are canonical; provider
/// object forms (OpenAI `{"type":"function",…}`, Anthropic
/// `{"type":"tool",…}`) ride in [`ToolChoice::Other`] verbatim — adapters
/// translate at the wire boundary. Anthropic's `"any"` normalizes to
/// [`ToolChoice::Required`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    /// A provider-specific object form, preserved verbatim.
    #[serde(untagged)]
    Other(Value),
}

/// A normalized chat request, the only request shape the router sees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The model the caller asked for. Routing may replace it; the original is
    /// preserved in the decision record, not here.
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Promoted from `extensions` in 0.3.0. The wire shape is unchanged —
    /// `extensions` is flattened, so `tool_choice` was already a top-level
    /// key; it is merely typed now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default)]
    pub sampling: Sampling,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl ChatRequest {
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            response_format: None,
            tool_choice: None,
            sampling: Sampling::default(),
            stream: false,
            extensions: Extensions::new(),
        }
    }
}

/// Why generation stopped.
///
/// Not `Copy` since 0.3.0: [`FinishReason::Other`] carries the raw wire
/// string so unknown reasons survive translation instead of collapsing
/// into a known variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    /// The model hit one of the caller's stop sequences (Anthropic
    /// `stop_sequence`). Which one is reported in [`Choice::stop_sequence`]
    /// (or `StreamEvent::Done`), not here — the reason and the matched
    /// string arrive at different times in a stream.
    StopSequence,
    /// A reason this crate does not model, preserved verbatim (0.3.0).
    /// Known values above always win during deserialization.
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// The stop sequence that fired, verbatim. Populated iff `finish_reason`
    /// is [`FinishReason::StopSequence`] and the provider reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    /// The model that actually served the request, which may differ from the
    /// model the caller asked for.
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

#[cfg(test)]
mod tests {
    use super::{
        ChatRequest, Choice, Content, ContentPart, FinishReason, Message, Role, ToolCall,
        ToolChoice,
    };

    #[test]
    fn text_content_stays_a_bare_string_on_the_wire() {
        let request = ChatRequest::new("gpt-5.5", vec![Message::text(Role::User, "hi")]);
        let json = serde_json::to_value(&request).expect("serializable request");

        assert_eq!(json["messages"][0]["content"], serde_json::json!("hi"));
    }

    #[test]
    fn multimodal_content_stays_an_array_on_the_wire() {
        let message = Message {
            content: Some(Content::Parts(vec![ContentPart::Text {
                text: "describe".to_owned(),
            }])),
            ..Message::text(Role::User, "")
        };
        let json = serde_json::to_value(&message).expect("serializable message");

        assert_eq!(json["content"][0]["type"], serde_json::json!("text"));
    }

    #[test]
    fn unknown_request_fields_survive_a_round_trip() {
        let raw = r#"{"model":"gpt-5.5","messages":[],"seed":42}"#;
        let request: ChatRequest = serde_json::from_str(raw).expect("valid request");

        assert_eq!(request.extensions["seed"], serde_json::json!(42));

        let reserialized = serde_json::to_value(&request).expect("serializable request");
        assert_eq!(reserialized["seed"], serde_json::json!(42));
    }

    #[test]
    fn tool_call_arguments_keep_the_exact_bytes_the_model_produced() {
        let call = ToolCall {
            id: "call_1".to_owned(),
            name: "get_weather".to_owned(),
            arguments: r#"{"city": "Beijing"}"#.to_owned(),
        };
        let round_tripped: ToolCall =
            serde_json::from_str(&serde_json::to_string(&call).expect("serializable call"))
                .expect("valid call");

        assert_eq!(round_tripped.arguments, r#"{"city": "Beijing"}"#);
    }

    // 0.3.0 (G2/G3) wire contract

    #[test]
    fn thinking_and_redacted_blocks_round_trip() {
        let parts = vec![
            ContentPart::Thinking {
                thinking: "let me see".to_owned(),
                signature: Some("EqQBCg".to_owned()),
            },
            ContentPart::RedactedThinking {
                data: "opaque-blob".to_owned(),
            },
        ];
        let json = serde_json::to_value(&parts).expect("serializable parts");
        assert_eq!(json[0]["type"], serde_json::json!("thinking"));
        assert_eq!(json[0]["signature"], serde_json::json!("EqQBCg"));
        assert_eq!(json[1]["type"], serde_json::json!("redacted_thinking"));

        let back: Vec<ContentPart> = serde_json::from_value(json).expect("valid parts");
        assert_eq!(back, parts);
    }

    #[test]
    fn unknown_part_survives_verbatim_and_known_tags_win() {
        // Round-trip the full object for unknown types without loss.
        let raw = serde_json::json!({"type": "audio", "audio_url": "https://x/a.mp3"});
        let part: ContentPart = serde_json::from_value(raw.clone()).expect("valid part");
        assert_eq!(part, ContentPart::Unknown(raw.clone()));
        assert_eq!(serde_json::to_value(&part).expect("serializable part"), raw);

        // Known tags always take precedence over the Unknown fallback.
        let known: ContentPart =
            serde_json::from_value(serde_json::json!({"type": "text", "text": "hi"}))
                .expect("valid part");
        assert!(matches!(known, ContentPart::Text { .. }));
    }

    #[test]
    fn unknown_finish_reason_survives_verbatim_and_known_values_win() {
        let other: FinishReason = serde_json::from_str(r#""model_yawned""#).expect("valid reason");
        assert_eq!(other, FinishReason::Other("model_yawned".to_owned()));
        assert_eq!(
            serde_json::to_string(&other).expect("serializable reason"),
            r#""model_yawned""#
        );

        let known: FinishReason = serde_json::from_str(r#""stop""#).expect("valid reason");
        assert_eq!(known, FinishReason::Stop);
    }

    #[test]
    fn stop_sequence_slot_rides_the_choice() {
        let choice = Choice {
            index: 0,
            message: Message::text(Role::Assistant, "…"),
            finish_reason: Some(FinishReason::StopSequence),
            stop_sequence: Some("\n\nHuman:".to_owned()),
        };
        let json = serde_json::to_value(&choice).expect("serializable choice");
        assert_eq!(json["finish_reason"], serde_json::json!("stop_sequence"));
        assert_eq!(json["stop_sequence"], serde_json::json!("\n\nHuman:"));

        // Omit defaults from the wire so 0.2.x JSON remains forward-compatible.
        let old: Choice = serde_json::from_value(
            serde_json::json!({"index": 0, "message": {"role": "assistant"}}),
        )
        .expect("0.2.x choice parses");
        assert_eq!(old.stop_sequence, None);
    }

    #[test]
    fn tool_choice_is_typed_but_wire_shape_is_unchanged() {
        // String form: preserve the same flattened top-level key and value used by 0.2.x extensions.
        let request: ChatRequest =
            serde_json::from_str(r#"{"model":"auto","messages":[],"tool_choice":"required"}"#)
                .expect("valid request");
        assert_eq!(request.tool_choice, Some(ToolChoice::Required));
        assert!(!request.extensions.contains_key("tool_choice"));
        let json = serde_json::to_value(&request).expect("serializable request");
        assert_eq!(json["tool_choice"], serde_json::json!("required"));

        // Preserve object form without reshaping or dropping provider-specific data.
        let obj = serde_json::json!({"type": "function", "function": {"name": "f"}});
        let request: ChatRequest = serde_json::from_value(serde_json::json!({
            "model": "auto", "messages": [], "tool_choice": obj.clone(),
        }))
        .expect("valid request");
        assert_eq!(request.tool_choice, Some(ToolChoice::Other(obj)));
    }

    #[test]
    fn canonical_arguments_are_stable_across_key_order_and_whitespace() {
        use super::CanonicalArguments;
        let a = ToolCall {
            id: "1".to_owned(),
            name: "f".to_owned(),
            arguments: r#"{ "b": 1, "a": 2 }"#.to_owned(),
        };
        let b = ToolCall {
            id: "2".to_owned(),
            name: "f".to_owned(),
            arguments: r#"{"a":2,"b":1}"#.to_owned(),
        };
        assert_eq!(a.canonical_arguments(), b.canonical_arguments());
        assert_eq!(
            a.canonical_arguments(),
            CanonicalArguments::Canonical(r#"{"a":2,"b":1}"#.to_owned())
        );
    }

    #[test]
    fn canonical_arguments_never_rewrite_invalid_json() {
        use super::CanonicalArguments;
        let raw = r#"{"city": "Beijing"  // trailing junk"#;
        let call = ToolCall {
            id: "1".to_owned(),
            name: "f".to_owned(),
            arguments: raw.to_owned(),
        };
        // The invalid arguments are preserved verbatim and flagged, not "fixed".
        assert_eq!(
            call.canonical_arguments(),
            CanonicalArguments::Unparseable(raw.to_owned())
        );
        // And the wire bytes are untouched.
        assert_eq!(call.arguments, raw);
    }

    #[test]
    fn defaults_let_a_minimal_request_parse() {
        let request: ChatRequest =
            serde_json::from_str(r#"{"model":"auto","messages":[]}"#).expect("valid request");

        assert!(!request.stream);
        assert!(request.tools.is_empty());
        assert_eq!(request.sampling.temperature, None);
    }
}
