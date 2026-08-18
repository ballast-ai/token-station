use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Extensions, is_credential_header};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
}

/// The name of a credential the host holds, never the credential itself.
///
/// A `provider-adapter` says "sign this with `provider_api_key`"; the host looks
/// the name up and injects the value after the adapter has returned. This is why
/// a plugin can build an authenticated request without ever being able to read,
/// log or exfiltrate a key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(String);

impl SecretRef {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A plugin tried to put a credential where credentials are not allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretBoundaryError {
    header: String,
    reason: HeaderBoundaryReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderBoundaryReason {
    Credential,
    HostOwned,
    Invalid,
}

impl SecretBoundaryError {
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }
}

impl fmt::Display for SecretBoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            HeaderBoundaryReason::Credential => write!(
                f,
                "header `{}` carries a credential; name it with a SecretRef and let the host inject it",
                self.header
            ),
            HeaderBoundaryReason::HostOwned => write!(
                f,
                "header `{}` controls HTTP routing or framing and is owned by the host",
                self.header
            ),
            HeaderBoundaryReason::Invalid => {
                write!(f, "header `{}` has an invalid HTTP field name", self.header)
            }
        }
    }
}

impl Error for SecretBoundaryError {}

/// A plugin chose a header the host's redaction does not cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPlacementError {
    header: String,
}

impl AuthPlacementError {
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }
}

impl fmt::Display for AuthPlacementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "header `{}` is not a credential header, so nothing downstream would redact the value the host writes into it",
            self.header
        )
    }
}

impl Error for AuthPlacementError {}

/// Wire form of [`Auth`]. Private, so the only way to build an [`Auth::Header`]
/// is through the checking constructor below.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
enum AuthWire {
    Bearer {
        secret: SecretRef,
    },
    Header {
        name: String,
        secret: SecretRef,
    },
    Oauth {
        secret: SecretRef,
        scopes: Vec<String>,
    },
}

/// How the host must present a credential the `provider-adapter` names.
///
/// The adapter picks the variant, because the upstream's dialect is exactly what
/// an adapter exists to know. The host holds the value: it resolves the
/// [`SecretRef`] and writes it in *after* the adapter has returned, so the plugin
/// never observes a credential it just caused to be sent.
///
/// Closed, like [`crate::ErrorCode`]. The host has one code path per variant, so
/// a variant it does not know is a version mismatch, not something to ignore. A
/// dialect `v1` cannot express — a `SigV4` signature over the request body, which
/// would need [`Auth`] to carry the output of the `host.sign` ABI call — goes to
/// `-v2` rather than being approximated here.
///
/// There is deliberately no query-parameter variant. No `v1` provider needs one:
/// the one that looks like it does, Gemini, accepts `x-goog-api-key`. Adding it
/// would put a live credential in a URL, which is the field most likely to reach
/// a log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AuthWire", into = "AuthWire")]
pub enum Auth {
    /// `Authorization: Bearer <secret>`. OpenAI and most compatible upstreams.
    Bearer { secret: SecretRef },
    /// `<name>: <secret>`, e.g. `x-api-key` for Anthropic, `x-goog-api-key` for
    /// Gemini. `name` must be a credential header.
    Header { name: String, secret: SecretRef },
    /// The host exchanges `secret` for an access token and presents it as a
    /// bearer token.
    ///
    /// The exchange happens at injection time, inside the host. That is why
    /// there is no `host.oauth_token` ABI call for the plugin to make: a plugin
    /// that could ask for a token would hold one.
    OAuth {
        secret: SecretRef,
        scopes: Vec<String>,
    },
}

impl Auth {
    #[must_use]
    pub fn bearer(secret: SecretRef) -> Self {
        Self::Bearer { secret }
    }

    /// Names the header the host writes the credential into.
    ///
    /// # Errors
    ///
    /// Returns [`AuthPlacementError`] unless `name` is a credential header.
    /// [`SafeHeaders`] refuses those names to a plugin and [`crate::HeaderDigest`]
    /// strips their values, so a name outside that catalog would be one the host
    /// injects a secret into and nothing afterwards knows to hide.
    pub fn header(name: impl Into<String>, secret: SecretRef) -> Result<Self, AuthPlacementError> {
        let name = name.into();
        if !is_credential_header(&name) {
            return Err(AuthPlacementError { header: name });
        }
        Ok(Self::Header { name, secret })
    }

    #[must_use]
    pub fn oauth(secret: SecretRef, scopes: impl IntoIterator<Item = String>) -> Self {
        Self::OAuth {
            secret,
            scopes: scopes.into_iter().collect(),
        }
    }

    /// Which credential the host must resolve.
    #[must_use]
    pub fn secret(&self) -> &SecretRef {
        match self {
            Self::Bearer { secret } | Self::Header { secret, .. } | Self::OAuth { secret, .. } => {
                secret
            }
        }
    }
}

impl TryFrom<AuthWire> for Auth {
    type Error = AuthPlacementError;

    fn try_from(wire: AuthWire) -> Result<Self, Self::Error> {
        Ok(match wire {
            AuthWire::Bearer { secret } => Self::Bearer { secret },
            AuthWire::Header { name, secret } => Self::header(name, secret)?,
            AuthWire::Oauth { secret, scopes } => Self::OAuth { secret, scopes },
        })
    }
}

impl From<Auth> for AuthWire {
    fn from(auth: Auth) -> Self {
        match auth {
            Auth::Bearer { secret } => Self::Bearer { secret },
            Auth::Header { name, secret } => Self::Header { name, secret },
            Auth::OAuth { secret, scopes } => Self::Oauth { secret, scopes },
        }
    }
}

/// Outbound headers a `provider-adapter` is allowed to set.
///
/// Rejects credential headers instead of dropping them. Dropping would let a
/// buggy adapter ship a request that quietly loses its authentication and then
/// fails far away from the cause; rejecting fails at the boundary, where the
/// mistake is.
///
/// The rejection also applies on deserialization, so a conformance fixture
/// cannot hand-craft a descriptor that carries a key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, String>",
    into = "BTreeMap<String, String>"
)]
pub struct SafeHeaders(BTreeMap<String, String>);

impl SafeHeaders {
    /// Builds headers after checking none of them carries a credential.
    ///
    /// # Errors
    ///
    /// Returns [`SecretBoundaryError`] naming the first credential header found.
    pub fn try_new<I, K, V>(headers: I) -> Result<Self, SecretBoundaryError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut checked = BTreeMap::new();
        for (name, value) in headers {
            let name = name.into();
            if is_credential_header(&name) {
                return Err(SecretBoundaryError {
                    header: name,
                    reason: HeaderBoundaryReason::Credential,
                });
            }
            let normalized = name.to_ascii_lowercase();
            if normalized.is_empty()
                || !normalized
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            {
                return Err(SecretBoundaryError {
                    header: name,
                    reason: HeaderBoundaryReason::Invalid,
                });
            }
            if matches!(
                normalized.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "proxy-authorization"
                    | "proxy-authenticate"
                    | "keep-alive"
                    | "te"
                    | "trailer"
                    | "upgrade"
            ) {
                return Err(SecretBoundaryError {
                    header: name,
                    reason: HeaderBoundaryReason::HostOwned,
                });
            }
            checked.insert(normalized, value.into());
        }
        Ok(Self(checked))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }
}

impl TryFrom<BTreeMap<String, String>> for SafeHeaders {
    type Error = SecretBoundaryError;

    fn try_from(headers: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::try_new(headers)
    }
}

impl From<SafeHeaders> for BTreeMap<String, String> {
    fn from(headers: SafeHeaders) -> Self {
        headers.0
    }
}

/// The upstream request a `provider-adapter` wants the host to send.
///
/// The adapter never sends it. The host checks it against the upstream's
/// [`crate::ProviderConfig`], resolves `auth`, injects the credential, applies
/// its own timeout, size limits and retry budget, and only then opens a socket.
/// That is what keeps billing, quota and audit un-bypassable: a plugin with no
/// network access cannot route around them.
///
/// Note that `url` is chosen by the plugin while `auth` is honoured by the host.
/// Those two together are an exfiltration primitive, and nothing in this type
/// prevents it — [`crate::ProviderConfig::authorize`] is what does, by refusing a
/// `url` outside the configured endpoint. A host that skips that check will send
/// the operator's key wherever a plugin asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpRequestDescriptor {
    pub method: HttpMethod,
    pub url: String,
    #[serde(default, skip_serializing_if = "SafeHeaders::is_empty")]
    pub headers: SafeHeaders,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Which credential the host should attach, and how to present it. `None`
    /// for unauthenticated upstreams such as a local Ollama endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Auth>,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

impl HttpRequestDescriptor {
    #[must_use]
    pub fn new(method: HttpMethod, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: SafeHeaders::default(),
            body: None,
            auth: None,
            extensions: Extensions::new(),
        }
    }
}

/// What the host got back, handed to a `provider-adapter` for parsing.
///
/// `body` is text because every provider modelled by `v1` speaks JSON or SSE.
/// Binary responses would need a `-v2` field rather than a lossy encoding here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpResponseParts {
    pub status: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub body: String,
    #[serde(default, flatten)]
    pub extensions: Extensions,
}

#[cfg(test)]
mod tests {
    use super::{Auth, HttpMethod, HttpRequestDescriptor, SafeHeaders, SecretRef};

    #[test]
    fn auth_header_must_be_one_the_host_redacts() {
        let anthropic = Auth::header("x-api-key", SecretRef::new("provider_api_key"))
            .expect("a credential header is where a credential goes");
        assert_eq!(anthropic.secret().as_str(), "provider_api_key");

        let error = Auth::header("x-trace-id", SecretRef::new("provider_api_key"))
            .expect_err("a header nothing redacts must be refused");
        assert_eq!(error.header(), "x-trace-id");
    }

    #[test]
    fn auth_header_accepts_every_south_sanctioned_name() {
        for name in [
            "api-key",
            "x-api-key",
            "x-goog-api-key",
            "xi-api-key",
            "ocp-apim-subscription-key",
        ] {
            let auth = Auth::header(name, SecretRef::new("provider_api_key"))
                .expect("every South sanctioned header must be covered by host redaction");
            assert_eq!(auth.secret().as_str(), "provider_api_key");
        }
    }

    #[test]
    fn auth_header_placement_is_checked_on_deserialization() {
        let smuggled: Result<Auth, _> =
            serde_json::from_str(r#"{"scheme":"header","name":"x-trace-id","secret":"k"}"#);

        assert!(
            smuggled.is_err(),
            "a fixture must not be able to place a credential where logs would keep it"
        );
    }

    #[test]
    fn auth_round_trips_every_variant() {
        let cases = [
            (
                Auth::bearer(SecretRef::new("provider_api_key")),
                r#"{"scheme":"bearer","secret":"provider_api_key"}"#,
            ),
            (
                Auth::header("x-goog-api-key", SecretRef::new("gemini_key")).expect("valid"),
                r#"{"scheme":"header","name":"x-goog-api-key","secret":"gemini_key"}"#,
            ),
            (
                Auth::oauth(SecretRef::new("platform_token"), ["inference".to_owned()]),
                r#"{"scheme":"oauth","secret":"platform_token","scopes":["inference"]}"#,
            ),
        ];

        for (auth, expected) in cases {
            let encoded = serde_json::to_string(&auth).expect("serializable auth");
            assert_eq!(encoded, expected);
            assert_eq!(
                serde_json::from_str::<Auth>(&encoded).expect("valid auth"),
                auth
            );
        }
    }

    #[test]
    fn safe_headers_reject_a_credential_header() {
        let error = SafeHeaders::try_new([("Authorization", "Bearer sk-live-abc")])
            .expect_err("authorization must be rejected");

        assert_eq!(error.header(), "Authorization");
    }

    #[test]
    fn safe_headers_reject_a_credential_header_on_deserialization() {
        let parsed: Result<SafeHeaders, _> = serde_json::from_str(r#"{"x-api-key":"sk-live-abc"}"#);

        assert!(
            parsed.is_err(),
            "a fixture must not be able to smuggle a credential back in"
        );
    }

    #[test]
    fn safe_headers_reject_routing_and_http_framing_headers_case_insensitively() {
        for name in [
            "Host",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "Proxy-Authorization",
            "TE",
            "Trailer",
            "Upgrade",
        ] {
            let error = SafeHeaders::try_new([(name, "attacker-controlled")])
                .expect_err("the host, not a plugin, owns routing and framing");
            assert_eq!(error.header(), name);
        }
    }

    #[test]
    fn safe_headers_accept_ordinary_headers() {
        let headers = SafeHeaders::try_new([
            ("content-type", "application/json"),
            ("anthropic-version", "2023-06-01"),
        ])
        .expect("ordinary headers are allowed");

        assert_eq!(headers.get("content-type"), Some("application/json"));
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn descriptor_names_a_credential_without_holding_one() {
        let mut descriptor = HttpRequestDescriptor::new(HttpMethod::Post, "https://api.example/v1");
        descriptor.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));

        let json = serde_json::to_value(&descriptor).expect("serializable descriptor");

        assert_eq!(
            json["auth"],
            serde_json::json!({"scheme": "bearer", "secret": "provider_api_key"})
        );
        assert_eq!(json["method"], serde_json::json!("POST"));
    }

    #[test]
    fn descriptor_round_trips() {
        let mut descriptor = HttpRequestDescriptor::new(HttpMethod::Post, "https://api.example/v1");
        descriptor.headers =
            SafeHeaders::try_new([("content-type", "application/json")]).expect("safe headers");
        descriptor.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));
        descriptor.body = Some(serde_json::json!({"model": "gpt-5.5"}));

        let encoded = serde_json::to_string(&descriptor).expect("serializable descriptor");
        let decoded: HttpRequestDescriptor = serde_json::from_str(&encoded).expect("valid");

        assert_eq!(decoded, descriptor);
    }
}
