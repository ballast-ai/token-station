use serde::{Deserialize, Serialize};

use crate::Extensions;

/// The stable error catalog every adapter maps onto.
///
/// Closed on purpose, and small on purpose. An adapter that cannot place a
/// provider error in one of these buckets must use [`ErrorCode::Internal`]
/// rather than invent a code, because routing reacts to these values: only
/// `RateLimit`, `Capacity`, `UpstreamUnavailable` and `Timeout` are retriable on
/// another upstream, and treating an `Auth` failure as retriable would replay a
/// bad credential across every provider the user configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is malformed or asks for something the schema forbids.
    InvalidRequest,
    /// The credential is missing, wrong, or lacks permission.
    Auth,
    /// The account is out of funds or the endpoint requires payment (HTTP 402).
    /// A config/billing problem, not the upstream's health — not retriable.
    PaymentRequired,
    /// The caller exceeded a rate limit. Retriable elsewhere.
    RateLimit,
    /// The upstream has no capacity right now. Retriable elsewhere.
    Capacity,
    /// The model cannot do what the request needs, e.g. vision or tools.
    Capability,
    /// The upstream refused on content-policy grounds. Not retriable: another
    /// provider would likely refuse too, and retrying looks like evasion.
    ContentPolicy,
    /// The upstream is unreachable or returned a transport-level failure.
    UpstreamUnavailable,
    /// The connection dropped mid-body: an incomplete answer, not a complete
    /// one. Retriable elsewhere, and a connection-level fault for health.
    TransportTruncated,
    /// The request exceeds the model's context window. The same request would
    /// fail on any deployment of the model — not retriable elsewhere.
    ContextLength,
    /// The upstream answered 2xx but the body is not valid provider protocol
    /// (e.g. an error object smuggled into a 200, or unparseable JSON). It
    /// answered, so this is not treated as an outage.
    ProviderProtocolError,
    /// The upstream did not answer within the host's budget.
    Timeout,
    /// Anything else, including adapter bugs.
    Internal,
}

impl ErrorCode {
    /// Whether the host may retry this on a different upstream.
    ///
    /// This is the routing-relevant property of the catalog, kept next to the
    /// catalog so the two cannot drift.
    #[must_use]
    pub const fn is_retriable_elsewhere(self) -> bool {
        matches!(
            self,
            Self::RateLimit
                | Self::Capacity
                | Self::UpstreamUnavailable
                | Self::TransportTruncated
                | Self::Timeout
        )
    }
}

/// A failure, normalized.
///
/// `provider_message` is a short, already-redacted summary from the upstream,
/// kept for operator diagnosis. It is never the upstream's raw body: an adapter
/// must not copy a body that may echo the request, because request content never
/// leaves the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub code: ErrorCode,
    /// The status the host should present to the caller.
    pub http_status: u16,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message: Option<String>,
    /// Honoured by the host's retry budget when the upstream supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(code: ErrorCode, http_status: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            http_status,
            message: message.into(),
            provider_message: None,
            retry_after_ms: None,
            extensions: Extensions::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, ErrorEnvelope};

    #[test]
    fn auth_failures_are_never_retried_on_another_upstream() {
        assert!(!ErrorCode::Auth.is_retriable_elsewhere());
        assert!(!ErrorCode::ContentPolicy.is_retriable_elsewhere());
        assert!(!ErrorCode::InvalidRequest.is_retriable_elsewhere());
    }

    #[test]
    fn transient_failures_are_retriable_on_another_upstream() {
        assert!(ErrorCode::RateLimit.is_retriable_elsewhere());
        assert!(ErrorCode::Capacity.is_retriable_elsewhere());
        assert!(ErrorCode::UpstreamUnavailable.is_retriable_elsewhere());
        assert!(ErrorCode::Timeout.is_retriable_elsewhere());
        // A dropped connection is an incomplete answer worth trying elsewhere.
        assert!(ErrorCode::TransportTruncated.is_retriable_elsewhere());
    }

    #[test]
    fn same_request_failures_are_not_retried_elsewhere() {
        // A billing problem, an over-length request, or a malformed 2xx body
        // would fail the same way (or is the upstream's answer) — not an outage
        // another upstream would dodge.
        assert!(!ErrorCode::PaymentRequired.is_retriable_elsewhere());
        assert!(!ErrorCode::ContextLength.is_retriable_elsewhere());
        assert!(!ErrorCode::ProviderProtocolError.is_retriable_elsewhere());
    }

    #[test]
    fn serializes_code_in_snake_case() {
        let envelope = ErrorEnvelope::new(ErrorCode::ContentPolicy, 400, "refused");
        let json = serde_json::to_value(&envelope).expect("serializable envelope");

        assert_eq!(json["code"], serde_json::json!("content_policy"));
    }

    #[test]
    fn unknown_code_is_a_version_mismatch() {
        let parsed: Result<ErrorEnvelope, _> =
            serde_json::from_str(r#"{"code":"quota_exhausted","http_status":402,"message":"x"}"#);

        assert!(parsed.is_err());
    }
}
