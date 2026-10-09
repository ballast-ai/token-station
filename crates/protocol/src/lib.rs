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
//!
//! # Changes in 0.6.0
//!
//! The kernel mirror carries this surface as `canonical_ir` contract 4.
//! Every addition is optional on the wire: an absent field deserializes to
//! `None` or to its default, and a `None` field is not serialized. JSON from a
//! 0.5.0 peer still parses, and a 0.5.0 peer ignores the new fields or keeps
//! them in [`Extensions`].
//!
//! - [`CacheControl`] and the closed [`CacheTtl`] (`"5m"`, `"1h"`): block-level
//!   prompt-cache markers on [`ContentPart::Text`], [`ContentPart::ImageUrl`]
//!   and [`ToolDef::cache_control`]. An unknown TTL is a deserialization error.
//!   A system message keeps one text part per system block. A cached tool
//!   result puts its marker on the last part of a [`Role::Tool`] message.
//! - [`Sampling::top_k`].
//! - [`ChatRequest::reasoning`] ([`Reasoning`], [`ReasoningMode`],
//!   [`ReasoningEffort`]): one reasoning field for OpenAI `reasoning_effort`
//!   and Anthropic `thinking`, with the caller's `budget_tokens` unchanged.
//! - Promotions from `extensions` keys that adapters already acted on:
//!   `reasoning_effort` becomes [`Reasoning::effort`] and
//!   `parallel_tool_calls` becomes [`ChatRequest::parallel_tool_calls`]. Both
//!   keep their wire key. `responses_tool_strict` becomes
//!   [`ToolDef::strict`]. That one changes shape, from a request-level map to
//!   a per-tool field, so a producer that still writes the old map leaves it
//!   in `extensions` as data.
//!
//! Rust code that builds these types with a struct literal, or that matches
//! `ContentPart::Text { text }` without `..`, must add the new fields. Use
//! [`ContentPart::text`] and [`ToolDef::new`] for the common cases.

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
mod values;

use std::collections::BTreeMap;

pub use capability::{CapabilityState, ModelCapability};
pub use chat::{
    CacheControl, CacheTtl, CanonicalArguments, ChatRequest, ChatResponse, Choice, Content,
    ContentPart, FinishReason, ImageUrl, Message, Reasoning, ReasoningEffort, ReasoningMode,
    ResponseFormat, Role, Sampling, ToolCall, ToolChoice, ToolDef,
};
pub use envelope::{AgentRequestEnvelope, HeaderDigest, Principal};
pub use error::{ErrorCode, ErrorEnvelope};
pub use hint::{AgentHint, HintKind};
pub use http::{
    Auth, AuthPlacementError, HttpMethod, HttpRequestDescriptor, HttpResponseParts, SafeHeaders,
    SecretBoundaryError, SecretRef,
};
pub use provider::{DescriptorError, EndpointError, ProviderApi, ProviderConfig, ProviderEndpoint};
pub use stream::{StreamChunk, StreamEvent, StreamOutcome};
pub use usage::Usage;
pub use values::{
    ComponentValueError, ComponentValues, MAX_COMPONENT_VALUE_BYTES, MAX_COMPONENT_VALUE_KEY_BYTES,
};

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
///
/// This is the host's **default** redaction set, not the list of names a
/// credential may be presented in. Since 0.5.0, [`Auth::header`] admits any
/// syntactically valid name outside [`NEVER_CREDENTIAL_HEADERS`]; the admitting
/// layer (south's descriptor admission and a package's declared secret headers)
/// decides which names a package may use. A host that presents a credential in
/// a declared name must redact that name as well as this set.
pub const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "xi-api-key",
    "ocp-apim-subscription-key",
    "cookie",
    "set-cookie",
];

/// Whether `name` is in [`CREDENTIAL_HEADERS`], ignoring ASCII case.
#[must_use]
pub fn is_credential_header(name: &str) -> bool {
    CREDENTIAL_HEADERS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// Header names that can never carry a credential, sorted.
///
/// Each one describes HTTP framing or a hop, routes the request, describes or
/// negotiates its content, identifies the client, or is a response-only
/// challenge. A credential written into one of them would corrupt the request
/// or reach proxies and logs that do not treat it as secret. [`Auth::header`]
/// and [`Auth::bearer_and_header`] refuse these names. The list is disjoint
/// from [`CREDENTIAL_HEADERS`], so every name the 0.4.0 kernel accepted is still
/// accepted. Admitting layers may refuse more names; south does.
pub const NEVER_CREDENTIAL_HEADERS: &[&str] = &[
    "accept",
    "accept-encoding",
    "connection",
    "content-encoding",
    "content-length",
    "content-type",
    "expect",
    "forwarded",
    "host",
    "http2-settings",
    "keep-alive",
    "proxy-authenticate",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
    "via",
    "www-authenticate",
];

/// The longest header name, in bytes, that [`Auth::header`] admits outside
/// [`CREDENTIAL_HEADERS`]. The longest catalog name,
/// `ocp-apim-subscription-key`, is 25 bytes.
pub const MAX_AUTH_HEADER_NAME_BYTES: usize = 64;

#[cfg(test)]
mod tests {
    use super::{CREDENTIAL_HEADERS, NEVER_CREDENTIAL_HEADERS, is_credential_header};

    #[test]
    fn credential_header_match_ignores_case() {
        assert!(is_credential_header("Authorization"));
        assert!(is_credential_header("X-Api-Key"));
        assert!(!is_credential_header("content-type"));
    }

    #[test]
    fn never_credential_headers_are_sorted_lowercase_and_disjoint_from_the_catalog() {
        assert!(
            NEVER_CREDENTIAL_HEADERS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        for name in NEVER_CREDENTIAL_HEADERS {
            assert_eq!(*name, name.to_ascii_lowercase());
            assert!(
                !CREDENTIAL_HEADERS.contains(name),
                "`{name}` would turn a 0.4.0 descriptor into a refusal"
            );
        }
    }
}
