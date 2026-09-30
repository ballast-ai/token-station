//! Search-only transport selection, independent of ordinary inference.
use crate::config::{ApiDialect, UpstreamConfig};
use serde::{Deserialize, Serialize};
use token_station_protocol::ProviderEndpoint;

/// Native server-tool transport on the configured credential's origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchTransport {
    pub api_dialect: ApiDialect,
    pub base_url: ProviderEndpoint,
    pub auth: NativeSearchAuth,
}

/// Credential wire format. The credential slot remains unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeSearchAuth {
    Bearer,
    XApiKey,
}

impl NativeSearchTransport {
    pub(crate) fn validate(&self, upstream: &UpstreamConfig) -> Result<(), String> {
        if self.api_dialect == ApiDialect::Translated {
            return Err("Native search requires a native API dialect.".into());
        }
        let origin = |endpoint: &ProviderEndpoint| {
            url::Url::parse(&endpoint.as_str()).map(|url| url.origin())
        };
        let search_origin = origin(&self.base_url).map_err(|_| "Invalid native search origin.")?;
        let upstream_origin = origin(&upstream.base_url).map_err(|_| "Invalid upstream origin.")?;
        if search_origin != upstream_origin {
            return Err("Native search must use the upstream origin.".into());
        }
        Ok(())
    }
}

/// Explicit declarations override verified official endpoint defaults.
pub(crate) fn resolve(upstream: &UpstreamConfig) -> Option<NativeSearchTransport> {
    if let Some(profile) = &upstream.native_search {
        return Some(profile.clone());
    }
    if upstream.api_dialect != ApiDialect::Translated {
        return None;
    }
    let url = url::Url::parse(&upstream.base_url.as_str()).ok()?;
    if !matches!(url.path(), "" | "/" | "/v1") {
        return None;
    }
    let (base, api_dialect, auth) = match url.origin().ascii_serialization().as_str() {
        "https://api.deepseek.com" => (
            "https://api.deepseek.com/anthropic/v1",
            ApiDialect::AnthropicNative,
            NativeSearchAuth::Bearer,
        ),
        "https://api.anthropic.com" => (
            "https://api.anthropic.com/v1",
            ApiDialect::AnthropicNative,
            NativeSearchAuth::XApiKey,
        ),
        "https://api.openai.com" => (
            "https://api.openai.com/v1",
            ApiDialect::ResponsesNative,
            NativeSearchAuth::Bearer,
        ),
        _ => return None,
    };
    Some(NativeSearchTransport {
        api_dialect,
        base_url: ProviderEndpoint::try_new(base).expect("official API root is valid"),
        auth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upstream(base: &str) -> UpstreamConfig {
        serde_json::from_value(json!({"provider":"openai-compatible","base_url":base,"models":[]}))
            .unwrap()
    }

    #[test]
    fn native_search_defaults_require_exact_official_origins_and_roots() {
        for (base, dialect, auth) in [
            (
                "https://api.deepseek.com/v1",
                ApiDialect::AnthropicNative,
                NativeSearchAuth::Bearer,
            ),
            (
                "https://api.anthropic.com",
                ApiDialect::AnthropicNative,
                NativeSearchAuth::XApiKey,
            ),
            (
                "https://api.openai.com/v1",
                ApiDialect::ResponsesNative,
                NativeSearchAuth::Bearer,
            ),
        ] {
            let profile = resolve(&upstream(base)).unwrap();
            assert_eq!(profile.api_dialect, dialect);
            assert_eq!(profile.auth, auth);
            profile.validate(&upstream(base)).unwrap();
        }
        for base in [
            "https://api.deepseek.com.evil/v1",
            "https://proxy.example/v1",
            "http://api.deepseek.com/v1",
            "https://api.deepseek.com:8443/v1",
            "https://api.deepseek.com/custom",
        ] {
            assert!(resolve(&upstream(base)).is_none());
        }
    }

    #[test]
    fn native_search_explicit_profiles_override_defaults_and_reject_other_origins() {
        let mut config = upstream("https://api.deepseek.com/v1");
        config.api_dialect = ApiDialect::ResponsesNative;
        assert!(resolve(&config).is_none());
        let mut profile = NativeSearchTransport {
            api_dialect: ApiDialect::AnthropicNative,
            base_url: ProviderEndpoint::try_new("https://api.deepseek.com/custom/v1").unwrap(),
            auth: NativeSearchAuth::XApiKey,
        };
        profile.validate(&config).unwrap();
        config.native_search = Some(profile.clone());
        assert_eq!(resolve(&config), Some(profile.clone()));
        profile.base_url = ProviderEndpoint::try_new("https://other.example/v1").unwrap();
        assert!(profile.validate(&config).is_err());
        profile.base_url = config.base_url.clone();
        profile.api_dialect = ApiDialect::Translated;
        assert!(profile.validate(&config).is_err());
    }
}
