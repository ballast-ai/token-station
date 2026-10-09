use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{ComponentValues, Extensions, Usage};

/// Who authored a [`Message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Instructions for the model.
    ///
    /// A system message may carry [`Content::Parts`] with several
    /// [`ContentPart::Text`] parts, one for each system block the caller sent
    /// (Anthropic `system: [{type: text, ...}, ...]`). When any of these parts
    /// carries a [`CacheControl`] marker, a `provider-adapter` must not join
    /// the parts into one string: the marker belongs to the end of its own
    /// block, and a join moves the cache breakpoint. Without a marker, joining
    /// is allowed.
    System,
    User,
    Assistant,
    /// The result of a tool the assistant called, fed back into the exchange.
    ///
    /// A cached tool result (Anthropic `tool_result` with `cache_control`)
    /// is carried as [`Content::Parts`] with the [`CacheControl`] marker on
    /// the last part. [`Message`] has no marker field of its own.
    Tool,
}

/// A single part of a multimodal message.
///
/// A [`ContentPart::Text`] or [`ContentPart::ImageUrl`] part can carry a
/// prompt-cache marker (0.6.0). An invalid marker on any part is a
/// deserialization error. It never turns the part into
/// [`ContentPart::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
        /// Ends a cacheable prompt prefix after this part (0.6.0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ImageUrl {
        image_url: ImageUrl,
        /// Ends a cacheable prompt prefix after this part (0.6.0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
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
    RedactedThinking { data: String },
    /// Any part whose `type` this crate does not model. The raw object is
    /// preserved verbatim so translation never silently drops content
    /// (0.3.0; known tags above always win during deserialization).
    ///
    /// Since 0.6.0, an object whose `cache_control` does not parse as a
    /// [`CacheControl`] is refused instead of landing here, so an unknown
    /// cache TTL cannot bypass the closed [`CacheTtl`] set.
    #[serde(untagged, deserialize_with = "unknown_part")]
    Unknown(Value),
}

impl ContentPart {
    /// A text part without a cache marker.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    /// The part's prompt-cache marker. Only [`ContentPart::Text`] and
    /// [`ContentPart::ImageUrl`] carry one.
    #[must_use]
    pub fn cache_control(&self) -> Option<CacheControl> {
        match self {
            Self::Text { cache_control, .. } | Self::ImageUrl { cache_control, .. } => {
                *cache_control
            }
            Self::Thinking { .. } | Self::RedactedThinking { .. } | Self::Unknown(_) => None,
        }
    }
}

/// The fallback for [`ContentPart::Unknown`].
///
/// A known part type falls back here when its own fields do not parse. That
/// keeps malformed parts verbatim, as 0.3.0 does. A malformed cache marker is
/// the exception: it must fail closed, so this refuses it. The check covers
/// every part type, so an unmodelled block (for example an Anthropic
/// `document`) keeps a valid marker verbatim and cannot carry an invalid one.
fn unknown_part<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    if let Some(marker) = value.get("cache_control") {
        CacheControl::deserialize(marker).map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}

/// A prompt-cache marker on a content part or a tool definition (0.6.0).
///
/// The marker says: cache the prompt prefix that ends here. It is a cost hint
/// and does not change what the model produces. A `provider-adapter` whose
/// provider has no inline cache marker drops it and does not refuse the
/// request.
///
/// The wire form is the Anthropic form, so the Anthropic dialect maps it
/// without change: `{"type":"ephemeral"}` or `{"type":"ephemeral","ttl":"1h"}`.
/// `type` is always written. On input it may be absent. Any value other than
/// `"ephemeral"` is refused, because a different cache kind may have a
/// different price. The Rust type does not model `type`: it has one value.
///
/// [`CacheControl::ttl`] is a closed set. An unknown TTL fails
/// deserialization. A key other than `type` and `ttl` also fails
/// deserialization, because dropping it could change what the provider
/// caches or charges. A north codec answers each of these with HTTP 400 and
/// does not forward the request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "CacheControlWire", into = "CacheControlWire")]
pub struct CacheControl {
    /// The cache lifetime. `None` keeps the provider default, which is five
    /// minutes on Anthropic. An explicit `"5m"` stays explicit.
    pub ttl: Option<CacheTtl>,
}

/// The lifetime of a prompt-cache entry (0.6.0). Closed: an unknown value is
/// an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheTtl {
    /// Wire `"5m"`.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// Wire `"1h"`.
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheTtl {
    /// The wire string: `"5m"` or `"1h"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheControlWire {
    #[serde(rename = "type", default)]
    kind: CacheKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl: Option<CacheTtl>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CacheKind {
    #[default]
    Ephemeral,
}

impl From<CacheControlWire> for CacheControl {
    fn from(wire: CacheControlWire) -> Self {
        let CacheControlWire {
            kind: CacheKind::Ephemeral,
            ttl,
        } = wire;
        Self { ttl }
    }
}

impl From<CacheControl> for CacheControlWire {
    fn from(marker: CacheControl) -> Self {
        Self {
            kind: CacheKind::Ephemeral,
            ttl: marker.ttl,
        }
    }
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
    /// Whether the caller asked the provider to enforce [`Self::parameters`]
    /// strictly (OpenAI `strict`) (0.6.0).
    ///
    /// Promoted from the `responses_tool_strict` extension key, a map from tool
    /// name to boolean. That key is still data when a producer writes it. A
    /// `provider-adapter` reads this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Ends a cacheable prompt prefix after this tool definition (0.6.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl ToolDef {
    /// A tool without `strict` and without a cache marker.
    #[must_use]
    pub fn new(name: impl Into<String>, description: Option<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
            strict: None,
            cache_control: None,
        }
    }
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
    /// Sample only from the `top_k` most likely tokens (Anthropic `top_k`,
    /// Gemini `topK`) (0.6.0). Providers without it drop it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

/// How the model should reason before it answers (0.6.0).
///
/// One dialect-neutral field for OpenAI `reasoning_effort` and Anthropic
/// `thinking`. Every part is optional and carries what the caller sent,
/// without conversion. A `provider-adapter` maps it to its own wire. When the
/// wire has no equivalent for a part, the adapter drops that part or refuses
/// the request under its own capability rules. It must not invent a value the
/// caller did not send.
///
/// The parts are flattened into [`ChatRequest`], so the wire keys are
/// `reasoning_mode`, `reasoning_effort` and `reasoning_budget_tokens`.
/// `reasoning_effort` is the extension key that carried the effort before
/// 0.6.0. Its wire shape does not change, so a 0.5.0 peer and a 0.6.0 peer
/// read the same JSON. This is how `tool_choice` was promoted in 0.3.0.
///
/// Mapping:
///
/// | Caller sends | Field values |
/// |---|---|
/// | OpenAI Chat `reasoning_effort: "high"` | `effort: High` |
/// | OpenAI Responses `reasoning: {effort: "low"}` | `effort: Low` |
/// | Anthropic `thinking: {type: "enabled", budget_tokens: 2048}` | `mode: Enabled`, `budget_tokens: 2048` |
/// | Anthropic `thinking: {type: "adaptive"}` with `output_config: {effort: "high"}` | `mode: Adaptive`, `effort: High` |
/// | Anthropic `thinking: {type: "disabled"}` | `mode: Disabled` |
///
/// The Anthropic dialect writes `mode` and `budget_tokens` back unchanged.
/// When only `effort` is present, it applies its per-model rule to choose a
/// thinking form. An OpenAI dialect sends `effort` unchanged. It may map
/// `mode` and `budget_tokens` to an effort under its own declared rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reasoning {
    /// Whether the caller turned reasoning on, off, or left the amount to the
    /// model (Anthropic `thinking.type`). Wire key `reasoning_mode`.
    #[serde(
        rename = "reasoning_mode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mode: Option<ReasoningMode>,
    /// How much the model should reason (OpenAI `reasoning_effort`, Anthropic
    /// `output_config.effort`). Wire key `reasoning_effort`.
    #[serde(
        rename = "reasoning_effort",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub effort: Option<ReasoningEffort>,
    /// The token budget for reasoning, exactly as the caller sent it
    /// (Anthropic `thinking.budget_tokens`). Wire key
    /// `reasoning_budget_tokens`.
    #[serde(
        rename = "reasoning_budget_tokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub budget_tokens: Option<u32>,
}

impl Reasoning {
    /// Whether the caller said nothing about reasoning.
    #[must_use]
    pub const fn is_unset(&self) -> bool {
        self.mode.is_none() && self.effort.is_none() && self.budget_tokens.is_none()
    }
}

/// Whether reasoning is on (0.6.0). Closed: an unknown mode is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    /// Reasoning is on. Anthropic `thinking.type: "enabled"`.
    Enabled,
    /// Reasoning is off. Anthropic `thinking.type: "disabled"`.
    Disabled,
    /// The model decides how much to reason. Anthropic
    /// `thinking.type: "adaptive"`.
    Adaptive,
}

/// How much the model should reason (0.6.0).
///
/// Providers add levels over time, so an unknown level is kept verbatim in
/// [`ReasoningEffort::Other`]. The adapter decides whether to send it or to
/// refuse it. Known values always win during deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// A level this crate does not model, preserved verbatim.
    #[serde(untagged)]
    Other(String),
}

impl ReasoningEffort {
    /// The wire string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Other(level) => level,
        }
    }
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
    /// The caller's reasoning request (0.6.0). Flattened: see [`Reasoning`]
    /// for the wire keys.
    #[serde(flatten)]
    pub reasoning: Reasoning,
    /// Whether the model may call several tools in one turn (OpenAI
    /// `parallel_tool_calls`) (0.6.0).
    ///
    /// Promoted from `extensions`. The wire shape is unchanged, as for
    /// `tool_choice` in 0.3.0: `extensions` is flattened, so this was already
    /// the top-level `parallel_tool_calls` key. A `provider-adapter` sends it
    /// only when its wire has the parameter and the request has tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub stream: bool,
    /// Per-request values the host mints for the provider adapter, such as a
    /// request attempt id (0.5.0).
    ///
    /// This is the typed channel for those values. The `extensions` fence is
    /// unchanged: an adapter still must not act on an [`Extensions`] key.
    ///
    /// - Only the host writes this map. A host clears it on every request an
    ///   agent adapter normalizes, so a client cannot supply a host value.
    /// - A provider adapter may read only keys its package declares. The host
    ///   passes only declared keys and validates every value first.
    /// - A secret never travels here.
    /// - Every key and value satisfies the [`ComponentValues`] grammar.
    ///
    /// A value the host derives from the caller also belongs here, not in a
    /// typed field the client could write. An example is an end-user id that
    /// the host rewrites to a per-tenant hash before a dialect maps it to
    /// Anthropic `metadata.user_id`. The key name is the package's to declare.
    #[serde(default, skip_serializing_if = "ComponentValues::is_empty")]
    pub host_values: ComponentValues,
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
            reasoning: Reasoning::default(),
            parallel_tool_calls: None,
            stream: false,
            host_values: ComponentValues::new(),
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
            content: Some(Content::Parts(vec![ContentPart::text("describe")])),
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

    // 0.6.0 wire contract

    use super::{CacheControl, CacheTtl, Reasoning, ReasoningEffort, ReasoningMode, ToolDef};
    use serde_json::{Value, json};

    fn round_trip<T>(wire: &Value) -> T
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let parsed: T = serde_json::from_value(wire.clone()).expect("wire parses");
        assert_eq!(
            &serde_json::to_value(&parsed).expect("serializable"),
            wire,
            "wire did not round-trip unchanged"
        );
        parsed
    }

    #[test]
    fn cache_ttl_wire_strings_are_5m_and_1h() {
        assert_eq!(json!(CacheTtl::FiveMinutes), json!("5m"));
        assert_eq!(json!(CacheTtl::OneHour), json!("1h"));
        assert_eq!(CacheTtl::FiveMinutes.as_str(), "5m");
        assert_eq!(CacheTtl::OneHour.as_str(), "1h");
        for unknown in ["2h", "1H", "5", "300s", "", "ephemeral"] {
            assert!(
                serde_json::from_value::<CacheTtl>(json!(unknown)).is_err(),
                "`{unknown}` must not parse as a TTL"
            );
        }
    }

    #[test]
    fn cache_marker_wire_form_is_the_anthropic_form() {
        let marker: CacheControl = round_trip(&json!({"type": "ephemeral", "ttl": "1h"}));
        assert_eq!(marker.ttl, Some(CacheTtl::OneHour));

        let marker: CacheControl = round_trip(&json!({"type": "ephemeral"}));
        assert_eq!(marker, CacheControl::default());

        // An explicit 5m stays explicit; it is not folded into the default.
        let marker: CacheControl = round_trip(&json!({"type": "ephemeral", "ttl": "5m"}));
        assert_eq!(marker.ttl, Some(CacheTtl::FiveMinutes));

        // `type` may be absent on input and is always written on output.
        let marker: CacheControl =
            serde_json::from_value(json!({"ttl": "1h"})).expect("type may be absent");
        assert_eq!(
            serde_json::to_value(marker).expect("serializable marker"),
            json!({"type": "ephemeral", "ttl": "1h"})
        );
    }

    #[test]
    fn cache_marker_refuses_an_unknown_ttl_or_kind() {
        for marker in [
            json!({"type": "ephemeral", "ttl": "2h"}),
            json!({"type": "ephemeral", "ttl": 3600}),
            json!({"type": "persistent"}),
            json!({"type": "persistent", "ttl": "1h"}),
            json!("ephemeral"),
            // Unknown sub-fields fail closed too (lv ruling, 2026-10-09).
            json!({"type": "ephemeral", "scope": "global"}),
            json!({"type": "ephemeral", "ttl": "1h", "priority": 1}),
            json!({"ttl": "5m", "extra": null}),
        ] {
            assert!(
                serde_json::from_value::<CacheControl>(marker.clone()).is_err(),
                "{marker} must be refused"
            );
        }
    }

    #[test]
    fn text_and_image_parts_carry_a_cache_marker() {
        let part: ContentPart = round_trip(&json!({
            "type": "text",
            "text": "long system prompt",
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        }));
        assert_eq!(
            part,
            ContentPart::Text {
                text: "long system prompt".to_owned(),
                cache_control: Some(CacheControl {
                    ttl: Some(CacheTtl::OneHour)
                }),
            }
        );
        assert_eq!(
            part.cache_control().and_then(|marker| marker.ttl),
            Some(CacheTtl::OneHour)
        );

        let part: ContentPart = round_trip(&json!({
            "type": "image_url",
            "image_url": {"url": "data:image/png;base64,AAAA"},
            "cache_control": {"type": "ephemeral"},
        }));
        assert_eq!(part.cache_control(), Some(CacheControl::default()));
    }

    #[test]
    fn parts_without_a_marker_keep_the_0_5_wire_shape() {
        let part: ContentPart = round_trip(&json!({"type": "text", "text": "hi"}));
        assert_eq!(part, ContentPart::text("hi"));
        assert_eq!(part.cache_control(), None);

        let part: ContentPart =
            round_trip(&json!({"type": "image_url", "image_url": {"url": "https://x/a.png"}}));
        assert!(matches!(
            part,
            ContentPart::ImageUrl {
                cache_control: None,
                ..
            }
        ));
    }

    #[test]
    fn an_invalid_marker_fails_the_part_instead_of_falling_back_to_unknown() {
        for part in [
            json!({"type": "text", "text": "x", "cache_control": {"type": "ephemeral", "ttl": "2h"}}),
            json!({"type": "image_url", "image_url": {"url": "https://x"},
                   "cache_control": {"type": "ephemeral", "ttl": "24h"}}),
            json!({"type": "text", "text": "x", "cache_control": {"type": "persistent"}}),
            // An unmodelled block cannot carry an invalid marker either.
            json!({"type": "document", "source": {}, "cache_control": {"ttl": "2h"}}),
            // A malformed known part with an invalid marker is refused too.
            json!({"type": "text", "cache_control": {"ttl": "2h"}}),
            // An unknown marker sub-field is refused and does not fall back.
            json!({"type": "text", "text": "x",
                   "cache_control": {"type": "ephemeral", "scope": "global"}}),
            json!({"type": "document", "source": {},
                   "cache_control": {"type": "ephemeral", "scope": "global"}}),
        ] {
            assert!(
                serde_json::from_value::<ContentPart>(part.clone()).is_err(),
                "{part} must be refused"
            );
        }

        let request = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "x", "cache_control": {"type": "ephemeral", "ttl": "2h"}}
            ]}],
        });
        assert!(serde_json::from_value::<ChatRequest>(request).is_err());
    }

    #[test]
    fn unmodelled_parts_keep_a_valid_marker_verbatim() {
        let raw = json!({
            "type": "document",
            "source": {"type": "text", "media_type": "text/plain", "data": "x"},
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        });
        let part: ContentPart = serde_json::from_value(raw.clone()).expect("valid part");
        assert_eq!(part, ContentPart::Unknown(raw));

        // A malformed known part without a marker still falls back, as in 0.3.0.
        let raw = json!({"type": "image_url", "image_url": "https://x/a.png"});
        let part: ContentPart = serde_json::from_value(raw.clone()).expect("valid part");
        assert_eq!(part, ContentPart::Unknown(raw));
    }

    #[test]
    fn system_blocks_and_tool_results_carry_markers_on_their_parts() {
        let system: Message = round_trip(&json!({
            "role": "system",
            "content": [
                {"type": "text", "text": "You are terse."},
                {"type": "text", "text": "Project rules.",
                 "cache_control": {"type": "ephemeral", "ttl": "1h"}},
            ],
        }));
        let Some(Content::Parts(parts)) = &system.content else {
            panic!("system blocks stay separate parts");
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].cache_control(), None);
        assert!(parts[1].cache_control().is_some());

        let tool: Message = round_trip(&json!({
            "role": "tool",
            "tool_call_id": "toolu_1",
            "content": [
                {"type": "text", "text": "line 1"},
                {"type": "text", "text": "line 2", "cache_control": {"type": "ephemeral"}},
            ],
        }));
        let Some(Content::Parts(parts)) = &tool.content else {
            panic!("a cached tool result is carried as parts");
        };
        assert!(parts.last().and_then(ContentPart::cache_control).is_some());
        assert!(tool.extensions.is_empty());
    }

    #[test]
    fn tool_definitions_carry_strict_and_a_cache_marker() {
        let tool: ToolDef = round_trip(&json!({
            "name": "read_file",
            "parameters": {"type": "object"},
            "strict": true,
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        }));
        assert_eq!(tool.strict, Some(true));
        assert_eq!(
            tool.cache_control,
            Some(CacheControl {
                ttl: Some(CacheTtl::OneHour)
            })
        );

        // 0.5.0 JSON parses, and a new definition without the fields omits them.
        let old: ToolDef = round_trip(&json!({"name": "f", "parameters": {"type": "object"}}));
        assert_eq!(old, ToolDef::new("f", None, json!({"type": "object"})));

        for marker in [
            json!({"type": "ephemeral", "ttl": "2h"}),
            json!({"type": "ephemeral", "scope": "global"}),
        ] {
            assert!(
                serde_json::from_value::<ToolDef>(json!({
                    "name": "f",
                    "parameters": {},
                    "cache_control": marker,
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn top_k_rides_sampling_and_is_omitted_when_unset() {
        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {"temperature": 0.5, "top_k": 40},
            "stream": false,
        }));
        assert_eq!(request.sampling.top_k, Some(40));
        assert!(!request.extensions.contains_key("top_k"));

        let request = ChatRequest::new("m", Vec::new());
        let json = serde_json::to_value(&request).expect("serializable request");
        assert_eq!(json["sampling"], json!({}));
    }

    #[test]
    fn reasoning_flattens_onto_the_legacy_effort_key() {
        // A 0.5.0 producer wrote `reasoning_effort` into extensions, which put it
        // at the top level. 0.6.0 reads the same key into the typed field.
        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {},
            "reasoning_effort": "high",
            "stream": false,
        }));
        assert_eq!(request.reasoning.effort, Some(ReasoningEffort::High));
        assert!(!request.extensions.contains_key("reasoning_effort"));

        // Anthropic thinking keeps its budget unchanged.
        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {"max_output_tokens": 8192},
            "reasoning_mode": "enabled",
            "reasoning_budget_tokens": 3000,
            "stream": false,
        }));
        assert_eq!(
            request.reasoning,
            Reasoning {
                mode: Some(ReasoningMode::Enabled),
                effort: None,
                budget_tokens: Some(3000),
            }
        );

        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {},
            "reasoning_mode": "adaptive",
            "reasoning_effort": "xhigh",
            "stream": false,
        }));
        assert_eq!(request.reasoning.mode, Some(ReasoningMode::Adaptive));
        assert_eq!(request.reasoning.effort, Some(ReasoningEffort::Xhigh));
    }

    #[test]
    fn reasoning_keeps_unknown_effort_and_refuses_unknown_mode() {
        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {},
            "reasoning_effort": "max",
            "stream": false,
        }));
        assert_eq!(
            request.reasoning.effort,
            Some(ReasoningEffort::Other("max".to_owned()))
        );
        assert_eq!(
            request
                .reasoning
                .effort
                .as_ref()
                .map(ReasoningEffort::as_str),
            Some("max")
        );

        for effort in ["none", "minimal", "low", "medium", "high", "xhigh"] {
            let parsed: ReasoningEffort = serde_json::from_value(json!(effort)).expect("level");
            assert!(
                !matches!(parsed, ReasoningEffort::Other(_)),
                "{effort} is known"
            );
            assert_eq!(parsed.as_str(), effort);
        }

        assert!(
            serde_json::from_value::<ChatRequest>(json!({
                "model": "m", "messages": [], "reasoning_mode": "auto",
            }))
            .is_err()
        );
    }

    #[test]
    fn parallel_tool_calls_is_typed_and_keeps_its_wire_key() {
        let request: ChatRequest = round_trip(&json!({
            "model": "m",
            "messages": [],
            "sampling": {},
            "parallel_tool_calls": false,
            "stream": false,
        }));
        assert_eq!(request.parallel_tool_calls, Some(false));
        assert!(!request.extensions.contains_key("parallel_tool_calls"));
    }

    #[test]
    fn a_0_5_request_parses_and_a_new_request_omits_every_unset_field() {
        let old = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
            "tools": [{"name": "f", "parameters": {"type": "object"}}],
            "sampling": {"temperature": 0.2},
            "stream": true,
            "responses_tool_strict": {"f": true},
        });
        let request: ChatRequest = round_trip(&old);
        assert!(request.reasoning.is_unset());
        assert_eq!(request.parallel_tool_calls, None);
        assert_eq!(request.sampling.top_k, None);
        assert_eq!(request.tools[0].strict, None);
        // The old strict map is not promoted: it stays data in extensions.
        assert_eq!(
            request.extensions["responses_tool_strict"],
            json!({"f": true})
        );

        let json = serde_json::to_value(ChatRequest::new("m", Vec::new())).expect("serializable");
        assert_eq!(
            json,
            json!({"model": "m", "messages": [], "sampling": {}, "stream": false})
        );
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
