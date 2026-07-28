use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use token_station_protocol::{ErrorCode, HintKind};

use crate::RequestFeatures;

/// The name of an upstream the operator configured, never its address or its
/// key.
///
/// The accepted shape is `[a-z][a-z0-9_]*`, exactly the one
/// `plugin_api::AdapterPermissions::secrets` accepts, and for the same reason:
/// it is deliberately too narrow for a credential. `sk-live-abc` has hyphens, a
/// base64 blob has uppercase and `+/=`, a JWT has dots. None of them can be
/// pasted here and mistaken for the name of an upstream.
///
/// This is also the string that reaches the metrics store and, later, the
/// cloud-sync whitelist — the docs call it the upstream reference name, and this is that type.
/// So the narrowness buys something concrete: whatever else leaks, it is not a
/// key someone typed into the wrong field.
///
/// Validation re-applies on deserialization. A config file is operator input;
/// without `try_from` the derive would accept any string and the shape would
/// hold only for names built in-process.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UpstreamRef(String);

impl UpstreamRef {
    /// # Errors
    ///
    /// Returns [`InvalidUpstreamRef`] unless `name` is `[a-z][a-z0-9_]*`.
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidUpstreamRef> {
        let name = name.into();
        let mut characters = name.chars();
        let starts_with_letter = characters.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest_is_tame =
            characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');

        if starts_with_letter && rest_is_tame {
            Ok(Self(name))
        } else {
            Err(InvalidUpstreamRef(name))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UpstreamRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for UpstreamRef {
    type Error = InvalidUpstreamRef;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl From<UpstreamRef> for String {
    fn from(reference: UpstreamRef) -> Self {
        reference.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidUpstreamRef(String);

impl fmt::Display for InvalidUpstreamRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not an upstream reference name; expected `[a-z][a-z0-9_]*`",
            self.0
        )
    }
}

impl Error for InvalidUpstreamRef {}

/// One place a request can be sent: an upstream, and a model on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamModel {
    pub upstream: UpstreamRef,
    pub model: String,
}

impl UpstreamModel {
    #[must_use]
    pub fn new(upstream: UpstreamRef, model: impl Into<String>) -> Self {
        Self {
            upstream,
            model: model.into(),
        }
    }
}

impl fmt::Display for UpstreamModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.upstream, self.model)
    }
}

/// Which layer of the router decided, and on what.
///
/// The four layers of the routing design, in priority order: user rules, agent
/// hints, the heuristic, then the configured default. The learned classifier
/// (`C3#2`) will arrive as a fifth variant between `Hint` and `Heuristic`; the
/// cascade layer is explicitly out of scope for the local client.
///
/// Every string in here comes from [`crate::RouterConfig`] — a rule the operator
/// named, a hint value the operator's routing table keys on. None of it comes
/// from the request. That is what makes a [`Decision`] safe to persist next to a
/// request it will outlive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tier", rename_all = "snake_case")]
pub enum DecidedBy {
    /// A rule the operator wrote matched. Highest priority, always.
    Rule { rule: String },
    /// The calling agent said what step it was on, and the routing table had an
    /// entry for it.
    ///
    /// `value` is the key from the routing table, not the value off the wire.
    /// They compare equal, so the choice looks cosmetic; it is not. Taking the
    /// configured key means an agent cannot write arbitrary text into a record
    /// that is destined for the metrics store by putting it in a hint header.
    Hint { kind: HintKind, value: String },
    /// Nothing matched, so the request was scored.
    Heuristic { score: u32, threshold: u32 },
    /// No rule, no hint, no heuristic configured.
    Default,
    /// The Agent honors exact model names: the caller pinned this model and the
    /// router served it as-is rather than choosing a tier. `model` is the pinned
    /// wire name, so a decision record still says exactly what was asked for.
    ExactModel { model: String },
    /// Quota-first mode chose this account because its allowance is closing
    /// soonest (see [`crate::Router::route_quota_first`]). Request difficulty
    /// played no part, so there is nothing tier-like to name.
    Quota,
}

/// Where a request is going, why, and what to try if it fails.
///
/// Deterministic: [`crate::Router::route`] reads no clock and draws no random
/// number, so replaying a decision from the audit log against the same config
/// reproduces it exactly. `C3#3`, the recent routing-decisions view, depends on that, and so
/// does the server and the client agreeing.
///
/// Content-free by construction. See [`RequestFeatures`] for how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub chosen: UpstreamModel,
    pub decided_by: DecidedBy,
    /// The rest of the pool, in the order the router would try them. Ordering is
    /// health first, then the operator's own order within a health class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<UpstreamModel>,
    /// What the router looked at. This is the classifier-feature transparency
    /// credential: for any request, the user can see exactly which features
    /// produced the route, and that none of them is their prompt.
    pub features: RequestFeatures,
    /// The pool the decision resolved into, for explaining a route without
    /// re-reading the config.
    pub pool: String,
}

/// Why nothing could be routed to.
///
/// The mapping onto [`ErrorCode`] lives here rather than in the host, because
/// the distinction is routing's to make: an unsatisfiable request must not be
/// retried elsewhere, and an unavailable pool must.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRoute {
    /// No candidate in the pool can do what the request needs — tools, vision, a
    /// JSON schema, or a context window long enough.
    Unsatisfiable {
        pool: String,
        reason: UnmetRequirement,
    },
    /// Candidates exist and are capable, but every one of them is currently
    /// removed from rotation.
    Unavailable { pool: String },
}

impl NoRoute {
    /// What the host should answer the caller with.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            // Another upstream would refuse for the same reason.
            Self::Unsatisfiable { .. } => ErrorCode::Capability,
            // Retriable: the pool may recover, and the host may have another.
            Self::Unavailable { .. } => ErrorCode::Capacity,
        }
    }
}

impl fmt::Display for NoRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsatisfiable { pool, reason } => {
                write!(f, "no candidate in pool `{pool}` {reason}")
            }
            Self::Unavailable { pool } => {
                write!(f, "every candidate in pool `{pool}` is out of rotation")
            }
        }
    }
}

impl Error for NoRoute {}

/// The first thing about a request that no candidate could satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmetRequirement {
    /// The pool names models no candidate reported, so nothing is known about
    /// them. Usually a config referencing an upstream that is not installed.
    Unknown,
    Tools,
    Vision,
    JsonSchema,
    ContextWindow {
        needed: u32,
    },
    /// The Agent honors exact model names and no installed candidate carries the
    /// one the caller pinned. Refusing is the honest answer — substituting a
    /// different model would break the promise the pin makes.
    ExactModelUnavailable,
    /// The route is locked to local upstreams (`local_only`), no local candidate
    /// could serve the request, and cloud fallback was not authorized. Refusing
    /// is the point: the data must not silently leave the machine.
    LocalOnly,
}

impl fmt::Display for UnmetRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("is installed"),
            Self::Tools => f.write_str("supports tool calls"),
            Self::Vision => f.write_str("supports vision"),
            Self::JsonSchema => f.write_str("supports JSON schema output"),
            Self::ContextWindow { needed } => {
                write!(f, "has a context window of at least {needed} tokens")
            }
            Self::ExactModelUnavailable => f.write_str("carries the pinned model"),
            Self::LocalOnly => f.write_str("runs locally, and cloud fallback is off"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecidedBy, Decision, NoRoute, UnmetRequirement, UpstreamModel, UpstreamRef};
    use crate::RequestFeatures;
    use token_station_protocol::ErrorCode;

    fn upstream(name: &str) -> UpstreamRef {
        UpstreamRef::new(name).expect("valid reference name")
    }

    #[test]
    fn an_upstream_reference_cannot_look_like_a_credential() {
        assert!(UpstreamRef::new("openai_personal").is_ok());
        assert!(UpstreamRef::new("ollama_local").is_ok());
        assert!(UpstreamRef::new("k2").is_ok());

        assert!(UpstreamRef::new("sk-live-abc123").is_err());
        assert!(UpstreamRef::new("eyJhbGciOiJIUzI1NiJ9.abc").is_err());
        assert!(UpstreamRef::new("Bearer abc").is_err());
        assert!(UpstreamRef::new("https://key@host").is_err());
        assert!(UpstreamRef::new("2fast").is_err());
        assert!(UpstreamRef::new("").is_err());
    }

    #[test]
    fn a_credential_cannot_be_smuggled_in_through_a_config_file() {
        let parsed: Result<UpstreamRef, _> = serde_json::from_str(r#""sk-live-abc123""#);

        assert!(
            parsed.is_err(),
            "the shape must survive deserialization, not only construction"
        );
    }

    #[test]
    fn an_unsatisfiable_route_is_never_retried_on_another_upstream() {
        let unsatisfiable = NoRoute::Unsatisfiable {
            pool: "cheap".to_owned(),
            reason: UnmetRequirement::Tools,
        };
        let unavailable = NoRoute::Unavailable {
            pool: "cheap".to_owned(),
        };

        assert_eq!(unsatisfiable.error_code(), ErrorCode::Capability);
        assert!(!unsatisfiable.error_code().is_retriable_elsewhere());

        assert_eq!(unavailable.error_code(), ErrorCode::Capacity);
        assert!(unavailable.error_code().is_retriable_elsewhere());
    }

    #[test]
    fn a_decision_round_trips() {
        let decision = Decision {
            chosen: UpstreamModel::new(upstream("openai_personal"), "gpt-5.5"),
            decided_by: DecidedBy::Heuristic {
                score: 51,
                threshold: 40,
            },
            fallbacks: vec![UpstreamModel::new(upstream("ollama_local"), "llama3.3")],
            features: RequestFeatures::default(),
            pool: "sota".to_owned(),
        };

        let encoded = serde_json::to_string(&decision).expect("serializable decision");
        let decoded: Decision = serde_json::from_str(&encoded).expect("valid decision");

        assert_eq!(decoded, decision);
        assert!(encoded.contains(r#""tier":"heuristic""#));
    }
}
