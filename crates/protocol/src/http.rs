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
}

impl SecretBoundaryError {
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }
}

impl fmt::Display for SecretBoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "header `{}` carries a credential; name it with a SecretRef and let the host inject it",
            self.header
        )
    }
}

impl Error for SecretBoundaryError {}

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
                return Err(SecretBoundaryError { header: name });
            }
            checked.insert(name, value.into());
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
/// The adapter never sends it. The host resolves `auth`, injects the credential,
/// applies its own timeout, size limits and retry budget, and only then opens a
/// socket. That is what keeps billing, quota and audit un-bypassable: a plugin
/// with no network access cannot route around them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpRequestDescriptor {
    pub method: HttpMethod,
    pub url: String,
    #[serde(default, skip_serializing_if = "SafeHeaders::is_empty")]
    pub headers: SafeHeaders,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Which credential the host should attach. `None` for unauthenticated
    /// upstreams such as a local Ollama endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<SecretRef>,
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
    use super::{HttpMethod, HttpRequestDescriptor, SafeHeaders, SecretRef};

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
        descriptor.auth = Some(SecretRef::new("provider_api_key"));

        let json = serde_json::to_value(&descriptor).expect("serializable descriptor");

        assert_eq!(json["auth"], serde_json::json!("provider_api_key"));
        assert_eq!(json["method"], serde_json::json!("POST"));
    }

    #[test]
    fn descriptor_round_trips() {
        let mut descriptor = HttpRequestDescriptor::new(HttpMethod::Post, "https://api.example/v1");
        descriptor.headers =
            SafeHeaders::try_new([("content-type", "application/json")]).expect("safe headers");
        descriptor.auth = Some(SecretRef::new("provider_api_key"));
        descriptor.body = Some(serde_json::json!({"model": "gpt-5.5"}));

        let encoded = serde_json::to_string(&descriptor).expect("serializable descriptor");
        let decoded: HttpRequestDescriptor = serde_json::from_str(&encoded).expect("valid");

        assert_eq!(decoded, descriptor);
    }
}
