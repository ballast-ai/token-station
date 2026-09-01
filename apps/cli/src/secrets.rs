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
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::{AuthConfig, ClientConfig, EgressConfig};

const EGRESS_SECRET_OWNER: &str = "egress-proxy";

/// The local secrets store file name, under the data directory.
pub const SECRETS_FILE: &str = "secrets.json";
const SECRETS_LOCK_FILE: &str = "secrets.lock";

fn store_key(upstream: &str, slot: &str) -> String {
    format!("{upstream}/{slot}")
}

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SECRETS_FILE)
}

fn store_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SECRETS_LOCK_FILE)
}

fn acquire_store_mutation_lock(data_dir: &Path) -> Result<std::fs::File, String> {
    let path = store_lock_path(data_dir);
    token_station_private_fs::create_private_file(&path, b"")
        .map_err(|error| format!("secrets store lock create: {error}"))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("secrets store lock open: {error}"))?;
    file.lock()
        .map_err(|error| format!("secrets store lock acquire: {error}"))?;
    Ok(file)
}

/// A loaded secrets map whose cross-process mutation lock remains held.
///
/// Callers can keep several secret writes and an adjacent durable commit in one
/// lock scope. This prevents a failed desktop save from rolling back over a CLI
/// key rotation that completed between two independent store mutations.
pub struct LockedStore<'a> {
    data_dir: &'a Path,
    map: BTreeMap<String, String>,
}

impl LockedStore<'_> {
    /// Read one value while the mutation lock is held.
    #[must_use]
    pub fn get(&self, upstream: &str, slot: &str) -> Option<&str> {
        self.map.get(&store_key(upstream, slot)).map(String::as_str)
    }

    /// Replace one value in the locked in-memory map.
    pub fn set(&mut self, upstream: &str, slot: &str, value: &str) {
        self.map.insert(store_key(upstream, slot), value.to_owned());
    }

    /// Remove one value from the locked in-memory map.
    pub fn remove(&mut self, upstream: &str, slot: &str) {
        self.map.remove(&store_key(upstream, slot));
    }

    /// Atomically persist the current locked map.
    ///
    /// # Errors
    ///
    /// The map cannot be serialized or written to the private store file.
    pub fn persist(&self) -> Result<(), String> {
        write_store(self.data_dir, &self.map)
    }
}

/// Run a multi-step secrets transaction under the shared cross-process lock.
/// The lock remains held until `operation` returns.
///
/// # Errors
///
/// The lock or existing store cannot be opened, or `operation` returns an
/// error.
pub fn with_locked_store<T>(
    data_dir: &Path,
    operation: impl FnOnce(&mut LockedStore<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let _lock = acquire_store_mutation_lock(data_dir)?;
    let map = load_store(data_dir)?;
    let mut store = LockedStore { data_dir, map };
    operation(&mut store)
}

fn mutate_store(
    data_dir: &Path,
    operation: impl FnOnce(&mut BTreeMap<String, String>) -> bool,
) -> Result<(), String> {
    with_locked_store(data_dir, |store| {
        if operation(&mut store.map) {
            store.persist()?;
        }
        Ok(())
    })
}

/// Load the secrets map. A missing file is an empty store; every other read or
/// parse failure is authoritative so a mutation cannot overwrite unknown data.
fn load_store(data_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let raw = match std::fs::read_to_string(store_path(data_dir)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("secrets store read: {error}")),
    };
    serde_json::from_str(&raw).map_err(|error| format!("secrets store parse: {error}"))
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
/// The existing store cannot be read or parsed, or the updated store cannot be
/// serialized or written. The message never carries a value.
pub fn store_set(data_dir: &Path, upstream: &str, slot: &str, value: &str) -> Result<(), String> {
    mutate_store(data_dir, |map| {
        map.insert(store_key(upstream, slot), value.to_owned());
        true
    })
}

/// Read `(upstream, slot)` from the local secrets store.
///
/// # Errors
///
/// The store cannot be read or parsed, or the slot is absent. The message never
/// carries a value.
pub fn store_get(data_dir: &Path, upstream: &str, slot: &str) -> Result<String, String> {
    load_store(data_dir)?
        .get(&store_key(upstream, slot))
        .cloned()
        .ok_or_else(|| format!("secret `{upstream}/{slot}` is not in the local store"))
}

/// Read a stored egress proxy credential without exposing its internal owner key.
///
/// # Errors
///
/// The slot is absent. The message never carries a value.
pub fn store_get_egress(data_dir: &Path, slot: &str) -> Result<String, String> {
    store_get(data_dir, EGRESS_SECRET_OWNER, slot)
}

/// Remove `(upstream, slot)` from the local secrets store. A no-op if absent.
///
/// # Errors
///
/// The existing store cannot be read or parsed, or it cannot be written back
/// after removing the entry.
pub fn store_remove(data_dir: &Path, upstream: &str, slot: &str) -> Result<(), String> {
    mutate_store(data_dir, |map| {
        map.remove(&store_key(upstream, slot)).is_some()
    })
}

/// Remove the local `provider_api_key` entries for several Provider Channels
/// as one store mutation. Missing entries are ignored and every other secret is
/// retained.
///
/// # Errors
///
/// The existing store cannot be read or parsed, or the updated store cannot be
/// written atomically. On a load failure no write is attempted.
pub fn store_remove_provider_api_keys(data_dir: &Path, upstreams: &[String]) -> Result<(), String> {
    mutate_store(data_dir, |map| {
        let mut changed = false;
        for upstream in upstreams {
            changed |= map
                .remove(&store_key(upstream, "provider_api_key"))
                .is_some();
        }
        changed
    })
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
    /// A store-backed source must surface an authoritative load failure rather
    /// than treating corrupt or unreadable credential data as an empty store.
    store_error: Option<String>,
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
        let (store, store_error) = match load_store(data_dir) {
            Ok(store) => (store, None),
            Err(error) => (BTreeMap::new(), Some(error)),
        };
        Self {
            sources,
            store,
            store_error,
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
        let (store, store_error) = match load_store(data_dir) {
            Ok(store) => (store, None),
            Err(error) => (BTreeMap::new(), Some(error)),
        };
        Self {
            sources,
            store,
            store_error,
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
            Source::Store => {
                if let Some(error) = &self.store_error {
                    return Err(error.clone());
                }
                self.store
                    .get(&store_key(upstream, slot))
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "upstream `{upstream}` secret `{slot}`: not in the local store (re-enter the key)"
                        )
                    })
            }
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
    use super::{
        SecretStore, store_get, store_lock_path, store_remove, store_remove_provider_api_keys,
        store_set, with_locked_store,
    };
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

    #[test]
    fn a_malformed_local_store_fails_closed_without_being_overwritten() {
        let dir = scratch_dir("malformed-fails-closed");
        let path = dir.join(super::SECRETS_FILE);
        let malformed = br#"{"openai_personal/provider_api_key":"existing""#;
        fs::write(&path, malformed).expect("write malformed fixture");

        let mut config: serde_json::Value =
            serde_json::from_str(crate::EXAMPLE_CONFIG).expect("example parses");
        config["upstreams"]["openai_personal"]["auth"] =
            serde_json::json!({ "slot": "provider_api_key", "store": true });
        let config: ClientConfig = serde_json::from_value(config).expect("store auth parses");
        let resolver_error = SecretStore::from_config(&config, &dir)
            .resolve("openai_personal", "provider_api_key")
            .expect_err("a resolver must retain the store load failure");
        assert!(
            resolver_error.contains("secrets store parse"),
            "{resolver_error}"
        );

        let error = store_set(
            &dir,
            "replacement",
            "provider_api_key",
            "must-not-be-written",
        )
        .expect_err("a malformed store must refuse mutation");

        assert!(error.contains("secrets store"), "{error}");
        assert_eq!(
            fs::read(&path).expect("fixture remains readable"),
            malformed
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_unreadable_local_store_fails_closed() {
        let dir = scratch_dir("unreadable-fails-closed");
        let path = dir.join(super::SECRETS_FILE);
        fs::create_dir(&path).expect("directory fixture makes the store unreadable as a file");

        let error = store_get(&dir, "provider", "provider_api_key")
            .expect_err("an unreadable store must not behave like an empty store");

        assert!(error.contains("secrets store read"), "{error}");
        assert!(
            path.is_dir(),
            "the unreadable fixture must remain untouched"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn several_provider_keys_are_removed_in_one_atomic_store_mutation() {
        let dir = scratch_dir("bulk-provider-key-removal");
        store_set(&dir, "first", "provider_api_key", "one").expect("set first key");
        store_set(&dir, "second", "provider_api_key", "two").expect("set second key");
        store_set(&dir, "first", "another_slot", "keep-first-slot").expect("set unrelated slot");
        store_set(&dir, "third", "provider_api_key", "keep-third-key")
            .expect("set unrelated provider key");

        store_remove_provider_api_keys(&dir, &["first".to_owned(), "second".to_owned()])
            .expect("remove provider keys as one store mutation");

        assert!(store_get(&dir, "first", "provider_api_key").is_err());
        assert!(store_get(&dir, "second", "provider_api_key").is_err());
        assert_eq!(
            store_get(&dir, "first", "another_slot").as_deref(),
            Ok("keep-first-slot")
        );
        assert_eq!(
            store_get(&dir, "third", "provider_api_key").as_deref(),
            Ok("keep-third-key")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_store_mutation_waits_for_the_cross_process_lock() {
        let dir = scratch_dir("cross-process-mutation-lock");
        fs::create_dir_all(&dir).expect("create the secrets directory");
        token_station_private_fs::create_private_file(&store_lock_path(&dir), b"")
            .expect("create the private shared lock file");
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store_lock_path(&dir))
            .expect("open the shared lock file");
        lock.lock().expect("hold the shared mutation lock");

        let mut worker = std::process::Command::new(
            std::env::current_exe().expect("resolve the current test binary"),
        )
        .arg("--exact")
        .arg("secrets::tests::store_lock_child_process")
        .arg("--nocapture")
        .env("TOKEN_STATION_SECRET_LOCK_TEST_DIR", &dir)
        .spawn()
        .expect("spawn a separate store mutation process");
        std::thread::sleep(std::time::Duration::from_millis(100));

        assert!(
            worker
                .try_wait()
                .expect("inspect the child process")
                .is_none(),
            "a mutation must not read and replace the store while another process owns the lock"
        );
        lock.unlock().expect("release the shared mutation lock");
        assert!(
            worker
                .wait()
                .expect("wait for the mutation child")
                .success(),
            "the mutation resumes after unlock"
        );
        assert_eq!(
            store_get(&dir, "concurrent", "provider_api_key").as_deref(),
            Ok("new-value")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn store_lock_child_process() {
        let Ok(dir) = std::env::var("TOKEN_STATION_SECRET_LOCK_TEST_DIR") else {
            return;
        };
        store_set(
            std::path::Path::new(&dir),
            "concurrent",
            "provider_api_key",
            "new-value",
        )
        .expect("the child mutation succeeds");
    }

    #[test]
    fn a_locked_store_transaction_cannot_rollback_over_a_cli_rotation() {
        let dir = scratch_dir("cross-process-transaction-lock");
        store_set(&dir, "concurrent", "provider_api_key", "original")
            .expect("seed the original value");
        let mut worker = None;

        with_locked_store(&dir, |store| {
            assert_eq!(
                store.get("concurrent", "provider_api_key"),
                Some("original")
            );
            store.set("concurrent", "provider_api_key", "desktop-pending");
            store.persist()?;

            let mut child = std::process::Command::new(
                std::env::current_exe().expect("resolve the current test binary"),
            )
            .arg("--exact")
            .arg("secrets::tests::store_lock_child_process")
            .arg("--nocapture")
            .env("TOKEN_STATION_SECRET_LOCK_TEST_DIR", &dir)
            .spawn()
            .expect("spawn a concurrent CLI rotation");
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                child
                    .try_wait()
                    .expect("inspect the child process")
                    .is_none(),
                "the CLI rotation must wait until desktop rollback completes"
            );

            store.set("concurrent", "provider_api_key", "original");
            store.persist()?;
            worker = Some(child);
            Ok(())
        })
        .expect("the desktop transaction completes under one lock");

        assert!(
            worker
                .expect("the child was started")
                .wait()
                .expect("wait for the CLI rotation")
                .success(),
            "the CLI rotation resumes after the desktop transaction"
        );
        assert_eq!(
            store_get(&dir, "concurrent", "provider_api_key").as_deref(),
            Ok("new-value")
        );
        fs::remove_dir_all(dir).ok();
    }
}
