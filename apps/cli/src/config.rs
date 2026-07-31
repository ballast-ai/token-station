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
use token_station_router_core::{
    ConfigSource, RouterConfig, RoutingMode, UpstreamModel, UpstreamRef,
};

const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

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
    /// Optional per-Agent three-tier overrides. An absent entry inherits the
    /// home router, keeping every pre-Agent-routes configuration compatible.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_routes: BTreeMap<String, AgentRouteConfig>,
    /// Named reusable three-tier routes that multiple Agents can mount.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, AgentTierRoutes>,
    /// Where the request log and metrics store live. Optional; defaults apply.
    #[serde(default)]
    pub data: DataConfig,
    /// When an upstream is taken out of rotation. Optional; defaults apply.
    #[serde(default)]
    pub health: HealthConfig,
    /// In-flight concurrency ceilings (global / per Agent / per Provider).
    /// Optional; defaults to finite process-safe ceilings.
    #[serde(default)]
    pub concurrency: crate::admission::Limits,
    /// Versioned per-model prices. Optional; an empty table leaves every cost
    /// unknown (never zero).
    #[serde(default)]
    pub pricing: crate::pricing::PriceTable,
    /// Display-only per-Agent spend and expiry thresholds. These values are
    /// intentionally absent from gateway admission/routing decisions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_budgets: BTreeMap<String, crate::budget::AgentBudget>,
    /// Explicit outbound network policy. Defaults to direct and never reads
    /// ambient proxy environment variables.
    #[serde(default)]
    pub egress: EgressConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    #[default]
    Direct,
    Http,
    Socks5,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressConfig {
    #[serde(default)]
    pub mode: EgressMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_proxy: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProxyAuthConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyAuthConfig {
    pub username: String,
    pub credential: AuthConfig,
}

impl AuthConfig {
    fn source_count(&self) -> usize {
        usize::from(self.store) + usize::from(self.env.is_some()) + usize::from(self.file.is_some())
    }
}

impl EgressConfig {
    /// Validated `(scheme, host, port)` for the configured proxy.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode and URL disagree, credentials are
    /// embedded in the URL, or an auth/no-proxy field is malformed.
    pub fn proxy_parts(&self) -> Result<Option<(String, String, u16)>, String> {
        if self.mode == EgressMode::Direct {
            if self.proxy_url.is_some() || self.auth.is_some() || !self.no_proxy.is_empty() {
                return Err(
                    "egress direct mode cannot carry proxy_url, auth, or no_proxy".to_string(),
                );
            }
            return Ok(None);
        }
        let raw = self
            .proxy_url
            .as_deref()
            .ok_or_else(|| "egress proxy mode requires proxy_url".to_string())?;
        let uri: ureq::http::Uri = raw
            .parse()
            .map_err(|_| "egress proxy_url is invalid".to_string())?;
        let scheme = uri
            .scheme_str()
            .ok_or_else(|| "egress proxy_url requires an explicit scheme".to_string())?;
        let allowed = match self.mode {
            EgressMode::Http => matches!(scheme, "http" | "https"),
            EgressMode::Socks5 => matches!(scheme, "socks5" | "socks5h"),
            EgressMode::Direct => false,
        };
        if !allowed {
            return Err("egress proxy_url scheme does not match mode".to_string());
        }
        let authority = uri
            .authority()
            .ok_or_else(|| "egress proxy_url requires a host".to_string())?;
        if authority.as_str().contains('@') {
            return Err(
                "egress proxy_url must not contain credentials; use auth.credential slot"
                    .to_string(),
            );
        }
        if uri
            .path_and_query()
            .is_some_and(|value| value.as_str() != "/")
        {
            return Err("egress proxy_url must not contain a path, query, or fragment".to_string());
        }
        let host = uri
            .host()
            .ok_or_else(|| "egress proxy_url requires a host".to_string())?;
        let port = uri.port_u16().unwrap_or(match scheme {
            "https" => 443,
            "socks5" | "socks5h" => 1080,
            _ => 80,
        });
        for entry in &self.no_proxy {
            let body = entry
                .strip_prefix("*.")
                .or_else(|| entry.strip_prefix('.'))
                .unwrap_or(entry);
            if entry != "*"
                && (entry.is_empty()
                    || entry.len() > 253
                    || body.is_empty()
                    || body.starts_with('.')
                    || body.ends_with('.')
                    || entry.contains(',')
                    || entry.chars().any(char::is_whitespace)
                    || !body.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':')
                    }))
            {
                return Err(format!("egress no_proxy entry `{entry}` is invalid"));
            }
        }
        if let Some(auth) = &self.auth
            && (auth.username.is_empty()
                || auth.username.len() > 128
                || auth.username.chars().any(char::is_control)
                || auth.credential.source_count() != 1)
        {
            return Err(
                "egress proxy auth requires a safe username and exactly one credential source"
                    .to_string(),
            );
        }
        Ok(Some((scheme.to_string(), host.to_string(), port)))
    }

    /// Whether `target` bypasses the configured proxy under the exact ureq
    /// matcher used by the data plane. Direct mode always returns true.
    ///
    /// # Errors
    ///
    /// Returns an error when either the egress policy or target URL is invalid.
    pub fn bypasses_proxy(&self, target: &str) -> Result<bool, String> {
        let Some((scheme, host, port)) = self.proxy_parts()? else {
            return Ok(true);
        };
        let protocol = match scheme.as_str() {
            "http" => ureq::ProxyProtocol::Http,
            "https" => ureq::ProxyProtocol::Https,
            "socks5" => ureq::ProxyProtocol::Socks5,
            "socks5h" => ureq::ProxyProtocol::Socks5h,
            _ => return Err("unsupported egress proxy protocol".to_string()),
        };
        let mut builder = ureq::Proxy::builder(protocol).host(&host).port(port);
        for entry in &self.no_proxy {
            builder = builder.no_proxy(entry);
        }
        let proxy = builder
            .build()
            .map_err(|_| "invalid egress proxy configuration".to_string())?;
        let uri = target
            .parse::<ureq::http::Uri>()
            .map_err(|_| "egress target URL is invalid".to_string())?;
        Ok(proxy.is_no_proxy(&uri))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRouteMode {
    Inherit,
    Custom,
    Profile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRouteConfig {
    pub mode: AgentRouteMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_route: Option<AgentTierRoutes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Per-Agent override of the top-level routing philosophy (tiered vs
    /// quota-first). Independent of `mode`, which only picks the *tier source*
    /// (home / custom / profile). `None` inherits the home router's
    /// [`RouterConfig::routing_mode`], so an Agent that never touched the toggle
    /// tracks the Home default. Set explicitly, it pins that Agent regardless of
    /// later Home changes — this is what makes each Agent switch independently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_mode: Option<RoutingMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTierRoutes {
    pub high: AgentRouteTarget,
    pub mid: AgentRouteTarget,
    pub low: AgentRouteTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRouteTarget {
    pub upstream: String,
    pub model: String,
}

impl AgentTierRoutes {
    fn entries(&self) -> [(&'static str, &AgentRouteTarget); 3] {
        [
            (TIER_HIGH, &self.high),
            (TIER_MID, &self.mid),
            (TIER_LOW, &self.low),
        ]
    }

    fn materialize(&self, home: &RouterConfig) -> Result<RouterConfig, String> {
        let mut router = home.clone();
        for (pool, target) in self.entries() {
            let upstream = UpstreamRef::new(target.upstream.clone())
                .map_err(|error| format!("Agent route {pool} has invalid upstream: {error}"))?;
            router.pools.insert(
                pool.to_owned(),
                vec![UpstreamModel::new(upstream, target.model.clone())],
            );
        }
        Ok(router)
    }
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
    /// `ClientConfig::validate` rejects.
    #[must_use]
    pub fn effective_agents(&self) -> Vec<String> {
        if self.agents.is_empty() {
            self.agent.iter().cloned().collect()
        } else {
            self.agents.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessTier {
    Free,
    #[default]
    Paid,
}

impl AccessTier {
    #[must_use]
    pub fn is_paid(&self) -> bool {
        *self == Self::Paid
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
    /// This upstream runs on the local machine (a local Ollama / LM Studio, say).
    /// The router honors `router.local_only` by keeping traffic on these; without
    /// it a request may not leave the box. Default false, so an ordinary cloud
    /// upstream is unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// Commercial identity of this Provider instance. Free and paid instances
    /// use different upstream names and keyring slots; routing order is unchanged.
    #[serde(default, skip_serializing_if = "AccessTier::is_paid")]
    pub access_tier: AccessTier,
    /// The wire dialect this upstream speaks natively. Default `translated`
    /// keeps the Canonical-IR path (inbound adapter → `ChatRequest` → provider
    /// plugin → OpenAI Chat Completions). `anthropic-native` forwards the caller's
    /// original Anthropic Messages body verbatim to `base_url` + `/messages`,
    /// preserving server tools (`web_search`), `tool_choice:{type:tool}`,
    /// server-tool result history and thinking — the things the Canonical IR
    /// cannot round-trip. Only meaningful for an anthropic-messages inbound whose
    /// upstream genuinely speaks the Anthropic wire (e.g. DeepSeek's `/anthropic`
    /// endpoint). `base_url` must end at the version segment, e.g.
    /// `https://api.deepseek.com/anthropic/v1`, so the resolved URL is
    /// `…/anthropic/v1/messages`.
    #[serde(default, skip_serializing_if = "ApiDialect::is_default")]
    pub api_dialect: ApiDialect,
    /// What this upstream serves. The provider adapter may refine it; with no
    /// network of its own it cannot replace it.
    pub models: Vec<ModelCapability>,
    /// This account's quota plan for quota-first routing: its reset windows and
    /// rate limit. Absent ⇒ the account is non-windowed (metered / pay-as-you-go)
    /// and used only as a last resort. Populated by the desktop app's quota-mode
    /// account picker; ignored entirely in tiered routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_plan: Option<crate::quota_tracker::QuotaPlan>,
}

/// The wire dialect an upstream speaks, which decides whether a request is
/// lowered through the Canonical IR or forwarded verbatim. Mirrors CC Switch's
/// `apiFormat`, which likewise defaults to native Anthropic passthrough.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiDialect {
    /// Lower the request into the Canonical IR and render it to the provider's
    /// OpenAI Chat Completions endpoint (today's behavior for every upstream).
    #[default]
    Translated,
    /// Forward the caller's original Anthropic Messages body verbatim to
    /// `base_url` + `/messages`, bypassing the Canonical IR.
    AnthropicNative,
}

impl ApiDialect {
    /// True for the default (`Translated`), so it is omitted on serialize and
    /// every existing config keeps loading under `deny_unknown_fields`.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Translated)
    }
}

/// Where a credential's value lives. Never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// The slot name the provider adapter will see, e.g. `provider_api_key`.
    pub slot: String,
    /// Read from the local secrets store (`secrets.json`, mode 0600, in the data
    /// directory). The default source. `alias = "keyring"` keeps configs written
    /// before the OS-keychain removal loading unchanged.
    #[serde(default, alias = "keyring", skip_serializing_if = "std::ops::Not::not")]
    pub store: bool,
    /// Read from this environment variable at request time — for users who keep
    /// credentials out of the store entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Read from this file (trimmed) at request time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl ClientConfig {
    #[must_use]
    pub fn is_valid_agent_id(agent_id: &str) -> bool {
        !agent_id.is_empty()
            && agent_id.len() <= 64
            && !agent_id.starts_with('-')
            && !agent_id.ends_with('-')
            && !agent_id.contains("--")
            && agent_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }

    /// Returns a validated custom router document for `agent_id`, or `None`
    /// when that Agent inherits the home router.
    ///
    /// # Errors
    ///
    /// The Agent ID is unknown, custom mode has no three-tier route, or one of
    /// its upstream references cannot be represented safely.
    pub fn custom_router_for_agent(&self, agent_id: &str) -> Result<Option<RouterConfig>, String> {
        if !Self::is_valid_agent_id(agent_id) {
            return Err(format!("invalid Agent route id `{agent_id}`"));
        }
        let Some(route) = self.agent_routes.get(agent_id) else {
            return Ok(None);
        };
        // The tier source (home / custom / profile) is one axis; the routing
        // philosophy (tiered / quota-first) is a second, independent one. Build
        // the tier base first, then stamp the effective mode over it.
        let tier_base: Option<RouterConfig> = match route.mode {
            AgentRouteMode::Inherit => None,
            AgentRouteMode::Custom => Some(
                route
                    .custom_route
                    .as_ref()
                    .ok_or_else(|| format!("Agent `{agent_id}` custom mode requires custom_route"))
                    .and_then(|tiers| tiers.materialize(&self.router))?,
            ),
            AgentRouteMode::Profile => {
                let name = route
                    .profile
                    .as_deref()
                    .ok_or_else(|| format!("Agent `{agent_id}` profile mode requires a profile"))?;
                Some(
                    self.profiles
                        .get(name)
                        .ok_or_else(|| {
                            format!("Agent `{agent_id}` mounts unknown profile `{name}`")
                        })?
                        .materialize(&self.router)?,
                )
            }
        };
        let effective_mode = route.routing_mode.unwrap_or(self.router.routing_mode);
        // Pure inheritance on both axes → no per-Agent router; the home router
        // serves it directly (keeps the fast path and the config minimal).
        if tier_base.is_none() && effective_mode == self.router.routing_mode {
            return Ok(None);
        }
        let mut router = tier_base.unwrap_or_else(|| self.router.clone());
        router.routing_mode = effective_mode;
        Ok(Some(router))
    }

    fn validate_agent_tiers(&self, owner: &str, tiers: &AgentTierRoutes) -> Result<(), String> {
        for (pool, target) in tiers.entries() {
            UpstreamRef::new(target.upstream.clone())
                .map_err(|error| format!("{owner} {pool} upstream: {error}"))?;
            let upstream = self.upstreams.get(&target.upstream).ok_or_else(|| {
                format!(
                    "{owner} {pool} routes to upstream `{}`, which is not configured",
                    target.upstream
                )
            })?;
            if !upstream
                .models
                .iter()
                .any(|capability| capability.model == target.model)
            {
                return Err(format!(
                    "{owner} {pool} routes to model `{}` not declared by upstream `{}`",
                    target.model, target.upstream
                ));
            }
        }
        Ok(())
    }

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
        let mut config: Self = serde_json::from_str(&source).map_err(|error| ConfigError {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
        // Early v1 files persisted `0` as the documented "unlimited" default.
        // Loading maps that legacy representation to today's finite defaults;
        // `save` remains strict and still rejects newly constructed zero limits.
        let defaults = crate::admission::Limits::default();
        if config.concurrency.global == 0 {
            config.concurrency.global = defaults.global;
        }
        if config.concurrency.per_agent == 0 {
            config.concurrency.per_agent = defaults.per_agent;
        }
        if config.concurrency.per_provider == 0 {
            config.concurrency.per_provider = defaults.per_provider;
        }
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

        crate::private_fs::write_atomic_private(path, rendered.as_bytes())
            .map_err(|error| fail(error.to_string()))?;
        Ok(())
    }

    /// Validate semantic and cross-field constraints after deserialization.
    ///
    /// Desktop candidate-edit transactions call this before replacing their
    /// authoritative draft so an invalid proxy or route never reaches disk.
    ///
    /// # Errors
    ///
    /// Returns the first closed, user-actionable semantic or cross-field
    /// validation failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("config version {} is not 1", self.version));
        }
        if self.concurrency.global == 0
            || self.concurrency.per_agent == 0
            || self.concurrency.per_provider == 0
        {
            return Err(
                "concurrency.global, concurrency.per_agent and concurrency.per_provider must all be greater than zero"
                    .to_owned(),
            );
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
            validate_local_identity(name, upstream)?;
            if let Some(auth) = &upstream.auth {
                let sources = auth.source_count();
                if sources != 1 {
                    return Err(format!(
                        "upstream `{name}` auth for slot `{}` must name exactly one source                          (store / env / file), found {sources}",
                        auth.slot
                    ));
                }
            }
        }

        self.egress.proxy_parts()?;

        for (agent_id, budget) in &self.agent_budgets {
            if !Self::is_valid_agent_id(agent_id) {
                return Err(format!("invalid Agent budget id `{agent_id}`"));
            }
            budget
                .validate()
                .map_err(|error| format!("Agent `{agent_id}` budget: {error}"))?;
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

        for (agent_id, route) in &self.agent_routes {
            if !Self::is_valid_agent_id(agent_id) {
                return Err(format!("invalid Agent route id `{agent_id}`"));
            }
            if route.mode == AgentRouteMode::Custom && route.custom_route.is_none() {
                return Err(format!(
                    "Agent `{agent_id}` custom mode requires custom_route"
                ));
            }
            if route.mode == AgentRouteMode::Profile {
                let profile = route
                    .profile
                    .as_deref()
                    .ok_or_else(|| format!("Agent `{agent_id}` profile mode requires a profile"))?;
                if !self.profiles.contains_key(profile) {
                    return Err(format!(
                        "Agent `{agent_id}` mounts unknown profile `{profile}`"
                    ));
                }
            }
            if let Some(tiers) = &route.custom_route {
                self.validate_agent_tiers(&format!("Agent `{agent_id}`"), tiers)?;
            }
        }

        for (name, tiers) in &self.profiles {
            if name.trim().is_empty()
                || name.len() > 80
                || name.trim() != name
                || name.chars().any(char::is_control)
            {
                return Err("profile name is invalid".to_string());
            }
            self.validate_agent_tiers(&format!("profile `{name}`"), tiers)?;
        }

        Ok(())
    }
}

fn validate_local_identity(name: &str, upstream: &UpstreamConfig) -> Result<(), String> {
    if upstream.local && !upstream.base_url.is_loopback() {
        return Err(format!(
            "upstream `{name}` is marked local but its base_url is not a loopback endpoint"
        ));
    }
    Ok(())
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
    use super::{AccessTier, ApiDialect, ClientConfig, EgressConfig, EgressMode, UpstreamConfig};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn api_dialect_defaults_to_translated_and_round_trips() {
        // A legacy upstream with no `api_dialect` still loads under
        // deny_unknown_fields and defaults to the Canonical-IR (translated) path.
        let legacy: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "provider": "openai-compatible",
            "base_url": "https://api.deepseek.com/v1",
            "models": [{ "model": "deepseek-chat" }]
        }))
        .expect("legacy upstream parses");
        assert_eq!(legacy.api_dialect, ApiDialect::Translated);

        // The default is omitted on serialize, so re-saving a legacy config does
        // not sprout the field.
        let reserialized = serde_json::to_value(&legacy).expect("serializes");
        assert!(reserialized.get("api_dialect").is_none());

        // An explicit anthropic-native upstream parses.
        let native: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "provider": "openai-compatible",
            "api_dialect": "anthropic-native",
            "base_url": "https://api.deepseek.com/anthropic/v1",
            "models": [{ "model": "deepseek-chat" }]
        }))
        .expect("anthropic-native upstream parses");
        assert_eq!(native.api_dialect, ApiDialect::AnthropicNative);
        assert_eq!(
            serde_json::to_value(&native).expect("serializes")["api_dialect"],
            serde_json::json!("anthropic-native")
        );
    }

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
    fn upstream_access_tier_defaults_to_paid_and_round_trips_free() {
        let legacy = example();
        let legacy_config: ClientConfig =
            serde_json::from_value(legacy).expect("legacy upstreams stay compatible");
        assert_eq!(
            legacy_config.upstreams["openai_personal"].access_tier,
            AccessTier::Paid
        );

        let mut free = example();
        free["upstreams"]["openai_personal"]["access_tier"] = serde_json::json!("free");
        let free_config: ClientConfig =
            serde_json::from_value(free).expect("free access tier is accepted");
        assert_eq!(
            free_config.upstreams["openai_personal"].access_tier,
            AccessTier::Free
        );
        let serialized = serde_json::to_value(free_config).expect("free config serializes");
        assert_eq!(
            serialized["upstreams"]["openai_personal"]["access_tier"],
            "free"
        );
    }

    #[test]
    fn a_local_upstream_label_cannot_authorize_a_remote_endpoint() {
        let mut value = example();
        value["upstreams"]["openai_personal"]["local"] = serde_json::json!(true);
        let path = scratch("remote-labelled-local", &value.to_string());
        let error = ClientConfig::load(&path)
            .expect_err("strict local routing must be grounded in a loopback endpoint");
        assert!(error.to_string().contains("loopback"), "{error}");
        fs::remove_file(path).ok();
    }

    #[test]
    fn egress_policy_supports_direct_http_socks_and_rejects_inline_credentials() {
        assert_eq!(EgressConfig::default().proxy_parts().unwrap(), None);
        let http = EgressConfig {
            mode: EgressMode::Http,
            proxy_url: Some("http://proxy.internal:8080".to_string()),
            no_proxy: vec!["localhost".to_string(), "*.corp.internal".to_string()],
            auth: None,
        };
        assert_eq!(
            http.proxy_parts().unwrap(),
            Some(("http".to_string(), "proxy.internal".to_string(), 8080))
        );
        let socks = EgressConfig {
            mode: EgressMode::Socks5,
            proxy_url: Some("socks5h://proxy.internal:1080".to_string()),
            no_proxy: Vec::new(),
            auth: None,
        };
        assert_eq!(socks.proxy_parts().unwrap().unwrap().0, "socks5h");
        let inline = EgressConfig {
            mode: EgressMode::Http,
            proxy_url: Some("http://user:secret@proxy.internal:8080".to_string()),
            no_proxy: Vec::new(),
            auth: None,
        };
        assert!(inline.proxy_parts().unwrap_err().contains("credentials"));
        let mut invalid_direct = EgressConfig::default();
        invalid_direct.no_proxy.push("localhost".to_string());
        assert!(invalid_direct.proxy_parts().is_err());
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
        assert!(config.agent_routes.is_empty());
        fs::remove_file(path).ok();
    }

    #[test]
    fn legacy_zero_concurrency_limits_load_as_finite_defaults_without_rewriting_source() {
        let mut value = example();
        value["concurrency"] = serde_json::json!({
            "global": 0,
            "per_agent": 0,
            "per_provider": 0
        });
        let original = serde_json::to_vec(&value).expect("legacy fixture serializes");
        let path = scratch(
            "legacy-zero-concurrency",
            std::str::from_utf8(&original).expect("JSON is UTF-8"),
        );

        let config =
            ClientConfig::load(&path).expect("legacy zero limits migrate to finite defaults");

        assert_eq!(config.concurrency.global, 64);
        assert_eq!(config.concurrency.per_agent, 16);
        assert_eq!(config.concurrency.per_provider, 16);
        assert_eq!(
            fs::read(&path).expect("legacy source remains readable"),
            original
        );
        fs::remove_file(path).ok();
    }

    #[test]
    fn legacy_zero_migration_is_per_field_while_new_saves_stay_strict() {
        let mut value = example();
        value["concurrency"] = serde_json::json!({
            "global": 0,
            "per_agent": 7,
            "per_provider": 0
        });
        let path = scratch("legacy-mixed-concurrency", &value.to_string());

        let migrated =
            ClientConfig::load(&path).expect("each legacy zero limit migrates independently");

        assert_eq!(migrated.concurrency.global, 64);
        assert_eq!(migrated.concurrency.per_agent, 7);
        assert_eq!(migrated.concurrency.per_provider, 16);

        let invalid: ClientConfig =
            serde_json::from_value(value).expect("raw zero limits still deserialize");
        let destination = path.with_extension("saved.json");
        let error = invalid
            .save(&destination)
            .expect_err("new saves must not reintroduce unlimited concurrency");
        assert!(error.to_string().contains("greater than zero"), "{error}");
        assert!(!destination.exists());
        fs::remove_file(path).ok();
    }

    fn three_tiers(upstream: &str, model: &str) -> serde_json::Value {
        serde_json::json!({
            "high": { "upstream": upstream, "model": model },
            "mid": { "upstream": upstream, "model": model },
            "low": { "upstream": upstream, "model": model }
        })
    }

    #[test]
    fn agent_routes_are_optional_and_empty_routes_do_not_serialize() {
        let config: ClientConfig = serde_json::from_value(example()).expect("example parses");
        assert!(config.agent_routes.is_empty());
        assert!(config.profiles.is_empty());
        let encoded = serde_json::to_value(config).expect("config serializes");
        assert!(encoded.get("agent_routes").is_none());
        assert!(encoded.get("profiles").is_none());
    }

    #[test]
    fn per_agent_budgets_round_trip_and_reject_invalid_agents_or_thresholds() {
        let mut value = example();
        value["agent_budgets"] = serde_json::json!({
            "codex": {
                "limit_micros": 25_000_000,
                "warning_percent": 75,
                "period_start_ms": 1000,
                "period_end_ms": 2000,
                "expiry_warning_days": 3
            }
        });
        let config: ClientConfig = serde_json::from_value(value.clone()).expect("budget validates");
        config
            .validate()
            .expect("cross-field budget validation passes");
        assert_eq!(config.agent_budgets["codex"].warning_percent, 75);
        assert_eq!(
            serde_json::to_value(&config).unwrap()["agent_budgets"],
            value["agent_budgets"]
        );

        value["agent_budgets"]["codex"]["warning_percent"] = serde_json::json!(0);
        let invalid: ClientConfig = serde_json::from_value(value.clone()).unwrap();
        assert!(invalid.validate().unwrap_err().contains("warning_percent"));
        value["agent_budgets"] = serde_json::json!({
            "bad agent!": { "limit_micros": 1 }
        });
        let invalid: ClientConfig = serde_json::from_value(value).unwrap();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("invalid Agent budget id")
        );
    }

    #[test]
    fn custom_agent_routes_validate_and_materialize_only_the_three_tier_pools() {
        let mut value = example();
        value["agent_routes"] = serde_json::json!({
            "codex": {
                "mode": "custom",
                "custom_route": three_tiers("openai_personal", "gpt-5.5")
            },
            "opencode": {
                "mode": "inherit",
                "custom_route": three_tiers("ollama_local", "llama3.3")
            }
        });
        let path = scratch("agent-routes", &value.to_string());
        let config = ClientConfig::load(&path).expect("Agent routes validate");

        let codex = config
            .custom_router_for_agent("codex")
            .expect("known Agent")
            .expect("custom RouterConfig");
        for pool in ["tier_high", "tier_mid", "tier_low"] {
            assert_eq!(codex.pools[pool][0].upstream.as_str(), "openai_personal");
            assert_eq!(codex.pools[pool][0].model, "gpt-5.5");
        }
        assert!(
            config
                .custom_router_for_agent("opencode")
                .expect("known Agent")
                .is_none()
        );
        assert!(config.agent_routes["opencode"].custom_route.is_some());
        fs::remove_file(path).ok();
    }

    #[test]
    fn per_agent_routing_mode_override_is_independent_of_tier_source() {
        let mut value = example();
        // Home stays tiered (the default). One Agent flips to quota-first while
        // otherwise inheriting Home's tiers; another keeps a custom tier route
        // but explicitly pins tiered. A third is pure inheritance.
        value["agent_routes"] = serde_json::json!({
            "codex": { "mode": "inherit", "routing_mode": "quota_first" },
            "opencode": {
                "mode": "custom",
                "custom_route": three_tiers("openai_personal", "gpt-5.5"),
                "routing_mode": "tiered"
            },
            "claude-code": { "mode": "inherit" }
        });
        let path = scratch("per-agent-mode", &value.to_string());
        let config = ClientConfig::load(&path).expect("routing_mode overrides validate");

        // Inherit tiers + quota override → a per-Agent router materializes with
        // the home tiers but quota-first mode.
        let codex = config
            .custom_router_for_agent("codex")
            .expect("known Agent")
            .expect("quota override forces a per-Agent router");
        assert_eq!(
            codex.routing_mode,
            token_station_router_core::RoutingMode::QuotaFirst
        );
        assert_eq!(
            codex.pools, config.router.pools,
            "quota override must not alter the inherited tier pools"
        );

        // Custom tiers + explicit tiered (matches home) → still a router (custom
        // tiers), and the mode is tiered.
        let opencode = config
            .custom_router_for_agent("opencode")
            .expect("known Agent")
            .expect("custom tiers still materialize");
        assert_eq!(
            opencode.routing_mode,
            token_station_router_core::RoutingMode::Tiered
        );
        assert_eq!(opencode.pools["tier_high"][0].model, "gpt-5.5");

        // Pure inheritance on both axes → no per-Agent router.
        assert!(
            config
                .custom_router_for_agent("claude-code")
                .expect("known Agent")
                .is_none()
        );
        fs::remove_file(path).ok();
    }

    #[test]
    fn several_agents_can_mount_one_named_profile() {
        let mut value = example();
        value["profiles"] = serde_json::json!({
            "cheap-and-local": three_tiers("ollama_local", "llama3.3"),
        });
        value["agent_routes"] = serde_json::json!({
            "codex": { "mode": "profile", "profile": "cheap-and-local" },
            "opencode": { "mode": "profile", "profile": "cheap-and-local" },
        });
        let path = scratch("agent-profiles", &value.to_string());
        let config = ClientConfig::load(&path).expect("profile routes validate");

        for agent in ["codex", "opencode"] {
            let router = config
                .custom_router_for_agent(agent)
                .unwrap()
                .expect("mounted profile materializes");
            for pool in ["tier_high", "tier_mid", "tier_low"] {
                assert_eq!(router.pools[pool][0].upstream.as_str(), "ollama_local");
                assert_eq!(router.pools[pool][0].model, "llama3.3");
            }
        }
        fs::remove_file(path).ok();
    }

    #[test]
    fn mounting_an_unknown_profile_is_refused() {
        let mut value = example();
        value["agent_routes"] = serde_json::json!({
            "codex": { "mode": "profile", "profile": "does-not-exist" },
        });
        let path = scratch("agent-profile-missing", &value.to_string());
        let error = ClientConfig::load(&path).expect_err("missing profile is refused");
        assert!(error.to_string().contains("does-not-exist"), "{error}");
        fs::remove_file(path).ok();
    }

    #[test]
    fn agent_route_ids_modes_and_targets_fail_closed() {
        let cases = [
            (
                "malformed-agent-id",
                serde_json::json!({
                    "Future/Agent": { "mode": "inherit" }
                }),
                "invalid Agent route id",
            ),
            (
                "custom-without-route",
                serde_json::json!({
                    "codex": { "mode": "custom" }
                }),
                "requires custom_route",
            ),
            (
                "unknown-upstream",
                serde_json::json!({
                    "codex": {
                        "mode": "custom",
                        "custom_route": three_tiers("nowhere", "gpt-5.5")
                    }
                }),
                "not configured",
            ),
            (
                "unknown-model",
                serde_json::json!({
                    "codex": {
                        "mode": "custom",
                        "custom_route": three_tiers("openai_personal", "missing-model")
                    }
                }),
                "not declared",
            ),
        ];

        for (name, routes, expected) in cases {
            let mut value = example();
            value["agent_routes"] = routes;
            let path = scratch(name, &value.to_string());
            let error = ClientConfig::load(&path).expect_err("invalid Agent route is refused");
            assert!(error.to_string().contains(expected), "{error}");
            fs::remove_file(path).ok();
        }
    }

    #[test]
    fn registry_driven_future_agent_route_ids_need_no_cli_enum_change() {
        let mut value = example();
        value["agent_routes"] = serde_json::json!({
            "future-agent": { "mode": "inherit" }
        });
        let path = scratch("future-agent-route", &value.to_string());

        let config = ClientConfig::load(&path).expect("valid dynamic Agent id is accepted");

        assert!(
            config
                .custom_router_for_agent("future-agent")
                .expect("valid dynamic Agent id")
                .is_none()
        );
        fs::remove_file(path).ok();
    }

    #[test]
    fn the_m4_agent_configs_load_and_use_isolated_runtime_paths() {
        let cases = [
            (
                "codex",
                include_str!("../codex-deepseek-config.json"),
                "127.0.0.1:8791",
                "agent-openai-responses",
            ),
            (
                "opencode",
                include_str!("../opencode-deepseek-config.json"),
                "127.0.0.1:8792",
                "agent-openai",
            ),
            (
                "openclaw",
                include_str!("../openclaw-deepseek-config.json"),
                "127.0.0.1:8793",
                "agent-openai",
            ),
        ];

        for (name, source, listen, agent) in cases {
            let path = scratch(name, source);
            let config = ClientConfig::load(&path).expect("the M4 config must stay loadable");

            assert_eq!(config.server.listen, listen);
            assert_ne!(config.server.listen, "127.0.0.1:8787");
            assert_eq!(config.plugins.agent.as_deref(), Some(agent));
            assert_eq!(
                config.plugins.dir,
                PathBuf::from("token-station-m4/plugins")
            );
            assert_eq!(
                config.data.dir,
                PathBuf::from(format!("token-station-m4/{name}/data"))
            );

            fs::remove_file(path).ok();
        }
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
