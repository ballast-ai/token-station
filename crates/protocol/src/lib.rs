//! Canonical protocol types shared by token-station components.
//!
//! The host understands only these types. Inbound agent protocols are
//! normalized into them by an `agent-adapter`, and outbound provider requests
//! are built from them by a `provider-adapter`. Adapters perform protocol
//! translation only: routing, budget, fallback, billing and audit stay in the
//! host, so nothing here can name a provider, a credential or a budget.
//!
//! Four boundaries are enforced by the type system rather than by convention:
//!
//! - [`HeaderDigest`] cannot hold the value of an authentication header, so an
//!   `agent-adapter` never observes an inbound credential.
//! - [`HttpRequestDescriptor`] carries an [`Auth`] and [`SafeHeaders`], so a
//!   `provider-adapter` can name a credential and say how to present it, but
//!   never spell one out. The host injects the real value before the request
//!   leaves the process.
//! - [`ProviderEndpoint`] cannot express a credential, so the operator's config
//!   cannot leak a key *into* the sandbox through a `?api-key=` query or a
//!   `user:pass@` authority.
//! - [`ProviderConfig::authorize`] refuses a descriptor addressed outside the
//!   configured upstream. Choosing the URL and naming the credential are both
//!   the plugin's to do; only this check stops them combining into an
//!   exfiltration.
//!
//! The first three survive deserialization, which is what makes them auditable:
//! a fixture cannot smuggle a credential back in through the wire format. The
//! fourth is a host obligation, checked here so both hosts check it alike.
//!
//! # Versioning
//!
//! These types are the `v1` ABI surface. Unknown JSON fields deserialize into
//! [`Extensions`] rather than failing, so a `v1` peer keeps working when a newer
//! peer adds a field. Enumerations are deliberately closed: an unknown variant
//! is a version mismatch and should surface as an error instead of being
//! silently coerced. Breaking changes go to `-v2`; they never mutate `v1` in
//! place.

#![allow(
    clippy::module_name_repetitions,
    reason = "IR types are re-exported at the crate root, where the module prefix is what disambiguates them"
)]

mod capability;
mod chat;
mod envelope;
mod error;
mod hint;
mod http;
mod provider;
mod stream;
mod usage;

use std::collections::BTreeMap;

pub use capability::ModelCapability;
pub use chat::{
    ChatRequest, ChatResponse, Choice, Content, ContentPart, FinishReason, ImageUrl, Message,
    ResponseFormat, Role, Sampling, ToolCall, ToolChoice, ToolDef,
};
pub use envelope::{AgentRequestEnvelope, HeaderDigest, Principal};
pub use error::{ErrorCode, ErrorEnvelope};
pub use hint::{AgentHint, HintKind};
pub use http::{
    Auth, AuthPlacementError, HttpMethod, HttpRequestDescriptor, HttpResponseParts, SafeHeaders,
    SecretBoundaryError, SecretRef,
};
pub use provider::{DescriptorError, EndpointError, ProviderConfig, ProviderEndpoint};
pub use stream::{StreamChunk, StreamEvent};
pub use usage::Usage;

/// Forward-compatible bag for fields this ABI version does not model.
///
/// Unknown keys land here instead of failing deserialization, which is how a
/// `v1` adapter tolerates a newer peer. Advanced capabilities live here until
/// they are promoted into a `-v2` ABI.
///
/// Ordering is stable because conformance fixtures must serialize
/// deterministically.
pub type Extensions = BTreeMap<String, serde_json::Value>;

/// Header names whose values must never reach a plugin, nor leave inside a
/// plugin-authored request.
///
/// Compared lowercase. Single source of truth for both the inbound redaction in
/// [`HeaderDigest`] and the outbound rejection in [`SafeHeaders`].
pub(crate) const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
];

pub(crate) fn is_credential_header(name: &str) -> bool {
    CREDENTIAL_HEADERS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::is_credential_header;

    #[test]
    fn credential_header_match_ignores_case() {
        assert!(is_credential_header("Authorization"));
        assert!(is_credential_header("X-Api-Key"));
        assert!(!is_credential_header("content-type"));
    }
}
