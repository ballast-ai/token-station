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
pub const SCHEMA_VERSION: u32 = 5;

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
    pub decided_by: DecidedBy,
    pub fallbacks: u32,
    pub features: RequestFeatures,
}

impl From<&Decision> for DecisionRecord {
    fn from(decision: &Decision) -> Self {
        Self {
            upstream: decision.chosen.upstream.as_str().to_owned(),
            model: decision.chosen.model.clone(),
            pool: decision.pool.clone(),
            decided_by: decision.decided_by.clone(),
            fallbacks: u32::try_from(decision.fallbacks.len()).unwrap_or(u32::MAX),
            features: decision.features,
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
    pub fallback_allowed: bool,
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
    pub succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
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
    pub decided_by: DecidedBy,
    pub fallbacks: u32,
    pub features: RequestFeatures,
}

impl From<&Decision> for RoutingRecord {
    fn from(decision: &Decision) -> Self {
        Self {
            upstream: decision.chosen.upstream.as_str().to_owned(),
            model: decision.chosen.model.clone(),
            pool: decision.pool.clone(),
            decided_by: decision.decided_by.clone(),
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
    use super::{RequestRecord, RoutingRecord};
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
}
