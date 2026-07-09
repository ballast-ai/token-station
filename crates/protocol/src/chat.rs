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
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
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
            sampling: Sampling::default(),
            stream: false,
            extensions: Extensions::new(),
        }
    }
}

/// Why generation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
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
    use super::{ChatRequest, Content, ContentPart, Message, Role, ToolCall};

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

    #[test]
    fn defaults_let_a_minimal_request_parse() {
        let request: ChatRequest =
            serde_json::from_str(r#"{"model":"auto","messages":[]}"#).expect("valid request");

        assert!(!request.stream);
        assert!(request.tools.is_empty());
        assert_eq!(request.sampling.temperature, None);
    }
}
