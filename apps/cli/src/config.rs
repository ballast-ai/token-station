//! The local client's configuration: one JSON file describing the server, the
//! plugins, the upstreams, and the routing table.
//!
//! The routing section is `router-core`'s `RouterConfig`, embedded verbatim —
//! the shape, its validation and its failure semantics live in that crate,
//! where neither the client nor the server gateway can fork them.
//!
//! # What is never in this file
//!
//! Credentials. An upstream's `auth` names a slot and says where the *value*
//! lives — an environment variable or a file — never what it is. The interim
//! sources exist until `C1#4` puts the OS keychain in front of them; the shape
//! (resolve at request time, by slot name) is already the keychain's.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use token_station_protocol::{ModelCapability, ProviderEndpoint};
use token_station_router_core::{ConfigSource, RouterConfig};

/// The whole client configuration file.
///
/// `deny_unknown_fields` for the same reason `RouterConfig` has it: a
/// misspelled key must fail loudly at load, not deserialize into a default
/// that silently serves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub version: u32,
    pub server: ServerConfig,
    pub plugins: PluginsConfig,
    /// Keyed by upstream reference name — the same names the router's pools
    /// use, validated to the same credential-proof shape by `UpstreamRef`.
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub router: RouterConfig,
    /// Where the request log and metrics store live. Optional; defaults apply.
    #[serde(default)]
    pub data: DataConfig,
    /// When an upstream is taken out of rotation. Optional; defaults apply.
    #[serde(default)]
    pub health: HealthConfig,
}

/// The client's ejection policy (C1#2's simplified health check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthConfig {
    /// Consecutive countable failures before an upstream is ejected.
    #[serde(default = "default_eject_after")]
    pub eject_after: u32,
    /// How long an ejected upstream stays out of rotation.
    #[serde(default = "default_cooldown_ms")]
    pub cooldown_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            eject_after: default_eject_after(),
            cooldown_ms: default_cooldown_ms(),
        }
    }
}

const fn default_eject_after() -> u32 {
    3
}

const fn default_cooldown_ms() -> u64 {
    30_000
}

/// The local data layer: file log always, metrics store by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataConfig {
    /// Holds `requests.log` (+ rotations) and `metrics.sqlite`.
    #[serde(default = "default_data_dir")]
    pub dir: PathBuf,
    /// The metrics store is on by default and local-only; turning it off
    /// leaves the file log, which is always written.
    #[serde(default = "default_true")]
    pub metrics: bool,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            dir: default_data_dir(),
            metrics: true,
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("token-station-data")
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// e.g. `127.0.0.1:8787`. The client binds loopback by design; a config
    /// that says otherwise is refused at startup, not warned about.
    pub listen: String,
    /// Require the local virtual key on every endpoint. On by default
    /// (authentication on by default): loopback is a network boundary, not a boundary against
    /// other processes on this machine.
    #[serde(default = "default_true")]
    pub auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginsConfig {
    /// Directory holding plugin packages, one subdirectory each
    /// (`manifest.json` + `adapter.wasm`). Scanned at startup: every provider
    /// dialect a discovered manifest declares registers itself
    /// (`crate::plugins::PluginRegistry`).
    pub dir: PathBuf,
    /// Deprecated single-adapter alias, kept so configs written before `agents`
    /// still load. Folded into [`PluginsConfig::effective_agents`] only when
    /// `agents` is empty. New configs should use `agents`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The agent adapter packages that own the inbound protocols, in
    /// match-priority order. More than one = several inbound protocols served
    /// at once (e.g. OpenAI + Anthropic); each request is dispatched to the
    /// first adapter whose `match_inbound` claims it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// Explicit provider dialect -> plugin package name entries. Optional
    /// since discovery: an entry may pre-declare a package not yet in `dir`,
    /// but may not contradict a discovered manifest. Writing an entry here is
    /// operator intent, so it binds without a conformance receipt.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, String>,
    /// Serve traffic through discovered packages that never passed
    /// `plugin install`'s conformance run. Off by default: dropping a file
    /// into the plugins directory must not be enough to receive requests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_unsigned: bool,
}

impl PluginsConfig {
    /// The agent adapter packages to load, in match-priority order. Prefers the
    /// `agents` list; falls back to the deprecated single `agent` alias so old
    /// configs keep working. Empty only for a config that names neither, which
    /// [`ClientConfig::validate`] rejects.
    #[must_use]
    pub fn effective_agents(&self) -> Vec<String> {
        if self.agents.is_empty() {
            self.agent.iter().cloned().collect()
        } else {
            self.agents.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    /// Which provider dialect this upstream speaks, e.g. `openai-compatible`.
    /// Must resolve in the plugin registry — a manifest under
    /// [`PluginsConfig::dir`] or an explicit [`PluginsConfig::providers`]
    /// entry. Checked where the registry exists (`upstream add`, gateway
    /// startup), not here: validation stays filesystem-free.
    pub provider: String,
    /// Validated by `protocol::ProviderEndpoint`: no userinfo, no query — the
    /// two places an API key gets pasted into a URL.
    pub base_url: ProviderEndpoint,
    /// Absent for unauthenticated upstreams such as a local Ollama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    /// What this upstream serves. The provider adapter may refine it; with no
    /// network of its own it cannot replace it.
    pub models: Vec<ModelCapability>,
}

/// Where a credential's value lives. Never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// The slot name the provider adapter will see, e.g. `provider_api_key`.
    pub slot: String,
    /// Read from the OS keychain, where `token-station-cli key set` put it.
    /// The preferred source: survives reboots, never sits in a file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keyring: bool,
    /// Read from this environment variable at request time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Read from this file (trimmed) at request time. The degraded path for
    /// hosts without a keychain; mind the file's permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl ClientConfig {
    /// Reads and structurally validates a configuration file.
    ///
    /// Routability of the embedded router section is *not* checked here — that
    /// is `Router::new`'s job, and the caller does it when building the
    /// gateway, so the two error channels stay distinct: an unreadable file and
    /// an unroutable table need different fixes.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the file and what disqualified it.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let source = fs::read_to_string(path).map_err(|error| ConfigError {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
        let config: Self = serde_json::from_str(&source).map_err(|error| ConfigError {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
        config.validate().map_err(|detail| ConfigError {
            path: path.to_path_buf(),
            detail,
        })?;
        Ok(config)
    }

    /// Validates and atomically writes this configuration to `path`.
    ///
    /// Written as a sibling temp file then renamed, so a crash mid-write can
    /// never leave a half-written file where a loadable config used to be.
    /// An invalid config writes nothing at all.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the file: either this config fails the
    /// same validation [`ClientConfig::load`] applies, or the filesystem
    /// refused the write.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let fail = |detail: String| ConfigError {
            path: path.to_path_buf(),
            detail,
        };
        self.validate().map_err(fail)?;

        let mut rendered =
            serde_json::to_string_pretty(self).map_err(|error| fail(error.to_string()))?;
        rendered.push('\n');

        let file_name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| fail("path has no file name".to_owned()))?;
        let temp = path.with_file_name(format!(".{file_name}.tmp"));
        fs::write(&temp, rendered).map_err(|error| fail(error.to_string()))?;
        fs::rename(&temp, path).map_err(|error| fail(error.to_string()))?;
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("config version {} is not 1", self.version));
        }

        if self.plugins.effective_agents().is_empty() {
            return Err(
                "plugins.agents must name at least one agent adapter (or the deprecated \
                 plugins.agent)"
                    .to_owned(),
            );
        }

        // The product promise is a loopback proxy. Binding anything else is a
        // different product with different security properties.
        let listen: std::net::SocketAddr = self
            .server
            .listen
            .parse()
            .map_err(|error| format!("server.listen `{}`: {error}", self.server.listen))?;
        if !listen.ip().is_loopback() {
            return Err(format!(
                "server.listen `{listen}` is not a loopback address; the local client only \
                 serves 127.0.0.1"
            ));
        }

        for (name, upstream) in &self.upstreams {
            token_station_router_core::UpstreamRef::new(name.clone())
                .map_err(|error| error.to_string())?;
            if let Some(auth) = &upstream.auth {
                let sources = usize::from(auth.keyring)
                    + usize::from(auth.env.is_some())
                    + usize::from(auth.file.is_some());
                if sources != 1 {
                    return Err(format!(
                        "upstream `{name}` auth for slot `{}` must name exactly one source                          (keyring / env / file), found {sources}",
                        auth.slot
                    ));
                }
            }
        }

        // Every pool member must name a configured upstream. Router::new
        // validates the table's internal coherence; this is the cross-check
        // against the world outside the table.
        for (pool, members) in &self.router.pools {
            for member in members {
                if !self.upstreams.contains_key(member.upstream.as_str()) {
                    return Err(format!(
                        "pool `{pool}` routes to upstream `{}`, which is not configured",
                        member.upstream
                    ));
                }
            }
        }

        Ok(())
    }
}

/// The file could not be read, parsed, or structurally validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    path: PathBuf,
    detail: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.detail)
    }
}

impl Error for ConfigError {}

/// The client's half of the [`ConfigSource`] pair: the server gateway reads a
/// database, this reads the operator's config file and hands the embedded
/// router section to a [`ConfigCache`](token_station_router_core::ConfigCache).
///
/// Loading re-reads the whole [`ClientConfig`], so the file's structural
/// validation runs on every refresh — a reload can never smuggle in a router
/// section from a file the client would refuse to start on.
#[derive(Debug, Clone)]
pub struct FileRouterSource {
    path: PathBuf,
}

impl FileRouterSource {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigSource for FileRouterSource {
    type Error = ConfigError;

    fn load(&self) -> Result<RouterConfig, ConfigError> {
        ClientConfig::load(&self.path).map(|config| config.router)
    }
}

#[cfg(test)]
mod tests {
    use super::ClientConfig;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-station-cfg-{}-{name}.json",
            std::process::id()
        ));
        fs::write(&path, contents).expect("temp dir is writable");
        path
    }

    fn example() -> serde_json::Value {
        serde_json::from_str(crate::EXAMPLE_CONFIG).expect("the shipped example parses")
    }

    #[test]
    fn the_file_router_source_reloads_and_survives_a_lost_file() {
        use std::sync::Arc;
        use token_station_router_core::{CacheError, ConfigCache};

        let path = scratch("file-source", crate::EXAMPLE_CONFIG);
        let mut cache = ConfigCache::load(super::FileRouterSource::new(&path))
            .expect("the example's router section is routable");

        // An edit is picked up by refresh: a new router replaces the old one.
        let before = Arc::clone(cache.current());
        let mut edited = example();
        edited["router"]["default_pool"] = serde_json::json!("sota");
        fs::write(&path, edited.to_string()).expect("temp dir is writable");
        cache.refresh().expect("the edited file is still valid");
        assert!(!Arc::ptr_eq(&before, cache.current()));

        // A file lost mid-flight fails the refresh and changes nothing: the
        // last configuration that validated keeps serving.
        let serving = Arc::clone(cache.current());
        fs::remove_file(&path).ok();
        let error = cache.refresh().expect_err("the file is gone");
        assert!(matches!(error, CacheError::Source(_)), "{error}");
        assert!(Arc::ptr_eq(&serving, cache.current()));
    }

    #[test]
    fn the_file_router_source_fails_the_first_load_loudly() {
        use token_station_router_core::{CacheError, ConfigCache};

        let missing = std::env::temp_dir().join("token-station-cfg-never-written.json");
        let error = ConfigCache::load(super::FileRouterSource::new(&missing))
            .expect_err("starting with no configuration is fatal");
        assert!(matches!(error, CacheError::Source(_)), "{error}");
    }

    #[test]
    fn the_shipped_example_config_loads() {
        let path = scratch("example", crate::EXAMPLE_CONFIG);
        let config = ClientConfig::load(&path).expect("the example must stay loadable");

        assert_eq!(config.plugins.effective_agents(), ["agent-openai"]);
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_non_loopback_listen_address_is_refused() {
        let mut broken = example();
        broken["server"]["listen"] = serde_json::json!("0.0.0.0:8787");
        let path = scratch("public", &broken.to_string());

        let error = ClientConfig::load(&path).expect_err("0.0.0.0 is a different product");
        assert!(error.to_string().contains("loopback"), "{error}");

        fs::remove_file(path).ok();
    }

    #[test]
    fn an_unmapped_provider_dialect_is_the_registry_s_problem_not_validation_s() {
        // Since discovery (B0), whether a dialect resolves depends on what is
        // on disk; `upstream add` and gateway startup check the registry.
        // Validation staying filesystem-free means this config *loads*.
        let mut moved = example();
        moved["upstreams"]["openai_personal"]["provider"] = serde_json::json!("anthropic");
        let path = scratch("unmapped-dialect", &moved.to_string());

        ClientConfig::load(&path).expect("resolution is deferred to the plugin registry");
        fs::remove_file(path).ok();
    }

    #[test]
    fn the_explicit_providers_map_is_optional() {
        let mut minimal = example();
        minimal["plugins"]
            .as_object_mut()
            .expect("plugins is an object")
            .remove("providers");
        let path = scratch("no-providers-map", &minimal.to_string());

        ClientConfig::load(&path).expect("discovery makes the map optional");
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_pool_naming_an_unconfigured_upstream_is_refused() {
        let mut broken = example();
        broken["router"]["pools"]["sota"][0]["upstream"] = serde_json::json!("nowhere");
        let path = scratch("ghost-upstream", &broken.to_string());

        let error = ClientConfig::load(&path).expect_err("no such upstream");
        assert!(error.to_string().contains("nowhere"), "{error}");

        fs::remove_file(path).ok();
    }

    #[test]
    fn auth_must_name_exactly_one_source() {
        let mut broken = example();
        broken["upstreams"]["openai_personal"]["auth"] =
            serde_json::json!({ "slot": "provider_api_key" });
        let path = scratch("no-source", &broken.to_string());

        let error = ClientConfig::load(&path).expect_err("slot with no source");
        assert!(error.to_string().contains("exactly one source"), "{error}");

        fs::remove_file(path).ok();
    }

    #[test]
    fn a_key_pasted_into_a_base_url_is_refused_by_the_endpoint_type() {
        let mut broken = example();
        broken["upstreams"]["openai_personal"]["base_url"] =
            serde_json::json!("https://api.openai.com/v1?api-key=sk-live-abc");
        let path = scratch("key-in-url", &broken.to_string());

        assert!(ClientConfig::load(&path).is_err());

        fs::remove_file(path).ok();
    }
}
