use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Extensions, HttpRequestDescriptor, ModelCapability, SecretRef};

/// The upstream a `provider-adapter` is configured against, and the boundary
/// every request it builds must stay inside.
///
/// Stored scheme-and-authority lowercased, with any trailing slash removed from
/// the path, so [`ProviderEndpoint::permits`] can compare origins exactly and
/// path prefixes on segment boundaries.
///
/// A credential cannot be expressed here. That matters because this is the one
/// value that travels *into* the sandbox: it comes from the operator's config,
/// and operators are used to writing `?api-key=` or `https://user:pass@host`.
/// Either would hand a plugin the key the rest of this crate works to keep from
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderEndpoint {
    origin: String,
    path: String,
}

impl ProviderEndpoint {
    /// Parses a base URL, rejecting every shape that could carry a credential.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] naming what disqualified the URL.
    pub fn try_new(url: &str) -> Result<Self, EndpointError> {
        let (origin, rest) = split_origin(url)?;

        if rest.contains('?') || rest.contains('#') {
            return Err(EndpointError::CarriesQueryOrFragment);
        }

        Ok(Self {
            origin,
            path: rest.trim_end_matches('/').to_owned(),
        })
    }

    /// Whether `url` addresses this upstream.
    ///
    /// Same origin, and a path at or below this endpoint's. Prefix matching is
    /// segment-aware: an endpoint of `https://api.example/v1` permits
    /// `/v1/chat/completions` and refuses `/v1beta/chat`.
    ///
    /// This is what makes [`crate::Auth`] safe. Without it, a plugin naming a
    /// credential could also name the host it wants the host to send it to.
    ///
    /// The *origin* is the security boundary and it compares exactly, so
    /// `https://api.example` does not permit `https://api.example.evil/`, nor
    /// `http://api.example`. Anything this method cannot parse as an origin —
    /// including a `user:pass@` authority — is refused rather than guessed at.
    ///
    /// The *path* prefix is a narrower check on top, and it does not resolve
    /// dot-segments: `/v1/../admin` passes. That is deliberate rather than
    /// overlooked. Dot-segments cannot change an origin, so the credential still
    /// only ever reaches the configured upstream, which is the property being
    /// defended.
    #[must_use]
    pub fn permits(&self, url: &str) -> bool {
        let Ok((origin, rest)) = split_origin(url) else {
            return false;
        };
        if origin != self.origin {
            return false;
        }

        let path = rest
            .split_once(['?', '#'])
            .map_or(rest, |(before, _)| before);

        path == self.path
            || path
                .strip_prefix(&self.path)
                .is_some_and(|tail| tail.starts_with('/'))
    }

    #[must_use]
    pub fn as_str(&self) -> String {
        format!("{}{}", self.origin, self.path)
    }
}

impl fmt::Display for ProviderEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.origin, self.path)
    }
}

impl TryFrom<String> for ProviderEndpoint {
    type Error = EndpointError;

    fn try_from(url: String) -> Result<Self, Self::Error> {
        Self::try_new(&url)
    }
}

impl From<ProviderEndpoint> for String {
    fn from(endpoint: ProviderEndpoint) -> Self {
        endpoint.as_str()
    }
}

/// Splits `http(s)://authority/rest` into a lowercased origin and the remainder.
///
/// Rejects userinfo, which is the URL syntax for a credential.
fn split_origin(url: &str) -> Result<(String, &str), EndpointError> {
    let (scheme, after_scheme) = ["https://", "http://"]
        .into_iter()
        .find_map(|scheme| url.strip_prefix(scheme).map(|rest| (scheme, rest)))
        .ok_or(EndpointError::NotHttp)?;

    let authority_len = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(authority_len);

    if authority.is_empty() {
        return Err(EndpointError::MissingHost);
    }
    if authority.contains('@') {
        return Err(EndpointError::CarriesUserinfo);
    }

    Ok((format!("{scheme}{}", authority.to_ascii_lowercase()), rest))
}

/// Why a base URL is not usable as a provider endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    NotHttp,
    MissingHost,
    CarriesUserinfo,
    CarriesQueryOrFragment,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHttp => f.write_str("a provider endpoint must be `http://` or `https://`"),
            Self::MissingHost => f.write_str("a provider endpoint must name a host"),
            Self::CarriesUserinfo => f.write_str(
                "a provider endpoint must not carry userinfo; name the credential in `auth` and let the host inject it",
            ),
            Self::CarriesQueryOrFragment => f.write_str(
                "a provider endpoint must not carry a query or fragment; that is where an API key gets pasted",
            ),
        }
    }
}

impl Error for EndpointError {}

/// One configured upstream, as the host hands it to a `provider-adapter`.
///
/// The split of responsibility is the point. The host says *which* credential
/// this upstream has — a slot name, never a value, and `None` for an
/// unauthenticated upstream such as a local Ollama. The adapter says *how* that
/// credential is presented, because the provider's dialect is the one thing an
/// adapter exists to know: `Authorization: Bearer` for OpenAI, `x-api-key` for
/// Anthropic, `x-goog-api-key` for Gemini.
///
/// Neither half can go wrong quietly. [`ProviderConfig::authorize`] rejects a
/// descriptor addressed outside [`ProviderConfig::base_url`] or naming a slot
/// this upstream does not have, and [`crate::Auth::header`] rejects a header name
/// the host's redaction does not cover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// One of the providers the adapter's manifest declares, e.g.
    /// `openai-compatible`.
    pub provider: String,
    pub base_url: ProviderEndpoint,
    /// The credential slot this upstream uses, declared in the adapter's
    /// manifest under `permissions.secrets`. `None` for an unauthenticated
    /// upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<SecretRef>,
    /// Models this upstream serves. A self-hosted upstream has no catalog to
    /// query and no network for the adapter to query it with, so its operator
    /// declares them here. Absent capabilities are unsupported, per
    /// [`ModelCapability`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCapability>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl ProviderConfig {
    #[must_use]
    pub fn new(provider: impl Into<String>, base_url: ProviderEndpoint) -> Self {
        Self {
            provider: provider.into(),
            base_url,
            auth: None,
            models: Vec::new(),
            extensions: Extensions::new(),
        }
    }

    /// The gate a host runs on a descriptor before it resolves the credential.
    ///
    /// A `provider-adapter` cannot open a socket, but it does choose the URL the
    /// host opens one to, and it does name the credential the host attaches. Run
    /// together, those two would exfiltrate a key to any host the plugin liked.
    /// This is the check that stops it, which is why the URL is tested before
    /// the credential is even looked at.
    ///
    /// # Errors
    ///
    /// Returns the first [`DescriptorError`] found.
    pub fn authorize(&self, descriptor: &HttpRequestDescriptor) -> Result<(), DescriptorError> {
        if !self.base_url.permits(&descriptor.url) {
            return Err(DescriptorError::UrlOutsideEndpoint {
                url: descriptor.url.clone(),
                endpoint: self.base_url.as_str(),
            });
        }

        match (self.auth.as_ref(), descriptor.auth.as_ref()) {
            (None, Some(auth)) => Err(DescriptorError::UnexpectedCredential {
                named: auth.secret().clone(),
            }),
            (Some(_), None) => Err(DescriptorError::MissingCredential),
            (Some(slot), Some(auth)) if auth.secret() != slot => {
                Err(DescriptorError::UndeclaredCredential {
                    named: auth.secret().clone(),
                    declared: slot.clone(),
                })
            }
            (None, None) | (Some(_), Some(_)) => Ok(()),
        }
    }
}

/// Why the host refused to send a descriptor a `provider-adapter` built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorError {
    /// The adapter addressed a host this upstream is not configured for. The
    /// serious one: with a credential attached, this is exfiltration.
    UrlOutsideEndpoint { url: String, endpoint: String },
    /// The adapter named a credential slot this upstream does not have.
    UndeclaredCredential {
        named: SecretRef,
        declared: SecretRef,
    },
    /// The adapter attached a credential to an upstream configured without one.
    UnexpectedCredential { named: SecretRef },
    /// The adapter forgot the credential this upstream needs.
    ///
    /// Refused rather than sent, for the reason [`crate::SafeHeaders`] refuses
    /// rather than drops: an unauthenticated request would fail at the upstream,
    /// far from the adapter bug that caused it.
    MissingCredential,
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UrlOutsideEndpoint { url, endpoint } => write!(
                f,
                "adapter addressed `{url}`, which is outside the configured endpoint `{endpoint}`"
            ),
            Self::UndeclaredCredential { named, declared } => write!(
                f,
                "adapter named credential `{named}`; this upstream holds `{declared}`"
            ),
            Self::UnexpectedCredential { named } => write!(
                f,
                "adapter named credential `{named}`; this upstream is unauthenticated"
            ),
            Self::MissingCredential => {
                f.write_str("adapter attached no credential; this upstream requires one")
            }
        }
    }
}

impl Error for DescriptorError {}

#[cfg(test)]
mod tests {
    use super::{DescriptorError, EndpointError, ProviderConfig, ProviderEndpoint};
    use crate::{Auth, HttpMethod, HttpRequestDescriptor, SecretRef};

    fn endpoint(url: &str) -> ProviderEndpoint {
        ProviderEndpoint::try_new(url).expect("valid endpoint")
    }

    fn openai() -> ProviderConfig {
        let mut config =
            ProviderConfig::new("openai-compatible", endpoint("https://api.openai.com/v1"));
        config.auth = Some(SecretRef::new("provider_api_key"));
        config
    }

    fn descriptor(url: &str) -> HttpRequestDescriptor {
        HttpRequestDescriptor::new(HttpMethod::Post, url)
    }

    #[test]
    fn endpoint_rejects_a_key_pasted_into_the_url() {
        assert_eq!(
            ProviderEndpoint::try_new("https://api.openai.com/v1?api-key=sk-live-abc"),
            Err(EndpointError::CarriesQueryOrFragment)
        );
        assert_eq!(
            ProviderEndpoint::try_new("https://user:sk-live-abc@api.openai.com/v1"),
            Err(EndpointError::CarriesUserinfo)
        );
    }

    #[test]
    fn endpoint_rejects_a_non_http_scheme_and_a_missing_host() {
        assert_eq!(
            ProviderEndpoint::try_new("file:///etc/passwd"),
            Err(EndpointError::NotHttp)
        );
        assert_eq!(
            ProviderEndpoint::try_new("https:///v1"),
            Err(EndpointError::MissingHost)
        );
    }

    #[test]
    fn endpoint_survives_a_round_trip_and_normalizes_its_host() {
        let parsed: ProviderEndpoint =
            serde_json::from_str(r#""https://API.OpenAI.com/v1/""#).expect("valid endpoint");

        assert_eq!(parsed.as_str(), "https://api.openai.com/v1");
        assert_eq!(
            serde_json::to_string(&parsed).expect("serializable"),
            r#""https://api.openai.com/v1""#
        );
    }

    #[test]
    fn endpoint_rejects_a_credential_url_on_deserialization() {
        let parsed: Result<ProviderEndpoint, _> =
            serde_json::from_str(r#""https://api.openai.com/v1?key=sk-live-abc""#);

        assert!(
            parsed.is_err(),
            "a config file must not be able to smuggle a key into the sandbox"
        );
    }

    #[test]
    fn permits_paths_below_the_endpoint_only_on_segment_boundaries() {
        let base = endpoint("https://api.openai.com/v1");

        assert!(base.permits("https://api.openai.com/v1"));
        assert!(base.permits("https://api.openai.com/v1/chat/completions"));
        assert!(base.permits("https://api.openai.com/v1/models?limit=10"));

        assert!(
            !base.permits("https://api.openai.com/v1beta/chat"),
            "a prefix that stops mid-segment is a different path"
        );
        assert!(!base.permits("https://api.openai.com/"));
    }

    #[test]
    fn permits_compares_origins_exactly() {
        let base = endpoint("https://api.openai.com");

        assert!(base.permits("https://api.openai.com/v1/chat/completions"));

        assert!(
            !base.permits("https://api.openai.com.evil.example/v1"),
            "a suffixed host is a different origin, not a longer path"
        );
        assert!(
            !base.permits("http://api.openai.com/v1"),
            "scheme is part of the origin"
        );
        assert!(!base.permits("https://attacker.example/collect"));
    }

    #[test]
    fn authorize_refuses_to_send_a_credential_to_another_host() {
        let config = openai();
        let mut exfiltrating = descriptor("https://attacker.example/collect");
        exfiltrating.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));

        assert_eq!(
            config.authorize(&exfiltrating),
            Err(DescriptorError::UrlOutsideEndpoint {
                url: "https://attacker.example/collect".to_owned(),
                endpoint: "https://api.openai.com/v1".to_owned(),
            })
        );
    }

    #[test]
    fn authorize_accepts_the_descriptor_the_official_adapter_builds() {
        let config = openai();
        let mut request = descriptor("https://api.openai.com/v1/chat/completions");
        request.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));

        assert_eq!(config.authorize(&request), Ok(()));
    }

    #[test]
    fn authorize_refuses_a_slot_this_upstream_does_not_hold() {
        let config = openai();
        let mut request = descriptor("https://api.openai.com/v1/chat/completions");
        request.auth = Some(Auth::bearer(SecretRef::new("someone_elses_key")));

        assert_eq!(
            config.authorize(&request),
            Err(DescriptorError::UndeclaredCredential {
                named: SecretRef::new("someone_elses_key"),
                declared: SecretRef::new("provider_api_key"),
            })
        );
    }

    #[test]
    fn authorize_refuses_a_credential_on_an_unauthenticated_upstream() {
        let config =
            ProviderConfig::new("openai-compatible", endpoint("http://localhost:11434/v1"));
        let mut request = descriptor("http://localhost:11434/v1/chat/completions");
        request.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));

        assert_eq!(
            config.authorize(&request),
            Err(DescriptorError::UnexpectedCredential {
                named: SecretRef::new("provider_api_key"),
            })
        );
    }

    #[test]
    fn authorize_accepts_an_unauthenticated_local_upstream() {
        let config =
            ProviderConfig::new("openai-compatible", endpoint("http://localhost:11434/v1"));
        let request = descriptor("http://localhost:11434/v1/chat/completions");

        assert_eq!(config.authorize(&request), Ok(()));
    }

    #[test]
    fn authorize_refuses_a_forgotten_credential() {
        let config = openai();
        let request = descriptor("https://api.openai.com/v1/chat/completions");

        assert_eq!(
            config.authorize(&request),
            Err(DescriptorError::MissingCredential)
        );
    }

    #[test]
    fn url_is_checked_before_the_credential() {
        let config = openai();
        let mut exfiltrating = descriptor("https://attacker.example/collect");
        exfiltrating.auth = Some(Auth::bearer(SecretRef::new("someone_elses_key")));

        assert!(
            matches!(
                config.authorize(&exfiltrating),
                Err(DescriptorError::UrlOutsideEndpoint { .. })
            ),
            "the destination is the exfiltration gate; it is decided first"
        );
    }
}
