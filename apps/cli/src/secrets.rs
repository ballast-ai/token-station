//! Credential resolution: slot names in, values out, never the reverse.
//!
//! The interim sources — environment variables and key files — hold the slot
//! shape the OS keychain (`C1#4`) will take over: resolve at request time, by
//! name, so a rotated key is picked up without a restart. Nothing here logs a
//! value, and errors name the slot and the source, never what was read.

use std::collections::BTreeMap;

use crate::config::{AuthConfig, ClientConfig};

/// Where each slot's value lives.
#[derive(Debug, Clone)]
enum Source {
    Env(String),
    File(std::path::PathBuf),
}

/// Resolves credential slots for every configured upstream.
#[derive(Debug, Clone, Default)]
pub struct SecretStore {
    /// Keyed by (upstream, slot): the same slot name may point at different
    /// values on different upstreams — two OpenAI-compatible upstreams both
    /// call their key `provider_api_key`.
    sources: BTreeMap<(String, String), Source>,
}

impl SecretStore {
    #[must_use]
    pub fn from_config(config: &ClientConfig) -> Self {
        let mut sources = BTreeMap::new();
        for (upstream, entry) in &config.upstreams {
            if let Some(AuthConfig { slot, env, file }) = &entry.auth {
                let source = match (env, file) {
                    (Some(name), _) => Source::Env(name.clone()),
                    (_, Some(path)) => Source::File(path.clone()),
                    // Refused by config validation before this runs.
                    (None, None) => continue,
                };
                sources.insert((upstream.clone(), slot.clone()), source);
            }
        }
        Self { sources }
    }

    /// The value for `slot` on `upstream`.
    ///
    /// # Errors
    ///
    /// A message that names the slot and where it was looked for — and never
    /// carries a value, because this string ends up in a client-visible error.
    pub fn resolve(&self, upstream: &str, slot: &str) -> Result<String, String> {
        let source = self
            .sources
            .get(&(upstream.to_owned(), slot.to_owned()))
            .ok_or_else(|| {
                format!("upstream `{upstream}` has no source configured for secret `{slot}`")
            })?;

        let value = match source {
            Source::Env(name) => std::env::var(name)
                .map_err(|_| format!("secret `{slot}`: environment variable `{name}` is not set")),
            Source::File(path) => std::fs::read_to_string(path).map_err(|error| {
                format!("secret `{slot}`: cannot read `{}`: {error}", path.display())
            }),
        }?;

        let value = value.trim();
        if value.is_empty() {
            return Err(format!("secret `{slot}` resolved to an empty value"));
        }
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::SecretStore;
    use crate::config::ClientConfig;
    use std::fs;

    fn store_with_file(key_contents: &str) -> (SecretStore, std::path::PathBuf) {
        let key_path = std::env::temp_dir().join(format!(
            "token-station-secret-{}-{key_contents_len}.txt",
            std::process::id(),
            key_contents_len = key_contents.len()
        ));
        fs::write(&key_path, key_contents).expect("temp dir writable");

        let mut config: serde_json::Value =
            serde_json::from_str(crate::EXAMPLE_CONFIG).expect("example parses");
        config["upstreams"]["openai_personal"]["auth"] = serde_json::json!({
            "slot": "provider_api_key",
            "file": key_path,
        });
        let config: ClientConfig =
            serde_json::from_value(config).expect("example with file auth parses");
        (SecretStore::from_config(&config), key_path)
    }

    #[test]
    fn a_file_backed_secret_is_read_and_trimmed() {
        let (store, path) = store_with_file("sk-test-abc\n");

        assert_eq!(
            store
                .resolve("openai_personal", "provider_api_key")
                .as_deref(),
            Ok("sk-test-abc")
        );

        fs::remove_file(path).ok();
    }

    #[test]
    fn an_empty_key_file_is_an_error_not_an_empty_bearer_token() {
        let (store, path) = store_with_file("  \n");

        let error = store
            .resolve("openai_personal", "provider_api_key")
            .expect_err("empty is not a credential");
        assert!(error.contains("empty"), "{error}");

        fs::remove_file(path).ok();
    }

    #[test]
    fn errors_name_the_slot_and_source_but_never_a_value() {
        let (store, path) = store_with_file("sk-live-secret");

        let error = store
            .resolve("openai_personal", "wrong_slot")
            .expect_err("no such slot");
        assert!(!error.contains("sk-live-secret"));
        assert!(error.contains("wrong_slot"));

        fs::remove_file(path).ok();
    }
}
