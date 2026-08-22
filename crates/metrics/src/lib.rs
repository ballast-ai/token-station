//! The record vocabulary: what one proxied request leaves behind.
//!
//! Shared by the local client (`SQLite`, `apps/cli`) and the server gateway
//! (its own store, closed repository) so the two lines' decision records stay
//! isomorphic — the plan calls this out as the field set that trains future
//! routing models, and the one place where "add the column later" means
//! rebuilding history that no longer exists.
//!
//! # Content cannot reach this record
//!
//! Almost every field is a number, a closed enum, or a string that originates
//! in the operator's configuration ([`token_station_router_core::Decision`]'s
//! guarantee). The deliberate exceptions are named and bounded:
//!
//! - [`RequestRecord::requested_model`] is caller-supplied text. Model names
//!   are classified as metadata (they appear in the cloud-sync whitelist
//!   design), but the *whitelist* decision is C2#3's. The gateway canonicalizes
//!   it before it lands here: a configured model name (or `auto`) is kept, and
//!   any other caller string collapses to a fixed-width `unlisted:<hash>` token,
//!   so this field can never carry free-form caller text.
//! - [`RequestRecord::request_method`] and [`RequestRecord::path_kind`] are
//!   transport metadata from closed host vocabularies. An arbitrary raw URL
//!   path is never retained.
//! - Token usage is [`Usage`] — the canonical vocabulary. There is
//!   deliberately no parallel usage type here: an earlier `TokenUsage`
//!   placeholder was removed the day this schema was decided.
//!
//! # Versioning
//!
//! [`SCHEMA_VERSION`] gates the persisted shape. Adding a field is a version
//! bump and a migration in every store that persists this record; that is the
//! cost the field list above was chosen to defer until the two lines co-review
//! the next feature set (reasoning-verb counts and caller retry history are
//! the named candidates).

use serde::{Deserialize, Serialize};
use token_station_protocol::{ErrorCode, StreamOutcome, Usage};
use token_station_router_core::{DecidedBy, Decision, RequestFeatures};

/// The persisted shape's version. Stores stamp it (`SQLite` `user_version`, a
/// column, a document field). A store older than this migrates forward (with a
/// backup first); a store newer than this is refused rather than written by a
/// schema that no longer means the same thing.
///
/// - v1: the original record shape.
/// - v2: adds `request_id` (the stable accounting id).
/// - v3: adds `price_version` (the price table a cost was computed under).
/// - v4: adds the normalized, content-free request receipt tables.
/// - v5: adds `conversation_tokens` (difficulty tokens excluding the system
///   prompt) alongside the whole-request `est_input_tokens`.
/// - v6: widens the `decisions.decision_kind` check to admit `quota`
///   (quota-first routing).
/// - v7: adds the quota-decision snapshot columns (why a quota-first route
///   picked its account: reset/headroom/pressured/exhausted).
/// - v8: adds content-free transport diagnostics and closed conversion
///   outcome/reason fields.
/// - v9: records the closed provider-call engine used by each real attempt.
pub const SCHEMA_VERSION: u32 = 11;

/// The content-free transport path classification recorded for diagnostics.
/// Raw, caller-controlled URL paths never enter the receipt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPathKind {
    ChatCompletions,
    Responses,
    Messages,
    GeminiGenerateContent,
    Models,
    Embeddings,
    Admin,
    UnknownAgentEndpoint,
    #[default]
    Unknown,
}

impl RequestPathKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::Models => "models",
            Self::Embeddings => "embeddings",
            Self::Admin => "admin",
            Self::UnknownAgentEndpoint => "unknown_agent_endpoint",
            Self::Unknown => "unknown",
        }
    }
}

/// Persisted copy of the router decision vocabulary. It deliberately gives the
/// matched heuristic band's lower bound a name distinct from the heuristic
/// configuration's top-level threshold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tier", rename_all = "snake_case")]
pub enum RecordedDecidedBy {
    Rule {
        rule: String,
    },
    Hint {
        kind: token_station_protocol::HintKind,
        value: String,
    },
    Heuristic {
        score: u32,
        matched_band_at_least: u32,
    },
    Default,
    ExactModel {
        model: String,
    },
    Quota,
}

impl From<&DecidedBy> for RecordedDecidedBy {
    fn from(value: &DecidedBy) -> Self {
        match value {
            DecidedBy::Rule { rule } => Self::Rule { rule: rule.clone() },
            DecidedBy::Hint { kind, value } => Self::Hint {
                kind: *kind,
                value: value.clone(),
            },
            DecidedBy::Heuristic { score, threshold } => Self::Heuristic {
                score: *score,
                matched_band_at_least: *threshold,
            },
            DecidedBy::Default => Self::Default,
            DecidedBy::ExactModel { model } => Self::ExactModel {
                model: model.clone(),
            },
            DecidedBy::Quota => Self::Quota,
        }
    }
}

/// Where the recorded cost came from. The local price table may only produce
/// [`Self::Estimated`]; [`Self::Actual`] is reserved for future bill
/// reconciliation. An unknown cost always persists without a numeric value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostKind {
    Actual,
    Estimated,
    #[default]
    Unknown,
}

impl CostKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Actual => "actual",
            Self::Estimated => "estimated",
            Self::Unknown => "unknown",
        }
    }
}

/// A route decision frozen before the fallback sweep starts. Unlike
/// [`RequestRecord::routing`], this record is never rewritten to name the
/// upstream that ultimately served the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub upstream: String,
    pub model: String,
    pub pool: String,
    pub decided_by: RecordedDecidedBy,
    pub fallbacks: u32,
    pub features: RequestFeatures,
    /// Why quota-first routing picked this account (its window/rate picture at
    /// decision time). Present only for quota-first routes; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaDecisionSnapshot>,
}

/// The chosen account's quota picture at the moment a quota-first route was
/// decided — a read-only diagnostic ("why this account"), never a credential or
/// free-form content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaDecisionSnapshot {
    /// Milliseconds until the binding window resets; `None` when non-windowed.
    pub reset_ms: Option<u64>,
    /// Remaining allowance of that window, in permille (`0..=1000`).
    pub remaining_permille: Option<u16>,
    /// Instantaneous rate headroom in permille (in-flight already folded in).
    pub headroom_permille: u16,
    pub pressured: bool,
    pub exhausted: bool,
}

impl From<&Decision> for DecisionRecord {
    fn from(decision: &Decision) -> Self {
        Self {
            upstream: decision.chosen.upstream.as_str().to_owned(),
            model: decision.chosen.model.clone(),
            pool: decision.pool.clone(),
            decided_by: RecordedDecidedBy::from(&decision.decided_by),
            fallbacks: u32::try_from(decision.fallbacks.len()).unwrap_or(u32::MAX),
            features: decision.features,
            quota: None,
        }
    }
}

/// One real southbound attempt. A row is created only after the Provider
/// admission permit has been obtained; local admission/budget refusals are not
/// upstream attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub ordinal: u32,
    pub upstream: String,
    pub model: String,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_outcome: Option<StreamOutcome>,
    #[serde(default)]
    pub provider_call_engine: ProviderCallEngine,
    /// Why a South-eligible configuration executed this attempt on the legacy
    /// engine instead. `None` when the attempt ran on South, or on legacy by
    /// explicit configuration with nothing to explain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub south_fallback_reason: Option<SouthFallbackReason>,
    pub fallback_allowed: bool,
}

/// The content-free reason one attempt fell back from South to the legacy
/// engine. Every variant is a host-local classification; none carries a
/// header value, a URL, or a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SouthFallbackReason {
    /// The upstream is configured `provider_call: legacy`.
    ConfiguredLegacy,
    /// The upstream's South tier covers buffered calls only and this was a stream.
    BufferedModeCannotStream,
    /// The upstream has no credential; South carries authenticated calls only.
    UnauthenticatedUpstream,
    /// The gateway was built without a server-owned async runtime.
    NoProviderRuntime,
    /// The credential resolver could not be built for this upstream.
    CredentialResolver,
    /// The provider dialect is outside the South slice.
    ProviderDialect,
    /// The dialect resolves to a package the South slice does not approve.
    ProviderPackageUnapproved,
    /// The upstream speaks a non-translated API dialect.
    ApiDialect,
    /// Egress goes through a proxy.
    Egress,
    /// The body mode did not match what the slice carries.
    Streaming,
    /// The descriptor is not a POST.
    Method,
    /// The credential shape is outside the slice.
    Auth,
    /// The descriptor carries no body.
    Body,
    /// The credential source is outside the slice (a key file, say).
    SecretSource,
    /// Response metadata compatibility could not be asserted.
    ResponseMetadata,
    /// A descriptor header fails the safe-header contract.
    Headers,
}

impl SouthFallbackReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfiguredLegacy => "configured_legacy",
            Self::BufferedModeCannotStream => "buffered_mode_cannot_stream",
            Self::UnauthenticatedUpstream => "unauthenticated_upstream",
            Self::NoProviderRuntime => "no_provider_runtime",
            Self::CredentialResolver => "credential_resolver",
            Self::ProviderDialect => "provider_dialect",
            Self::ProviderPackageUnapproved => "provider_package_unapproved",
            Self::ApiDialect => "api_dialect",
            Self::Egress => "egress",
            Self::Streaming => "streaming",
            Self::Method => "method",
            Self::Auth => "auth",
            Self::Body => "body",
            Self::SecretSource => "secret_source",
            Self::ResponseMetadata => "response_metadata",
            Self::Headers => "headers",
        }
    }

    /// Every variant, in token order, for schema constraints and parsers.
    pub const ALL: [Self; 16] = [
        Self::ConfiguredLegacy,
        Self::BufferedModeCannotStream,
        Self::UnauthenticatedUpstream,
        Self::NoProviderRuntime,
        Self::CredentialResolver,
        Self::ProviderDialect,
        Self::ProviderPackageUnapproved,
        Self::ApiDialect,
        Self::Egress,
        Self::Streaming,
        Self::Method,
        Self::Auth,
        Self::Body,
        Self::SecretSource,
        Self::ResponseMetadata,
        Self::Headers,
    ];

    /// The inverse of [`Self::as_str`].
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == token)
    }
}

/// The content-free HTTP engine that actually performed one upstream attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCallEngine {
    Legacy,
    SouthV1Buffered,
    SouthV1Streaming,
    #[default]
    Unknown,
}

impl ProviderCallEngine {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::SouthV1Buffered => "south_v1_buffered",
            Self::SouthV1Streaming => "south_v1_streaming",
            Self::Unknown => "unknown",
        }
    }
}

/// The only conversion phases a receipt may name. Keeping this closed prevents
/// a generic stage/detail field from becoming a content logging side channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionStage {
    InboundNormalize,
    ProviderRequest,
    ProviderResponse,
    OutboundRender,
    StreamTranslate,
}

/// A conversion can be interrupted by the caller without the adapter having
/// failed. Keeping this separate from `succeeded` preserves old readers while
/// preventing cancellation from being diagnosed as a protocol defect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionOutcome {
    Succeeded,
    Failed,
    Cancelled,
    #[default]
    Unknown,
}

impl ConversionOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// Closed, content-free reasons for adapter conversion failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionReasonCode {
    UnsupportedToolType,
    ProviderToolUnsupported,
    StatefulChaining,
    StructuredOutput,
    ReasoningItem,
    UnsupportedMedia,
    InvalidJson,
    InvalidProtocolShape,
    AdapterFailure,
}

impl ConversionReasonCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedToolType => "unsupported_tool_type",
            Self::ProviderToolUnsupported => "provider_tool_unsupported",
            Self::StatefulChaining => "stateful_chaining",
            Self::StructuredOutput => "structured_output",
            Self::ReasoningItem => "reasoning_item",
            Self::UnsupportedMedia => "unsupported_media",
            Self::InvalidJson => "invalid_json",
            Self::InvalidProtocolShape => "invalid_protocol_shape",
            Self::AdapterFailure => "adapter_failure",
        }
    }
}

/// Optional closed detail for the protocol shape named by a conversion reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionReasonDetail {
    LocalShell,
    WebSearch,
    FunctionTool,
    PreviousResponseId,
    JsonSchema,
    Reasoning,
    Image,
    RequestBody,
    OtherToolType,
}

impl ConversionReasonDetail {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalShell => "local_shell",
            Self::WebSearch => "web_search",
            Self::FunctionTool => "function_tool",
            Self::PreviousResponseId => "previous_response_id",
            Self::JsonSchema => "json_schema",
            Self::Reasoning => "reasoning",
            Self::Image => "image",
            Self::RequestBody => "request_body",
            Self::OtherToolType => "other_tool_type",
        }
    }
}

impl ConversionStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InboundNormalize => "inbound_normalize",
            Self::ProviderRequest => "provider_request",
            Self::ProviderResponse => "provider_response",
            Self::OutboundRender => "outbound_render",
            Self::StreamTranslate => "stream_translate",
        }
    }
}

/// One content-free adapter conversion result. Protocol names are adapter
/// manifest identifiers; no converted payload or arbitrary diagnostic text is
/// retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionRecord {
    pub ordinal: u32,
    pub stage: ConversionStage,
    pub source_protocol: String,
    pub target_protocol: String,
    /// Legacy convenience bit retained for existing clients. New clients
    /// should use `outcome`, which distinguishes failure from cancellation.
    pub succeeded: bool,
    #[serde(default)]
    pub outcome: ConversionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<ConversionReasonCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_detail: Option<ConversionReasonDetail>,
}

/// Everything one request leaves behind. One per request, exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestRecord {
    /// A stable per-request accounting id, assigned once at arrival. Internal
    /// retries (the dispatch fallback sweep) share it, so a single logical
    /// request is one accounting unit however many upstreams it touched; and a
    /// record written twice (a rebuild of a derived table) collapses to one row
    /// rather than double-counting.
    pub request_id: String,
    /// The known Agent namespace (`codex`, `claude-code`, ...). The
    /// backward-compatible unnamespaced route leaves this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The Desktop runtime revision that actually served this request. A
    /// standalone CLI serve has no revision ledger and leaves this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_revision: Option<u64>,
    /// Upper-case method from the host's closed HTTP method vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_method: Option<String>,
    /// Content-free host classification; never the arbitrary raw request path.
    #[serde(default)]
    pub path_kind: RequestPathKind,
    /// Unix milliseconds, from the host clock, at request arrival.
    pub started_at_ms: u64,
    pub latency_ms: u64,
    /// The inbound protocol served, e.g. `openai-chat-completions`.
    pub protocol: String,
    /// What the caller asked for, verbatim (e.g. `auto`). Caller-supplied
    /// text; see the crate docs for its classification.
    pub requested_model: String,
    pub stream: bool,
    /// HTTP status returned to the caller.
    pub status: u16,
    /// Set when the exchange failed, including a mid-stream failure after a
    /// 200 was already committed — which is why this is not derivable from
    /// `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    /// Upstreams tried, including the one that answered. Zero when routing
    /// itself refused.
    pub attempts: u32,
    /// `None` when the request failed before a routing decision existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingRecord>,
    /// The immutable route decision before any fallback changed `routing` to
    /// the final actual server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionRecord>,
    /// Ordered real upstream attempts, never inferred from the legacy counter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempt_records: Vec<AttemptRecord>,
    /// Ordered, content-free conversion stage outcomes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversion_reports: Vec<ConversionRecord>,
    /// `None` when the upstream reported none (or the stream carried no usage
    /// event). Absence is information; it is not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Micro-units of the account currency. `None` when the model has no price
    /// (an unknown cost, never a claimed-free zero).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<i64>,
    /// Whether `cost_micros` is billed, locally estimated, or unavailable.
    /// `Unknown` requires both numeric cost fields to remain absent.
    #[serde(default)]
    pub cost_kind: CostKind,
    /// The price table version `cost_micros` was computed under, pinned so a
    /// later price change never re-values this request. `None` when unpriced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_version: Option<u32>,
}

impl RequestRecord {
    /// A record at request arrival: everything else is filled in as the
    /// exchange progresses.
    #[must_use]
    pub fn begin(started_at_ms: u64, protocol: impl Into<String>) -> Self {
        Self {
            request_id: String::new(),
            agent_id: None,
            running_revision: None,
            request_method: None,
            path_kind: RequestPathKind::Unknown,
            started_at_ms,
            latency_ms: 0,
            protocol: protocol.into(),
            requested_model: String::new(),
            stream: false,
            status: 0,
            error_code: None,
            attempts: 0,
            routing: None,
            decision: None,
            attempt_records: Vec::new(),
            conversion_reports: Vec::new(),
            usage: None,
            cost_micros: None,
            cost_kind: CostKind::Unknown,
            price_version: None,
        }
    }
}

/// The fixed read model returned by both local admin transports. It mirrors a
/// request summary and its optional normalized children without exposing raw
/// database rows or a generic JSON extension point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiptView {
    pub request_id: String,
    pub started_at_ms: u64,
    pub latency_ms: u64,
    pub protocol: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub running_revision: Option<u64>,
    #[serde(default)]
    pub request_method: Option<String>,
    #[serde(default)]
    pub path_kind: RequestPathKind,
    pub requested_model: String,
    pub stream: bool,
    pub status: u16,
    #[serde(default)]
    pub error_code: Option<ErrorCode>,
    /// Legacy flat attempt count, retained so migrated v1-v3 rows still have a
    /// truthful summary even though no per-attempt history can be reconstructed.
    pub attempts: u32,
    #[serde(default)]
    pub routing: Option<RoutingRecord>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub cost_kind: CostKind,
    #[serde(default)]
    pub cost_micros: Option<i64>,
    #[serde(default)]
    pub price_version: Option<u32>,
    #[serde(default)]
    pub decision: Option<DecisionRecord>,
    #[serde(default)]
    pub attempt_records: Vec<AttemptRecord>,
    #[serde(default)]
    pub conversion_reports: Vec<ConversionRecord>,
}

/// The routing decision, flattened for storage.
///
/// Derived from [`Decision`] and nothing else, which is what carries
/// router-core's guarantee across: every string here originates in the
/// operator's configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingRecord {
    pub upstream: String,
    pub model: String,
    pub pool: String,
    pub decided_by: RecordedDecidedBy,
    pub fallbacks: u32,
    pub features: RequestFeatures,
}

impl From<&Decision> for RoutingRecord {
    fn from(decision: &Decision) -> Self {
        Self {
            upstream: decision.chosen.upstream.as_str().to_owned(),
            model: decision.chosen.model.clone(),
            pool: decision.pool.clone(),
            decided_by: RecordedDecidedBy::from(&decision.decided_by),
            fallbacks: u32::try_from(decision.fallbacks.len()).unwrap_or(u32::MAX),
            features: decision.features,
        }
    }
}

/// Where records go. The request path calls this on every exchange.
///
/// Infallible by signature: metrics are observability, and observability must
/// never fail a request. An implementation that cannot persist logs its own
/// failure and drops the record.
pub trait Recorder: Send + Sync {
    fn record(&self, record: &RequestRecord);
}

/// Discards everything. For tests and for hosts that opt out.
pub struct NoopRecorder;

impl Recorder for NoopRecorder {
    fn record(&self, _: &RequestRecord) {}
}

#[cfg(test)]
mod tests {
    use super::{ProviderCallEngine, RequestRecord, RoutingRecord};
    use token_station_protocol::Usage;
    use token_station_router_core::{
        DecidedBy, Decision, RequestFeatures, UpstreamModel, UpstreamRef,
    };

    fn decision() -> Decision {
        Decision {
            chosen: UpstreamModel::new(
                UpstreamRef::new("openai_personal").expect("valid"),
                "gpt-5.5",
            ),
            decided_by: DecidedBy::Rule {
                rule: "tool-calls".to_owned(),
            },
            fallbacks: vec![UpstreamModel::new(
                UpstreamRef::new("ollama_local").expect("valid"),
                "llama3.3",
            )],
            features: RequestFeatures {
                estimated_input_tokens: 42,
                tool_count: 1,
                ..RequestFeatures::default()
            },
            pool: "sota".to_owned(),
        }
    }

    #[test]
    fn a_routing_record_carries_the_decision_and_only_the_decision() {
        let record = RoutingRecord::from(&decision());

        assert_eq!(record.upstream, "openai_personal");
        assert_eq!(record.pool, "sota");
        assert_eq!(record.fallbacks, 1);
        assert_eq!(record.features.estimated_input_tokens, 42);
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let mut record = RequestRecord::begin(1_752_000_000_000, "openai-chat-completions");
        record.requested_model = "auto".to_owned();
        record.status = 200;
        record.attempts = 1;
        record.routing = Some(RoutingRecord::from(&decision()));
        record.usage = Some(Usage {
            input_tokens: 9,
            output_tokens: 3,
            ..Usage::default()
        });

        let encoded = serde_json::to_string(&record).expect("serializable record");
        let decoded: RequestRecord = serde_json::from_str(&encoded).expect("valid record");

        assert_eq!(decoded, record);
    }

    #[test]
    fn absent_usage_stays_absent_rather_than_becoming_zero() {
        let record = RequestRecord::begin(0, "openai-chat-completions");
        let encoded = serde_json::to_string(&record).expect("serializable record");

        assert!(!encoded.contains("usage"), "{encoded}");
        assert!(!encoded.contains("cost_micros"), "{encoded}");
    }

    #[test]
    fn south_streaming_engine_has_an_exact_content_free_token() {
        assert_eq!(
            ProviderCallEngine::SouthV1Streaming.as_str(),
            "south_v1_streaming"
        );
        assert_eq!(
            serde_json::to_value(ProviderCallEngine::SouthV1Streaming)
                .expect("provider call engine serializes"),
            serde_json::json!("south_v1_streaming")
        );
    }
}
