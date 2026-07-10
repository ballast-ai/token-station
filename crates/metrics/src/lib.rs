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
//!   design), but the *whitelist* decision is C2#3's; locally it is stored
//!   as-is.
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
use token_station_protocol::{ErrorCode, Usage};
use token_station_router_core::{DecidedBy, Decision, RequestFeatures};

/// The persisted shape's version. Stores stamp it (`SQLite` `user_version`, a
/// column, a document field) and refuse to write a record shape they do not
/// know.
pub const SCHEMA_VERSION: u32 = 1;

/// Everything one request leaves behind. One per request, exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestRecord {
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
    /// `None` when the upstream reported none (or the stream carried no usage
    /// event). Absence is information; it is not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Micro-units of the account currency. `None` until the pricing table
    /// arrives (C2#4); the column exists so history starts accumulating the
    /// moment it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<i64>,
}

impl RequestRecord {
    /// A record at request arrival: everything else is filled in as the
    /// exchange progresses.
    #[must_use]
    pub fn begin(started_at_ms: u64, protocol: impl Into<String>) -> Self {
        Self {
            started_at_ms,
            latency_ms: 0,
            protocol: protocol.into(),
            requested_model: String::new(),
            stream: false,
            status: 0,
            error_code: None,
            attempts: 0,
            routing: None,
            usage: None,
            cost_micros: None,
        }
    }
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
