//! Credential resolution: slot names in, values out, never the reverse.
//!
//! Secrets live in a local **store file** — `secrets.json` (mode 0600) under the
//! data directory — a JSON map of `"<upstream>/<slot>" -> value`. This replaces
//! the OS keychain: no per-key OS prompt, one uniform mechanism across macOS /
//! Windows / Linux. Environment variables and standalone key files stay
//! supported for users who prefer to keep credentials out of the store.
//!
//! The store is plaintext-on-disk (0600), like the tools this integrates with
//! (e.g. cc-switch's `auth.json`). It is written atomically with a private
//! mode via the shared private-filesystem helper. Nothing here logs a value, and errors name
//! the slot and the source, never what was read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{AuthConfig, ClientConfig, EgressConfig};

const EGRESS_SECRET_OWNER: &str = "egress-proxy";

/// The local secrets store file name, under the data directory.
pub const SECRETS_FILE: &str = "secrets.json";

fn store_key(upstream: &str, slot: &str) -> String {
    format!("{upstream}/{slot}")
}

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SECRETS_FILE)
}

/// Load the secrets map, or an empty one if the file is absent or unreadable.
fn load_store(data_dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(store_path(data_dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_store(data_dir: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    let mut rendered = serde_json::to_string_pretty(map)
        .map_err(|error| format!("secrets store does not serialize: {error}"))?;
    rendered.push('\n');
    crate::private_fs::write_atomic_private(&store_path(data_dir), rendered.as_bytes())
        .map_err(|error| format!("secrets store write: {error}"))
}

/// Write `value` for `(upstream, slot)` into the local secrets store (0600).
///
/// # Errors
///
/// The store cannot be serialized or written. The message never carries a value.
pub fn store_set(data_dir: &Path, upstream: &str, slot: &str, value: &str) -> Result<(), String> {
    let mut map = load_store(data_dir);
    map.insert(store_key(upstream, slot), value.to_owned());
    write_store(data_dir, &map)
}

/// Read `(upstream, slot)` from the local secrets store.
///
/// # Errors
///
/// The slot is absent. The message never carries a value.
pub fn store_get(data_dir: &Path, upstream: &str, slot: &str) -> Result<String, String> {
    load_store(data_dir)
        .get(&store_key(upstream, slot))
        .cloned()
        .ok_or_else(|| format!("secret `{upstream}/{slot}` is not in the local store"))
}

/// Remove `(upstream, slot)` from the local secrets store. A no-op if absent.
///
/// # Errors
///
/// The store cannot be written back after removing the entry.
pub fn store_remove(data_dir: &Path, upstream: &str, slot: &str) -> Result<(), String> {
    let mut map = load_store(data_dir);
    if map.remove(&store_key(upstream, slot)).is_some() {
        write_store(data_dir, &map)?;
    }
    Ok(())
}

/// Where each slot's value lives.
#[derive(Debug, Clone)]
enum Source {
    /// The local secrets store file (`secrets.json`).
    Store,
    Env(String),
    File(PathBuf),
}

/// Resolves configured credential slots to their values at request time.
#[derive(Default)]
pub struct SecretStore {
    /// Keyed by (upstream, slot): the same slot name may point at different
    /// values on different upstreams — two OpenAI-compatible upstreams both
    /// call their key `provider_api_key`.
    sources: BTreeMap<(String, String), Source>,
    /// The loaded local secrets store (`secrets.json`), for `Source::Store`.
    store: BTreeMap<String, String>,
}

fn source_of(auth: &AuthConfig) -> Option<Source> {
    if auth.store {
        Some(Source::Store)
    } else {
        match (&auth.env, &auth.file) {
            (Some(name), _) => Some(Source::Env(name.clone())),
            (_, Some(path)) => Some(Source::File(path.clone())),
            // Refused by config validation before this runs.
            (None, None) => None,
        }
    }
}

impl SecretStore {
    /// Build the resolver from a full client config, loading the local secrets
    /// store from `data_dir` for any slot that lives in it.
    #[must_use]
    pub fn from_config(config: &ClientConfig, data_dir: &Path) -> Self {
        let mut sources = BTreeMap::new();
        for (upstream, entry) in &config.upstreams {
            if let Some(auth) = &entry.auth
                && let Some(source) = source_of(auth)
            {
                sources.insert((upstream.clone(), auth.slot.clone()), source);
            }
        }
        if let Some(egress_auth) = &config.egress.auth
            && let Some(source) = source_of(&egress_auth.credential)
        {
            sources.insert(
                (
                    EGRESS_SECRET_OWNER.to_string(),
                    egress_auth.credential.slot.clone(),
                ),
                source,
            );
        }
        Self {
            sources,
            store: load_store(data_dir),
        }
    }

    /// Build a resolver for just the egress proxy credential.
    #[must_use]
    pub fn from_egress_config(egress: &EgressConfig, data_dir: &Path) -> Self {
        let mut sources = BTreeMap::new();
        if let Some(egress_auth) = &egress.auth
            && let Some(source) = source_of(&egress_auth.credential)
        {
            sources.insert(
                (
                    EGRESS_SECRET_OWNER.to_string(),
                    egress_auth.credential.slot.clone(),
                ),
                source,
            );
        }
        Self {
            sources,
            store: load_store(data_dir),
        }
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
            Source::Store => self
                .store
                .get(&store_key(upstream, slot))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "upstream `{upstream}` secret `{slot}`: not in the local store (re-enter the key)"
                    )
                }),
            Source::Env(name) => std::env::var(name)
                .map_err(|_| {
                    format!(
                        "upstream `{upstream}` secret `{slot}`: environment variable `{name}` is not set"
                    )
                }),
            Source::File(path) => std::fs::read_to_string(path).map_err(|error| {
                format!(
                    "upstream `{upstream}` secret `{slot}`: cannot read `{}`: {error}",
                    path.display()
                )
            }),
        }?;

        let value = value.trim();
        if value.is_empty() {
            return Err(format!(
                "upstream `{upstream}` secret `{slot}` resolved to an empty value"
            ));
        }
        Ok(value.to_owned())
    }

    /// Resolve the configured proxy credential without coupling it to an
    /// upstream name.
    ///
    /// # Errors
    ///
    /// Returns an error when the slot is unknown, inaccessible, or empty.
    pub fn resolve_egress(&self, slot: &str) -> Result<String, String> {
        self.resolve(EGRESS_SECRET_OWNER, slot)
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretStore, store_get, store_remove, store_set};
    use crate::config::ClientConfig;
    use std::fs;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "token-station-secrets-{}-{tag}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

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
        (
            SecretStore::from_config(&config, &std::env::temp_dir()),
            key_path,
        )
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
    fn the_local_store_round_trips_a_secret_and_is_removable() {
        let dir = scratch_dir("round-trip");
        store_set(&dir, "openai_personal", "provider_api_key", "sk-test-abc").expect("sets");
        assert_eq!(
            store_get(&dir, "openai_personal", "provider_api_key").as_deref(),
            Ok("sk-test-abc")
        );

        // Wired through a config, `store: true` resolves from the same file.
        let mut config: serde_json::Value =
            serde_json::from_str(crate::EXAMPLE_CONFIG).expect("example parses");
        config["upstreams"]["openai_personal"]["auth"] =
            serde_json::json!({ "slot": "provider_api_key", "store": true });
        let config: ClientConfig = serde_json::from_value(config).expect("store auth parses");
        let store = SecretStore::from_config(&config, &dir);
        assert_eq!(
            store
                .resolve("openai_personal", "provider_api_key")
                .as_deref(),
            Ok("sk-test-abc")
        );

        store_remove(&dir, "openai_personal", "provider_api_key").expect("removes");
        assert!(store_get(&dir, "openai_personal", "provider_api_key").is_err());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_store_entry_points_the_operator_at_re_entering() {
        let dir = scratch_dir("missing");
        let mut config: serde_json::Value =
            serde_json::from_str(crate::EXAMPLE_CONFIG).expect("example parses");
        config["upstreams"]["openai_personal"]["auth"] =
            serde_json::json!({ "slot": "provider_api_key", "store": true });
        let config: ClientConfig = serde_json::from_value(config).expect("store auth parses");
        let store = SecretStore::from_config(&config, &dir);

        let missing = store
            .resolve("openai_personal", "provider_api_key")
            .expect_err("nothing set");
        assert!(missing.contains("re-enter"), "{missing}");
        assert!(missing.contains("openai_personal"), "{missing}");
        assert!(missing.contains("provider_api_key"), "{missing}");
        assert!(!missing.contains("sk-"), "no value material in errors");
        fs::remove_dir_all(dir).ok();
    }
}
