use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentHint, Extensions, is_credential_header};

/// Wire form of [`HeaderDigest`]. Private, so the only way to build a digest is
/// through the redacting conversion below.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HeaderDigestWire {
    #[serde(default)]
    names: BTreeSet<String>,
    #[serde(default)]
    values: BTreeMap<String, String>,
}

/// Inbound headers, with credential values removed.
///
/// An `agent-adapter` needs to know *which* headers arrived — that is how it
/// recognises its own protocol and finds hint headers — but it must never see an
/// inbound key. So a digest keeps every normalized header *name* and values
/// only for a closed metadata catalog the agent ABI actually consumes.
///
/// Redaction is re-applied on deserialization. Without that, the invariant would
/// hold only for digests built in-process, and a fixture or a cached envelope
/// could reintroduce a credential.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "HeaderDigestWire", into = "HeaderDigestWire")]
pub struct HeaderDigest {
    names: BTreeSet<String>,
    values: BTreeMap<String, String>,
}

impl HeaderDigest {
    /// Digests raw inbound headers, dropping credential values.
    #[must_use]
    pub fn redacting<I, K, V>(headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut names = BTreeSet::new();
        let mut values = BTreeMap::new();
        for (name, value) in headers {
            let name = name.into().to_ascii_lowercase();
            let value = value.into();
            if !is_credential_header(&name)
                && is_agent_metadata_value_header(&name)
                && value.len() <= 4096
            {
                values.insert(name.clone(), value);
            }
            names.insert(name);
        }
        Self { names, values }
    }

    /// Every header name that arrived, including the redacted ones.
    #[must_use]
    pub fn names(&self) -> &BTreeSet<String> {
        &self.names
    }

    /// The value of a header, or `None` if it is absent or was redacted.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Whether a header arrived, whether or not its value survived redaction.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

fn is_agent_metadata_value_header(name: &str) -> bool {
    matches!(
        name,
        // Content negotiation and hint metadata.
        "content-type" | "anthropic-version" | "x-agent-step"
            // Capability opt-in headers. `anthropic-beta` (and the OpenAI
            // equivalent) name beta features whose body fields the adapter must
            // handle as a group — the beta header and its paired body field are
            // one capability. Without the value, an adapter sees only that a
            // beta was requested, not which one, and cannot forward or refuse it
            // as a group. These are feature flags, not credentials
            // (`is_credential_header` still runs first), so their values belong
            // in the closed metadata catalog.
            | "anthropic-beta"
            | "openai-beta"
    )
}

impl From<HeaderDigestWire> for HeaderDigest {
    fn from(wire: HeaderDigestWire) -> Self {
        let mut digest = Self::redacting(wire.values);
        digest.names.extend(wire.names);
        digest
    }
}

impl From<HeaderDigest> for HeaderDigestWire {
    fn from(digest: HeaderDigest) -> Self {
        Self {
            names: digest.names,
            values: digest.values,
        }
    }
}

/// Who the host decided is calling, after it has authenticated them.
///
/// Carries identity, never proof of identity: the virtual key or token that
/// established it stays in the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// Stable identifier for the caller within this deployment.
    pub subject: String,
    /// Set on server and gateway deployments; `None` on the local client, which
    /// serves a single user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
}

/// An inbound request as an `agent-adapter` sees it.
///
/// The host has already authenticated the caller and redacted the headers. The
/// adapter's job is to turn `body` into a [`crate::ChatRequest`] and to pull
/// [`AgentHint`]s out; it decides nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRequestEnvelope {
    /// The inbound protocol, e.g. `openai-chat-completions`.
    pub protocol: String,
    /// The calling tool when it identifies itself, e.g. `claude-code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_tool: Option<String>,
    #[serde(default)]
    pub headers: HeaderDigest,
    pub principal: Principal,
    /// Hints the host already extracted from transport-level headers. An adapter
    /// may add more from the body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<AgentHint>,
    /// The raw inbound body, in the inbound protocol's own shape.
    pub body: Value,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

#[cfg(test)]
mod tests {
    use super::{AgentRequestEnvelope, HeaderDigest, Principal};

    #[test]
    fn digest_keeps_credential_names_but_drops_their_values() {
        let digest = HeaderDigest::redacting([
            ("authorization", "Bearer sk-live-abc"),
            ("content-type", "application/json"),
        ]);

        assert!(digest.contains("authorization"));
        assert_eq!(digest.value("authorization"), None);
        assert_eq!(digest.value("content-type"), Some("application/json"));
    }

    #[test]
    fn digest_exposes_values_only_for_the_closed_agent_metadata_catalog() {
        let digest = HeaderDigest::redacting([
            ("x-agent-step", "planning"),
            ("content-type", "application/json"),
            (
                "anthropic-beta",
                "context-management-2025-06-27,web-search-2025-03-05",
            ),
            ("x-private-session-token", "internal-secret"),
        ]);

        assert_eq!(digest.value("x-agent-step"), Some("planning"));
        assert_eq!(digest.value("content-type"), Some("application/json"));
        // Beta capability flags reach the adapter so header/body pairs stay
        // grouped; they are metadata, not credentials.
        assert_eq!(
            digest.value("anthropic-beta"),
            Some("context-management-2025-06-27,web-search-2025-03-05")
        );
        assert!(digest.contains("x-private-session-token"));
        assert_eq!(
            digest.value("x-private-session-token"),
            None,
            "an unknown private header value must never enter an agent plugin"
        );
    }

    #[test]
    fn redaction_survives_deserialization() {
        let raw = r#"{"names":["authorization"],"values":{"authorization":"Bearer sk-live-abc"}}"#;
        let digest: HeaderDigest = serde_json::from_str(raw).expect("valid digest");

        assert!(digest.contains("authorization"));
        assert_eq!(
            digest.value("authorization"),
            None,
            "a serialized digest must not be able to reintroduce a credential"
        );
    }

    #[test]
    fn digest_round_trips_without_losing_names() {
        let digest =
            HeaderDigest::redacting([("x-api-key", "sk-live-abc"), ("x-agent-step", "planning")]);

        let encoded = serde_json::to_string(&digest).expect("serializable digest");
        let decoded: HeaderDigest = serde_json::from_str(&encoded).expect("valid digest");

        assert_eq!(decoded, digest);
        assert!(decoded.contains("x-api-key"));
        assert_eq!(decoded.value("x-agent-step"), Some("planning"));
    }

    #[test]
    fn envelope_never_serializes_an_inbound_credential() {
        let envelope = AgentRequestEnvelope {
            protocol: "openai-chat-completions".to_owned(),
            agent_tool: Some("claude-code".to_owned()),
            headers: HeaderDigest::redacting([("authorization", "Bearer sk-live-abc")]),
            principal: Principal {
                subject: "local".to_owned(),
                tenant: None,
            },
            hints: Vec::new(),
            body: serde_json::json!({"model": "auto"}),
            extensions: crate::Extensions::new(),
        };

        let encoded = serde_json::to_string(&envelope).expect("serializable envelope");

        assert!(!encoded.contains("sk-live-abc"));
        assert!(encoded.contains("authorization"));
    }
}
