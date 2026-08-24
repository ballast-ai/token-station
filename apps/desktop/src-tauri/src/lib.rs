//! Token Station desktop backend.
//!
//! This crate does not rewrite routing or gateway logic. It uses
//! `token-station-cli` as a library and reuses the same `Gateway`, `ClientConfig`,
//! `server::serve`, and keychain. The GUI is a panel over that core. Its three
//! routing tiers populate tier_high, tier_mid, and tier_low pools with one
//! provider-model pair each, then heuristic bands select among them.
//!
//! Partially configured tiers are invalid under RouterConfig validation, so the
//! draft remains a serde_json::Value and materializes as ClientConfig only when
//! saving or starting. Failed validation is reported to the user without writing.

pub mod agent_integration;
mod config_state;
mod cursor_tunnel;
mod desktop_shell;
pub mod desktop_update;
mod free_provider_catalog;
mod model_catalog;
mod pricing_catalog;
mod provider_tombstones;
mod recovery;
mod serve_lifecycle;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::UpdaterExt;
use zeroize::Zeroizing;

use token_station_cli::bodylog::{valid_request_id, BodyLog, PlaintextExchange};
use token_station_cli::budget::{AgentBudget, BudgetStatus};
use token_station_cli::cancel::{CancelReason, CancelToken};
use token_station_cli::config::{
    ClientConfig, EgressConfig, PluginsConfig, RoutingMode as HostRoutingMode,
};
use token_station_cli::gateway::{FeatureLayer, Gateway, HealthLayer, Reply, StageStatus};
use token_station_cli::plugins::{PackageManifest, PluginRegistry, Receipts};
use token_station_cli::pricing::{ModelPrice, PriceTable};
use token_station_cli::request_context::RequestContext;
use token_station_cli::{
    secrets, stats,
    store::{ReceiptQuery, SqliteStore},
    upgrade,
};
use token_station_metrics::ReceiptView;
use token_station_protocol::{CapabilityState, ModelCapability, ProviderApi, ProviderEndpoint};
use token_station_router_core::{UpstreamModel, UpstreamRef};

use agent_integration::commands::{
    apply_agent_plan, apply_snapshot_restore, force_forget_agent, get_agent_drift,
    get_cached_agent_views, list_agent_registry, list_agent_snapshots, plan_agent_connection,
    plan_agent_disconnect, plan_snapshot_restore, runtime_from_app, scan_agents, AgentCommandState,
};
use agent_integration::registry::AgentRegistry;
use agent_integration::types::AdmissionStatus;
use config_state::ConfigState;
use cursor_tunnel::{
    configure_cursor_provider, get_cursor_provider_status, restore_cursor_provider,
    CursorTunnelState,
};
use desktop_update::{
    official_update_manifest_endpoint, DesktopUpdateCandidate, DesktopUpdateOperation,
    DesktopUpdateProgress, DesktopUpdateView, OFFICIAL_PUBLIC_KEY, PROGRESS_EVENT,
};
use model_catalog::ModelDiscoveryView;
use pricing_catalog::{
    ModelPriceSuggestionView, PublicProviderModelsView, RequestedModelPriceSuggestion,
};
use recovery::{
    DiagnosticPreview, FrontendDiagnosticInput, FrontendDiagnosticRecord, RecoveryMode,
    RecoveryState,
};
use serve_lifecycle::{prepare_server, PreparedServer, RunningServer, StartFailure};

/// Pool names for the three tier slots shown as the panel's high, middle, and low rows.
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// Stable ID for each tier's keyword override rule. User keywords enter the
/// rule's keywords_any list and force that tier ahead of complexity scoring at
/// router-core layer 1. IDs remain stable because decision records and audits
/// also store them as matched routing-rule IDs.
const KW_RULE_HIGH: &str = "kw-high";
const KW_RULE_MID: &str = "kw-mid";
const KW_RULE_LOW: &str = "kw-low";

/// Map UI slots to pool names and keyword-rule IDs. Rule order is priority from
/// high to mid to low, so phrases matching multiple tiers move upward safely.
fn tier_pool_and_rule(slot: &str) -> Result<(&'static str, &'static str), String> {
    match slot {
        "high" => Ok((TIER_HIGH, KW_RULE_HIGH)),
        "mid" => Ok((TIER_MID, KW_RULE_MID)),
        "low" => Ok((TIER_LOW, KW_RULE_LOW)),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

/// Three tiers from high to low as UI slot, pool name, and keyword-rule ID; preserve this order in router.rules.
const TIER_ORDER: [(&str, &str, &str); 3] = [
    ("high", TIER_HIGH, KW_RULE_HIGH),
    ("mid", TIER_MID, KW_RULE_MID),
    ("low", TIER_LOW, KW_RULE_LOW),
];

/// Tier thresholds mapping heuristic scores to tiers. Bands descend strictly by
/// at_least, with a final zero fallback. Evaluation will calibrate these defaults later.
const CUT_HIGH: u32 = 55;
const CUT_MID: u32 = 22;

/// Derive required desktop inbound adapters from Connector capabilities.
/// Deduplicate adapters while preserving build-time registry order so adding a
/// Connector no longer requires changes here.
fn desktop_agents() -> Vec<&'static str> {
    let mut agents = Vec::new();
    for connector in agent_integration::connectors::builtin_connectors() {
        let adapter = connector.capabilities().adapter_id;
        if !agents.contains(&adapter) {
            agents.push(adapter);
        }
    }
    agents
}

const SERVE_STATE_CHANGED_EVENT: &str = "serve-state-changed";

enum ServerLifecycle {
    Stopped {
        generation: u64,
    },
    Starting {
        generation: u64,
        listen: String,
        revision: u64,
    },
    Applying {
        generation: u64,
        revision: u64,
        old: RunningServer,
    },
    Stopping {
        generation: u64,
        listen: String,
        draining: bool,
    },
    Running {
        generation: u64,
        server: RunningServer,
        apply_error: Option<String>,
    },
    Failed {
        generation: u64,
        listen: String,
        error: String,
    },
}

impl ServerLifecycle {
    fn stopped() -> Self {
        Self::Stopped { generation: 0 }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Stopped { generation }
            | Self::Starting { generation, .. }
            | Self::Applying { generation, .. }
            | Self::Stopping { generation, .. }
            | Self::Running { generation, .. }
            | Self::Failed { generation, .. } => *generation,
        }
    }
}

/// Global backend state protected by one lock; commands are short transactions.
struct AppInner {
    /// Actual token-station.json configuration path.
    config_path: PathBuf,
    /// Authoritative config draft. Materialize and validate candidates before replacing current state.
    draft: Value,
    /// Preserve startup read or validation errors. Show a safe template but block
    /// writes so Save cannot silently overwrite the user's original file.
    load_error: Option<String>,
    /// Persistent identity of the editable saved config; Runtime Supervisor owns the running revision.
    config_state: ConfigState,
    /// In-process editing state for Agent-specific routes. Tiers may be empty but never enter the savable global draft.
    agent_route_drafts: BTreeMap<String, BTreeMap<String, TierView>>,
    /// Authoritative proxy-service lifecycle state.
    server: ServerLifecycle,
    /// Free-provider verification sends real upstream requests; an in-memory single-flight set limits duplication and abuse.
    pending_free_providers: BTreeSet<String>,
    /// Verified but unsaved provider keys. Clear them on exit to avoid orphaned keys without config references.
    pending_provider_keys: BTreeMap<String, Zeroizing<String>>,
    /// In-flight model discovery is bounded and single-flight per Provider name.
    pending_provider_discoveries: BTreeSet<String>,
    /// Official provider dialects approved for South at startup or explicit plugin refresh.
    south_approved_dialects: BTreeSet<String>,
    /// Monotonic in-process identities for Provider definitions. Value snapshots
    /// alone cannot detect an A -> B -> A edit while an async operation is in flight.
    upstream_epochs: BTreeMap<String, u64>,
    /// Latest model-discovery operation for each Provider name. Provider
    /// identity can stay unchanged while two network responses finish out of order.
    discovery_generations: BTreeMap<String, u64>,
}

pub struct AppStateManaged(Mutex<AppInner>);

struct FreeProviderValidationGuard<'a> {
    inner: &'a Mutex<AppInner>,
    upstream: String,
}

struct ProviderDiscoveryGuard<'a> {
    inner: &'a Mutex<AppInner>,
    provider: String,
}

impl Drop for FreeProviderValidationGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_free_providers.remove(&self.upstream);
    }
}

impl Drop for ProviderDiscoveryGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_provider_discoveries.remove(&self.provider);
        if inner.draft["upstreams"].get(&self.provider).is_none() {
            inner.discovery_generations.remove(&self.provider);
        }
    }
}

/// Writable runtime locations resolved from Tauri's per-application roots.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopPaths {
    config_file: PathBuf,
    data_dir: PathBuf,
    plugins_dir: PathBuf,
    agent_data_root: PathBuf,
}

impl DesktopPaths {
    fn from_app_roots(config_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            config_file: config_root.join("token-station.json"),
            data_dir: data_root.join("token-station-data"),
            plugins_dir: data_root.join("plugins"),
            agent_data_root: data_root.join("agent-integration"),
        }
    }

    fn create_writable_dirs(&self) -> Result<(), std::io::Error> {
        for path in [
            self.config_file
                .parent()
                .expect("desktop config file always has a parent"),
            self.data_dir.as_path(),
            self.plugins_dir.as_path(),
            self.agent_data_root.as_path(),
        ] {
            crate::agent_integration::safe_fs::ensure_private_dir(path)?;
        }
        let config_root = self
            .config_file
            .parent()
            .expect("desktop config file always has a parent");
        for path in [
            self.config_file.clone(),
            config_root.join("token-station.state.json"),
            self.data_dir.join("virtual-key"),
            self.data_dir.join("plugin-receipts.json"),
            self.data_dir.join("model-catalog-cache.json"),
            self.data_dir.join("provider-tombstones.json"),
            self.data_dir.join("metrics.sqlite"),
            self.data_dir.join("metrics.sqlite-wal"),
            self.data_dir.join("metrics.sqlite-shm"),
            self.data_dir.join("requests.log"),
            self.data_dir.join("requests.log.1"),
            self.data_dir.join("requests.log.2"),
            self.data_dir.join("requests.log.3"),
            self.data_dir.join("diagnostics/frontend.jsonl"),
        ] {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => crate::agent_integration::safe_fs::harden_private_file(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// OS application data roots injected by Tauri for Agent snapshots and
/// ownership records.
#[derive(Clone)]
pub struct AgentIntegrationPaths {
    pub snapshot_root: PathBuf,
    pub ownership_root: PathBuf,
}

/// New-config template. Empty upstreams and an unset Direct target form a valid
/// editing draft until the user chooses a provider-model pair. Tauri injects
/// runtime directories.
fn template(data_dir: &std::path::Path, plugins_dir: &std::path::Path) -> Value {
    let pricing = serde_json::to_value(PriceTable::builtin())
        .expect("the built-in price table always serializes");
    json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:8787", "auth": true },
        "data": { "dir": data_dir, "metrics": true },
        "plugins": {
            "dir": plugins_dir,
            "agents": desktop_agents(),
            "providers": { "openai-compatible": "provider-openai-compatible-v2" }
        },
        "upstreams": {},
        "pricing": pricing,
        "routing": {
            "mode": "direct"
        },
        "router": {
            "version": 1,
            "routing_mode": "tiered",
            "pools": {},
            "rules": [],
            "hint_routes": [],
            "default_pool": "",
            "assumed_context_window": 8192
        }
    })
}

const BUNDLED_PLUGIN_IDS: [&str; 6] = [
    "agent-openai",
    "agent-anthropic",
    "agent-openai-responses",
    "agent-gemini",
    "provider-openai-compatible",
    "provider-anthropic",
];

#[derive(Serialize)]
struct InstalledPluginSelfTest {
    id: String,
    version: String,
    kind: String,
    source: &'static str,
    protocols: Vec<String>,
    agent_tools: Vec<String>,
    providers: Vec<String>,
    capabilities: Vec<String>,
    loadable: bool,
}

fn self_test_scratch_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir().join(format!(
        "token-station-installed-self-test-{}-{nonce}",
        std::process::id()
    ))
}

fn collect_installed_self_test() -> Result<Value, String> {
    let scratch = self_test_scratch_dir();
    let data_dir = scratch.join("data");
    let permission_probe = data_dir.join("permission-probe");
    let missing_plugins = scratch.join("intentionally-missing-plugins");
    let result = (|| {
        token_station_private_fs::ensure_private_dir(&data_dir)
            .map_err(|error| format!("private data directory: {error}"))?;
        token_station_private_fs::verify_private_dir(&data_dir)
            .map_err(|error| format!("private data directory verification: {error}"))?;
        token_station_private_fs::write_atomic_private(&permission_probe, b"permission-probe")
            .map_err(|error| format!("private file: {error}"))?;
        token_station_private_fs::verify_private_file(&permission_probe)
            .map_err(|error| format!("private file verification: {error}"))?;

        let mut draft = template(&data_dir, &missing_plugins);
        draft["upstreams"]["installed_self_test"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:1/v1",
            "models": [{
                "model": "installed-self-test",
                "tool": true,
                "vision": false,
                "json_schema": true,
                "tool_state": "declared",
                "vision_state": "unsupported",
                "json_schema_state": "declared",
                "context_window": 8192
            }]
        });
        draft["router"]["pools"]["installed_self_test"] = json!([{
            "upstream": "installed_self_test",
            "model": "installed-self-test"
        }]);
        draft["router"]["default_pool"] = json!("installed_self_test");
        draft["routing"]["direct_target"] = json!({
            "upstream": "installed_self_test",
            "model": "installed-self-test"
        });
        let config: ClientConfig = serde_json::from_value(draft)
            .map_err(|error| format!("self-test configuration: {error}"))?;
        config
            .validate()
            .map_err(|error| format!("self-test configuration: {error}"))?;

        let registry = PluginRegistry::for_config(&config)
            .map_err(|error| format!("builtin plugin registry: {error}"))?;
        let mut plugins = Vec::with_capacity(BUNDLED_PLUGIN_IDS.len());
        for id in BUNDLED_PLUGIN_IDS {
            let package = registry
                .package(id)
                .ok_or_else(|| format!("builtin plugin `{id}` is missing"))?;
            if !matches!(
                package.source,
                token_station_cli::plugins::PackageSource::Builtin { .. }
            ) {
                return Err(format!(
                    "plugin `{id}` did not come from the signed builtin tier"
                ));
            }
            fn capability_names<T: serde::Serialize>(capabilities: &T) -> Vec<String> {
                serde_json::to_value(capabilities)
                    .ok()
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            }
            let (kind, protocols, agent_tools, providers, capabilities) = match &package.manifest {
                PackageManifest::Agent(agent) => (
                    "agent-adapter",
                    agent.agent_protocols.clone(),
                    agent.agent_tools.clone(),
                    Vec::new(),
                    capability_names(&agent.capabilities),
                ),
                PackageManifest::Provider(component) => (
                    "provider-component",
                    Vec::new(),
                    Vec::new(),
                    component.providers.clone(),
                    capability_names(&component.capabilities),
                ),
            };
            plugins.push(InstalledPluginSelfTest {
                id: package.manifest.name().to_owned(),
                version: package.manifest.version().to_owned(),
                kind: kind.to_owned(),
                source: "builtin",
                protocols,
                agent_tools,
                providers,
                capabilities,
                loadable: true,
            });
        }
        for dialect in ["openai-compatible", "azure-openai-v1", "anthropic"] {
            if registry.provider_binding(dialect).is_none() {
                return Err(format!("builtin provider dialect `{dialect}` is not bound"));
            }
        }

        let gateway = Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder))
            .map_err(|error| format!("gateway plugin load: {error}"))?;
        if !gateway.skipped_agents().is_empty() {
            let skipped = gateway
                .skipped_agents()
                .iter()
                .map(|(package, error)| format!("{package}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "one or more builtin agent plugins failed to load: {skipped}"
            ));
        }

        Ok(json!({
            "passed": true,
            "bundle": {
                "id": "com.tokenstation.desktop",
                "desktop_version": env!("CARGO_PKG_VERSION"),
                "core_version": upgrade::CURRENT_VERSION,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            },
            "storage": {
                "isolated": true,
                "data_directory_private": true,
                "private_file_verified": true,
                "credential_read": false
            },
            "plugins": plugins,
            "gateway": {
                "loadable": true,
                "skipped_agents": [],
                "catalog_size": gateway.catalog_size(),
                "provider_dialects": registry.provider_dialects()
            }
        }))
    })();
    std::fs::remove_dir_all(&scratch).ok();
    result
}

/// Runs the final desktop executable's read-only, credential-free artifact
/// self-test and writes a private JSON report for release automation.
///
/// # Errors
///
/// Returns a closed failure reason when storage protection, builtin plugin
/// identity, WASM loading, or the complete gateway composition fails. The
/// report is still written with `passed: false` whenever the output path itself
/// is writable.
pub fn run_installed_self_test(output: &std::path::Path) -> Result<(), String> {
    let collected = collect_installed_self_test();
    let report = match &collected {
        Ok(report) => report.clone(),
        Err(error) => json!({
            "passed": false,
            "bundle": {
                "id": "com.tokenstation.desktop",
                "desktop_version": env!("CARGO_PKG_VERSION"),
                "core_version": upgrade::CURRENT_VERSION,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            },
            "error": error
        }),
    };
    let mut rendered = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("self-test report serialization: {error}"))?;
    rendered.push(b'\n');
    token_station_private_fs::write_atomic_private(output, &rendered)
        .map_err(|error| format!("self-test report `{}`: {error}", output.display()))?;
    collected.map(|_| ())
}

fn seed_builtin_pricing(draft: &mut Value) -> Result<bool, String> {
    let current: PriceTable = draft
        .get("pricing")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("定价表配置不合法：{error}"))?
        .unwrap_or_default();
    if current.version != 0 || !current.models.is_empty() {
        return Ok(false);
    }
    draft["pricing"] =
        serde_json::to_value(PriceTable::builtin()).map_err(|error| error.to_string())?;
    Ok(true)
}

/// Upgrade a CLI-era single-Chat inbound config into the desktop three-inbound
/// draft and anchor relative runtime paths to the config directory. Change only
/// the in-memory draft until the user saves.
fn prepare_desktop_draft(mut draft: Value, config_dir: &std::path::Path) -> Value {
    let agents = draft["plugins"]["agents"].as_array();
    let legacy_alias = agents.is_none_or(Vec::is_empty)
        && draft["plugins"]["agent"].as_str() == Some("agent-openai");
    let legacy_desktop_list = agents.is_some_and(|agents| {
        agents.len() == 1
            && agents[0].as_str() == Some("agent-openai")
            && draft["plugins"]["agent"].is_null()
    });
    if legacy_alias || legacy_desktop_list {
        if let Some(plugins) = draft["plugins"].as_object_mut() {
            plugins.remove("agent");
        }
        draft["plugins"]["agents"] = json!(desktop_agents());
    }

    // Ensure agents contains every built-in connector adapter. Legacy configs
    // captured a fixed snapshot, so newer adapters such as agent-gemini would be
    // missing and their Agents rejected because the gateway did not load them.
    // Add missing adapters while preserving existing order and custom entries.
    if !draft["plugins"]["agents"].is_array() {
        draft["plugins"]["agents"] = json!([]);
    }
    if let Some(agents) = draft["plugins"]["agents"].as_array_mut() {
        for adapter in desktop_agents() {
            if !agents.iter().any(|value| value.as_str() == Some(adapter)) {
                agents.push(json!(adapter));
            }
        }
    }

    fn anchor(path: &mut Value, config_dir: &std::path::Path) {
        let Some(raw) = path.as_str() else {
            return;
        };
        let value = PathBuf::from(raw);
        if value.is_relative() {
            *path = json!(config_dir.join(value));
        }
    }
    anchor(&mut draft["plugins"]["dir"], config_dir);
    anchor(&mut draft["data"]["dir"], config_dir);

    // Capability migration: promote tool_state and json_schema_state from
    // unknown to declared. Early add_provider versions wrote unknown, while
    // tool routing fails closed and rejected every tool-using Agent. Catalog
    // entries are OpenAI-compatible chat providers whose contract includes tools
    // and structured output. Keep vision unchanged because it varies by model,
    // and never overwrite explicit unsupported or verified states.
    if let Some(upstreams) = draft["upstreams"].as_object_mut() {
        for upstream in upstreams.values_mut() {
            if upstream["access_tier"].as_str() == Some("free") {
                continue;
            }
            let Some(models) = upstream["models"].as_array_mut() else {
                continue;
            };
            for model in models.iter_mut() {
                if model["tool_state"] == json!("unknown") {
                    model["tool_state"] = json!("declared");
                    model["tool"] = json!(true);
                }
                if model["json_schema_state"] == json!("unknown") {
                    model["json_schema_state"] = json!("declared");
                    model["json_schema"] = json!(true);
                }
            }
            apply_builtin_model_limits_to_upstream(upstream);
        }
    }

    // Remove dangling references after provider or model deletion and migration.
    // Agent-specific routes and profiles created before validation may retain
    // missing targets. Reset them to unselected so the UI does not show stale
    // choices. Run only when upstreams is a valid object to avoid treating every
    // target in a damaged config as dangling.
    if draft["upstreams"].is_object() {
        let valid: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = draft
            ["upstreams"]
            .as_object()
            .map(|upstreams| {
                upstreams
                    .iter()
                    .map(|(name, upstream)| {
                        let models = upstream["models"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|model| model["model"].as_str().map(str::to_owned))
                            .collect();
                        (name.clone(), models)
                    })
                    .collect()
            })
            .unwrap_or_default();

        fn prune_dangling_tier(
            tier: &mut Value,
            valid: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        ) {
            let Some(upstream) = tier["upstream"].as_str().map(str::to_owned) else {
                return;
            };
            match valid.get(&upstream) {
                // The provider was removed; reset the entire tier to unselected.
                None => {
                    tier["upstream"] = Value::Null;
                    tier["model"] = Value::Null;
                }
                // The provider remains but the model was removed; keep the provider for reselection.
                Some(models) => {
                    if tier["model"]
                        .as_str()
                        .is_some_and(|model| !models.contains(model))
                    {
                        tier["model"] = Value::Null;
                    }
                }
            }
        }

        fn prune_dangling_direct_target(
            target: &mut Value,
            valid: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
            preserve_explicit_empty: bool,
        ) {
            if !target.is_object() {
                return;
            }
            prune_dangling_tier(target, valid);
            if target["upstream"].is_null() && !preserve_explicit_empty {
                *target = Value::Null;
            }
        }

        if let Some(routing) = draft.get_mut("routing").and_then(Value::as_object_mut) {
            if let Some(target) = routing.get_mut("direct_target") {
                prune_dangling_direct_target(target, &valid, false);
            }
        }
        if let Some(router) = draft.get_mut("router").and_then(Value::as_object_mut) {
            if let Some(accounts) = router
                .get_mut("quota_accounts")
                .and_then(Value::as_array_mut)
            {
                accounts.retain(|account| {
                    let Some(upstream) = account["upstream"].as_str() else {
                        return false;
                    };
                    let Some(model) = account["model"].as_str() else {
                        return false;
                    };
                    valid
                        .get(upstream)
                        .is_some_and(|models| models.contains(model))
                });
            }
        }

        if let Some(agent_routes) = draft.get_mut("agent_routes").and_then(Value::as_object_mut) {
            for route in agent_routes.values_mut() {
                if let Some(target) = route
                    .as_object_mut()
                    .and_then(|route| route.get_mut("direct_target"))
                {
                    // Keep an explicit empty object as a tombstone. `null`
                    // means "inherit Home" for an Agent, which would silently
                    // route traffic to a different target after deletion.
                    prune_dangling_direct_target(target, &valid, true);
                }
                for slot in ["high", "mid", "low"] {
                    if route["custom_route"][slot].is_object() {
                        prune_dangling_tier(&mut route["custom_route"][slot], &valid);
                    }
                }
            }
        }
        if let Some(profiles) = draft.get_mut("profiles").and_then(Value::as_object_mut) {
            for profile in profiles.values_mut() {
                for slot in ["high", "mid", "low"] {
                    if profile[slot].is_object() {
                        prune_dangling_tier(&mut profile[slot], &valid);
                    }
                }
            }
        }
    }

    draft
}

/// Validate a structurally sound configuration after removing only the known
/// stale provider/model references that the desktop editor can repair safely.
///
/// The returned configuration is an audit projection only: Direct modes whose
/// target was just removed are temporarily treated as Tiered so every unrelated
/// semantic constraint can still run. The projection is never shown, saved, or
/// served. The real draft keeps Direct selected with an empty target and remains
/// dirty until the operator chooses a replacement.
fn validate_dangling_route_recovery(config: &ClientConfig) -> Result<(), String> {
    let mut audit = config.clone();
    let valid: BTreeMap<String, BTreeSet<String>> = audit
        .upstreams
        .iter()
        .map(|(name, upstream)| {
            let models = upstream
                .models
                .iter()
                .map(|capability| capability.model.clone())
                .collect();
            (name.clone(), models)
        })
        .collect();
    let target_exists = |target: &UpstreamModel| {
        valid
            .get(target.upstream.as_str())
            .is_some_and(|models| models.contains(&target.model))
    };

    let mut repaired_references = 0_usize;
    let mut home_target_removed = false;
    if let Some(routing) = audit.routing.as_mut() {
        if routing
            .direct_target
            .as_ref()
            .is_some_and(|target| !target_exists(target))
        {
            routing.direct_target = None;
            home_target_removed = true;
            repaired_references += 1;
        }
    }

    let quota_before = audit.router.quota_accounts.len();
    audit
        .router
        .quota_accounts
        .retain(|target| target_exists(target));
    repaired_references += quota_before - audit.router.quota_accounts.len();

    let mut agent_targets_removed = BTreeSet::new();
    for (agent_id, route) in &mut audit.agent_routes {
        if route
            .direct_target
            .as_ref()
            .is_some_and(|target| !target_exists(target))
        {
            route.direct_target = None;
            agent_targets_removed.insert(agent_id.clone());
            repaired_references += 1;
        }
    }

    if repaired_references == 0 {
        return Err("配置不包含可安全清理的悬空 Direct 或额度路由引用".to_owned());
    }

    // A target that was present but became stale is an editable selection, not
    // permission to accept a Direct configuration that was already missing its
    // required target. Only suppress the derivative missing-target error when
    // this exact recovery removed the target.
    if home_target_removed && audit.effective_home_routing_mode() == HostRoutingMode::Direct {
        let routing = audit
            .routing
            .as_mut()
            .expect("only top-level routing can encode host Direct mode");
        routing.mode = HostRoutingMode::Tiered;
    }
    let home_mode = audit.effective_home_routing_mode();
    let home_target = audit.effective_home_direct_target().cloned();
    for (agent_id, route) in &mut audit.agent_routes {
        let effective_mode = route.routing_mode.unwrap_or(home_mode);
        let has_effective_target = route
            .direct_target
            .as_ref()
            .or(home_target.as_ref())
            .is_some();
        if effective_mode == HostRoutingMode::Direct
            && !has_effective_target
            && (home_target_removed || agent_targets_removed.contains(agent_id))
        {
            route.routing_mode = Some(HostRoutingMode::Tiered);
        }
    }

    audit.validate()
}

/// Existing configs must pass complete CLI loading, defaulting, and structural
/// validation. On failure, return a safe display template with a read-only gate
/// that blocks saving and starting to protect the damaged file.
#[cfg(test)]
fn load_draft(config_path: &std::path::Path, root: &std::path::Path) -> (Value, Option<String>) {
    let (draft, _saved, error) = load_draft_state(
        config_path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );
    (draft, error)
}

fn load_draft_state(
    config_path: &std::path::Path,
    data_dir: &std::path::Path,
    plugins_dir: &std::path::Path,
) -> (Value, Value, Option<String>) {
    if !config_path.exists() {
        let draft = template(data_dir, plugins_dir);
        return (draft.clone(), draft, None);
    }
    match ClientConfig::load(config_path) {
        Ok(config) => {
            let saved = serde_json::to_value(config).expect("ClientConfig always serializes");
            let config_dir = config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            (
                prepare_desktop_draft(saved.clone(), config_dir),
                saved,
                None,
            )
        }
        Err(error) => {
            let recovered = std::fs::read_to_string(config_path)
                .map_err(|read_error| read_error.to_string())
                .and_then(|source| {
                    ClientConfig::parse_with_load_migrations(&source)
                        .map_err(|parse_error| parse_error.to_string())
                })
                .and_then(|config| {
                    validate_dangling_route_recovery(&config)?;
                    Ok(config)
                });
            if let Ok(config) = recovered {
                let saved = serde_json::to_value(config).expect("ClientConfig always serializes");
                let config_dir = config_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                return (
                    prepare_desktop_draft(saved.clone(), config_dir),
                    saved,
                    None,
                );
            }

            let draft = template(data_dir, plugins_dir);
            (
                draft.clone(),
                draft,
                Some(format!(
                    "现有配置无法读取，已进入只读保护；请先修复或移走 {}：{error}",
                    config_path.display()
                )),
            )
        }
    }
}

/// Full ten-dimensional heuristic weights that make content-driven tiers effective even for short difficult prompts.
fn default_weights() -> Value {
    json!({
        "tokens_per_point": 100,
        "per_tool": 20,
        "json_schema": 10,
        "image": 15,
        "per_code_block": 8,
        "per_extra_turn": 3,
        "per_reasoning_marker": 10,
        "per_technical_term": 8,
        "per_code_keyword": 6,
        "per_math_term": 12,
        "per_creative_term": 6,
        "per_multi_step_point": 3,
        "per_question": 2,
        "system_format": 10,
        "per_simple_indicator": 8
    })
}

// ---- Frontend view types ----------------------------------------------------

#[derive(Serialize)]
struct ProviderView {
    name: String,
    brand_id: Option<&'static str>,
    provider: String,
    base_url: String,
    models: Vec<String>,
    model_capabilities: Vec<ModelCapabilityView>,
    catalog_revision: u64,
    catalog: Vec<model_catalog::CatalogModelView>,
    has_auth: bool,
    credential_source: String,
    credential_reference: String,
    provider_call: String,
    south_v1_available: bool,
    south_v1_unavailable_reason: Option<&'static str>,
    south_header_auth_v1_available: bool,
    south_header_auth_v1_unavailable_reason: Option<&'static str>,
    /// This upstream runs on the local machine; `local_only` routing keeps to it.
    local: bool,
    access_tier: String,
    /// The declared quota plan (window + limit + unit) used for local estimation
    /// in quota-first mode, if the user set one. `None` ⇒ non-windowed / metered.
    quota_plan: Option<QuotaPlanView>,
}

const PROVIDER_BRANDS_BY_BASE_URL: &[(&str, &str)] = &[
    ("https://api.openai.com/v1", "openai"),
    ("https://api.anthropic.com/v1", "anthropic"),
    (
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "gemini",
    ),
    ("https://api.deepseek.com/v1", "deepseek"),
    ("https://open.bigmodel.cn/api/paas/v4", "glm_cn"),
    ("https://api.z.ai/api/paas/v4", "glm"),
    ("https://api.z.ai/api/coding/paas/v4", "glm_coding"),
    ("https://api.moonshot.cn/v1", "kimi"),
    ("https://api.moonshot.ai/v1", "kimi_global"),
    ("https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen"),
    (
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        "qwen_singapore",
    ),
    (
        "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
        "qwen_us",
    ),
    ("https://api.minimaxi.com/v1", "minimax_cn"),
    ("https://api.minimax.io/v1", "minimax_global"),
    ("https://api.groq.com/openai/v1", "groq"),
    ("https://integrate.api.nvidia.com/v1", "nvidia_nim"),
    ("https://api.mistral.ai/v1", "mistral"),
    ("https://api.x.ai/v1", "xai"),
    ("https://ark.cn-beijing.volces.com/api/v3", "volcengine_ark"),
    (
        "https://ark.cn-beijing.volces.com/api/coding/v3",
        "volcengine_ark_coding",
    ),
    (
        "https://ark.ap-southeast.bytepluses.com/api/v3",
        "byteplus_ark",
    ),
    (
        "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
        "byteplus_ark_coding",
    ),
    ("https://api.siliconflow.cn/v1", "siliconflow"),
    ("https://api.siliconflow.com/v1", "siliconflow_global"),
    ("https://api.together.ai/v1", "together"),
    ("https://api.fireworks.ai/inference/v1", "fireworks"),
    ("https://api.deepinfra.com/v1/openai", "deepinfra"),
    ("https://api.cerebras.ai/v1", "cerebras"),
    ("https://api.sambanova.ai/v1", "sambanova"),
    ("https://api.cohere.ai/compatibility/v1", "cohere"),
    ("https://models.github.ai/inference", "github_models"),
    ("https://qianfan.baidubce.com/v2", "qianfan"),
    ("https://api.hunyuan.cloud.tencent.com/v1", "hunyuan"),
    ("https://api.stepfun.com/v1", "stepfun"),
    ("https://api.stepfun.com/step_plan/v1", "stepfun_plan"),
    ("https://api.xiaomimimo.com/v1", "xiaomi_mimo"),
    ("https://api.perplexity.ai", "perplexity"),
    ("https://api.novita.ai/v3/openai", "novita"),
    ("https://api.hyperbolic.xyz/v1", "hyperbolic"),
    ("https://api.studio.nebius.com/v1", "nebius"),
    ("http://127.0.0.1:11434/v1", "ollama"),
    ("http://localhost:11434/v1", "ollama"),
    ("https://openrouter.ai/api/v1", "openrouter"),
];

fn provider_brand_id(
    upstream_name: &str,
    base_url: &str,
    access_tier: &str,
) -> Option<&'static str> {
    let normalized = base_url.trim().trim_end_matches('/');
    if access_tier == "free" {
        if let Some(preset) = free_provider_catalog::presets().iter().find(|preset| {
            preset.upstream_name == upstream_name
                && preset.base_url.trim_end_matches('/') == normalized
        }) {
            return Some(preset.id);
        }
    }
    PROVIDER_BRANDS_BY_BASE_URL
        .iter()
        .find_map(|(known_url, brand_id)| (*known_url == normalized).then_some(*brand_id))
}

/// The official OpenAI-compatible South component, by its builtin manifest
/// name or by the package directory the staged copy lives in.
fn is_official_openai_compatible_package(package: &str) -> bool {
    matches!(
        package,
        "provider-openai-compatible" | "provider-openai-compatible-v2"
    )
}

/// The engine an upstream runs on when its config names none: the full South
/// transport. Mirrors `ProviderCallEngine::default()` in the CLI crate.
const DEFAULT_PROVIDER_CALL: &str = "south_v1_buffered_streaming_header_auth";

fn south_v1_unavailable_reason(
    draft: &Value,
    upstream: &Value,
    package_verified: bool,
) -> Option<&'static str> {
    if upstream
        .get("api_dialect")
        .and_then(Value::as_str)
        .is_some_and(|dialect| dialect != "translated")
    {
        return Some("api_dialect");
    }
    if !package_verified
        || upstream["provider"].as_str() != Some("openai-compatible")
        || draft["plugins"]["providers"]["openai-compatible"]
            .as_str()
            .is_some_and(|package| !is_official_openai_compatible_package(package))
    {
        return Some("provider_package");
    }

    if draft["egress"]["mode"]
        .as_str()
        .is_some_and(|mode| mode != "direct")
    {
        return Some("egress");
    }
    if upstream["auth"]["store"].as_bool() != Some(true) && !upstream["auth"]["env"].is_string() {
        return Some("auth");
    }
    None
}

fn south_header_auth_v1_unavailable_reason(
    draft: &Value,
    upstream: &Value,
    package_verified: bool,
) -> Option<&'static str> {
    let provider = upstream["provider"].as_str().unwrap_or_default();
    if upstream
        .get("api_dialect")
        .and_then(Value::as_str)
        .is_some_and(|dialect| dialect != "translated")
    {
        return Some("api_dialect");
    }
    if !package_verified
        || !matches!(provider, "openai-compatible" | "azure-openai-v1")
        || draft["plugins"]["providers"][provider]
            .as_str()
            .is_some_and(|package| !is_official_openai_compatible_package(package))
    {
        return Some("provider_package");
    }

    if draft["egress"]["mode"]
        .as_str()
        .is_some_and(|mode| mode != "direct")
    {
        return Some("egress");
    }
    if upstream["auth"]["store"].as_bool() != Some(true) && !upstream["auth"]["env"].is_string() {
        return Some("auth");
    }
    None
}

/// A provider's declared quota plan, flattened to its primary reset window for
/// the UI (the common case is one window, e.g. a token allowance per 5 hours).
#[derive(Serialize)]
struct QuotaPlanView {
    len_ms: u64,
    limit: u64,
    unit: String,
    rate_limit_per_min: Option<u64>,
}

#[derive(Serialize)]
struct ModelCapabilityView {
    model: String,
    tool: CapabilityState,
    vision: CapabilityState,
    json_schema: CapabilityState,
    context_window: u32,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens_source: Option<String>,
}

#[derive(Serialize)]
struct ProviderEndpointPreview {
    chat: String,
    responses: String,
    messages: String,
    loopback: bool,
}

#[derive(Serialize)]
struct ProviderRemovalPreview {
    name: String,
    references: Vec<String>,
    can_remove: bool,
}

#[derive(Serialize)]
struct ProviderTestStage {
    layer: String,
    status: StageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timing_kind: Option<&'static str>,
}

#[derive(Serialize)]
struct ProviderTestResult {
    model: String,
    stages: Vec<ProviderTestStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
struct ModelTestMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ModelTestReply {
    content: String,
    first_token_ms: u64,
    latency_ms: u64,
}

#[derive(Clone, Serialize)]
struct ModelTestStreamEvent {
    request_id: String,
    delta: String,
    first_token_ms: Option<u64>,
}

#[derive(Default)]
struct ModelTestStreamRegistry {
    active: BTreeMap<String, CancelToken>,
    pending_cancellations: BTreeSet<String>,
}

impl ModelTestStreamRegistry {
    fn register(&mut self, request_id: &str, token: CancelToken) -> Result<(), String> {
        if self.pending_cancellations.remove(request_id) {
            return Err("Model test cancelled".to_owned());
        }
        if self.active.contains_key(request_id) {
            return Err("This model test request is already active".to_owned());
        }
        if self.active.len() >= MODEL_TEST_MAX_ACTIVE_STREAMS {
            return Err("Too many model test requests are active".to_owned());
        }
        self.active.insert(request_id.to_owned(), token);
        Ok(())
    }

    fn cancel(&mut self, request_id: String) {
        if let Some(token) = self.active.get(&request_id).cloned() {
            token.cancel();
            return;
        }
        if self.pending_cancellations.len() >= MODEL_TEST_MAX_PENDING_CANCELLATIONS {
            let eviction = self.pending_cancellations.iter().next().cloned();
            if let Some(eviction) = eviction {
                self.pending_cancellations.remove(&eviction);
            }
        }
        self.pending_cancellations.insert(request_id);
    }
}

#[derive(Clone, Default)]
struct ModelTestStreamState(Arc<Mutex<ModelTestStreamRegistry>>);

struct ModelTestStreamRegistration {
    registry: Arc<Mutex<ModelTestStreamRegistry>>,
    request_id: String,
}

impl Drop for ModelTestStreamRegistration {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        registry.active.remove(&self.request_id);
        registry.pending_cancellations.remove(&self.request_id);
    }
}

#[derive(Clone, Serialize)]
struct TierView {
    upstream: Option<String>,
    model: Option<String>,
}

#[derive(Serialize)]
struct AgentRouteView {
    mode: String,
    tiers: std::collections::BTreeMap<String, TierView>,
    config_error: Option<String>,
    profile: Option<String>,
    /// Effective routing philosophy for this Agent: its own override if set,
    /// otherwise the Home default. Drives the per-Agent top-bar toggle and which
    /// page body (three-tier vs quota-first) the Agent renders.
    routing_mode: String,
    direct_target: Option<DirectTargetView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct DirectTargetView {
    upstream: String,
    model: Option<String>,
}

/// One account (upstream + model) in the quota-first rotation, in priority
/// order. Shared across every scope in quota mode — the pool of allowances to
/// drain is global; only the per-Agent *mode* is independent.
#[derive(Serialize)]
struct QuotaAccountView {
    upstream: String,
    model: String,
}

#[derive(serde::Deserialize)]
struct QuotaAccountArg {
    upstream: String,
    model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServePhase {
    Stopped,
    Starting,
    Stopping,
    Running,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AppRuntime {
    Stopped,
    Running,
}

#[derive(Clone, Debug, Serialize)]
struct ServeView {
    phase: ServePhase,
    app_runtime: AppRuntime,
    listener_reachable: bool,
    agent_connected: bool,
    running_revision: Option<u64>,
    instance_id: Option<String>,
    listen: String,
    virtual_key: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct StateView {
    providers: Vec<ProviderView>,
    deleted_providers: Vec<String>,
    provider_recovery_error: Option<String>,
    tiers: std::collections::BTreeMap<String, TierView>,
    agent_routes: std::collections::BTreeMap<String, AgentRouteView>,
    profiles: Vec<String>,
    /// Per-tier user keyword libraries that provide direct routing control through router.rules keywords_any.
    keywords: std::collections::BTreeMap<String, Vec<String>>,
    /// Local-only routing uses providers marked local and keeps requests on the machine.
    local_only: bool,
    /// Whether local_only can use cloud fallback when no local target is available; false is strict local routing.
    allow_cloud_fallback: bool,
    /// Routing mode: direct, tiered intelligent routing, or quota_first.
    routing_mode: String,
    direct_target: Option<DirectTargetView>,
    /// Globally shared quota-first rotation accounts, provider plus model, in priority order.
    quota_accounts: Vec<QuotaAccountView>,
    serve: ServeView,
    draft_revision: u64,
    saved_revision: u64,
    config_dirty: bool,
    /// Whether the draft materializes as a valid config and can be saved or started.
    config_error: Option<String>,
    /// Settings read model: switches, egress policy, and read-only environment information.
    settings: SettingsView,
}

#[derive(Serialize)]
struct ModelPriceImportResultView {
    state: StateView,
    imported: usize,
    existing: usize,
    missing_model_ids: Vec<String>,
    price_version: u32,
}

/// Settings view for proxy switches, egress policy, and read-only environment information.
#[derive(Serialize)]
struct SettingsView {
    listen: String,
    auth: bool,
    metrics: bool,
    data_dir: String,
    plugins_dir: String,
    agent: String,
    /// Backward-compatible alias for the desktop package version.
    version: String,
    desktop_version: String,
    core_version: String,
    egress_mode: String,
    egress_proxy_url: String,
    egress_no_proxy: Vec<String>,
    egress_auth_username: String,
    egress_auth_slot: String,
}

// ---- Subpage view types for full-capability subpages (#5) ------------------

/// Serializable mirror of stats::Aggregate for one tier or group.
#[derive(Serialize)]
struct AggView {
    requests: u64,
    errors: u64,
    p50_latency_ms: u64,
    p95_latency_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    cost_micros: Option<i64>,
    priced_requests: u64,
    unpriced_requests: u64,
}

impl AggView {
    fn zero() -> Self {
        Self {
            requests: 0,
            errors: 0,
            p50_latency_ms: 0,
            p95_latency_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_micros: None,
            priced_requests: 0,
            unpriced_requests: 0,
        }
    }
    fn from(a: &stats::Aggregate) -> Self {
        Self {
            requests: a.requests,
            errors: a.errors,
            p50_latency_ms: a.p50_latency_ms,
            p95_latency_ms: a.p95_latency_ms,
            input_tokens: a.input_tokens,
            output_tokens: a.output_tokens,
            cache_read_tokens: a.cache_read_tokens,
            cache_write_tokens: a.cache_write_tokens,
            reasoning_tokens: a.reasoning_tokens,
            cost_micros: a.cost_micros,
            priced_requests: a.priced_requests,
            unpriced_requests: a.unpriced_requests,
        }
    }
}

/// Usage-page view. empty=true means the metrics database does not exist because
/// serve never ran with metrics enabled, so the frontend shows guidance instead of an empty table.
#[derive(Serialize)]
struct StatsView {
    total: AggView,
    groups: Vec<(String, AggView)>,
    by: Option<String>,
    empty: bool,
}

#[derive(Serialize)]
struct ReceiptPageView {
    items: Vec<ReceiptView>,
    plaintext_by_request_id: BTreeMap<String, PlaintextExchange>,
    plaintext_errors_by_request_id: BTreeMap<String, String>,
    total: u64,
    page: usize,
    page_size: usize,
}

/// Four-layer routing-table view in order: rules, hint routes, heuristic bands,
/// then default-pool fallback. It reads only the draft and performs no API calls.
#[derive(Serialize)]
struct RouterTableView {
    default_pool: String,
    assumed_context_window: u64,
    threshold: Option<u32>,
    rules: Vec<Value>,
    hint_routes: Vec<Value>,
    bands: Vec<BandView>,
    pools: Vec<PoolView>,
}

/// One heuristic band: scores at or above at_least select its pool and current provider-model pair.
#[derive(Serialize)]
struct BandView {
    at_least: u32,
    pool: String,
    upstream: Option<String>,
    model: Option<String>,
}

#[derive(Serialize)]
struct PoolView {
    pool: String,
    upstream: Option<String>,
    model: Option<String>,
}

/// Plugins-page view whose monospace listing reuses core render_list(), shared with CLI `plugin list`.
#[derive(Serialize)]
struct PluginsView {
    dir: String,
    agent: String,
    dialects: Vec<String>,
    listing: String,
}

#[derive(Debug, Serialize)]
struct SettingsCommandError {
    field: String,
    reason_code: String,
    message: String,
}

fn settings_error(
    field: impl Into<String>,
    reason_code: impl Into<String>,
    message: impl Into<String>,
) -> SettingsCommandError {
    SettingsCommandError {
        field: field.into(),
        reason_code: reason_code.into(),
        message: message.into(),
    }
}

fn south_approved_dialects(registry: &PluginRegistry) -> BTreeSet<String> {
    // Every bound dialect. Provenance is no longer judged here: a component is
    // admitted at Gateway startup — source trust, compatibility handshake, Wasm
    // gates, identity — and a package that fails any of them fails startup
    // rather than quietly losing South. Re-deciding it in a settings view could
    // only produce a second, weaker opinion.
    registry
        .provider_dialects()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn south_approved_dialects_for_draft(draft: &Value) -> BTreeSet<String> {
    let Ok(plugins) = serde_json::from_value::<PluginsConfig>(draft["plugins"].clone()) else {
        return BTreeSet::new();
    };
    let Some(data_dir) = draft["data"]["dir"].as_str().map(PathBuf::from) else {
        return BTreeSet::new();
    };
    let Ok(receipts) = Receipts::load(&data_dir) else {
        return BTreeSet::new();
    };
    PluginRegistry::discover(&plugins, &receipts)
        .map(|registry| south_approved_dialects(&registry))
        .unwrap_or_default()
}

fn record_changed_upstream_epochs(
    before: &Value,
    after: &Value,
    epochs: &mut BTreeMap<String, u64>,
) {
    let before = before.get("upstreams").and_then(Value::as_object);
    let after = after.get("upstreams").and_then(Value::as_object);
    let names = before
        .into_iter()
        .flat_map(|values| values.keys())
        .chain(after.into_iter().flat_map(|values| values.keys()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let previous = before.and_then(|values| values.get(&name));
        let current = after.and_then(|values| values.get(&name));
        if previous != current {
            let epoch = epochs.entry(name).or_default();
            *epoch = epoch.saturating_add(1).max(1);
        }
    }
}

// ---- helpers ------------------------------------------------------------------

impl AppInner {
    #[cfg(test)]
    fn new(config_path: PathBuf, draft: Value, load_error: Option<String>) -> Self {
        Self::new_with_saved(config_path, draft.clone(), draft, load_error)
    }

    fn new_with_saved(
        config_path: PathBuf,
        mut draft: Value,
        saved: Value,
        mut load_error: Option<String>,
    ) -> Self {
        let mut config_state = ConfigState::load(&config_path, &saved).unwrap_or_else(|error| {
            load_error
                .get_or_insert_with(|| format!("配置版本状态无法持久化，已进入只读保护：{error}"));
            ConfigState::read_only(&config_path, &saved)
        });
        if load_error.is_none() {
            if let Err(error) = config_state.observe_draft(&draft) {
                load_error = Some(format!("配置版本状态无法持久化，已进入只读保护：{error}"));
                draft = config_state.draft().clone();
            }
        }
        let south_approved_dialects = south_approved_dialects_for_draft(&draft);
        Self {
            config_path,
            draft,
            load_error,
            config_state,
            agent_route_drafts: BTreeMap::new(),
            server: ServerLifecycle::stopped(),
            pending_free_providers: BTreeSet::new(),
            pending_provider_keys: BTreeMap::new(),
            pending_provider_discoveries: BTreeSet::new(),
            south_approved_dialects,
            upstream_epochs: BTreeMap::new(),
            discovery_generations: BTreeMap::new(),
        }
    }

    fn observe_draft(&mut self) -> Result<(), String> {
        let previous = self.config_state.draft().clone();
        let draft = self.draft.clone();
        if let Err(error) = self.config_state.observe_draft(&draft) {
            self.draft = self.config_state.draft().clone();
            return Err(error);
        }
        record_changed_upstream_epochs(&previous, &draft, &mut self.upstream_epochs);
        Ok(())
    }

    fn bump_upstream_epoch(&mut self, name: &str) {
        let epoch = self.upstream_epochs.entry(name.to_owned()).or_default();
        *epoch = epoch.saturating_add(1).max(1);
    }

    /// Build a candidate config under the lock and replace the authoritative draft
    /// only after materialization and revision recording succeed. The callback may
    /// edit only the config draft; update other AppInner state after commit.
    fn edit_validated_draft<T>(
        &mut self,
        edit: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.ensure_editable()?;
        let previous = self.draft.clone();
        let result = match edit(self) {
            Ok(result) => result,
            Err(error) => {
                self.draft = previous.clone();
                return Err(error);
            }
        };
        if let Err(error) = self.materialize() {
            self.draft = previous.clone();
            return Err(error);
        }
        let candidate = self.draft.clone();
        self.draft = previous.clone();
        let mut candidate_state = self.config_state.clone();
        candidate_state.observe_draft(&candidate)?;
        self.draft = candidate;
        self.config_state = candidate_state;
        record_changed_upstream_epochs(&previous, &self.draft, &mut self.upstream_epochs);
        Ok(result)
    }

    fn save_draft(&mut self) -> Result<u64, String> {
        self.ensure_editable()?;
        let config = self.materialize()?;
        let draft = self.draft.clone();
        let revision = self.config_state.prepare_save(&draft)?;
        let data_dir = self.data_dir();
        let mut applied_keys: Vec<(String, Option<String>)> = Vec::new();
        for (upstream, value) in &self.pending_provider_keys {
            let previous = secrets::store_get(&data_dir, upstream, "provider_api_key").ok();
            if let Err(error) = secrets::store_set(&data_dir, upstream, "provider_api_key", value) {
                for (applied, old_value) in applied_keys.iter().rev() {
                    restore_provider_key(
                        &data_dir,
                        applied,
                        "provider_api_key",
                        old_value.as_deref(),
                    )
                    .ok();
                }
                return Err(error);
            }
            applied_keys.push((upstream.clone(), previous));
        }
        if let Err(error) = config.save(&self.config_path) {
            let mut rollback_errors = Vec::new();
            for (upstream, previous) in applied_keys.iter().rev() {
                if let Err(rollback_error) = restore_provider_key(
                    &data_dir,
                    upstream,
                    "provider_api_key",
                    previous.as_deref(),
                ) {
                    rollback_errors.push(rollback_error);
                }
            }
            let mut message = format!("写配置失败: {error}");
            if !rollback_errors.is_empty() {
                message.push_str(&format!(
                    "；同时恢复 Provider 凭据失败：{}",
                    rollback_errors.join("；")
                ));
            }
            return Err(message);
        }
        if let Err(error) = self.config_state.finish_save(&draft) {
            // The config committed atomically. The pending journal will be
            // promoted on next startup, so do not misreport a trailing state-write
            // failure as a failed config save.
            eprintln!("configuration saved but revision finalization failed: {error}");
        }
        if let Err(error) =
            SqliteStore::backfill_unknown_costs(&data_dir.join("metrics.sqlite"), &config.pricing)
        {
            // The configuration is already atomically committed. Keep save
            // semantics truthful and retry this idempotent backfill on the next
            // save or startup instead of reporting a rollback that did not occur.
            eprintln!("configuration saved but historical cost backfill failed: {error}");
        }
        for upstream in self.pending_provider_keys.keys() {
            if let Err(error) = provider_tombstones::discard(&self.data_dir(), upstream) {
                eprintln!(
                    "configuration saved but free Provider tombstone cleanup failed: {error}"
                );
            }
        }
        self.pending_provider_keys.clear();
        Ok(revision)
    }

    fn ensure_editable(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn upstreams(&self) -> Vec<ProviderView> {
        let Some(map) = self.draft["upstreams"].as_object() else {
            return vec![];
        };
        map.iter()
            .map(|(name, up)| {
                let model_values = up["models"].as_array().cloned().unwrap_or_default();
                let models = model_values
                    .iter()
                    .filter_map(|model| model["model"].as_str().map(str::to_owned))
                    .collect();
                let configured_capabilities: Vec<ModelCapability> = model_values
                    .into_iter()
                    .filter_map(|model| serde_json::from_value::<ModelCapability>(model).ok())
                    .collect();
                let model_capabilities = configured_capabilities
                    .iter()
                    .map(|capability| {
                        let tool = capability.tool_state();
                        let vision = capability.vision_state();
                        let json_schema = capability.json_schema_state();
                        ModelCapabilityView {
                            model: capability.model.clone(),
                            tool,
                            vision,
                            json_schema,
                            context_window: capability.context_window,
                            max_output_tokens: capability.max_output_tokens,
                            context_window_source: model_limit_source(
                                capability,
                                CONTEXT_WINDOW_SOURCE_KEY,
                            ),
                            max_output_tokens_source: model_limit_source(
                                capability,
                                MAX_OUTPUT_TOKENS_SOURCE_KEY,
                            ),
                        }
                    })
                    .collect();
                let base_url = up["base_url"].as_str().unwrap_or_default().to_string();
                let access_tier = up
                    .get("access_tier")
                    .and_then(Value::as_str)
                    .unwrap_or("paid");
                let (catalog_revision, catalog) = model_catalog::catalog_for_provider(
                    &self.data_dir(),
                    name,
                    &base_url,
                    &configured_capabilities,
                );
                let package_verified = self
                    .south_approved_dialects
                    .contains(up["provider"].as_str().unwrap_or_default());
                let south_v1_unavailable_reason =
                    south_v1_unavailable_reason(&self.draft, up, package_verified);
                let south_header_auth_v1_unavailable_reason =
                    south_header_auth_v1_unavailable_reason(&self.draft, up, package_verified);
                ProviderView {
                    name: name.clone(),
                    brand_id: provider_brand_id(name, &base_url, access_tier),
                    provider: up["provider"].as_str().unwrap_or_default().to_string(),
                    base_url,
                    models,
                    model_capabilities,
                    catalog_revision,
                    catalog,
                    has_auth: up.get("auth").map(|a| !a.is_null()).unwrap_or(false),
                    credential_source: if up["auth"]["store"].as_bool() == Some(true) {
                        "store"
                    } else if up["auth"]["env"].is_string() {
                        "env"
                    } else if up["auth"]["file"].is_string() {
                        "file"
                    } else {
                        "none"
                    }
                    .to_owned(),
                    credential_reference: up["auth"]["env"]
                        .as_str()
                        .or_else(|| up["auth"]["file"].as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    provider_call: up
                        .get("provider_call")
                        .and_then(Value::as_str)
                        .unwrap_or(DEFAULT_PROVIDER_CALL)
                        .to_owned(),
                    south_v1_available: south_v1_unavailable_reason.is_none(),
                    south_v1_unavailable_reason,
                    south_header_auth_v1_available: south_header_auth_v1_unavailable_reason
                        .is_none(),
                    south_header_auth_v1_unavailable_reason,
                    local: up.get("local").and_then(Value::as_bool).unwrap_or(false),
                    access_tier: access_tier.to_owned(),
                    quota_plan: up["quota_plan"]["windows"][0].as_object().map(|window| {
                        QuotaPlanView {
                            len_ms: window["len_ms"].as_u64().unwrap_or(0),
                            limit: window["limit"].as_u64().unwrap_or(0),
                            unit: up["quota_plan"]["unit"]
                                .as_str()
                                .unwrap_or("tokens")
                                .to_owned(),
                            rate_limit_per_min: up["quota_plan"]["rate_limit_per_min"].as_u64(),
                        }
                    }),
                }
            })
            .collect()
    }

    fn tier(&self, pool: &str) -> TierView {
        let member = self.draft["router"]["pools"][pool]
            .as_array()
            .and_then(|arr| arr.first());
        match member {
            Some(m) => TierView {
                upstream: m["upstream"].as_str().map(str::to_string),
                model: m["model"].as_str().map(str::to_string),
            },
            None => TierView {
                upstream: None,
                model: None,
            },
        }
    }

    fn home_tiers(&self) -> std::collections::BTreeMap<String, TierView> {
        let mut tiers = std::collections::BTreeMap::new();
        tiers.insert("high".to_string(), self.tier(TIER_HIGH));
        tiers.insert("mid".to_string(), self.tier(TIER_MID));
        tiers.insert("low".to_string(), self.tier(TIER_LOW));
        tiers
    }

    /// Whether a tier pool has members. Keywords require this or their rule would target an empty pool.
    fn pool_present(&self, pool: &str) -> bool {
        self.draft["router"]["pools"][pool]
            .as_array()
            .is_some_and(|members| !members.is_empty())
    }

    /// Read the current keywords_any list for a keyword-rule ID.
    fn rule_keywords(&self, rule_id: &str) -> Vec<String> {
        self.draft["router"]["rules"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|rule| rule["id"].as_str() == Some(rule_id))
            .and_then(|rule| rule["when"]["keywords_any"].as_array())
            .map(|words| {
                words
                    .iter()
                    .filter_map(|w| w.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keyword libraries for high, mid, and low tiers, exposed to the frontend.
    fn home_keywords(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        TIER_ORDER
            .iter()
            .map(|(slot, _pool, rule_id)| ((*slot).to_string(), self.rule_keywords(rule_id)))
            .collect()
    }

    /// Current mapping from tier slots to keyword lists, used as the pre-write snapshot.
    fn keyword_map(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        self.home_keywords()
    }

    /// Rewrite router.rules from the supplied tier-keyword map in high-to-low
    /// priority order. Emit rules only for tiers with keywords and configured
    /// pools. Preserve operator-authored non-keyword rules afterward. Empty or
    /// unconfigured tiers emit no rule, avoiding references to missing pools.
    fn apply_keyword_map(&mut self, map: &std::collections::BTreeMap<String, Vec<String>>) {
        let mut rules: Vec<Value> = Vec::new();
        for (slot, pool, rule_id) in TIER_ORDER {
            let words = map.get(slot).cloned().unwrap_or_default();
            if words.is_empty() || !self.pool_present(pool) {
                continue;
            }
            rules.push(json!({
                "id": rule_id,
                "when": { "keywords_any": words },
                "route_to": pool,
            }));
        }
        // Preserve existing rules not managed by this module and append them afterward.
        let managed = [KW_RULE_HIGH, KW_RULE_MID, KW_RULE_LOW];
        if let Some(existing) = self.draft["router"]["rules"].as_array() {
            for rule in existing {
                let is_managed = rule["id"].as_str().is_some_and(|id| managed.contains(&id));
                if !is_managed {
                    rules.push(rule.clone());
                }
            }
        }
        self.draft["router"]["rules"] = Value::Array(rules);
    }

    /// Normalize keywords by trimming whitespace. Deduplicate case-insensitively
    /// to match core keywords_any behavior while preserving original case for display.
    fn add_tier_keyword(&mut self, slot: &str, keyword: &str) -> Result<(), String> {
        let (pool, _rule_id) = tier_pool_and_rule(slot)?;
        if !self.pool_present(pool) {
            return Err("请先为该档配置供应商和模型,再添加关键词".to_string());
        }
        let word = keyword.trim();
        if word.is_empty() {
            return Err("关键词不能为空".to_string());
        }
        if word.chars().count() > 64 {
            return Err("单个关键词过长(最多 64 字)".to_string());
        }
        let mut map = self.keyword_map();
        let list = map.entry(slot.to_string()).or_default();
        if list
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(word))
        {
            return Err(format!("关键词「{word}」已在该档"));
        }
        if list.len() >= 100 {
            return Err("单档关键词过多(最多 100 个)".to_string());
        }
        list.push(word.to_string());
        self.apply_keyword_map(&map);
        Ok(())
    }

    fn remove_tier_keyword(&mut self, slot: &str, keyword: &str) -> Result<(), String> {
        tier_pool_and_rule(slot)?;
        let mut map = self.keyword_map();
        if let Some(list) = map.get_mut(slot) {
            list.retain(|existing| !existing.eq_ignore_ascii_case(keyword.trim()));
        }
        self.apply_keyword_map(&map);
        Ok(())
    }

    /// Remove a pool's keyword rule when clearing it so route_to cannot reference an empty pool.
    fn drop_keyword_rule_for_pool(&mut self, pool: &str) {
        let Some(rule_id) = TIER_ORDER
            .iter()
            .find(|(_, p, _)| *p == pool)
            .map(|(_, _, id)| *id)
        else {
            return;
        };
        if let Some(rules) = self.draft["router"]["rules"].as_array_mut() {
            rules.retain(|rule| rule["id"].as_str() != Some(rule_id));
        }
    }

    fn agent_route_mode(&self, agent_id: &str) -> &str {
        self.draft["agent_routes"][agent_id]["mode"]
            .as_str()
            .unwrap_or("inherit")
    }

    fn agent_route_view_mode(&self, agent_id: &str) -> &str {
        if self.agent_route_drafts.contains_key(agent_id) {
            "custom"
        } else {
            self.agent_route_mode(agent_id)
        }
    }

    fn agent_tier(&self, agent_id: &str, slot: &str) -> TierView {
        if let Some(tier) = self
            .agent_route_drafts
            .get(agent_id)
            .and_then(|tiers| tiers.get(slot))
        {
            return tier.clone();
        }
        let target = match self.agent_route_mode(agent_id) {
            "custom" => &self.draft["agent_routes"][agent_id]["custom_route"][slot],
            "profile" => {
                let name = self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .unwrap_or_default();
                &self.draft["profiles"][name][slot]
            }
            _ => return self.tier(pool_key(slot).expect("known UI tier slot")),
        };
        TierView {
            upstream: target["upstream"].as_str().map(str::to_string),
            model: target["model"].as_str().map(str::to_string),
        }
    }

    fn agent_profile(&self, agent_id: &str) -> Option<String> {
        (!self.agent_route_drafts.contains_key(agent_id)
            && self.agent_route_mode(agent_id) == "profile")
            .then(|| {
                self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .map(str::to_string)
            })
            .flatten()
    }

    fn profile_names(&self) -> Vec<String> {
        self.draft["profiles"]
            .as_object()
            .map(|profiles| profiles.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn agent_routes_view(&self) -> std::collections::BTreeMap<String, AgentRouteView> {
        let home_mode = self.home_routing_mode();
        supported_agent_ids()
            .into_iter()
            .map(|agent_id| {
                let mode = self.agent_route_view_mode(&agent_id).to_string();
                let routing_mode = self.draft["agent_routes"][&agent_id]["routing_mode"]
                    .as_str()
                    .unwrap_or(home_mode)
                    .to_string();
                let tiers = ["high", "mid", "low"]
                    .into_iter()
                    .map(|slot| (slot.to_string(), self.agent_tier(&agent_id, slot)))
                    .collect();
                let direct_target = self.agent_direct_target_view(&agent_id);
                let direct_target_incomplete = direct_target
                    .as_ref()
                    .and_then(|target| target.model.as_ref())
                    .is_none();
                let config_error = if routing_mode == "direct" && direct_target_incomplete {
                    Some(format!("Agent `{agent_id}` 的单独路由缺少供应商和模型"))
                } else if mode == "custom" || mode == "profile" {
                    ["high", "mid", "low"].into_iter().find_map(|slot| {
                        let tier = self.agent_tier(&agent_id, slot);
                        (tier.upstream.is_none() || tier.model.is_none())
                            .then(|| format!("Agent `{agent_id}` 的 {slot} 档缺少供应商和模型"))
                    })
                } else {
                    None
                };
                (
                    agent_id.clone(),
                    AgentRouteView {
                        mode,
                        tiers,
                        config_error,
                        profile: self.agent_profile(&agent_id),
                        routing_mode,
                        direct_target,
                    },
                )
            })
            .collect()
    }

    fn direct_target_view(value: &Value) -> Option<DirectTargetView> {
        Some(DirectTargetView {
            upstream: value["upstream"].as_str()?.to_owned(),
            model: value["model"].as_str().map(str::to_owned),
        })
    }

    fn home_direct_target_view(&self) -> Option<DirectTargetView> {
        Self::direct_target_view(&self.draft["routing"]["direct_target"])
    }

    fn home_routing_mode(&self) -> &str {
        self.draft["routing"]["mode"]
            .as_str()
            .or_else(|| self.draft["router"]["routing_mode"].as_str())
            .unwrap_or("tiered")
    }

    fn agent_direct_target_view(&self, agent_id: &str) -> Option<DirectTargetView> {
        let target = &self.draft["agent_routes"][agent_id]["direct_target"];
        if target.is_object() {
            Self::direct_target_view(target)
        } else {
            self.home_direct_target_view()
        }
    }

    fn quota_accounts_view(&self) -> Vec<QuotaAccountView> {
        self.draft["router"]["quota_accounts"]
            .as_array()
            .map(|accounts| {
                accounts
                    .iter()
                    .filter_map(|account| {
                        Some(QuotaAccountView {
                            upstream: account["upstream"].as_str()?.to_owned(),
                            model: account["model"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Rebuild tier-pool references, heuristic bands, and default from configured
    /// tiers. Include only tiers with a selected upstream-model pair.
    fn rebuild_routing(&mut self) {
        // Collect configured tiers from high to low.
        let present: Vec<(&str, u32)> =
            [(TIER_HIGH, CUT_HIGH), (TIER_MID, CUT_MID), (TIER_LOW, 0u32)]
                .into_iter()
                .filter(|(pool, _)| {
                    self.draft["router"]["pools"][*pool]
                        .as_array()
                        .map(|a| !a.is_empty())
                        .unwrap_or(false)
                })
                .collect();

        if present.is_empty() {
            // With no configured tiers, clear heuristic and default so saving reports the empty-pool error.
            self.draft["router"]["heuristic"] = Value::Null;
            self.draft["router"]["default_pool"] = json!("");
            return;
        }

        // `present` is high to low; force the last band's at_least to zero so no request is missed.
        let last = present.len() - 1;
        let bands: Vec<Value> = present
            .iter()
            .enumerate()
            .map(|(i, (pool, cut))| {
                let at_least = if i == last { 0 } else { *cut };
                json!({ "at_least": at_least, "pool": pool })
            })
            .collect();

        let highest = present.first().unwrap().0;
        let lowest = present.last().unwrap().0;

        self.draft["router"]["heuristic"] = json!({
            "weights": default_weights(),
            "threshold": CUT_MID,
            "above": highest,
            "below": lowest,
            "bands": bands
        });
        self.draft["router"]["default_pool"] = json!(lowest);
    }

    /// Materialize and validate the draft as ClientConfig, returning a human-readable error on failure.
    fn materialize(&self) -> Result<ClientConfig, String> {
        if let Some(upstreams) = self.draft["upstreams"].as_object() {
            for (name, provider) in upstreams {
                if provider["access_tier"].as_str() == Some("free") {
                    free_provider_catalog::validate_stored_provider(name, provider)?;
                }
            }
        }
        serde_json::from_value::<ClientConfig>(self.draft.clone())
            .map_err(|e| format!("配置结构不合法: {e}"))
    }

    fn config_error(&self) -> Option<String> {
        self.load_error.clone().or_else(|| self.materialize().err())
    }

    fn serve_view(&self) -> ServeView {
        match &self.server {
            ServerLifecycle::Stopped { .. } => ServeView {
                phase: ServePhase::Stopped,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: self.draft["server"]["listen"]
                    .as_str()
                    .unwrap_or("127.0.0.1:8787")
                    .to_string(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Starting { listen, .. } => ServeView {
                phase: ServePhase::Starting,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Applying { old, .. } => {
                let alive = old.is_task_alive();
                let reachable = alive && old.listener_reachable();
                ServeView {
                    phase: ServePhase::Starting,
                    app_runtime: if alive {
                        AppRuntime::Running
                    } else {
                        AppRuntime::Stopped
                    },
                    listener_reachable: reachable,
                    agent_connected: false,
                    running_revision: alive.then(|| old.running_revision()),
                    instance_id: alive.then(|| old.instance_id().to_owned()),
                    listen: old.listen().to_owned(),
                    virtual_key: old.virtual_key().map(str::to_string),
                    error: None,
                }
            }
            ServerLifecycle::Stopping { listen, .. } => ServeView {
                phase: ServePhase::Stopping,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Running {
                server,
                apply_error,
                ..
            } => {
                let alive = server.is_task_alive();
                let reachable = alive && server.listener_reachable();
                ServeView {
                    phase: if alive {
                        ServePhase::Running
                    } else {
                        ServePhase::Error
                    },
                    app_runtime: if alive {
                        AppRuntime::Running
                    } else {
                        AppRuntime::Stopped
                    },
                    listener_reachable: reachable,
                    agent_connected: false,
                    running_revision: alive.then(|| server.running_revision()),
                    instance_id: alive.then(|| server.instance_id().to_owned()),
                    listen: server.listen().to_string(),
                    virtual_key: server.virtual_key().map(str::to_string),
                    error: if alive {
                        apply_error.clone()
                    } else {
                        Some("serve_task_exited: 代理任务已退出".to_owned())
                    },
                }
            }
            ServerLifecycle::Failed { listen, error, .. } => ServeView {
                phase: ServePhase::Error,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: Some(error.clone()),
            },
        }
    }

    fn snapshot(&self) -> StateView {
        let (deleted_providers, provider_recovery_error) =
            match provider_tombstones::list(&self.data_dir()) {
                Ok(providers) => (providers, None),
                Err(error) => (Vec::new(), Some(error)),
            };
        StateView {
            providers: self.upstreams(),
            deleted_providers,
            provider_recovery_error,
            tiers: self.home_tiers(),
            agent_routes: self.agent_routes_view(),
            profiles: self.profile_names(),
            keywords: self.home_keywords(),
            local_only: self.draft["router"]["local_only"]
                .as_bool()
                .unwrap_or(false),
            allow_cloud_fallback: self.draft["router"]["allow_cloud_fallback"]
                .as_bool()
                .unwrap_or(false),
            routing_mode: self.home_routing_mode().to_string(),
            direct_target: self.home_direct_target_view(),
            quota_accounts: self.quota_accounts_view(),
            serve: self.serve_view(),
            draft_revision: self.config_state.draft_revision(),
            saved_revision: self.config_state.saved_revision(),
            config_dirty: self.config_state.is_dirty(),
            config_error: self.config_error(),
            settings: self.settings_view(),
        }
    }

    fn settings_view(&self) -> SettingsView {
        let d = &self.draft;
        SettingsView {
            listen: d["server"]["listen"]
                .as_str()
                .unwrap_or("127.0.0.1:8787")
                .to_string(),
            auth: d["server"]["auth"].as_bool().unwrap_or(true),
            metrics: d["data"]["metrics"].as_bool().unwrap_or(true),
            data_dir: d["data"]["dir"].as_str().unwrap_or_default().to_string(),
            plugins_dir: d["plugins"]["dir"].as_str().unwrap_or_default().to_string(),
            agent: agents_display(&d["plugins"]),
            version: env!("CARGO_PKG_VERSION").to_string(),
            desktop_version: env!("CARGO_PKG_VERSION").to_string(),
            core_version: upgrade::CURRENT_VERSION.to_string(),
            egress_mode: d["egress"]["mode"].as_str().unwrap_or("direct").to_string(),
            egress_proxy_url: d["egress"]["proxy_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            egress_no_proxy: d["egress"]["no_proxy"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            egress_auth_username: d["egress"]["auth"]["username"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            egress_auth_slot: d["egress"]["auth"]["credential"]["slot"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Absolute data directory from the draft, anchoring stats, receipts, and plugins.
    fn data_dir(&self) -> PathBuf {
        PathBuf::from(self.draft["data"]["dir"].as_str().unwrap_or_default())
    }

    /// Resolve a pool's first member as provider and model for routing-table and band display.
    fn pool_member(&self, pool: &str) -> (Option<String>, Option<String>) {
        let m = self.draft["router"]["pools"][pool]
            .as_array()
            .and_then(|a| a.first());
        match m {
            Some(m) => (
                m["upstream"].as_str().map(str::to_string),
                m["model"].as_str().map(str::to_string),
            ),
            None => (None, None),
        }
    }

    fn set_tier_value(
        &mut self,
        pool: &str,
        upstream: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        match (upstream, model) {
            (Some(upstream), Some(model)) => {
                let configured = self.draft["upstreams"][&upstream]
                    .as_object()
                    .ok_or_else(|| format!("未知供应商 `{upstream}`"))?;
                let model_exists = configured["models"].as_array().is_some_and(|models| {
                    models
                        .iter()
                        .any(|entry| entry["model"].as_str() == Some(model.as_str()))
                });
                if !model_exists {
                    return Err(format!("供应商 `{upstream}` 未配置模型 `{model}`"));
                }
                self.draft["router"]["pools"][pool] =
                    json!([{ "upstream": upstream, "model": model }]);
            }
            (None, None) => {
                if let Some(pools) = self.draft["router"]["pools"].as_object_mut() {
                    pools.remove(pool);
                }
                // Remove the keyword rule with the pool so it cannot target an empty pool and break saving.
                self.drop_keyword_rule_for_pool(pool);
            }
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_string()),
        }
        self.rebuild_routing();
        Ok(())
    }

    fn validate_route_target(&self, upstream: &str, model: &str) -> Result<(), String> {
        let configured = self.draft["upstreams"][upstream]
            .as_object()
            .ok_or_else(|| format!("未知供应商 `{upstream}`"))?;
        let model_exists = configured["models"].as_array().is_some_and(|models| {
            models
                .iter()
                .any(|entry| entry["model"].as_str() == Some(model))
        });
        model_exists
            .then_some(())
            .ok_or_else(|| format!("供应商 `{upstream}` 未配置模型 `{model}`"))
    }

    fn begin_agent_route_draft(&mut self, agent_id: &str) {
        if self.agent_route_drafts.contains_key(agent_id) {
            return;
        }
        let stored = &self.draft["agent_routes"][agent_id]["custom_route"];
        let tiers = ["high", "mid", "low"]
            .into_iter()
            .map(|slot| {
                let target = &stored[slot];
                let tier = if stored.is_object() {
                    TierView {
                        upstream: target["upstream"].as_str().map(str::to_owned),
                        model: target["model"].as_str().map(str::to_owned),
                    }
                } else {
                    self.tier(pool_key(slot).expect("known UI tier slot"))
                };
                (slot.to_owned(), tier)
            })
            .collect();
        self.agent_route_drafts.insert(agent_id.to_owned(), tiers);
    }

    fn set_agent_route_draft_tier(
        &mut self,
        agent_id: &str,
        slot: &str,
        upstream: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        pool_key(slot)?;
        let tier = match (upstream, model) {
            (Some(upstream), Some(model)) => {
                self.validate_route_target(&upstream, &model)?;
                TierView {
                    upstream: Some(upstream),
                    model: Some(model),
                }
            }
            (None, None) => TierView {
                upstream: None,
                model: None,
            },
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_owned()),
        };
        self.begin_agent_route_draft(agent_id);
        self.agent_route_drafts
            .get_mut(agent_id)
            .expect("route draft was just initialized")
            .insert(slot.to_owned(), tier);
        Ok(())
    }

    fn complete_agent_route_draft(
        agent_id: &str,
        tiers: &BTreeMap<String, TierView>,
    ) -> Result<Value, String> {
        let mut route = serde_json::Map::new();
        for slot in ["high", "mid", "low"] {
            let tier = tiers.get(slot);
            let upstream = tier.and_then(|tier| tier.upstream.as_deref());
            let model = tier.and_then(|tier| tier.model.as_deref());
            let target = match (upstream, model) {
                (Some(upstream), Some(model)) => {
                    json!({ "upstream": upstream, "model": model })
                }
                (None, None) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少供应商和模型"));
                }
                (None, Some(_)) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少供应商"));
                }
                (Some(_), None) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少模型"));
                }
            };
            route.insert(slot.to_owned(), target);
        }
        Ok(Value::Object(route))
    }

    fn promote_agent_route_drafts(&mut self) -> Result<(), String> {
        let routes: BTreeMap<String, Value> = self
            .agent_route_drafts
            .iter()
            .map(|(agent_id, tiers)| {
                Self::complete_agent_route_draft(agent_id, tiers)
                    .map(|route| (agent_id.clone(), route))
            })
            .collect::<Result<_, _>>()?;
        if routes.is_empty() {
            return Ok(());
        }
        self.edit_validated_draft(|inner| {
            if !inner.draft["agent_routes"].is_object() {
                inner.draft["agent_routes"] = json!({});
            }
            for (agent_id, route) in &routes {
                if !inner.draft["agent_routes"][agent_id].is_object() {
                    inner.draft["agent_routes"][agent_id] = json!({});
                }
                if let Some(agent_route) = inner.draft["agent_routes"][agent_id].as_object_mut() {
                    agent_route.remove("profile");
                }
                inner.draft["agent_routes"][agent_id]["mode"] = json!("custom");
                inner.draft["agent_routes"][agent_id]["custom_route"] = route.clone();
            }
            Ok(())
        })
    }

    fn set_agent_inherit_value(&mut self, agent_id: &str) {
        if !self.draft["agent_routes"].is_object() {
            self.draft["agent_routes"] = json!({});
        }
        if !self.draft["agent_routes"][agent_id].is_object() {
            self.draft["agent_routes"][agent_id] = json!({});
        }
        if let Some(route) = self.draft["agent_routes"][agent_id].as_object_mut() {
            route.remove("profile");
            route.remove("routing_mode");
            route.remove("direct_target");
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!("inherit");
    }

    fn save_home_route_as_profile_value(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err("策略组名称无效".to_string());
        }
        let mut tiers = serde_json::Map::new();
        for (slot, pool) in [("high", TIER_HIGH), ("mid", TIER_MID), ("low", TIER_LOW)] {
            let tier = self.tier(pool);
            let (Some(upstream), Some(model)) = (tier.upstream, tier.model) else {
                return Err(format!("{slot} 档尚未配置，无法另存为策略组"));
            };
            self.validate_route_target(&upstream, &model)?;
            tiers.insert(
                slot.to_string(),
                json!({ "upstream": upstream, "model": model }),
            );
        }
        if !self.draft["profiles"].is_object() {
            self.draft["profiles"] = json!({});
        }
        self.draft["profiles"][name] = Value::Object(tiers);
        Ok(())
    }

    fn mount_agent_profile_value(&mut self, agent_id: &str, profile: &str) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        if !self.draft["profiles"][profile].is_object() {
            return Err(format!("策略组 `{profile}` 不存在"));
        }
        if !self.draft["agent_routes"][agent_id].is_object() {
            self.draft["agent_routes"][agent_id] = json!({});
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!("profile");
        self.draft["agent_routes"][agent_id]["profile"] = json!(profile);
        Ok(())
    }

    fn delete_profile_value(&mut self, name: &str) -> Result<(), String> {
        let mounted: Vec<_> = supported_agent_ids()
            .into_iter()
            .filter(|agent_id| self.agent_profile(agent_id).as_deref() == Some(name))
            .collect();
        if !mounted.is_empty() {
            return Err(format!("策略组 `{name}` 仍被挂载：{}", mounted.join(", ")));
        }
        let profiles = self.draft["profiles"]
            .as_object_mut()
            .ok_or_else(|| format!("策略组 `{name}` 不存在"))?;
        if profiles.remove(name).is_none() {
            return Err(format!("策略组 `{name}` 不存在"));
        }
        Ok(())
    }
}

fn pool_key(slot: &str) -> Result<&'static str, String> {
    match slot {
        "high" => Ok(TIER_HIGH),
        "mid" => Ok(TIER_MID),
        "low" => Ok(TIER_LOW),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

fn ensure_known_agent_id(agent_id: &str) -> Result<(), String> {
    supported_agent_ids()
        .iter()
        .any(|candidate| candidate == agent_id)
        .then_some(())
        .ok_or_else(|| format!("未知 Agent `{agent_id}`"))
}

fn supported_agent_ids() -> Vec<String> {
    AgentRegistry::builtin()
        .expect("built-in Agent Registry must be valid")
        .descriptors()
        .iter()
        .filter(|descriptor| {
            descriptor.admission == AdmissionStatus::Supported
                // Cursor has a verified protocol route and a dedicated SQLite
                // configurator, but intentionally has no generic local Connector.
                || descriptor.agent_id == "cursor"
        })
        .map(|descriptor| descriptor.agent_id.clone())
        .collect()
}

// ---- Tauri commands --------------------------------------------------------

fn dock_icon_bytes(theme: &str) -> Result<&'static [u8], String> {
    match theme {
        "light" => Ok(include_bytes!("../icons/icon-light.png")),
        "dark" => Ok(include_bytes!("../icons/icon-dark.png")),
        _ => Err(format!("unsupported Dock icon theme: {theme}")),
    }
}

#[tauri::command]
async fn set_dock_theme_icon(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let icon_bytes = dock_icon_bytes(&theme)?;

    #[cfg(target_os = "macos")]
    {
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        app.run_on_main_thread(move || {
            let _ = result_tx.send(apply_macos_dock_icon(icon_bytes));
        })
        .map_err(|error| format!("failed to schedule Dock icon update: {error}"))?;

        let apply_result = tauri::async_runtime::spawn_blocking(move || {
            result_rx.recv_timeout(Duration::from_secs(2))
        })
        .await
        .map_err(|error| format!("failed to join Dock icon update: {error}"))?
        .map_err(|error| format!("timed out waiting for Dock icon update: {error}"))?;
        apply_result?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (app, icon_bytes);

    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_macos_dock_icon(icon_bytes: &'static [u8]) -> Result<(), String> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApp, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| "Dock icon update did not run on the AppKit main thread".to_string())?;
    let data = NSData::with_bytes(icon_bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "failed to decode the embedded Dock icon".to_string())?;
    if !image.isValid() {
        return Err("decoded Dock icon is not a valid AppKit image".to_string());
    }
    let application = NSApp(main_thread);

    // AppKit requires application icon updates on the main thread.
    unsafe { application.setApplicationIconImage(Some(&image)) };
    let applied_image = application
        .applicationIconImage()
        .ok_or_else(|| "AppKit did not retain the Dock icon".to_string())?;
    if !applied_image.isValid() {
        return Err("AppKit did not apply the requested Dock icon".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod dock_icon_tests {
    use super::dock_icon_bytes;

    #[test]
    fn accepts_supported_dock_icon_themes() {
        for theme in ["light", "dark"] {
            assert!(dock_icon_bytes(theme).is_ok());
        }
    }

    #[test]
    fn rejects_unknown_dock_icon_theme() {
        assert!(dock_icon_bytes("system").is_err());
    }

    #[test]
    fn embeds_png_dock_icons() {
        for theme in ["light", "dark"] {
            assert!(dock_icon_bytes(theme)
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }

    #[test]
    fn embeds_distinct_light_and_dark_dock_icons() {
        assert_ne!(
            dock_icon_bytes("light").unwrap(),
            dock_icon_bytes("dark").unwrap()
        );
    }
}

#[tauri::command]
fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
}

#[tauri::command]
fn get_runtime_state(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
) -> ServeView {
    // Agent config inspection is file I/O and therefore intentionally outside
    // the App lock. Revalidate the immutable instance identity afterwards so
    // a concurrent publish can never combine old Agent facts with a new
    // running_revision/instance_id.
    for _ in 0..3 {
        let Ok(runtime) = runtime_from_app(state.inner()) else {
            let view = state.0.lock().unwrap().serve_view();
            desktop_shell::update_proxy_menu(&app);
            return view;
        };
        let identity = runtime.instance_id().to_owned();
        let agent_connected = agents.any_connected_to(&runtime).unwrap_or(false);
        let mut view = state.0.lock().unwrap().serve_view();
        if view.instance_id.as_deref() == Some(identity.as_str()) {
            view.agent_connected = agent_connected;
            desktop_shell::update_proxy_menu(&app);
            return view;
        }
    }
    // Continuous handoffs are rare; if all snapshots raced, return a truthful
    // current runtime view with the conservative independent Agent fact.
    let view = state.0.lock().unwrap().serve_view();
    desktop_shell::update_proxy_menu(&app);
    view
}

/// Preview the provider URL selected by each inbound protocol before saving.
#[tauri::command]
fn preview_provider_endpoints(base_url: String) -> Result<ProviderEndpointPreview, String> {
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    Ok(ProviderEndpointPreview {
        chat: endpoint.resolve(ProviderApi::ChatCompletions),
        responses: endpoint.resolve(ProviderApi::Responses),
        messages: endpoint.resolve(ProviderApi::Messages),
        loopback: endpoint.is_loopback(),
    })
}

fn ensure_credential_transport(
    endpoint: &ProviderEndpoint,
    egress: &EgressConfig,
) -> Result<(), String> {
    if !endpoint.uses_https() && !endpoint.is_loopback() {
        return Err("Remote Provider endpoints must use HTTPS".to_owned());
    }
    if !endpoint.uses_https() && !egress.bypasses_proxy(&endpoint.as_str())? {
        return Err(
            "Plaintext loopback Provider endpoints must bypass the configured proxy".to_owned(),
        );
    }
    Ok(())
}

fn draft_egress_config(draft: &Value) -> Result<EgressConfig, String> {
    match draft.get("egress").filter(|value| !value.is_null()) {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("出站配置不合法：{error}")),
        None => Ok(EgressConfig::default()),
    }
}

fn ensure_generic_provider_mutation_allowed(inner: &AppInner, name: &str) -> Result<(), String> {
    if inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("access_tier"))
        .and_then(Value::as_str)
        == Some("free")
    {
        return Err(format!(
            "免费供应商 `{name}` 由内置目录管理，不能通过通用 Provider 接口修改"
        ));
    }
    Ok(())
}

fn normalize_provider_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > model_catalog::MAX_MODELS_PER_PROVIDER {
        return Err(format!(
            "A Provider may configure at most {} models",
            model_catalog::MAX_MODELS_PER_PROVIDER
        ));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if model.len() > model_catalog::MAX_MODEL_ID_BYTES || model.chars().any(char::is_control) {
            return Err(format!(
                "Model IDs must be 1-{} bytes and contain no control characters",
                model_catalog::MAX_MODEL_ID_BYTES
            ));
        }
        if seen.insert(model.to_owned()) {
            normalized.push(model.to_owned());
        }
    }
    if normalized.is_empty() {
        return Err("请至少填一个模型".to_owned());
    }
    Ok(normalized)
}

fn provider_auth_value(source: &str, reference: Option<&str>) -> Result<Option<Value>, String> {
    let reference = reference.map(str::trim).filter(|value| !value.is_empty());
    match source {
        "none" => Ok(None),
        "store" => Ok(Some(json!({ "slot": "provider_api_key", "store": true }))),
        "env" => {
            let name = reference.ok_or("环境变量凭据需要填写变量名")?;
            if name.len() > 128
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || name.as_bytes().first().is_some_and(u8::is_ascii_digit)
            {
                return Err("环境变量名只能包含字母、数字和下划线，且不能以数字开头".to_owned());
            }
            Ok(Some(json!({ "slot": "provider_api_key", "env": name })))
        }
        "file" => {
            let path = std::path::Path::new(reference.ok_or("文件凭据需要填写绝对路径")?);
            if !path.is_absolute() {
                return Err("文件凭据必须使用绝对路径".to_owned());
            }
            Ok(Some(json!({
                "slot": "provider_api_key",
                "file": path.to_string_lossy()
            })))
        }
        other => Err(format!("未知凭据来源 `{other}`")),
    }
}

fn is_free_provider_value(provider: &Value) -> bool {
    provider["access_tier"].as_str() == Some("free")
}

fn restore_provider_key(
    data_dir: &std::path::Path,
    upstream: &str,
    slot: &str,
    previous: Option<&str>,
) -> Result<(), String> {
    match previous {
        Some(value) => secrets::store_set(data_dir, upstream, slot, value),
        None => secrets::store_remove(data_dir, upstream, slot),
    }
}

#[tauri::command]
fn list_free_provider_presets() -> Vec<free_provider_catalog::FreeProviderPreset> {
    free_provider_catalog::presets().to_vec()
}

#[tauri::command]
async fn add_free_provider(
    state: State<'_, AppStateManaged>,
    preset_id: String,
    selected_models: Vec<String>,
    api_key: String,
    guard_confirmed: bool,
) -> Result<StateView, String> {
    let preset = free_provider_catalog::find(preset_id.trim())
        .ok_or_else(|| format!("未知免费供应商 `{}`", preset_id.trim()))?;
    let api_key = api_key.trim().to_owned();
    if api_key.is_empty() {
        return Err("请填写 API Key".to_owned());
    }
    if preset.overage_policy == free_provider_catalog::OveragePolicy::UserMustEnableGuard
        && !guard_confirmed
    {
        return Err("请先确认已在供应商控制台启用免费额度保护".to_owned());
    }

    let allowed: std::collections::BTreeMap<&str, &free_provider_catalog::FreeModelPreset> = preset
        .models
        .iter()
        .map(|model| (model.id, model))
        .collect();
    let mut selected = Vec::new();
    for raw in selected_models {
        let model = raw.trim();
        if model.is_empty() || selected.iter().any(|existing: &String| existing == model) {
            continue;
        }
        if !allowed.contains_key(model) {
            return Err(format!("模型 `{model}` 不在该供应商的免费目录中"));
        }
        selected.push(model.to_owned());
    }
    if selected.is_empty() {
        return Err("请至少选择一个免费模型".to_owned());
    }

    let (egress, egress_secrets, expected_revision, expected_tombstone) = {
        let mut inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        if inner.draft["upstreams"].get(preset.upstream_name).is_some() {
            return Err(format!(
                "免费供应商 `{}` 已存在，请先在主页管理该实例",
                preset.upstream_name
            ));
        }
        let tombstone = provider_tombstones::get(&inner.data_dir(), preset.upstream_name)?;
        if tombstone
            .as_ref()
            .is_some_and(|provider| !is_free_provider_value(provider))
        {
            return Err(format!(
                "Provider 回收站中已有同名普通供应商 `{}`，请先恢复或彻底处理该实例",
                preset.upstream_name
            ));
        }
        let egress = draft_egress_config(&inner.draft)?;
        if inner.pending_free_providers.contains(preset.upstream_name) {
            return Err(format!(
                "免费供应商 `{}` 正在验证，请等待当前请求完成",
                preset.upstream_name
            ));
        }
        const MAX_CONCURRENT_FREE_VALIDATIONS: usize = 2;
        if inner.pending_free_providers.len() >= MAX_CONCURRENT_FREE_VALIDATIONS {
            return Err("免费供应商验证任务已达并发上限，请稍后重试".to_owned());
        }
        inner
            .pending_free_providers
            .insert(preset.upstream_name.to_owned());
        (
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
            inner.config_state.draft_revision(),
            tombstone,
        )
    };
    let _validation_guard = FreeProviderValidationGuard {
        inner: &state.0,
        upstream: preset.upstream_name.to_owned(),
    };

    let validate_preset = *preset;
    let validate_models = selected.clone();
    let validate_key = api_key.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for model in validate_models {
            free_provider_catalog::validate_chat_completion(
                &validate_preset,
                &model,
                &validate_key,
                &egress,
                &egress_secrets,
            )
            .map_err(|error| format!("模型 `{model}` 验证失败：{error}"))?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| format!("免费模型验证任务异常结束：{error}"))??;

    let model_objs: Vec<Value> = selected
        .iter()
        .filter_map(|id| allowed.get(id.as_str()).copied())
        .map(|model| {
            let capability_bool = |state: CapabilityState| {
                matches!(state, CapabilityState::Verified | CapabilityState::Declared)
            };
            json!({
                "model": model.id,
                "tool": capability_bool(model.tool),
                "vision": capability_bool(model.vision),
                "json_schema": capability_bool(model.json_schema),
                "tool_state": model.tool,
                "vision_state": model.vision,
                "json_schema_state": model.json_schema,
                "context_window": model.context_window,
            })
        })
        .collect();

    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if inner.config_state.draft_revision() != expected_revision {
        return Err("验证期间配置已变化，请按当前出站设置重新验证".to_owned());
    }
    if inner.draft["upstreams"].get(preset.upstream_name).is_some() {
        return Err(format!("免费供应商 `{}` 已存在", preset.upstream_name));
    }
    let data_dir = inner.data_dir();
    let current_tombstone = provider_tombstones::get(&data_dir, preset.upstream_name)?;
    if current_tombstone != expected_tombstone {
        return Err(format!(
            "验证期间 Provider `{}` 的回收状态已变化，请重试",
            preset.upstream_name
        ));
    }
    let previous_draft = inner.draft.clone();
    let previous_config_state = inner.config_state.clone();
    inner.draft["upstreams"][preset.upstream_name] = json!({
        "provider": "openai-compatible",
        "base_url": preset.base_url,
        "access_tier": "free",
        "auth": { "slot": "provider_api_key", "store": true },
        "models": model_objs,
    });
    if let Err(error) = inner.observe_draft() {
        inner.draft = previous_draft;
        inner.config_state = previous_config_state;
        return Err(error);
    }
    inner
        .pending_provider_keys
        .insert(preset.upstream_name.to_owned(), Zeroizing::new(api_key));
    Ok(inner.snapshot())
}

/// Infer context windows from explicit size markers in model IDs such as
/// `moonshot-v1-128k`, `glm-5.2[1m]`, and `qwen-turbo-1m`. Use the largest
/// numeric k or m marker within 8k to 10M, avoiding version numbers like `glm-4.6`.
fn context_window_from_marker(name: &str) -> Option<u64> {
    let bytes = name.as_bytes();
    let mut best: Option<u64> = None;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && (bytes[i] == b'k' || bytes[i] == b'm') {
            if let Ok(n) = name[start..i].parse::<u64>() {
                let unit = if bytes[i] == b'm' { 1_000_000 } else { 1_000 };
                let window = n.saturating_mul(unit);
                if (8_000..=10_000_000).contains(&window) {
                    best = Some(best.map_or(window, |current| current.max(window)));
                }
            }
            i += 1;
        }
    }
    best
}

/// A best-effort real context window for a freshly added model, so it is not
/// stuck at a blanket default that under-reports big-context models (the user's
/// `glm-5.2[1m]` is 1M, not 128k). An explicit size marker in the id wins;
/// otherwise a small family table; otherwise 128k. Only a starting value — the
/// operator can override it per model, and routing forwards over-context
/// requests rather than refusing them, so an imperfect guess never hard-fails.
fn known_context_window(model: &str) -> u64 {
    let name = model.to_ascii_lowercase();
    if let Some(window) = context_window_from_marker(&name) {
        return window;
    }
    if name.contains("gemini") {
        return 1_000_000;
    }
    if name.contains("claude") {
        return 200_000;
    }
    128_000
}

const CONTEXT_WINDOW_SOURCE_KEY: &str = "x-token-station-context-window-source";
const MAX_OUTPUT_TOKENS_SOURCE_KEY: &str = "x-token-station-max-output-tokens-source";
const LIMIT_SOURCE_PROVIDER: &str = "provider";
const LIMIT_SOURCE_BUILTIN_PRESET: &str = "builtin_preset";
const LIMIT_SOURCE_OPERATOR: &str = "operator";
const LIMIT_SOURCE_HEURISTIC: &str = "heuristic";

#[derive(Clone, Copy)]
struct BuiltinModelLimits {
    context_window: u32,
    max_output_tokens: u32,
}

fn builtin_model_limits(base_url: &str, model: &str) -> Option<BuiltinModelLimits> {
    let endpoint = base_url.trim().trim_end_matches('/');
    if !matches!(
        endpoint,
        "https://api.moonshot.cn/v1" | "https://api.moonshot.ai/v1"
    ) {
        return None;
    }
    match model {
        "kimi-k2.6" => Some(BuiltinModelLimits {
            context_window: 262_144,
            max_output_tokens: 262_144,
        }),
        "kimi-k3" => Some(BuiltinModelLimits {
            context_window: 1_048_576,
            max_output_tokens: 131_072,
        }),
        _ => None,
    }
}

fn model_limit_source(capability: &ModelCapability, key: &str) -> Option<String> {
    capability
        .extensions
        .get(key)
        .and_then(Value::as_str)
        .filter(|source| {
            matches!(
                *source,
                LIMIT_SOURCE_PROVIDER
                    | LIMIT_SOURCE_BUILTIN_PRESET
                    | LIMIT_SOURCE_OPERATOR
                    | LIMIT_SOURCE_HEURISTIC
            )
        })
        .map(str::to_owned)
}

fn json_limit_source<'a>(capability: &'a Value, key: &str) -> Option<&'a str> {
    capability.get(key).and_then(Value::as_str)
}

fn source_is_default(source: Option<&str>) -> bool {
    matches!(
        source,
        Some(LIMIT_SOURCE_BUILTIN_PRESET | LIMIT_SOURCE_HEURISTIC)
    )
}

fn apply_builtin_model_limits_to_upstream(upstream: &mut Value) -> bool {
    let base_url = upstream["base_url"].as_str().unwrap_or_default().to_owned();
    let Some(models) = upstream["models"].as_array_mut() else {
        return false;
    };
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(preset) = builtin_model_limits(&base_url, &model) else {
            continue;
        };
        let context = capability["context_window"].as_u64().unwrap_or_default();
        let output = capability["max_output_tokens"].as_u64().unwrap_or_default();
        let context_source =
            json_limit_source(capability, CONTEXT_WINDOW_SOURCE_KEY).map(str::to_owned);
        let output_source =
            json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY).map(str::to_owned);
        let legacy_heuristic = output == 0
            && output_source.is_none()
            && context == known_context_window(&model)
            && context_source
                .as_deref()
                .is_none_or(|source| source == LIMIT_SOURCE_HEURISTIC);

        if (context == 0 || legacy_heuristic || source_is_default(context_source.as_deref()))
            && (context != u64::from(preset.context_window)
                || context_source.as_deref() != Some(LIMIT_SOURCE_BUILTIN_PRESET))
        {
            capability["context_window"] = json!(preset.context_window);
            capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
            changed = true;
        }

        let effective_context = capability["context_window"].as_u64().unwrap_or_default();
        if (output == 0 || source_is_default(output_source.as_deref()))
            && u64::from(preset.max_output_tokens) <= effective_context
            && (output != u64::from(preset.max_output_tokens)
                || output_source.as_deref() != Some(LIMIT_SOURCE_BUILTIN_PRESET))
        {
            capability["max_output_tokens"] = json!(preset.max_output_tokens);
            capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
            changed = true;
        }
    }
    changed
}

fn provider_uses_builtin_model_limits(upstream: &Value) -> bool {
    upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|capability| {
            json_limit_source(capability, CONTEXT_WINDOW_SOURCE_KEY)
                == Some(LIMIT_SOURCE_BUILTIN_PRESET)
                || json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY)
                    == Some(LIMIT_SOURCE_BUILTIN_PRESET)
        })
}

/// Add an OpenAI-compatible upstream provider, storing its key in the system keychain when present.
#[tauri::command]
fn add_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
) -> Result<StateView, String> {
    let source = if api_key.as_deref().is_some_and(|key| !key.trim().is_empty()) {
        "store"
    } else {
        "none"
    };
    add_provider_impl(
        state,
        name,
        base_url,
        models,
        api_key,
        local,
        source,
        None,
        "openai-compatible",
        false,
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn add_provider_with_credential(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
    credential_source: String,
    credential_reference: Option<String>,
    provider_dialect: Option<String>,
) -> Result<StateView, String> {
    let provider_dialect = provider_dialect.as_deref().unwrap_or("openai-compatible");
    add_provider_impl(
        state,
        name,
        base_url,
        models,
        api_key,
        local,
        credential_source.trim(),
        credential_reference.as_deref(),
        provider_dialect,
        false,
    )
}

#[tauri::command]
fn add_managed_enterprise_route(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: String,
) -> Result<StateView, String> {
    add_provider_impl(
        state,
        name,
        base_url,
        vec!["auto".to_owned()],
        Some(api_key),
        false,
        "store",
        None,
        "openai-compatible",
        true,
    )
}

fn restore_managed_route_mutation(
    draft: &mut Value,
    previous_routing: &Option<Value>,
    previous_router: &Value,
) {
    draft["router"] = previous_router.clone();
    if let Some(previous_routing) = previous_routing {
        draft["routing"] = previous_routing.clone();
    } else if let Some(root) = draft.as_object_mut() {
        root.remove("routing");
    }
}

#[allow(clippy::too_many_arguments)]
fn add_provider_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
    credential_source: &str,
    credential_reference: Option<&str>,
    provider_dialect: &str,
    managed_route: bool,
) -> Result<StateView, String> {
    if !matches!(provider_dialect, "openai-compatible" | "azure-openai-v1") {
        return Err("Provider dialect 不受支持".to_owned());
    }
    if name.trim().is_empty() {
        return Err("供应商名不能为空".into());
    }
    let name = name.trim().to_string();
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    if provider_dialect == "azure-openai-v1" && !azure_openai_v1_base_url_is_exact(&base_url) {
        return Err("Azure OpenAI v1 的 Base URL 路径必须精确为 `/openai/v1`".to_owned());
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let egress = draft_egress_config(&inner.draft)?;
    ensure_credential_transport(&endpoint, &egress)?;
    if inner.draft["upstreams"].get(&name).is_some() {
        return Err(format!("供应商 `{name}` 已存在，请在 Provider 详情中编辑"));
    }
    let data_dir = inner.data_dir();
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err(format!(
            "Provider 回收站中已有 `{name}`，请先恢复它，再在详情中编辑"
        ));
    }

    let models = normalize_provider_model_ids(models)?;
    if managed_route && models.as_slice() != ["auto"] {
        return Err("Enterprise managed route must use only the `auto` alias".to_owned());
    }
    let model_objs: Vec<Value> = models
        .iter()
        .map(|m| {
            let mut capability = json!({
                // OpenAI Chat Completions includes tools and structured output,
                // and catalog entries are compatible chat providers, so declare
                // support by default. Leaving these unknown would fail closed and
                // reject every tool-using Agent. Ordinary models keep vision
                // unknown because support varies. A managed alias declares
                // pass-through support below because the service selects the model.
                "model": m,
                "tool": true,
                "vision": false,
                "json_schema": true,
                "tool_state": "declared",
                "vision_state": "unknown",
                "json_schema_state": "declared",
                "context_window": known_context_window(m)
            });
            if managed_route {
                capability["vision"] = json!(true);
                capability["vision_state"] = json!("declared");
                capability["supported_parameters"] = json!(["reasoning_effort"]);
            }
            capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_HEURISTIC);
            if let Some(preset) = builtin_model_limits(&base_url, m) {
                capability["context_window"] = json!(preset.context_window);
                capability["max_output_tokens"] = json!(preset.max_output_tokens);
                capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
                capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
            }
            capability
        })
        .collect();
    // A previous interrupted removal may have left only derived catalog data.
    // New Provider identity must never inherit it, even with the same name/URL.
    model_catalog::remove_provider(&data_dir, &name)?;

    let mut up = json!({
        "provider": provider_dialect,
        "base_url": base_url,
        "models": model_objs,
    });
    // Write the local key only when marked local, preserving ordinary cloud
    // provider configs in line with serde skip_serializing_if. local_only uses it
    // to keep traffic on the machine.
    if local {
        up["local"] = json!(true);
    }
    if managed_route {
        up["managed_route"] = json!(true);
    }
    // Store a key in the keychain and point auth to its slot; omit auth when no key exists, as with local Ollama.
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    let auth = provider_auth_value(credential_source, credential_reference)?;
    if credential_source == "store" && api_key.is_none() {
        return Err("本地凭据存储需要填写 API Key".to_owned());
    }
    if credential_source != "store" && api_key.is_some() {
        return Err("env/file 凭据只保存引用，不能同时提交 API Key 明文".to_owned());
    }
    if let Some(auth) = auth {
        up["auth"] = auth;
    }

    let previous_routing = if managed_route {
        inner.draft.get("routing").cloned()
    } else {
        None
    };
    let previous_router = inner.draft["router"].clone();
    inner.draft["upstreams"][&name] = up;
    if managed_route {
        if !inner.draft["routing"].is_object() {
            inner.draft["routing"] = json!({});
        }
        inner.draft["routing"]["mode"] = json!("direct");
        inner.draft["routing"]["direct_target"] = json!({ "upstream": name, "model": "auto" });
        inner.draft["router"]["routing_mode"] = json!("tiered");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(&name);
        if managed_route {
            restore_managed_route_mutation(&mut inner.draft, &previous_routing, &previous_router);
        }
        return Err(error);
    }
    if credential_source == "store" {
        let Some(key) = api_key else {
            unreachable!("store source was validated above");
        };
        if let Err(key_error) =
            secrets::store_set(&inner.data_dir(), &name, "provider_api_key", &key)
        {
            inner.draft["upstreams"]
                .as_object_mut()
                .expect("upstreams is an object")
                .remove(&name);
            if managed_route {
                restore_managed_route_mutation(
                    &mut inner.draft,
                    &previous_routing,
                    &previous_router,
                );
            }
            return match inner.observe_draft() {
                Ok(()) => Err(key_error),
                Err(rollback_error) => Err(format!(
                    "{key_error}；同时回滚新增 Provider 草稿失败：{rollback_error}"
                )),
            };
        }
    }
    Ok(inner.snapshot())
}

fn azure_openai_v1_base_url_is_exact(base_url: &str) -> bool {
    base_url
        .split_once("://")
        .and_then(|(_, authority_and_path)| {
            authority_and_path
                .find('/')
                .map(|path_start| &authority_and_path[path_start..])
        })
        == Some("/openai/v1")
}

/// Set local-only routing and cloud fallback in the home router so inherited
/// Agents follow automatically. Remove both keys when disabled to preserve
/// ordinary configs and serde's false default.
#[tauri::command]
fn set_local_routing(
    state: State<'_, AppStateManaged>,
    local_only: bool,
    allow_cloud_fallback: bool,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft["router"].clone();
    if local_only {
        inner.draft["router"]["local_only"] = json!(true);
        inner.draft["router"]["allow_cloud_fallback"] = json!(allow_cloud_fallback);
    } else if let Some(router) = inner.draft["router"].as_object_mut() {
        // Cloud fallback is meaningless when local-only is off, so clear both to avoid stale state.
        router.remove("local_only");
        router.remove("allow_cloud_fallback");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Switch between direct, tiered (difficulty-based), and quota-first
/// (allowance-draining) routing. Agent overrides are always explicit. The
/// top-level host routing contract is authoritative; keep the embedded
/// core router in quota-first only for quota mode; Direct is compiled from the
/// host target and therefore keeps the embedded router in tiered mode.
#[tauri::command]
fn set_routing_mode(
    state: State<'_, AppStateManaged>,
    mode: String,
    agent_id: Option<String>,
) -> Result<StateView, String> {
    if mode != "tiered" && mode != "direct" && mode != "quota_first" {
        return Err(format!("未知路由模式：{mode}"));
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    // Per-Agent switch: pin this one Agent's routing_mode without touching Home
    // or its siblings. Unlike Home, we write BOTH modes explicitly — for an
    // Agent a missing key means "inherit Home", so clearing it on tiered would
    // wrongly re-inherit a quota-first Home. An explicit value is what keeps the
    // Agent independent of later Home changes. ("Restore home routing" is the
    // path back to inheritance; it clears the key.)
    if let Some(agent_id) = agent_id {
        if !supported_agent_ids().contains(&agent_id) {
            return Err(format!("未知 Agent：{agent_id}"));
        }
        let previous = inner.draft["agent_routes"][&agent_id].clone();
        if !inner.draft["agent_routes"][&agent_id].is_object() {
            inner.draft["agent_routes"][&agent_id] = json!({ "mode": "inherit" });
        }
        inner.draft["agent_routes"][&agent_id]["routing_mode"] = json!(mode);
        if let Err(error) = inner.observe_draft() {
            inner.draft["agent_routes"][&agent_id] = previous;
            return Err(error);
        }
        return Ok(inner.snapshot());
    }

    let previous_routing = inner.draft.get("routing").cloned();
    let previous_router = inner.draft["router"].clone();
    if !inner.draft["routing"].is_object() {
        inner.draft["routing"] = json!({});
    }
    inner.draft["routing"]["mode"] = json!(mode);
    inner.draft["router"]["routing_mode"] = json!(if mode == "quota_first" {
        "quota_first"
    } else {
        "tiered"
    });
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous_router;
        if let Some(previous_routing) = previous_routing {
            inner.draft["routing"] = previous_routing;
        } else if let Some(draft) = inner.draft.as_object_mut() {
            draft.remove("routing");
        }
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn set_direct_route(
    state: State<'_, AppStateManaged>,
    upstream: String,
    model: String,
    agent_id: Option<String>,
) -> Result<StateView, String> {
    let upstream = upstream.trim();
    let model = model.trim();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.validate_route_target(upstream, model)?;

    if let Some(agent_id) = agent_id {
        ensure_known_agent_id(&agent_id)?;
        let previous = inner.draft["agent_routes"][&agent_id].clone();
        if !inner.draft["agent_routes"][&agent_id].is_object() {
            inner.draft["agent_routes"][&agent_id] = json!({ "mode": "inherit" });
        }
        inner.draft["agent_routes"][&agent_id]["direct_target"] =
            json!({ "upstream": upstream, "model": model });
        if let Err(error) = inner.observe_draft() {
            inner.draft["agent_routes"][&agent_id] = previous;
            return Err(error);
        }
    } else {
        let previous = inner.draft.get("routing").cloned();
        if !inner.draft["routing"].is_object() {
            let mode = inner.home_routing_mode().to_owned();
            inner.draft["routing"] = json!({ "mode": mode });
        }
        inner.draft["routing"]["direct_target"] = json!({ "upstream": upstream, "model": model });
        if let Err(error) = inner.observe_draft() {
            if let Some(previous) = previous {
                inner.draft["routing"] = previous;
            } else if let Some(draft) = inner.draft.as_object_mut() {
                draft.remove("routing");
            }
            return Err(error);
        }
    }
    Ok(inner.snapshot())
}

/// Persists the quota-first rotation list (ordered upstream+model accounts). The
/// order is the operator's priority; it lands verbatim in
/// `router.quota_accounts` and drives `Router::route_quota_first`. At least one
/// complete row is required; incomplete rows are rejected without changing the
/// draft. Complete rows are validated against the configured catalog, and exact
/// duplicates are collapsed while keeping first-seen order.
#[tauri::command]
fn set_quota_accounts(
    state: State<'_, AppStateManaged>,
    accounts: Vec<QuotaAccountArg>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    if accounts.is_empty() {
        return Err("额度优先至少需要一个完整账户，请选择供应商和模型".to_owned());
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut clean: Vec<Value> = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        let upstream = account.upstream.trim();
        let model = account.model.trim();
        if upstream.is_empty() || model.is_empty() {
            return Err(format!("额度优先账户 #{} 缺少供应商或模型", index + 1));
        }
        inner.validate_route_target(upstream, model)?;
        if seen.insert((upstream.to_owned(), model.to_owned())) {
            clean.push(json!({ "upstream": upstream, "model": model }));
        }
    }

    let previous = inner.draft["router"].clone();
    inner.draft["router"]["quota_accounts"] = Value::Array(clean);
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Declares (or clears) a provider's quota plan for local estimation: one reset
/// window (`len_ms` + `limit`), the unit it counts in, and an optional
/// requests-per-minute rate limit. A zero limit or window clears the plan (the
/// account falls back to non-windowed / authoritative-only). Written under
/// `upstreams[name].quota_plan`, feeding the account ledger's estimate.
#[tauri::command]
fn set_quota_plan(
    state: State<'_, AppStateManaged>,
    upstream: String,
    len_ms: u64,
    limit: u64,
    unit: String,
    rate_limit_per_min: Option<u64>,
) -> Result<StateView, String> {
    let upstream = upstream.trim().to_owned();
    if unit != "tokens" && unit != "requests" {
        return Err(format!("未知额度单位：{unit}"));
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if !inner.draft["upstreams"][&upstream].is_object() {
        return Err(format!("未知供应商 `{upstream}`"));
    }

    let previous = inner.draft["upstreams"][&upstream].clone();
    if limit == 0 || len_ms == 0 {
        if let Some(up) = inner.draft["upstreams"][&upstream].as_object_mut() {
            up.remove("quota_plan");
        }
    } else {
        let mut plan = json!({
            "windows": [{ "len_ms": len_ms, "limit": limit }],
            "unit": unit,
        });
        if let Some(rate) = rate_limit_per_min.filter(|rate| *rate > 0) {
            plan["rate_limit_per_min"] = json!(rate);
        }
        inner.draft["upstreams"][&upstream]["quota_plan"] = plan;
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&upstream] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Live quota snapshot for the viewer: queries the running gateway's
/// `/admin/quota` over loopback with the virtual key. Runtime-only — the quota
/// picture lives in the gateway's memory, not the config or the metrics store —
/// so this needs the proxy running.
#[tauri::command]
fn get_quota_snapshot(state: State<'_, AppStateManaged>) -> Result<serde_json::Value, String> {
    let (listen, key) = {
        let inner = state.0.lock().unwrap();
        let serve = inner.serve_view();
        (serve.listen, serve.virtual_key)
    };
    let Some(key) = key else {
        return Err("代理未运行——启动代理后可查看实时额度".to_owned());
    };
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(3)))
            .build(),
    );
    let response = agent
        .get(format!("http://{listen}/admin/quota"))
        .header("authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| format!("查询额度失败：{error}"))?;
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("读取额度响应失败：{error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("额度响应不是合法 JSON：{error}"))
}

#[tauri::command]
fn edit_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<StateView, String> {
    edit_provider_impl(state, name, base_url, api_key, None, None, None)
}

#[tauri::command]
fn edit_provider_with_credential(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: String,
    credential_reference: Option<String>,
    provider_call: Option<String>,
) -> Result<StateView, String> {
    edit_provider_impl(
        state,
        name,
        base_url,
        api_key,
        Some(credential_source.trim()),
        credential_reference.as_deref(),
        provider_call.as_deref().map(str::trim),
    )
}

fn edit_provider_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: Option<&str>,
    credential_reference: Option<&str>,
    provider_call: Option<&str>,
) -> Result<StateView, String> {
    let name = name.trim().to_owned();
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(&inner, &name)?;
    let previous = inner.draft["upstreams"]
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    if previous["provider"].as_str() == Some("azure-openai-v1")
        && !azure_openai_v1_base_url_is_exact(&base_url)
    {
        return Err("Azure OpenAI v1 的 Base URL 路径必须精确为 `/openai/v1`".to_owned());
    }
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    let auth = credential_source
        .map(|source| provider_auth_value(source, credential_reference))
        .transpose()?;
    if credential_source.is_some_and(|source| source != "store") && api_key.is_some() {
        return Err("env/file 凭据只保存引用，不能同时提交 API Key 明文".to_owned());
    }
    // An engine is written exactly as named, `legacy` included: South is the
    // default, so an explicit legacy choice must survive in the document.
    let provider_call = match provider_call {
        None => None,
        Some(
            engine @ ("legacy"
            | "south_v1_buffered"
            | "south_v1_buffered_streaming"
            | "south_v1_buffered_streaming_header_auth"),
        ) => Some(engine),
        Some(_) => return Err("Provider call engine 不受支持".to_owned()),
    };
    let previous_auth = previous.get("auth").filter(|value| !value.is_null());
    let egress = draft_egress_config(&inner.draft)?;
    ensure_credential_transport(&endpoint, &egress)?;
    let auth_changed = auth
        .as_ref()
        .is_some_and(|next| next.as_ref() != previous_auth);
    let identity_changed = previous["base_url"].as_str() != Some(base_url.as_str())
        || api_key.is_some()
        || auth_changed;
    let previous_pricing = inner.draft["pricing"].clone();
    let previous_state = inner.config_state.clone();
    if identity_changed {
        // A URL or credential change may select a different Provider account.
        // Invalidate first: losing derived cache on a later rollback is safe;
        // presenting the old account's catalog as trusted is not.
        model_catalog::remove_provider(&inner.data_dir(), &name)?;
        clear_provider_scoped_prices(&mut inner, &name)?;
    }
    if let Some(provider_call) = provider_call {
        inner.draft["upstreams"][&name]["provider_call"] = json!(provider_call);
    }
    inner.draft["upstreams"][&name]["base_url"] = json!(base_url);
    if let Some(auth) = auth {
        match auth {
            Some(value) => inner.draft["upstreams"][&name]["auth"] = value,
            None => {
                inner.draft["upstreams"][&name]
                    .as_object_mut()
                    .expect("upstream is an object")
                    .remove("auth");
            }
        }
    } else if api_key.is_some() {
        inner.draft["upstreams"][&name]["auth"] =
            json!({ "slot": "provider_api_key", "store": true });
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&name] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(error);
    }
    if let Some(key) = api_key {
        if let Err(key_error) =
            secrets::store_set(&inner.data_dir(), &name, "provider_api_key", &key)
        {
            inner.draft["upstreams"][&name] = previous;
            inner.draft["pricing"] = previous_pricing;
            inner.config_state = previous_state;
            return match inner.observe_draft() {
                Ok(()) => Err(key_error),
                Err(rollback_error) => Err(format!(
                    "{key_error}；同时回滚 Provider 草稿失败：{rollback_error}"
                )),
            };
        }
        // Secret-store contents are intentionally absent from the serialized
        // Provider definition, so a successful key rotation needs its own epoch.
        inner.bump_upstream_epoch(&name);
    }
    Ok(inner.snapshot())
}

#[derive(Debug, PartialEq, Eq)]
enum DiscoveryCredential {
    Explicit(Option<String>),
    Stored { provider: String, slot: String },
}

impl DiscoveryCredential {
    fn is_explicit_secret(&self) -> bool {
        matches!(self, Self::Explicit(Some(_)))
    }
}

#[derive(Clone, Debug)]
struct ProviderDiscoveryTarget {
    upstream: Option<Value>,
    upstream_epoch: u64,
    discovery_generation: u64,
}

fn capture_provider_discovery_target(inner: &AppInner, name: &str) -> ProviderDiscoveryTarget {
    ProviderDiscoveryTarget {
        upstream: inner.draft["upstreams"].get(name).cloned(),
        upstream_epoch: inner.upstream_epochs.get(name).copied().unwrap_or_default(),
        discovery_generation: inner
            .discovery_generations
            .get(name)
            .copied()
            .unwrap_or_default(),
    }
}

fn begin_provider_discovery_target(inner: &mut AppInner, name: &str) -> ProviderDiscoveryTarget {
    let generation = inner
        .discovery_generations
        .entry(name.to_owned())
        .or_default();
    *generation = generation.saturating_add(1).max(1);
    capture_provider_discovery_target(inner, name)
}

fn ensure_provider_discovery_target_unchanged(
    inner: &AppInner,
    name: &str,
    expected: &ProviderDiscoveryTarget,
) -> Result<(), String> {
    let current = capture_provider_discovery_target(inner, name);
    if current.upstream != expected.upstream
        || current.upstream_epoch != expected.upstream_epoch
        || current.discovery_generation != expected.discovery_generation
    {
        return Err(format!(
            "Provider `{name}` changed while its model catalog was loading. Retry discovery"
        ));
    }
    Ok(())
}

fn provider_health_uses_south(draft: &Value, upstream: &Value, package_verified: bool) -> bool {
    match upstream["provider_call"]
        .as_str()
        .unwrap_or(DEFAULT_PROVIDER_CALL)
    {
        "south_v1_buffered" | "south_v1_buffered_streaming" => {
            south_v1_unavailable_reason(draft, upstream, package_verified).is_none()
        }
        "south_v1_buffered_streaming_header_auth" => {
            south_header_auth_v1_unavailable_reason(draft, upstream, package_verified).is_none()
        }
        _ => false,
    }
}

fn prepare_discovery_credential(
    inner: &AppInner,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<DiscoveryCredential, String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
    if inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream["provider"].as_str())
        == Some("azure-openai-v1")
    {
        return Err("model_catalog_azure_deployment_manual".to_owned());
    }
    let explicit = api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    if explicit.is_some() {
        return Ok(DiscoveryCredential::Explicit(explicit));
    }
    // OpenRouter's model catalog is public. Avoid an unnecessary Keychain read here:
    // it can trigger a macOS authorization round-trip even though `/models` needs no key.
    if base_url == "https://openrouter.ai/api/v1" {
        return Ok(DiscoveryCredential::Explicit(None));
    }
    if let Some(upstream) = inner.draft["upstreams"].get(name) {
        let configured_base = upstream["base_url"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('/');
        if configured_base != base_url {
            return Err("使用已保存 Key 刷新时，Base URL 必须与供应商配置一致".to_owned());
        }
        return match upstream["auth"]["slot"].as_str() {
            Some(slot) => Ok(DiscoveryCredential::Stored {
                provider: name.to_owned(),
                slot: slot.to_owned(),
            }),
            None => Ok(DiscoveryCredential::Explicit(None)),
        };
    }
    Ok(DiscoveryCredential::Explicit(None))
}

fn catalog_cost_to_model_price(cost: &model_catalog::CatalogCostView) -> Option<ModelPrice> {
    fn micros(value: f64) -> Option<u64> {
        let scaled = value * 1_000_000.0;
        (scaled.is_finite() && scaled >= 0.0 && scaled <= u64::MAX as f64)
            .then(|| scaled.round() as u64)
    }

    Some(ModelPrice {
        input_per_mtok: micros(cost.input?)?,
        output_per_mtok: micros(cost.output?)?,
        cache_read_per_mtok: micros(cost.cache_read?)?,
        cache_write_per_mtok: micros(cost.cache_write?)?,
        reasoning_per_mtok: None,
    })
}

/// Fetch the provider's current model catalog on a blocking worker without
/// blocking the Tauri UI. When using a saved key, require the request URL to
/// match provider configuration so credentials cannot be forwarded elsewhere.
fn apply_discovered_model_capabilities(
    inner: &mut AppInner,
    name: &str,
    catalog: &[model_catalog::CatalogModelView],
) -> Result<bool, String> {
    let Some(upstream) = inner.draft["upstreams"].get(name) else {
        return Ok(false);
    };
    let previous = upstream
        .get("models")
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let facts: std::collections::BTreeMap<&str, &model_catalog::CatalogModelView> = catalog
        .iter()
        .filter(|model| model.catalog_state == model_catalog::CatalogState::Active)
        .filter(|model| {
            model.vision != CapabilityState::Unknown
                || model.context_window.is_some()
                || model.max_output_tokens.is_some()
                || model.cost.is_some()
        })
        .map(|model| (model.model.as_str(), model))
        .collect();
    inner.ensure_editable()?;
    let previous_state = inner.config_state.clone();
    let previous_pricing = inner.draft["pricing"].clone();
    let mut next_pricing = draft_price_table(inner)?;
    let mut pricing_changed = false;
    let selected_models: std::collections::BTreeSet<String> = upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|capability| capability["model"].as_str().map(str::to_owned))
        .collect();
    for (model, fact) in &facts {
        if !selected_models.contains(*model) {
            continue;
        }
        let Some(price) = fact.cost.as_ref().and_then(catalog_cost_to_model_price) else {
            continue;
        };
        let scoped_model = format!("{name}/{model}");
        // Supplier catalogs are useful defaults, but the versioned pricing
        // editor is operator-owned. Without per-entry provenance we cannot
        // distinguish a previous catalog value from an explicit edit, so a
        // refresh may fill an unknown scoped price but must never overwrite an
        // existing one.
        if next_pricing.models.contains_key(&scoped_model) {
            continue;
        }
        next_pricing = next_pricing.next_with_model(&scoped_model, price)?;
        pricing_changed = true;
    }
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str() else {
            continue;
        };
        if let Some(fact) = facts.get(model).copied() {
            if let Some((supported, serialized)) = match fact.vision {
                CapabilityState::Verified => Some((true, "verified")),
                CapabilityState::Unsupported => Some((false, "unsupported")),
                CapabilityState::Declared | CapabilityState::Unknown => None,
            } {
                if capability["vision"].as_bool() != Some(supported)
                    || capability["vision_state"].as_str() != Some(serialized)
                {
                    capability["vision"] = json!(supported);
                    capability["vision_state"] = json!(serialized);
                    changed = true;
                }
            }
            if let Some(context) = fact.context_window {
                let source = json_limit_source(capability, CONTEXT_WINDOW_SOURCE_KEY);
                if (capability["context_window"].as_u64().unwrap_or_default() == 0
                    || source_is_default(source))
                    && (capability["context_window"].as_u64() != Some(u64::from(context))
                        || source != Some(LIMIT_SOURCE_PROVIDER))
                {
                    capability["context_window"] = json!(context);
                    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                    changed = true;
                }
            }
            let effective_context = capability["context_window"].as_u64().unwrap_or_default();
            let configured_output = capability["max_output_tokens"].as_u64().unwrap_or_default();
            if configured_output > effective_context
                && source_is_default(json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY))
            {
                capability
                    .as_object_mut()
                    .expect("model capability is an object")
                    .remove("max_output_tokens");
                capability
                    .as_object_mut()
                    .expect("model capability is an object")
                    .remove(MAX_OUTPUT_TOKENS_SOURCE_KEY);
                changed = true;
            }
            if let Some(output) = fact.max_output_tokens {
                let source = json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY);
                if capability["max_output_tokens"].as_u64().unwrap_or_default() == 0
                    || source_is_default(source)
                {
                    let effective_context = capability["context_window"]
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok());
                    if effective_context.is_some_and(|context| output <= context)
                        && (capability["max_output_tokens"].as_u64() != Some(u64::from(output))
                            || source != Some(LIMIT_SOURCE_PROVIDER))
                    {
                        capability["max_output_tokens"] = json!(output);
                        capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                        changed = true;
                    }
                }
            }
            if let Some(cost) = &fact.cost {
                let serialized = serde_json::to_value(cost)
                    .map_err(|error| format!("序列化模型价格失败：{error}"))?;
                if capability["catalog_cost"] != serialized {
                    capability["catalog_cost"] = serialized;
                    changed = true;
                }
            }
        }
    }
    changed |= apply_builtin_model_limits_to_upstream(&mut inner.draft["upstreams"][name]);
    if !changed && !pricing_changed {
        return Ok(false);
    }
    if pricing_changed {
        inner.draft["pricing"] =
            serde_json::to_value(next_pricing).map_err(|error| error.to_string())?;
    }

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(format!("保存模型目录能力失败：{error}"));
    }
    Ok(true)
}

#[tauri::command]
async fn discover_provider_models(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<ModelDiscoveryView, String> {
    discover_provider_models_impl(state, name, base_url, api_key, true).await
}

#[tauri::command]
async fn verify_enterprise_route(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: String,
) -> Result<ModelDiscoveryView, String> {
    discover_provider_models_impl(state, name, base_url, Some(api_key), false).await
}

async fn discover_provider_models_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    persist_derived_state: bool,
) -> Result<ModelDiscoveryView, String> {
    let name = name.trim().to_owned();
    let base_url = base_url.trim().trim_end_matches('/').to_owned();
    if name.is_empty() {
        return Err("请先填写供应商名称".to_owned());
    }
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let endpoint = ProviderEndpoint::try_new(&base_url)
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();

    let (data_dir, credential, egress, egress_secrets, expected_target, mutate_derived_state) = {
        let mut inner = state.0.lock().unwrap();
        if inner.pending_provider_discoveries.contains(&name) {
            return Err(format!(
                "Provider `{name}` model discovery is already in progress"
            ));
        }
        const MAX_CONCURRENT_PROVIDER_DISCOVERIES: usize = 8;
        if inner.pending_provider_discoveries.len() >= MAX_CONCURRENT_PROVIDER_DISCOVERIES {
            return Err("Provider model discovery concurrency limit reached".to_owned());
        }
        let credential =
            prepare_discovery_credential(&inner, &name, &base_url, api_key.as_deref())?;
        let config = inner.materialize()?;
        ensure_credential_transport(&endpoint, &config.egress)?;
        let expected_target = begin_provider_discovery_target(&mut inner, &name);
        let mutate_derived_state = persist_derived_state
            && (expected_target.upstream.is_none() || !credential.is_explicit_secret());
        inner.pending_provider_discoveries.insert(name.clone());
        (
            inner.data_dir(),
            credential,
            config.egress.clone(),
            secrets::SecretStore::from_config(&config, &inner.data_dir()),
            expected_target,
            mutate_derived_state,
        )
    };
    let _discovery_guard = ProviderDiscoveryGuard {
        inner: &state.0,
        provider: name.clone(),
    };

    let task_name = name.clone();
    let task_base_url = base_url.clone();
    let task_data_dir = data_dir.clone();
    let mut result = tauri::async_runtime::spawn_blocking(move || {
        let resolved_key = match credential {
            DiscoveryCredential::Explicit(key) => key,
            DiscoveryCredential::Stored { provider, slot } => {
                Some(egress_secrets.resolve(&provider, &slot)?)
            }
        };
        model_catalog::discover_candidate_with_cache_egress(
            &task_data_dir,
            &task_name,
            &task_base_url,
            resolved_key.as_deref(),
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("模型目录任务异常结束：{error}"))??;
    let mut inner = state.0.lock().unwrap();
    ensure_provider_discovery_target_unchanged(&inner, &name, &expected_target)?;
    if mutate_derived_state {
        if let Err(error) =
            model_catalog::commit_live_discovery_cache(&data_dir, &name, &base_url, &result)
        {
            result.warning = Some(error);
        }
        result.capabilities_updated =
            apply_discovered_model_capabilities(&mut inner, &name, &result.catalog)?;
    }
    if result.source == "none"
        && inner.draft["upstreams"]
            .get(&name)
            .is_some_and(provider_uses_builtin_model_limits)
    {
        result.source = "preset".to_owned();
    }
    Ok(result)
}

#[tauri::command]
async fn test_provider(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<Vec<ProviderTestResult>, String> {
    let (config, name, tests_south) = {
        let inner = state.0.lock().unwrap();
        let name = name.trim().to_owned();
        let upstream = inner.draft["upstreams"]
            .get(&name)
            .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
        let provider = upstream["provider"].as_str().unwrap_or_default();
        let package_verified = inner.south_approved_dialects.contains(provider);
        let tests_south = provider_health_uses_south(&inner.draft, upstream, package_verified);
        (inner.materialize()?, name, tests_south)
    };
    let provider_runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
        let gateway =
            Gateway::new_with_provider_runtime(&config, recorder, provider_runtime.clone())?;
        if tests_south {
            let outcomes = provider_runtime.block_on(gateway.probe_south_v1(&name, None))?;
            return Ok(outcomes
                .into_iter()
                .map(|outcome| {
                    let (status, detail, latency_ms) = match outcome.latency_ms {
                        Ok(latency) => (
                            StageStatus::Pass,
                            Some("Configured South engine probe passed".to_owned()),
                            Some(latency),
                        ),
                        Err(error) => (StageStatus::Fail, Some(error), None),
                    };
                    let stages = ["network", "http", "auth", "model", "generation"]
                        .into_iter()
                        .map(|layer| ProviderTestStage {
                            layer: layer.to_owned(),
                            status: if status == StageStatus::Pass || layer == "generation" {
                                status
                            } else {
                                StageStatus::Skipped
                            },
                            detail: detail.clone(),
                            duration_ms: latency_ms,
                            timing_kind: latency_ms.map(|_| "cumulative"),
                        })
                        .collect();
                    ProviderTestResult {
                        model: outcome.model,
                        stages,
                        latency_ms,
                    }
                })
                .collect());
        }
        let probes = gateway.probe_layered(&name, None)?;
        Ok(probes
            .into_iter()
            .map(|probe| {
                let generation_passed = probe
                    .stages
                    .last()
                    .is_some_and(|stage| stage.status == StageStatus::Pass);
                let mut stages: Vec<ProviderTestStage> = probe
                    .stages
                    .into_iter()
                    .map(|stage| ProviderTestStage {
                        layer: match stage.layer {
                            HealthLayer::Network => "network",
                            HealthLayer::Http => "http",
                            HealthLayer::Auth => "auth",
                            HealthLayer::Model => "model",
                            HealthLayer::Generation => "generation",
                        }
                        .to_owned(),
                        status: stage.status,
                        detail: stage.detail,
                        duration_ms: (stage.status != StageStatus::Skipped)
                            .then_some(probe.latency_ms)
                            .flatten(),
                        timing_kind: (stage.status != StageStatus::Skipped).then_some("cumulative"),
                    })
                    .collect();
                if generation_passed {
                    match gateway.probe_features(&name, &probe.model) {
                        Ok(features) => stages.extend(features.stages.into_iter().map(|stage| {
                            ProviderTestStage {
                                layer: match stage.layer {
                                    FeatureLayer::Stream => "stream",
                                    FeatureLayer::Tool => "tool",
                                    FeatureLayer::Json => "json",
                                }
                                .to_owned(),
                                status: stage.status,
                                detail: stage.detail,
                                duration_ms: Some(stage.duration_ms),
                                timing_kind: Some("stage"),
                            }
                        })),
                        Err(error) => stages.extend(["stream", "tool", "json"].map(|layer| {
                            ProviderTestStage {
                                layer: layer.to_owned(),
                                status: StageStatus::Fail,
                                detail: Some(error.clone()),
                                duration_ms: None,
                                timing_kind: None,
                            }
                        })),
                    }
                } else {
                    stages.extend(["stream", "tool", "json"].map(|layer| ProviderTestStage {
                        layer: layer.to_owned(),
                        status: StageStatus::Skipped,
                        detail: Some("基础生成测试未通过".to_owned()),
                        duration_ms: None,
                        timing_kind: None,
                    }));
                }
                ProviderTestResult {
                    model: probe.model,
                    stages,
                    latency_ms: probe.latency_ms,
                }
            })
            .collect())
    })
    .await
    .map_err(|error| format!("Provider 测试任务异常结束：{error}"))?
}

const MODEL_TEST_MAX_MESSAGES: usize = 20;
const MODEL_TEST_MAX_MESSAGE_BYTES: usize = 16_000;
const MODEL_TEST_MAX_TOTAL_BYTES: usize = 64_000;
const MODEL_TEST_MAX_REQUEST_ID_BYTES: usize = 64;
const MODEL_TEST_MAX_ACTIVE_STREAMS: usize = 4;
const MODEL_TEST_MAX_PENDING_CANCELLATIONS: usize = 32;
const MODEL_TEST_MAX_SSE_BUFFER_BYTES: usize = 1_048_576;
const MODEL_TEST_MAX_RESPONSE_BYTES: usize = 16_000;
const MODEL_TEST_MAX_STREAM_EVENTS: usize = 1_024;
const MODEL_TEST_MAX_STREAM_BYTES: usize = 4 * 1_048_576;
const MODEL_TEST_STREAM_EVENT: &str = "model-test-stream";

#[derive(Default)]
struct ModelTestOutputBudget {
    bytes: usize,
    events: usize,
    wire_bytes: usize,
}

impl ModelTestOutputBudget {
    fn accept_wire(&mut self, bytes: usize) -> Result<(), String> {
        let next_bytes = self.wire_bytes.saturating_add(bytes);
        if next_bytes > MODEL_TEST_MAX_STREAM_BYTES {
            return Err("The model stream exceeded the wire response limit".to_owned());
        }
        self.wire_bytes = next_bytes;
        Ok(())
    }

    fn accept(&mut self, delta: &str) -> Result<(), String> {
        let next_bytes = self.bytes.saturating_add(delta.len());
        if next_bytes > MODEL_TEST_MAX_RESPONSE_BYTES {
            return Err("The model output exceeded the response limit".to_owned());
        }
        if self.events >= MODEL_TEST_MAX_STREAM_EVENTS {
            return Err("The model output exceeded the stream event limit".to_owned());
        }
        self.bytes = next_bytes;
        self.events += 1;
        Ok(())
    }
}

#[derive(Default)]
struct ModelTestSseDecoder {
    buffer: Vec<u8>,
}

impl ModelTestSseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MODEL_TEST_MAX_SSE_BUFFER_BYTES {
            return Err("The model stream exceeded the response buffer limit".to_owned());
        }
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some((boundary, delimiter_len)) = find_model_test_sse_boundary(&self.buffer) {
            let frame = self.buffer.drain(..boundary).collect::<Vec<_>>();
            self.buffer.drain(..delimiter_len);
            let frame = String::from_utf8(frame)
                .map_err(|_| "The model stream returned invalid UTF-8".to_owned())?;
            if !frame.trim().is_empty() {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<String>, String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            self.buffer.clear();
            return Ok(Vec::new());
        }
        let frame = String::from_utf8(std::mem::take(&mut self.buffer))
            .map_err(|_| "The model stream ended with invalid UTF-8".to_owned())?;
        Ok(vec![frame])
    }
}

fn find_model_test_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

fn validate_model_test_request_id(request_id: &str) -> Result<(), String> {
    if request_id.is_empty()
        || request_id.len() > MODEL_TEST_MAX_REQUEST_ID_BYTES
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("The model test request ID is invalid".to_owned());
    }
    Ok(())
}

fn model_test_stream_delta(frame: &str) -> Result<Option<String>, String> {
    let data = frame
        .lines()
        .filter_map(|line| {
            let data = line.strip_prefix("data:")?;
            Some(data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|_| "The model stream returned invalid JSON".to_owned())?;
    if value.get("error").is_some() {
        return Err("The Provider returned a stream error".to_owned());
    }
    let content = value.pointer("/choices/0/delta/content");
    if let Some(text) = content
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Ok(Some(text.to_owned()));
    }
    if let Some(parts) = content.and_then(Value::as_array) {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        if !text.is_empty() {
            return Ok(Some(text));
        }
    }
    Ok(value
        .pointer("/choices/0/text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned))
}

fn emit_model_test_frames<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    started: Instant,
    frames: Vec<String>,
    content: &mut String,
    first_token_ms: &mut Option<u64>,
    output_budget: &mut ModelTestOutputBudget,
) -> Result<(), String> {
    for frame in frames {
        let Some(delta) = model_test_stream_delta(&frame)? else {
            continue;
        };
        output_budget.accept(&delta)?;
        if first_token_ms.is_none() {
            *first_token_ms =
                Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
        content.push_str(&delta);
        app.emit(
            MODEL_TEST_STREAM_EVENT,
            ModelTestStreamEvent {
                request_id: request_id.to_owned(),
                delta,
                first_token_ms: *first_token_ms,
            },
        )
        .map_err(|_| "The model stream could not reach the test console".to_owned())?;
    }
    Ok(())
}

fn validate_model_test_messages(messages: &[ModelTestMessage]) -> Result<(), String> {
    if messages.is_empty() {
        return Err("Enter a message before sending".to_owned());
    }
    if messages.len() > MODEL_TEST_MAX_MESSAGES {
        return Err(format!(
            "A model test supports at most {MODEL_TEST_MAX_MESSAGES} messages"
        ));
    }

    let mut total_bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("Model test messages support only user and assistant roles".to_owned());
        }
        let expected_role = if index % 2 == 0 { "user" } else { "assistant" };
        if message.role != expected_role {
            return Err("Model test messages must alternate user and assistant roles".to_owned());
        }
        let message_bytes = message.content.len();
        if message_bytes == 0 || message.content.trim().is_empty() {
            return Err("Model test messages cannot be empty".to_owned());
        }
        if message_bytes > MODEL_TEST_MAX_MESSAGE_BYTES {
            return Err(format!(
                "One model test message exceeds {MODEL_TEST_MAX_MESSAGE_BYTES} bytes"
            ));
        }
        total_bytes = total_bytes.saturating_add(message_bytes);
        if total_bytes > MODEL_TEST_MAX_TOTAL_BYTES {
            return Err(format!(
                "The model test conversation exceeds {MODEL_TEST_MAX_TOTAL_BYTES} bytes"
            ));
        }
    }
    if messages.last().map(|message| message.role.as_str()) != Some("user") {
        return Err("The last model test message must be from the user".to_owned());
    }
    Ok(())
}

fn model_test_http_error(status: u16) -> String {
    let summary = match status {
        400 => "The model rejected the request. Check the prompt and model limits",
        401 | 403 => "Provider authentication failed. Check this Provider credential",
        402 => "The Provider account has no available balance",
        404 => "The selected model or endpoint is unavailable",
        408 | 504 => "The model request timed out",
        409 => "The Provider rejected the current request state",
        429 => "The Provider rate limit is active. Try again later",
        500..=599 => "The Provider is temporarily unavailable",
        _ => "The model request failed",
    };
    format!("{summary} (HTTP {status})")
}

fn model_test_assistant_content(body: &Value) -> Option<String> {
    let content = body.pointer("/choices/0/message/content")?;
    if let Some(text) = content.as_str().filter(|text| !text.trim().is_empty()) {
        return Some(text.to_owned());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!parts.trim().is_empty()).then_some(parts)
}

fn extract_model_test_reply(status: u16, body: &str) -> Result<String, String> {
    if body.len() > MODEL_TEST_MAX_STREAM_BYTES {
        return Err("The model response exceeded the wire response limit".to_owned());
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|_| format!("The model returned invalid JSON (HTTP {status})"))?;
    if !(200..300).contains(&status) {
        return Err(model_test_http_error(status));
    }
    let content = model_test_assistant_content(&value)
        .ok_or_else(|| "The model returned no assistant text".to_owned())?;
    if content.len() > MODEL_TEST_MAX_RESPONSE_BYTES {
        return Err("The model output exceeded the response limit".to_owned());
    }
    Ok(content)
}

#[tauri::command]
async fn test_model_chat_stream(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
    stream_state: State<'_, ModelTestStreamState>,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    run_model_test_chat(
        app,
        state.inner(),
        stream_state.inner(),
        messages,
        request_id,
    )
    .await
}

async fn run_model_test_chat<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    stream_state: &ModelTestStreamState,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    validate_model_test_messages(&messages)?;
    validate_model_test_request_id(&request_id)?;
    let config = {
        let inner = state.0.lock().unwrap();
        inner.materialize()?
    };

    let request_context =
        RequestContext::detached(Duration::from_secs(120), Duration::from_secs(120))
            .with_upstream_response_limit(MODEL_TEST_MAX_STREAM_BYTES as u64);
    let registry = Arc::clone(&stream_state.0);
    {
        let mut streams = registry.lock().unwrap();
        streams.register(&request_id, request_context.token())?;
    }
    let registration = ModelTestStreamRegistration {
        registry,
        request_id: request_id.clone(),
    };

    let provider_runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        let _registration = registration;
        let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
        let gateway = Gateway::new_with_provider_runtime(&config, recorder, provider_runtime)?;
        let body = serde_json::to_vec(&json!({
            "model": "auto",
            "messages": messages,
            "stream": true,
            "max_tokens": 1024
        }))
        .map_err(|_| "Failed to encode the model test request".to_owned())?;
        let started = Instant::now();
        let mut json_response = None;
        let mut decoder = ModelTestSseDecoder::default();
        let mut content = String::new();
        let mut first_token_ms = None;
        let mut output_budget = ModelTestOutputBudget::default();
        let mut stream_error = None;
        gateway.chat_scoped(
            &request_context,
            None,
            None,
            "POST",
            "/v1/chat/completions",
            &[("content-type".to_owned(), "application/json".to_owned())],
            &body,
            &mut |reply| {
                if request_context.is_cancelled() {
                    return false;
                }
                match reply {
                    Reply::BeginJson(reply) => {
                        json_response = Some((reply.status, reply.body));
                    }
                    Reply::BeginStream => {}
                    Reply::Chunk(chunk) => match output_budget
                        .accept_wire(chunk.len())
                        .and_then(|()| decoder.push(chunk.as_bytes()))
                        .and_then(|frames| {
                            emit_model_test_frames(
                                &app,
                                &request_id,
                                started,
                                frames,
                                &mut content,
                                &mut first_token_ms,
                                &mut output_budget,
                            )
                        }) {
                        Ok(()) => {}
                        Err(error) => {
                            stream_error = Some(error);
                            request_context.cancel();
                            return false;
                        }
                    },
                }
                true
            },
        );
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(error) = stream_error {
            return Err(error);
        }
        if let Some(reason) = request_context.cancel_reason() {
            return Err(match reason {
                CancelReason::Deadline => "The model request timed out".to_owned(),
                CancelReason::ClientDisconnect | CancelReason::ServerDrain => {
                    "Model test cancelled".to_owned()
                }
            });
        }
        if let Some((status, body)) = json_response {
            let content = extract_model_test_reply(status, &body)?;
            return Ok(ModelTestReply {
                content,
                first_token_ms: latency_ms,
                latency_ms,
            });
        }
        let frames = decoder.finish()?;
        emit_model_test_frames(
            &app,
            &request_id,
            started,
            frames,
            &mut content,
            &mut first_token_ms,
            &mut output_budget,
        )?;
        if content.trim().is_empty() {
            return Err("The model returned no assistant text".to_owned());
        }
        Ok(ModelTestReply {
            content,
            first_token_ms: first_token_ms.unwrap_or(latency_ms),
            latency_ms,
        })
    })
    .await
    .map_err(|error| format!("Model test task stopped unexpectedly: {error}"))?
}

#[tauri::command]
fn cancel_model_test_chat(
    stream_state: State<'_, ModelTestStreamState>,
    request_id: String,
) -> Result<(), String> {
    validate_model_test_request_id(&request_id)?;
    let mut streams = stream_state.0.lock().unwrap();
    streams.cancel(request_id);
    Ok(())
}

/// Update an existing provider's model set while protecting models referenced by routing tiers.
fn replace_provider_models(
    inner: &mut AppInner,
    name: &str,
    models: Vec<String>,
) -> Result<(), String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
    let normalized = normalize_provider_model_ids(models)?;

    let upstream = inner.draft["upstreams"]
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    let base_url = upstream
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let configured_capabilities: Vec<ModelCapability> = upstream
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| serde_json::from_value(model.clone()).ok())
        .collect();
    let (_, discovered_catalog) = model_catalog::catalog_for_provider(
        &inner.data_dir(),
        name,
        &base_url,
        &configured_capabilities,
    );
    let discovered_by_model: std::collections::BTreeMap<&str, &model_catalog::CatalogModelView> =
        discovered_catalog
            .iter()
            .filter(|entry| entry.catalog_state == model_catalog::CatalogState::Active)
            .map(|entry| (entry.model.as_str(), entry))
            .collect();
    let discovered_prices: Vec<(String, ModelPrice)> = normalized
        .iter()
        .filter_map(|model| {
            discovered_by_model
                .get(model.as_str())
                .and_then(|entry| entry.cost.as_ref())
                .and_then(catalog_cost_to_model_price)
                .map(|price| (format!("{name}/{model}"), price))
        })
        .collect();
    let removed_reference = |target: &Value| {
        target["upstream"].as_str() == Some(name)
            && target["model"]
                .as_str()
                .is_some_and(|model| !normalized.iter().any(|candidate| candidate == model))
    };
    let mut direct_and_quota_blocked = Vec::new();
    if removed_reference(&inner.draft["routing"]["direct_target"]) {
        direct_and_quota_blocked.push("主页/单独路由".to_owned());
    }
    for (index, account) in inner.draft["router"]["quota_accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if removed_reference(account) {
            direct_and_quota_blocked.push(format!("主页/额度优先#{}", index + 1));
        }
    }
    if let Some(agent_routes) = inner.draft["agent_routes"].as_object() {
        for (agent_id, route) in agent_routes {
            if removed_reference(&route["direct_target"]) {
                direct_and_quota_blocked.push(format!("Agent/{agent_id}/单独路由"));
            }
        }
    }
    direct_and_quota_blocked.sort();
    direct_and_quota_blocked.dedup();
    if !direct_and_quota_blocked.is_empty() {
        return Err(format!(
            "不能移除 {} 正在使用的模型，请先调整对应路由",
            direct_and_quota_blocked.join("、")
        ));
    }
    let blocked: Vec<&str> = [(TIER_HIGH, "上档"), (TIER_MID, "中档"), (TIER_LOW, "下档")]
        .into_iter()
        .filter_map(|(pool, label)| {
            let member = inner.draft["router"]["pools"][pool]
                .as_array()
                .and_then(|members| members.first());
            let refers_to_provider = member
                .and_then(|item| item["upstream"].as_str())
                .is_some_and(|upstream| upstream == name);
            let retained = member
                .and_then(|item| item["model"].as_str())
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            (refers_to_provider && !retained).then_some(label)
        })
        .collect();
    if !blocked.is_empty() {
        return Err(format!(
            "不能移除 {} 正在使用的模型，请先调整对应档位",
            blocked.join("、")
        ));
    }

    let mut agent_blocked = Vec::new();
    for agent_id in supported_agent_ids() {
        for slot in ["high", "mid", "low"] {
            let target = &inner.draft["agent_routes"][&agent_id]["custom_route"][slot];
            let refers_to_provider = target["upstream"].as_str() == Some(name);
            let retained = target["model"]
                .as_str()
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            if refers_to_provider && !retained {
                agent_blocked.push(format!("{agent_id}/{slot}"));
            }
        }
    }
    for (agent_id, tiers) in &inner.agent_route_drafts {
        for (slot, target) in tiers {
            let refers_to_provider = target.upstream.as_deref() == Some(name);
            let retained = target
                .model
                .as_deref()
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            if refers_to_provider && !retained {
                agent_blocked.push(format!("{agent_id}/{slot}"));
            }
        }
    }
    agent_blocked.sort();
    agent_blocked.dedup();
    if !agent_blocked.is_empty() {
        return Err(format!(
            "不能移除 Agent 独立路由 {} 正在使用的模型，请先调整对应档位",
            agent_blocked.join("、")
        ));
    }

    // Strategy groups (profiles) pin a provider+model per tier; a model still used
    // by one must not be silently removed, or the profile is left dangling.
    let mut profile_blocked = Vec::new();
    if let Some(profiles) = inner.draft["profiles"].as_object() {
        for (profile_name, tiers) in profiles {
            for slot in ["high", "mid", "low"] {
                let target = &tiers[slot];
                let refers_to_provider = target["upstream"].as_str() == Some(name);
                let retained = target["model"]
                    .as_str()
                    .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
                if refers_to_provider && !retained {
                    profile_blocked.push(format!("{profile_name}/{slot}"));
                }
            }
        }
    }
    profile_blocked.sort();
    profile_blocked.dedup();
    if !profile_blocked.is_empty() {
        return Err(format!(
            "不能移除策略组 {} 正在使用的模型，请先调整对应档位",
            profile_blocked.join("、")
        ));
    }

    let existing: std::collections::BTreeMap<String, Value> = upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item["model"]
                .as_str()
                .map(|model| (model.to_owned(), item.clone()))
        })
        .collect();
    let model_objects: Vec<Value> = normalized
        .into_iter()
        .map(|model| {
            existing.get(&model).cloned().unwrap_or_else(|| {
                let mut capability = json!({
                    // As in add_provider, OpenAI-compatible chat declares tools and structured output by default.
                    "model": model,
                    "tool": true,
                    "vision": false,
                    "json_schema": true,
                    "tool_state": "declared",
                    "vision_state": "unknown",
                    "json_schema_state": "declared",
                    "context_window": known_context_window(&model)
                });
                if let Some(preset) = builtin_model_limits(&base_url, &model) {
                    capability["context_window"] = json!(preset.context_window);
                    capability["max_output_tokens"] = json!(preset.max_output_tokens);
                    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
                    capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
                }
                if let Some(discovered) = discovered_by_model.get(model.as_str()) {
                    if let Some(context) = discovered.context_window {
                        capability["context_window"] = json!(context);
                        capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                    }
                    if let Some(output) = discovered.max_output_tokens {
                        capability["max_output_tokens"] = json!(output);
                        capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                    }
                    if discovered.vision != CapabilityState::Unknown {
                        let supported = discovered.vision.is_supported();
                        capability["vision"] = json!(supported);
                        capability["vision_state"] = json!(match discovered.vision {
                            CapabilityState::Verified => "verified",
                            CapabilityState::Declared => "declared",
                            CapabilityState::Unsupported => "unsupported",
                            CapabilityState::Unknown => "unknown",
                        });
                    }
                }
                capability
            })
        })
        .collect();

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let previous_pricing = inner.draft["pricing"].clone();
    let next_pricing = if discovered_prices.is_empty() {
        None
    } else {
        let mut pricing = draft_price_table(inner)?;
        let mut changed = false;
        for (model, price) in discovered_prices {
            // Model selection may fill a previously unknown supplier-scoped
            // price, but cannot replace an operator-owned versioned value.
            if pricing.models.contains_key(&model) {
                continue;
            }
            pricing = pricing.next_with_model(&model, price)?;
            changed = true;
        }
        changed
            .then(|| serde_json::to_value(pricing).map_err(|error| error.to_string()))
            .transpose()?
    };
    inner.draft["upstreams"][name]["models"] = json!(model_objects);
    if let Some(pricing) = next_pricing {
        inner.draft["pricing"] = pricing;
    }
    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(format!("保存供应商模型失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
fn update_provider_models(
    state: State<'_, AppStateManaged>,
    name: String,
    models: Vec<String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_models(&mut inner, name.trim(), models)?;
    Ok(inner.snapshot())
}

fn replace_provider_model_vision(
    inner: &mut AppInner,
    name: &str,
    model: &str,
    supported: bool,
) -> Result<(), String> {
    inner.ensure_editable()?;
    let name = name.trim();
    let model = model.trim();
    if name.is_empty() || model.is_empty() {
        return Err("供应商和模型 ID 不能为空".to_owned());
    }
    ensure_generic_provider_mutation_allowed(inner, name)?;

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let capability = models
        .iter_mut()
        .find(|candidate| candidate["model"].as_str() == Some(model))
        .ok_or_else(|| format!("供应商 `{name}` 未配置模型 `{model}`"))?;
    capability["vision"] = json!(supported);
    capability["vision_state"] = json!(if supported { "declared" } else { "unsupported" });

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.config_state = previous_state;
        return Err(format!("保存模型视觉能力失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
fn set_provider_model_vision(
    state: State<'_, AppStateManaged>,
    name: String,
    model: String,
    supported: bool,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_model_vision(&mut inner, &name, &model, supported)?;
    Ok(inner.snapshot())
}

fn replace_provider_model_limits(
    inner: &mut AppInner,
    name: &str,
    model: &str,
    context_window: u32,
    max_output_tokens: u32,
) -> Result<(), String> {
    inner.ensure_editable()?;
    let name = name.trim();
    let model = model.trim();
    if name.is_empty() || model.is_empty() {
        return Err("供应商和模型 ID 不能为空".to_owned());
    }
    if context_window == 0 || max_output_tokens == 0 {
        return Err("上下文上限和最大输出 Token 必须大于 0".to_owned());
    }
    if max_output_tokens > context_window {
        return Err("最大输出 Token 不能大于上下文上限".to_owned());
    }
    ensure_generic_provider_mutation_allowed(inner, name)?;

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let capability = models
        .iter_mut()
        .find(|candidate| candidate["model"].as_str() == Some(model))
        .ok_or_else(|| format!("供应商 `{name}` 未配置模型 `{model}`"))?;
    capability["context_window"] = json!(context_window);
    capability["max_output_tokens"] = json!(max_output_tokens);
    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_OPERATOR);
    capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_OPERATOR);

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.config_state = previous_state;
        return Err(format!("保存模型限制失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
fn set_provider_model_limits(
    state: State<'_, AppStateManaged>,
    name: String,
    model: String,
    context_window: u32,
    max_output_tokens: u32,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_model_limits(&mut inner, &name, &model, context_window, max_output_tokens)?;
    Ok(inner.snapshot())
}

fn provider_references(inner: &AppInner, name: &str) -> Vec<String> {
    let mut references = Vec::new();
    if inner.draft["routing"]["direct_target"]["upstream"].as_str() == Some(name) {
        references.push("主页/单独路由".to_owned());
    }
    for (index, account) in inner.draft["router"]["quota_accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if account["upstream"].as_str() == Some(name) {
            references.push(format!("主页/额度优先#{}", index + 1));
        }
    }
    if let Some(agent_routes) = inner.draft["agent_routes"].as_object() {
        for (agent_id, route) in agent_routes {
            if route["direct_target"]["upstream"].as_str() == Some(name) {
                references.push(format!("Agent/{agent_id}/单独路由"));
            }
        }
    }
    for (pool, label) in [
        (TIER_HIGH, "主页/上档"),
        (TIER_MID, "主页/中档"),
        (TIER_LOW, "主页/下档"),
    ] {
        for (index, member) in inner.draft["router"]["pools"][pool]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if member["upstream"].as_str() == Some(name) {
                references.push(format!("{label}#{}", index + 1));
            }
        }
    }
    for agent_id in supported_agent_ids() {
        for slot in ["high", "mid", "low"] {
            if inner.draft["agent_routes"][&agent_id]["custom_route"][slot]["upstream"].as_str()
                == Some(name)
            {
                references.push(format!("Agent/{agent_id}/{slot}"));
            }
        }
    }
    for (agent_id, tiers) in &inner.agent_route_drafts {
        for (slot, target) in tiers {
            if target.upstream.as_deref() == Some(name) {
                references.push(format!("Agent/{agent_id}/{slot}"));
            }
        }
    }
    // Saved strategy groups (profiles) reference providers by name too; without
    // this scan a provider used only by a profile would pass the removal gate and
    // leave that profile pointing at a deleted upstream, causing stale-option residue.
    if let Some(profiles) = inner.draft["profiles"].as_object() {
        for (profile_name, tiers) in profiles {
            for slot in ["high", "mid", "low"] {
                if tiers[slot]["upstream"].as_str() == Some(name) {
                    references.push(format!("策略组/{profile_name}/{slot}"));
                }
            }
        }
    }
    references.sort();
    references.dedup();
    references
}

#[tauri::command]
fn preview_provider_removal(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<ProviderRemovalPreview, String> {
    let inner = state.0.lock().unwrap();
    let name = name.trim();
    if inner.draft["upstreams"].get(name).is_none() {
        return Err(format!("供应商 `{name}` 不存在"));
    }
    let references = provider_references(&inner, name);
    Ok(ProviderRemovalPreview {
        name: name.to_owned(),
        can_remove: references.is_empty(),
        references,
    })
}

#[tauri::command]
fn remove_provider(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let name = name.trim();
    let references = provider_references(&inner, name);
    if !references.is_empty() {
        return Err(format!(
            "供应商仍被引用，不能删除：{}。请先调整这些路由",
            references.join("、")
        ));
    }
    let provider = inner.draft["upstreams"]
        .get(name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    let data_dir = inner.data_dir();
    model_catalog::remove_provider(&data_dir, name)?;
    provider_tombstones::archive(&data_dir, name, &provider)?;
    let pending_key = inner.pending_provider_keys.remove(name);
    inner.draft["upstreams"]
        .as_object_mut()
        .expect("upstreams is an object")
        .remove(name);
    inner.rebuild_routing();
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][name] = provider;
        if let Some(key) = pending_key {
            inner.pending_provider_keys.insert(name.to_owned(), key);
        }
        provider_tombstones::discard(&data_dir, name).ok();
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn restore_provider(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let name = name.trim();
    if inner.draft["upstreams"].get(name).is_some() {
        return Err(format!("同名供应商 `{name}` 已存在，不能覆盖恢复"));
    }
    let data_dir = inner.data_dir();
    let archived = provider_tombstones::get(&data_dir, name)?
        .ok_or_else(|| format!("Provider 回收站中没有 `{name}`"))?;
    if is_free_provider_value(&archived) {
        return Err(format!(
            "免费供应商 `{name}` 必须从免费目录重新验证，不能恢复旧目录快照"
        ));
    }
    model_catalog::remove_provider(&data_dir, name)?;
    let provider = provider_tombstones::take(&data_dir, name)?
        .ok_or_else(|| format!("Provider 回收站中没有 `{name}`"))?;
    inner.draft["upstreams"][name] = provider.clone();
    inner.rebuild_routing();
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(name);
        provider_tombstones::archive(&data_dir, name, &provider).ok();
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Set a tier to a provider-model pair, or pass null to clear it.
#[tauri::command]
fn set_tier(
    state: State<'_, AppStateManaged>,
    slot: String,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StateView, String> {
    let pool = pool_key(&slot)?;
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.set_tier_value(pool, upstream, model))?;
    Ok(inner.snapshot())
}

/// Add a keyword to a high, mid, or low tier; matches force that tier at router-core layer 1.
#[tauri::command]
fn add_keyword(
    state: State<'_, AppStateManaged>,
    slot: String,
    keyword: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.add_tier_keyword(&slot, &keyword))?;
    Ok(inner.snapshot())
}

/// Remove a keyword from a tier.
#[tauri::command]
fn remove_keyword(
    state: State<'_, AppStateManaged>,
    slot: String,
    keyword: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.remove_tier_keyword(&slot, &keyword))?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn set_agent_route_mode(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    mode: String,
) -> Result<StateView, String> {
    ensure_known_agent_id(&agent_id)?;
    if mode != "inherit" && mode != "custom" {
        return Err("路由模式必须是 inherit 或 custom".to_string());
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if mode == "custom" {
        inner.begin_agent_route_draft(&agent_id);
        return Ok(inner.snapshot());
    }
    inner.edit_validated_draft(|candidate| {
        candidate.set_agent_inherit_value(&agent_id);
        Ok(())
    })?;
    inner.agent_route_drafts.remove(&agent_id);
    Ok(inner.snapshot())
}

#[tauri::command]
fn set_agent_tier(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    slot: String,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.set_agent_route_draft_tier(&agent_id, &slot, upstream, model)?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn save_home_route_as_profile(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.save_home_route_as_profile_value(&name))?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn mount_agent_profile(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    profile: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| {
        candidate.mount_agent_profile_value(&agent_id, &profile)
    })?;
    inner.agent_route_drafts.remove(&agent_id);
    Ok(inner.snapshot())
}

#[tauri::command]
fn delete_profile(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.delete_profile_value(&name))?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn save_agent_routes(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.promote_agent_route_drafts()?;
    inner.save_draft()?;
    inner.agent_route_drafts.clear();
    Ok(inner.snapshot())
}

/// Save one Agent's route and apply it immediately by hot-swapping just that
/// Agent's router on the running gateway — no full restart, so every other
/// Agent's in-flight and new requests are undisturbed. If the proxy is not
/// running, this behaves like `save_agent_routes` (the route applies on next
/// start). Drives the per-Agent "Save & restart" button.
#[tauri::command]
fn restart_agent_route(
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
    agent_id: String,
) -> Result<StateView, String> {
    let snapshot = {
        let mut inner = state.0.lock().unwrap();
        if !supported_agent_ids().contains(&agent_id) {
            return Err(format!("未知 Agent：{agent_id}"));
        }
        if matches!(
            inner.server,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
        ) {
            return Err("apply_in_progress: 配置正在应用，请完成后再保存 Agent 路由".to_owned());
        }
        let applying_direct = inner.draft["agent_routes"][&agent_id]["routing_mode"]
            .as_str()
            .unwrap_or_else(|| inner.home_routing_mode())
            == "direct";
        // Tier editor drafts are a separate axis from Direct routing. Applying a
        // Direct target must neither validate nor silently commit an incomplete
        // hidden tier draft; keep it in memory for when the operator switches back.
        if !applying_direct {
            inner.promote_agent_route_drafts()?;
        }
        // Prepare every fallible hot-reload step before persisting. A successful
        // config save must never be followed by a recoverable router-build failure,
        // which would split the durable route from the running Gateway.
        let config = inner.materialize()?;
        let router = config.custom_router_for_agent(&agent_id)?;
        let prepared = match &inner.server {
            ServerLifecycle::Running { server, .. } => Some(
                server
                    .prepare_agent_router_reload(&agent_id, router)
                    .map_err(|error| format!("热重启 Agent 路由失败：{error}"))?,
            ),
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Failed { .. }
            | ServerLifecycle::Stopping { .. } => None,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                unreachable!("transitional lifecycles were rejected before editing")
            }
        };

        inner.save_draft()?;
        if !applying_direct {
            inner.agent_route_drafts.clear();
        }
        if let (Some(prepared), ServerLifecycle::Running { server, .. }) =
            (prepared, &mut inner.server)
        {
            server.install_prevalidated_agent_router(prepared);
        }
        inner.snapshot()
    };
    if let Ok(runtime) = runtime_from_app(state.inner()) {
        agents
            .refresh_model_metadata(Some(&agent_id), &runtime)
            .map_err(|error| {
                format!("Agent 路由已应用，但模型元数据刷新失败：{}", error.message)
            })?;
    }
    Ok(snapshot)
}

#[tauri::command]
fn apply_home_route_to_all_agents(
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
) -> Result<StateView, String> {
    let snapshot = {
        let mut inner = state.0.lock().unwrap();
        if matches!(
            inner.server,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
        ) {
            return Err(
                "apply_in_progress: Finish the current config apply before you apply the Home route."
                    .to_owned(),
            );
        }
        inner.edit_validated_draft(|candidate| {
            for agent_id in supported_agent_ids() {
                candidate.set_agent_inherit_value(&agent_id);
            }
            Ok(())
        })?;
        let prepared = match &inner.server {
            ServerLifecycle::Running { server, .. } => supported_agent_ids()
                .into_iter()
                .map(|agent_id| {
                    server
                        .prepare_agent_router_reload(&agent_id, None)
                        .map_err(|error| format!("Failed to hot-reload the Agent route: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Failed { .. }
            | ServerLifecycle::Stopping { .. } => Vec::new(),
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                unreachable!("transitional lifecycles were rejected before editing")
            }
        };

        inner.save_draft()?;
        inner.agent_route_drafts.clear();
        if let ServerLifecycle::Running { server, .. } = &mut inner.server {
            for prepared in prepared {
                server.install_prevalidated_agent_router(prepared);
            }
        }
        inner.snapshot()
    };
    if let Ok(runtime) = runtime_from_app(state.inner()) {
        agents
            .refresh_model_metadata(None, &runtime)
            .map_err(|error| {
                format!(
                    "The Home route was applied, but the model metadata refresh failed: {}",
                    error.message
                )
            })?;
    }
    Ok(snapshot)
}

/// Validate and write atomically. Return validation errors without writing, matching config edit semantics.
#[tauri::command]
fn save_config(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let tiered = inner.home_routing_mode() == "tiered";
    if tiered
        && inner.draft["router"]["pools"]
            .as_object()
            .map(|p| p.is_empty())
            .unwrap_or(true)
    {
        return Err("请至少配置一档(供应商 + 模型)再保存".into());
    }
    inner.save_draft()?;
    Ok(inner.snapshot())
}

fn emit_serve_state<R: Runtime>(app: &AppHandle<R>, view: &ServeView) {
    desktop_shell::update_proxy_menu(app);
    let _ = app.emit(SERVE_STATE_CHANGED_EVENT, view.clone());
}

#[cfg(target_os = "macos")]
fn publish_status_menu_start_error<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppStateManaged,
    error: String,
) {
    let view = {
        let mut inner = state.0.lock().unwrap();
        if matches!(
            inner.server,
            ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. }
        ) {
            let generation = inner.server.generation();
            let listen = inner
                .draft
                .pointer("/server/listen")
                .and_then(Value::as_str)
                .unwrap_or("127.0.0.1:8787")
                .to_owned();
            inner.server = ServerLifecycle::Failed {
                generation,
                listen,
                error,
            };
        }
        inner.serve_view()
    };
    emit_serve_state(app, &view);
    desktop_shell::restore_main_window(app);
}

#[cfg(any(target_os = "macos", test))]
fn desktop_shell_applying_phase(
    task_alive: bool,
    accepting: bool,
) -> desktop_shell::ProxyMenuPhase {
    if task_alive && accepting {
        desktop_shell::ProxyMenuPhase::Applying
    } else {
        desktop_shell::ProxyMenuPhase::Switching
    }
}

#[cfg(target_os = "macos")]
fn desktop_shell_snapshot(inner: &AppInner) -> desktop_shell::ProxyMenuSnapshot {
    let generation = inner.server.generation();
    let (phase, listen) = match &inner.server {
        ServerLifecycle::Stopped { .. } => (
            desktop_shell::ProxyMenuPhase::Stopped,
            inner.draft["server"]["listen"]
                .as_str()
                .unwrap_or("127.0.0.1:8787"),
        ),
        ServerLifecycle::Starting { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Starting, listen.as_str())
        }
        ServerLifecycle::Applying { old, .. } => (
            desktop_shell_applying_phase(old.is_task_alive(), old.is_accepting()),
            old.listen(),
        ),
        ServerLifecycle::Stopping { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Stopping, listen.as_str())
        }
        ServerLifecycle::Running { server, .. } if server.is_task_alive() => {
            (desktop_shell::ProxyMenuPhase::Running, server.listen())
        }
        ServerLifecycle::Running { server, .. } => {
            (desktop_shell::ProxyMenuPhase::Failed, server.listen())
        }
        ServerLifecycle::Failed { listen, .. } => {
            (desktop_shell::ProxyMenuPhase::Failed, listen.as_str())
        }
    };
    desktop_shell::ProxyMenuSnapshot::new(generation, phase, listen)
}

fn lifecycle_proxy_action(server: &ServerLifecycle) -> desktop_shell::ProxyMenuAction {
    match server {
        ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. } => {
            desktop_shell::ProxyMenuAction::Start
        }
        ServerLifecycle::Running { server, .. } if server.is_task_alive() => {
            desktop_shell::ProxyMenuAction::Stop
        }
        ServerLifecycle::Running { .. } => desktop_shell::ProxyMenuAction::Start,
        ServerLifecycle::Starting { .. }
        | ServerLifecycle::Applying { .. }
        | ServerLifecycle::Stopping { .. } => desktop_shell::ProxyMenuAction::None,
    }
}

fn menu_action_expectation_matches(
    expected_generation: u64,
    current_generation: u64,
    requested: desktop_shell::ProxyMenuAction,
    current: desktop_shell::ProxyMenuAction,
) -> bool {
    expected_generation == current_generation && requested == current
}

fn complete_serve_start<R: Runtime>(
    app: &AppHandle<R>,
    generation: u64,
    result: Result<PreparedServer, StartFailure>,
    applied_pricing: PriceTable,
    metrics_db: PathBuf,
) {
    // Same-port handoff must first release the old accept socket. This state
    // mutation is instant; the candidate bind/retry itself happens below,
    // outside the App mutex.
    let resume_listen = result.as_ref().ok().and_then(|prepared| {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Applying {
                generation: current,
                old,
                ..
            } if *current == generation && old.listen() == prepared.listen() => {
                old.stop_accepting();
                Some(old.listen().to_owned())
            }
            _ => None,
        }
    });
    if resume_listen.is_some() {
        // Publish the listener handoff immediately. Candidate bind retries can
        // last almost one second, so the periodic refresh alone could leave a
        // stale checked “still running” item visible for the whole outage.
        desktop_shell::update_proxy_menu(app);
    }
    let result = result.and_then(PreparedServer::bind);
    // A failed candidate bind must restore the old listener, but that retry is
    // equally forbidden under the global lock. The reserved socket is only
    // installed if the same generation is still Applying.
    let mut resume_listener = if result.is_err() {
        resume_listen.as_deref().map(PreparedServer::bind_listener)
    } else {
        None
    };
    let mut discard = None;
    let mut retire = None;
    let mut published = false;
    let mut view = {
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        inner.server = match (current, result) {
            (
                ServerLifecycle::Starting {
                    generation: current,
                    listen,
                    revision,
                },
                Ok(prepared),
            ) if current == generation => match prepared.publish(revision) {
                Ok(server) => {
                    published = true;
                    ServerLifecycle::Running {
                        generation,
                        server,
                        apply_error: None,
                    }
                }
                Err(failure) => ServerLifecycle::Failed {
                    generation,
                    listen,
                    error: failure.public_message(),
                },
            },
            (
                ServerLifecycle::Applying {
                    generation: current,
                    revision,
                    mut old,
                    ..
                },
                Ok(prepared),
            ) if current == generation => {
                let same_listener = old.listen() == prepared.listen();
                match prepared.publish(revision) {
                    Ok(server) => {
                        published = true;
                        old.stop_accepting();
                        retire = Some(old);
                        ServerLifecycle::Running {
                            generation,
                            server,
                            apply_error: None,
                        }
                    }
                    Err(failure) => {
                        let mut message = failure.public_message();
                        if same_listener {
                            let restore = resume_listener
                                .take()
                                .unwrap_or_else(|| {
                                    Err(StartFailure::new("listen_restore", "旧 listener 未能预留"))
                                })
                                .and_then(|listener| old.resume_accepting(listener));
                            if let Err(restore) = restore {
                                message = format!(
                                    "切换失败且旧 listener 恢复失败：{message}; {}",
                                    restore.public_message()
                                );
                                let listen = old.listen().to_owned();
                                retire = Some(old);
                                ServerLifecycle::Failed {
                                    generation,
                                    listen,
                                    error: message,
                                }
                            } else {
                                ServerLifecycle::Running {
                                    generation,
                                    server: old,
                                    apply_error: Some(format!("已保存尚未应用：{message}")),
                                }
                            }
                        } else {
                            ServerLifecycle::Running {
                                generation,
                                server: old,
                                apply_error: Some(format!("已保存尚未应用：{message}")),
                            }
                        }
                    }
                }
            }
            (
                ServerLifecycle::Starting {
                    generation: current,
                    listen,
                    ..
                },
                Err(failure),
            ) if current == generation => ServerLifecycle::Failed {
                generation,
                listen,
                error: failure.public_message(),
            },
            (
                ServerLifecycle::Applying {
                    generation: current,
                    old,
                    ..
                },
                Err(failure),
            ) if current == generation => ServerLifecycle::Running {
                generation,
                server: old,
                apply_error: Some(format!("已保存尚未应用：{}", failure.public_message())),
            },
            (
                ServerLifecycle::Stopping {
                    generation: current,
                    listen,
                    draining,
                },
                Ok(prepared),
            ) if current == generation => {
                discard = Some(prepared);
                if draining {
                    ServerLifecycle::Stopping {
                        generation,
                        listen,
                        draining,
                    }
                } else {
                    ServerLifecycle::Stopped { generation }
                }
            }
            (
                ServerLifecycle::Stopping {
                    generation: current,
                    listen,
                    draining,
                },
                Err(_),
            ) if current == generation => {
                if draining {
                    ServerLifecycle::Stopping {
                        generation,
                        listen,
                        draining,
                    }
                } else {
                    ServerLifecycle::Stopped { generation }
                }
            }
            (current, Ok(prepared)) => {
                discard = Some(prepared);
                current
            }
            (current, Err(_)) => current,
        };
        Some(inner.serve_view())
    };
    if published {
        let app_state = app.state::<AppStateManaged>();
        let agents = app.state::<AgentCommandState>();
        if let Ok(runtime) = runtime_from_app(app_state.inner()) {
            if let Err(error) = agents.refresh_model_metadata(None, &runtime) {
                let mut inner = app_state.0.lock().unwrap();
                if let ServerLifecycle::Running {
                    generation: current,
                    apply_error,
                    ..
                } = &mut inner.server
                {
                    if *current == generation {
                        *apply_error = Some(format!(
                            "代理已启动，但 Agent 模型元数据刷新失败：{}",
                            error.message
                        ));
                        view = Some(inner.serve_view());
                    }
                }
            }
        }
    }
    if let Some(prepared) = discard {
        prepared.discard();
    }
    if let Some(old) = retire {
        tauri::async_runtime::spawn_blocking(move || {
            old.drain_and_shutdown();
            if let Err(error) = SqliteStore::backfill_unknown_costs(&metrics_db, &applied_pricing) {
                eprintln!("post-apply historical cost backfill failed: {error}");
            }
        });
    }
    if let Some(view) = view {
        emit_serve_state(app, &view);
    }
}

fn begin_serve_start_inner<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_stopped_generation: Option<u64>,
    prepare: F,
) -> Result<Option<StateView>, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    let (config, generation, snapshot, serve_view, metrics_db) = {
        let mut inner = state.0.lock().unwrap();
        if let Some(expected) = expected_stopped_generation {
            if !menu_action_expectation_matches(
                expected,
                inner.server.generation(),
                desktop_shell::ProxyMenuAction::Start,
                lifecycle_proxy_action(&inner.server),
            ) {
                return Ok(None);
            }
        }
        inner.ensure_editable()?;
        match &inner.server {
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                return Err("apply_in_progress: 已有配置正在应用".to_owned());
            }
            ServerLifecycle::Stopping { .. } => {
                return Err(
                    "startup_cleanup_in_progress: 上一次代理正在停止，请稍后重试".to_string(),
                );
            }
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Running { .. }
            | ServerLifecycle::Failed { .. } => {}
        }
        let config = inner.materialize()?;
        let revision = inner.save_draft()?;
        let metrics_db = inner.data_dir().join("metrics.sqlite");
        let generation = inner
            .server
            .generation()
            .checked_add(1)
            .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_string())?;
        let listen = config.server.listen.clone();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        inner.server = match current {
            ServerLifecycle::Running { server: old, .. } => ServerLifecycle::Applying {
                generation,
                revision,
                old,
            },
            _ => ServerLifecycle::Starting {
                generation,
                listen,
                revision,
            },
        };
        let snapshot = inner.snapshot();
        let serve_view = snapshot.serve.clone();
        (config, generation, snapshot, serve_view, metrics_db)
    };

    emit_serve_state(&app, &serve_view);
    let completion_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let applied_pricing = config.pricing.clone();
        let result =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(config))) {
                Ok(result) => result,
                Err(_) => Err(StartFailure::new("startup_task", "后台启动任务异常退出")),
            };
        complete_serve_start(
            &completion_app,
            generation,
            result,
            applied_pricing,
            metrics_db,
        );
    });
    Ok(Some(snapshot))
}

fn begin_serve_start<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    prepare: F,
) -> Result<StateView, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    begin_serve_start_inner(app, state, None, prepare)
        .map(|snapshot| snapshot.expect("unconditional proxy start cannot be rejected as stale"))
}

#[cfg(target_os = "macos")]
fn begin_serve_start_if_generation<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_generation: u64,
    prepare: F,
) -> Result<bool, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    begin_serve_start_inner(app, state, Some(expected_generation), prepare)
        .map(|snapshot| snapshot.is_some())
}

#[tauri::command]
fn serve_start(app: AppHandle, state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    begin_serve_start(app, state.inner(), prepare_server)
}

async fn ensure_serve_running_with<R, F>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    prepare: F,
    timeout: Duration,
) -> Result<StateView, String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    enum EnsureAction {
        Ready(Box<StateView>),
        Wait {
            generation: u64,
            fail_on_apply_error: bool,
        },
        Start {
            generation: u64,
        },
    }

    let action = {
        let inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Running { server, .. }
                if server.is_task_alive() && server.listener_reachable() =>
            {
                EnsureAction::Ready(Box::new(inner.snapshot()))
            }
            ServerLifecycle::Running { server, .. } if !server.is_task_alive() => {
                return Err(
                    "ensure_serve_running_start_failed: 代理任务已退出，请先停止后重试".to_owned(),
                );
            }
            ServerLifecycle::Running { generation, .. }
            | ServerLifecycle::Starting { generation, .. } => EnsureAction::Wait {
                generation: *generation,
                fail_on_apply_error: false,
            },
            ServerLifecycle::Applying { generation, .. } => EnsureAction::Wait {
                generation: *generation,
                fail_on_apply_error: true,
            },
            ServerLifecycle::Stopped { generation }
            | ServerLifecycle::Failed { generation, .. } => EnsureAction::Start {
                generation: generation
                    .checked_add(1)
                    .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_owned())?,
            },
            ServerLifecycle::Stopping { .. } => {
                return Err("ensure_serve_running_stopping: 代理正在停止，请稍后重试".to_owned());
            }
        }
    };

    let (expected_generation, fail_on_apply_error) = match action {
        EnsureAction::Ready(view) => return Ok(*view),
        EnsureAction::Wait {
            generation,
            fail_on_apply_error,
        } => (generation, fail_on_apply_error),
        EnsureAction::Start { generation } => {
            if let Err(error) = begin_serve_start(app.clone(), state, prepare) {
                let joined_existing_start = {
                    let inner = state.0.lock().unwrap();
                    inner.server.generation() == generation
                        && matches!(
                            inner.server,
                            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
                        )
                };
                if !joined_existing_start {
                    return Err(error);
                }
            }
            (generation, false)
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        enum WaitObservation {
            Complete(Box<Result<StateView, String>>),
            Probe(String),
            Pending,
        }

        let observation = {
            let inner = state.0.lock().unwrap();
            let actual_generation = inner.server.generation();
            if actual_generation != expected_generation {
                WaitObservation::Complete(Box::new(Err(format!(
                    "ensure_serve_running_interrupted: 启动目标 generation {expected_generation} 已被 {actual_generation} 取代"
                ))))
            } else {
                match &inner.server {
                    ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                        WaitObservation::Pending
                    }
                    ServerLifecycle::Running {
                        server,
                        apply_error,
                        ..
                    } => {
                        if !server.is_task_alive() {
                            WaitObservation::Complete(Box::new(Err(
                                "ensure_serve_running_start_failed: 代理任务已退出".to_owned(),
                            )))
                        } else if fail_on_apply_error && apply_error.is_some() {
                            WaitObservation::Complete(Box::new(Err(format!(
                                "ensure_serve_running_start_failed: {}",
                                apply_error.as_deref().unwrap_or("代理应用失败")
                            ))))
                        } else {
                            WaitObservation::Probe(server.listen().to_owned())
                        }
                    }
                    ServerLifecycle::Failed { error, .. } => WaitObservation::Complete(Box::new(
                        Err(format!("ensure_serve_running_start_failed: {error}")),
                    )),
                    ServerLifecycle::Stopped { .. } => WaitObservation::Complete(Box::new(Err(
                        "ensure_serve_running_interrupted: 代理启动被停止".to_owned(),
                    ))),
                    ServerLifecycle::Stopping { .. } => WaitObservation::Complete(Box::new(Err(
                        "ensure_serve_running_stopping: 代理正在停止，请稍后重试".to_owned(),
                    ))),
                }
            }
        };

        match observation {
            WaitObservation::Complete(outcome) => return *outcome,
            WaitObservation::Probe(listen) => {
                let reachable = match listen.parse::<std::net::SocketAddr>() {
                    Ok(address) => tokio::time::timeout(
                        Duration::from_millis(200),
                        tokio::net::TcpStream::connect(address),
                    )
                    .await
                    .is_ok_and(|result| result.is_ok()),
                    Err(_) => false,
                };
                if reachable {
                    let inner = state.0.lock().unwrap();
                    if inner.server.generation() == expected_generation {
                        if let ServerLifecycle::Running {
                            server,
                            apply_error,
                            ..
                        } = &inner.server
                        {
                            if server.listen() == listen
                                && server.is_task_alive()
                                && (!fail_on_apply_error || apply_error.is_none())
                            {
                                return Ok(inner.snapshot());
                            }
                        }
                    }
                }
            }
            WaitObservation::Pending => {}
        }
        if Instant::now() >= deadline {
            return Err("ensure_serve_running_timeout: 等待代理启动并可达超时".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tauri::command]
async fn ensure_serve_running(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
) -> Result<StateView, String> {
    ensure_serve_running_with(app, state.inner(), prepare_server, Duration::from_secs(30)).await
}

fn complete_serve_stop<R: Runtime>(app: &AppHandle<R>, generation: u64) {
    let view = {
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        match &inner.server {
            ServerLifecycle::Stopping {
                generation: current,
                ..
            } if *current == generation => {
                inner.server = ServerLifecycle::Stopped { generation };
                Some(inner.serve_view())
            }
            _ => None,
        }
    };
    if let Some(view) = view {
        emit_serve_state(app, &view);
    }
}

fn begin_serve_stop_inner<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_running_generation: Option<u64>,
) -> Option<StateView> {
    let (generation, snapshot, serve_view, running) = {
        let mut inner = state.0.lock().unwrap();
        if let Some(expected) = expected_running_generation {
            if !menu_action_expectation_matches(
                expected,
                inner.server.generation(),
                desktop_shell::ProxyMenuAction::Stop,
                lifecycle_proxy_action(&inner.server),
            ) {
                return None;
            }
        }
        let generation = inner.server.generation();
        let current = std::mem::replace(&mut inner.server, ServerLifecycle::Stopped { generation });
        let mut running = None;
        let changed = match current {
            ServerLifecycle::Running { server, .. }
            | ServerLifecycle::Applying { old: server, .. } => {
                let listen = server.listen().to_string();
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining: true,
                };
                running = Some(server);
                true
            }
            ServerLifecycle::Starting { listen, .. } => {
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining: false,
                };
                true
            }
            ServerLifecycle::Stopping {
                listen, draining, ..
            } => {
                inner.server = ServerLifecycle::Stopping {
                    generation,
                    listen,
                    draining,
                };
                false
            }
            ServerLifecycle::Failed { .. } => true,
            ServerLifecycle::Stopped { .. } => false,
        };
        let snapshot = inner.snapshot();
        let serve_view = changed.then(|| snapshot.serve.clone());
        (generation, snapshot, serve_view, running)
    };

    if let Some(serve_view) = serve_view {
        emit_serve_state(&app, &serve_view);
    }
    if let Some(running) = running {
        let completion_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ =
                tauri::async_runtime::spawn_blocking(move || running.drain_and_shutdown()).await;
            complete_serve_stop(&completion_app, generation);
        });
    }
    Some(snapshot)
}

fn begin_serve_stop<R: Runtime>(app: AppHandle<R>, state: &AppStateManaged) -> StateView {
    begin_serve_stop_inner(app, state, None)
        .expect("unconditional proxy stop cannot be rejected as stale")
}

#[cfg(target_os = "macos")]
fn begin_serve_stop_if_generation<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    expected_generation: u64,
) -> Option<StateView> {
    begin_serve_stop_inner(app, state, Some(expected_generation))
}

#[tauri::command]
fn serve_stop(app: AppHandle, state: State<'_, AppStateManaged>) -> StateView {
    begin_serve_stop(app, state.inner())
}

/// Display inbound adapters from the comma-joined agents list, falling back to the single agent value.
fn agents_display(plugins: &Value) -> String {
    let list: Vec<&str> = plugins["agents"]
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if list.is_empty() {
        plugins["agent"].as_str().unwrap_or_default().to_string()
    } else {
        list.join(", ")
    }
}

// ---- Subpage commands (#5) -------------------------------------------------

fn classify_settings_error(message: String) -> SettingsCommandError {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("proxy")
        || normalized.contains("egress")
        || normalized.contains("socks")
        || normalized.contains("url")
    {
        settings_error(
            "egress_proxy_url",
            "invalid_proxy_url",
            "代理地址无效；HTTP 代理请使用 http:// 或 https://，SOCKS5 请使用 socks5:// 或 socks5h://",
        )
    } else {
        settings_error("settings", "validation_failed", message)
    }
}

/// Settings page command for server.auth and data.metrics. Persist after
/// successful materialization, matching config set; otherwise keep draft-only
/// changes until a full save. Running serve instances require a proxy restart.
#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps this stable command boundary to named frontend arguments"
)]
fn set_settings(
    state: State<'_, AppStateManaged>,
    auth: bool,
    metrics: bool,
    egress_mode: String,
    egress_proxy_url: String,
    egress_no_proxy: Vec<String>,
    egress_auth_username: String,
    egress_auth_slot: String,
) -> Result<StateView, SettingsCommandError> {
    let mut inner = state.0.lock().unwrap();
    inner
        .ensure_editable()
        .map_err(|message| settings_error("settings", "read_only", message))?;
    let previous_draft = inner.draft.clone();
    let previous_config_state = inner.config_state.clone();
    let edit_result = inner.edit_validated_draft(|candidate| {
        candidate.draft["server"]["auth"] = json!(auth);
        candidate.draft["data"]["metrics"] = json!(metrics);
        candidate.draft["egress"] = if egress_mode == "direct" {
            json!({ "mode": "direct" })
        } else {
            let mut egress = json!({
                "mode": egress_mode,
                "proxy_url": egress_proxy_url,
                "no_proxy": egress_no_proxy,
            });
            if !egress_auth_username.is_empty() || !egress_auth_slot.is_empty() {
                egress["auth"] = json!({
                    "username": egress_auth_username,
                    "credential": { "slot": egress_auth_slot, "store": true }
                });
            }
            egress
        };
        candidate.materialize()?.validate()?;
        Ok(())
    });
    if let Err(message) = edit_result {
        return Err(classify_settings_error(message));
    }
    if let Err(message) = inner.save_draft() {
        inner.draft = previous_draft;
        inner.config_state = previous_config_state;
        return Err(settings_error("settings", "save_failed", message));
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn get_egress(state: State<'_, AppStateManaged>) -> Result<Value, String> {
    let inner = state.0.lock().unwrap();
    let config = inner.materialize()?;
    let mut routes = Vec::new();
    for (upstream, entry) in &config.upstreams {
        let target = String::from(entry.base_url.clone());
        let bypassed = config.egress.bypasses_proxy(&target)?;
        let route =
            if config.egress.mode == token_station_cli::config::EgressMode::Direct || bypassed {
                "direct"
            } else {
                "proxy"
            };
        for request_class in ["provider_request", "model_catalog", "health_probe"] {
            routes.push(json!({
                "request_class": request_class,
                "upstream": upstream,
                "target": target,
                "route": route,
                "matched_no_proxy": bypassed && config.egress.mode != token_station_cli::config::EgressMode::Direct,
            }));
        }
    }
    Ok(json!({
        "mode": config.egress.mode,
        "proxy_url": config.egress.proxy_url,
        "no_proxy": config.egress.no_proxy,
        "auth_slot": config.egress.auth.map(|auth| auth.credential.slot),
        "routes": routes,
        "fixed_direct_classes": ["update_check"],
    }))
}

fn budget_statuses(inner: &AppInner) -> Result<Vec<BudgetStatus>, String> {
    let budgets: std::collections::BTreeMap<String, AgentBudget> = inner
        .draft
        .get("agent_budgets")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Agent 预算配置不合法：{error}"))?
        .unwrap_or_default();
    let db = inner.data_dir().join("metrics.sqlite");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    budgets
        .iter()
        .map(|(agent_id, budget)| {
            let aggregate = if db.exists() {
                stats::collect_range(
                    &db,
                    budget.period_start_ms,
                    budget.period_end_ms,
                    Some(stats::GroupBy::Agent),
                )?
                .groups
                .into_iter()
                .find(|(candidate, _)| candidate == agent_id)
                .map(|(_, aggregate)| aggregate)
                .unwrap_or_default()
            } else {
                stats::Aggregate::default()
            };
            let used_micros = aggregate
                .cost_micros
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or(0);
            Ok(BudgetStatus::evaluate(
                agent_id,
                budget,
                used_micros,
                aggregate.unpriced_requests,
                now_ms,
            ))
        })
        .collect()
}

#[tauri::command]
fn get_agent_budgets(state: State<'_, AppStateManaged>) -> Result<Vec<BudgetStatus>, String> {
    budget_statuses(&state.0.lock().unwrap())
}

#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps this stable command boundary to named form fields"
)]
fn set_agent_budget(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    limit_micros: u64,
    warning_percent: u8,
    period_start_ms: Option<u64>,
    period_end_ms: Option<u64>,
    expiry_warning_days: u16,
) -> Result<Vec<BudgetStatus>, String> {
    ensure_known_agent_id(&agent_id)?;
    let budget = AgentBudget {
        limit_micros,
        warning_percent,
        period_start_ms,
        period_end_ms,
        expiry_warning_days,
    };
    budget.validate()?;
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if !inner.draft["agent_budgets"].is_object() {
        inner.draft["agent_budgets"] = json!({});
    }
    inner.draft["agent_budgets"][&agent_id] =
        serde_json::to_value(budget).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    budget_statuses(&inner)
}

#[tauri::command]
fn remove_agent_budget(
    state: State<'_, AppStateManaged>,
    agent_id: String,
) -> Result<Vec<BudgetStatus>, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let budgets = inner.draft["agent_budgets"]
        .as_object_mut()
        .ok_or_else(|| format!("Agent `{agent_id}` 尚未配置预算"))?;
    if budgets.remove(&agent_id).is_none() {
        return Err(format!("Agent `{agent_id}` 尚未配置预算"));
    }
    inner.observe_draft()?;
    inner.save_draft()?;
    budget_statuses(&inner)
}

fn draft_price_table(inner: &AppInner) -> Result<PriceTable, String> {
    inner
        .draft
        .get("pricing")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("定价表配置不合法：{error}"))
        .map(Option::unwrap_or_default)
}

/// Remove catalog-derived prices bound to one Provider identity in a single
/// table revision. A Base URL or credential change may select another account;
/// carrying the old account's scoped prices across that boundary would make
/// both settlement and Agent `Spent` metadata factually wrong.
fn clear_provider_scoped_prices(inner: &mut AppInner, name: &str) -> Result<bool, String> {
    let mut pricing = draft_price_table(inner)?;
    let prefix = format!("{name}/");
    let before = pricing.models.len();
    pricing
        .models
        .retain(|model, _| !model.starts_with(&prefix));
    if pricing.models.len() == before {
        return Ok(false);
    }
    pricing.version = pricing
        .version
        .checked_add(1)
        .ok_or_else(|| "pricing version exhausted; start a new configuration".to_owned())?;
    inner.draft["pricing"] = serde_json::to_value(pricing).map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
fn get_price_table(state: State<'_, AppStateManaged>) -> Result<PriceTable, String> {
    draft_price_table(&state.0.lock().unwrap())
}

#[tauri::command]
async fn list_public_provider_models(
    state: State<'_, AppStateManaged>,
    provider_ids: Vec<String>,
) -> Result<PublicProviderModelsView, String> {
    if provider_ids.is_empty() || provider_ids.len() > 128 {
        return Err("Request 1 to 128 Provider IDs.".to_owned());
    }
    let mut requested = BTreeSet::new();
    for provider_id in provider_ids {
        let provider_id = provider_id.trim().to_owned();
        if provider_id.is_empty()
            || provider_id.len() > 128
            || provider_id.chars().any(char::is_control)
        {
            return Err(
                "Each Provider ID must be 1 to 128 bytes and contain no control characters."
                    .to_owned(),
            );
        }
        requested.insert(provider_id);
    }
    let requested: Vec<String> = requested.into_iter().collect();
    let (data_dir, egress, egress_secrets) = {
        let inner = state.0.lock().unwrap();
        let egress = draft_egress_config(&inner.draft)
            .map_err(|error| format!("The egress configuration is invalid: {error}"))?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::list_public_provider_models_with_cache_egress(
            &data_dir,
            &requested,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("The public model catalog task ended unexpectedly: {error}"))?
}

#[tauri::command]
async fn suggest_model_price(
    state: State<'_, AppStateManaged>,
    provider_id: Option<String>,
    model_id: String,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let model_id = model_id.trim().to_owned();
    if model_id.is_empty() || model_id.len() > 256 {
        return Err("模型 ID 必须是 1–256 个字符".to_owned());
    }
    let provider_id = provider_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let (data_dir, egress, egress_secrets) = {
        let inner = state.0.lock().unwrap();
        let egress = draft_egress_config(&inner.draft)?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::suggest_with_cache_egress(
            &data_dir,
            provider_id.as_deref(),
            &model_id,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("公开价格目录任务异常结束：{error}"))?
}

fn configured_upstream_models(inner: &AppInner, name: &str) -> Result<BTreeSet<String>, String> {
    let upstream = inner.draft["upstreams"]
        .get(name)
        .ok_or_else(|| format!("Provider `{name}` is not configured"))?;
    upstream["models"]
        .as_array()
        .ok_or_else(|| format!("Provider `{name}` has invalid model configuration"))?
        .iter()
        .map(|capability| {
            capability["model"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("Provider `{name}` has an invalid model entry"))
        })
        .collect()
}

fn configured_public_price_provider_id(inner: &AppInner, name: &str) -> Result<String, String> {
    let upstream = inner.draft["upstreams"]
        .get(name)
        .ok_or_else(|| format!("Provider `{name}` is not configured"))?;
    if upstream["provider"].as_str() != Some("openai-compatible") {
        return Err(format!(
            "Provider `{name}` does not support automatic public price import"
        ));
    }
    let base_url = upstream["base_url"]
        .as_str()
        .ok_or_else(|| format!("Provider `{name}` has no valid Base URL"))?;
    let endpoint = ProviderEndpoint::try_new(base_url).map_err(|error| error.to_string())?;
    if endpoint.is_loopback() {
        return Err("Local Provider prices must be configured explicitly".to_owned());
    }
    let access_tier = upstream["access_tier"].as_str().unwrap_or_default();
    let brand_id = provider_brand_id(name, &endpoint.as_str(), access_tier).ok_or_else(|| {
        format!("Provider `{name}` has no authoritative public price catalog mapping")
    })?;
    Ok(pricing_catalog::normalize_provider_id(brand_id))
}

#[derive(Clone, Debug)]
struct PriceImportTarget {
    upstream: Value,
    upstream_epoch: u64,
    price_version: u32,
}

fn capture_price_import_target(
    inner: &AppInner,
    upstream_name: &str,
) -> Result<PriceImportTarget, String> {
    let upstream = inner.draft["upstreams"]
        .get(upstream_name)
        .cloned()
        .ok_or_else(|| format!("Provider `{upstream_name}` is not configured"))?;
    Ok(PriceImportTarget {
        upstream,
        upstream_epoch: inner
            .upstream_epochs
            .get(upstream_name)
            .copied()
            .unwrap_or_default(),
        price_version: draft_price_table(inner)?.version,
    })
}

fn ensure_price_import_target_unchanged(
    inner: &AppInner,
    upstream_name: &str,
    expected: &PriceImportTarget,
) -> Result<(), String> {
    let current = capture_price_import_target(inner, upstream_name)?;
    if current.upstream != expected.upstream
        || current.upstream_epoch != expected.upstream_epoch
        || current.price_version != expected.price_version
    {
        return Err(format!(
            "Provider `{upstream_name}` or its price table changed while public prices were loading. Retry the import"
        ));
    }
    Ok(())
}

fn ensure_automatic_price_suggestions_fresh(
    suggestions: &[RequestedModelPriceSuggestion],
) -> Result<(), String> {
    if suggestions
        .iter()
        .any(|value| value.suggestion.catalog_source != "live")
    {
        return Err(
            "Automatic price import requires a live public catalog; cached prices are advisory only"
                .to_owned(),
        );
    }
    Ok(())
}

fn apply_public_model_prices(
    inner: &mut AppInner,
    upstream_name: &str,
    requested_models: &BTreeSet<String>,
    suggestions: Vec<RequestedModelPriceSuggestion>,
) -> Result<(usize, usize, Vec<String>, u32), String> {
    let configured = configured_upstream_models(inner, upstream_name)?;
    if let Some(model) = requested_models
        .iter()
        .find(|model| !configured.contains(*model))
    {
        return Err(format!(
            "Model `{model}` is not configured for Provider `{upstream_name}`"
        ));
    }

    let current = draft_price_table(inner)?;
    let suggestions: BTreeMap<String, ModelPriceSuggestionView> = suggestions
        .into_iter()
        .map(|value| (value.requested_model_id, value.suggestion))
        .collect();
    let mut additions = BTreeMap::new();
    let mut existing = 0;
    let mut missing_model_ids = Vec::new();
    for model in requested_models {
        let scoped_model = format!("{upstream_name}/{model}");
        if current.models.contains_key(&scoped_model) {
            existing += 1;
            continue;
        }
        let Some(suggestion) = suggestions.get(model) else {
            missing_model_ids.push(model.clone());
            continue;
        };
        additions.insert(
            scoped_model,
            ModelPrice {
                input_per_mtok: suggestion.input_per_mtok,
                output_per_mtok: suggestion.output_per_mtok,
                cache_read_per_mtok: suggestion.cache_read_per_mtok,
                cache_write_per_mtok: suggestion.cache_write_per_mtok,
                reasoning_per_mtok: suggestion.reasoning_per_mtok,
            },
        );
    }

    let imported = additions.len();
    if additions.is_empty() {
        return Ok((0, existing, missing_model_ids, current.version));
    }
    let next = current.next_with_models(additions)?;
    let next_version = next.version;
    let previous_pricing = inner.draft["pricing"].clone();
    let previous_state = inner.config_state.clone();
    inner.draft["pricing"] = serde_json::to_value(next).map_err(|error| error.to_string())?;
    if let Err(error) = inner.observe_draft() {
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(error);
    }
    Ok((imported, existing, missing_model_ids, next_version))
}

#[tauri::command]
async fn import_model_prices_for_provider(
    state: State<'_, AppStateManaged>,
    upstream_name: String,
    model_ids: Vec<String>,
) -> Result<ModelPriceImportResultView, String> {
    let upstream_name = upstream_name.trim().to_owned();
    if upstream_name.is_empty() {
        return Err("Provider name is required for a price import".to_owned());
    }
    let mut requested_models = BTreeSet::new();
    for model in model_ids {
        let model = model.trim().to_owned();
        if model.is_empty() || model.len() > 512 || model.chars().any(char::is_control) {
            return Err(
                "Model IDs must be 1-512 bytes and contain no control characters".to_owned(),
            );
        }
        requested_models.insert(model);
    }
    if requested_models.is_empty() || requested_models.len() > 512 {
        return Err("A price import must contain 1-512 unique models".to_owned());
    }

    let (data_dir, egress, egress_secrets, expected_target, catalog_provider_id) = {
        let inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        let configured = configured_upstream_models(&inner, &upstream_name)?;
        let catalog_provider_id = configured_public_price_provider_id(&inner, &upstream_name)?;
        if let Some(model) = requested_models
            .iter()
            .find(|model| !configured.contains(*model))
        {
            return Err(format!(
                "Model `{model}` is not configured for Provider `{upstream_name}`"
            ));
        }
        let egress = draft_egress_config(&inner.draft)?;
        let expected_target = capture_price_import_target(&inner, &upstream_name)?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
            expected_target,
            catalog_provider_id,
        )
    };
    let requested_for_catalog: Vec<String> = requested_models.iter().cloned().collect();
    let suggestions = tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::suggest_many_live_with_egress(
            &data_dir,
            Some(&catalog_provider_id),
            &requested_for_catalog,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("Public price catalog task ended unexpectedly: {error}"))??;
    ensure_automatic_price_suggestions_fresh(&suggestions)?;

    let mut inner = state.0.lock().unwrap();
    ensure_price_import_target_unchanged(&inner, &upstream_name, &expected_target)?;
    let (imported, existing, missing_model_ids, price_version) =
        apply_public_model_prices(&mut inner, &upstream_name, &requested_models, suggestions)?;
    Ok(ModelPriceImportResultView {
        state: inner.snapshot(),
        imported,
        existing,
        missing_model_ids,
        price_version,
    })
}

#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps the five price classes and expected version to named form fields"
)]
fn set_model_price(
    state: State<'_, AppStateManaged>,
    model: String,
    input_per_mtok: u64,
    output_per_mtok: u64,
    cache_read_per_mtok: u64,
    cache_write_per_mtok: u64,
    reasoning_per_mtok: Option<u64>,
    expected_version: u32,
) -> Result<PriceTable, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let current = draft_price_table(&inner)?;
    if current.version != expected_version {
        return Err(format!(
            "定价表版本冲突：当前为 v{}，页面基于 v{expected_version}；请刷新后重试",
            current.version
        ));
    }
    let next = current.next_with_model(
        &model,
        ModelPrice {
            input_per_mtok,
            output_per_mtok,
            cache_read_per_mtok,
            cache_write_per_mtok,
            reasoning_per_mtok,
        },
    )?;
    inner.draft["pricing"] = serde_json::to_value(&next).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    Ok(next)
}

#[tauri::command]
fn remove_model_price(
    state: State<'_, AppStateManaged>,
    model: String,
    expected_version: u32,
) -> Result<PriceTable, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let current = draft_price_table(&inner)?;
    if current.version != expected_version {
        return Err(format!(
            "定价表版本冲突：当前为 v{}，页面基于 v{expected_version}；请刷新后重试",
            current.version
        ));
    }
    let next = current.next_without_model(&model)?;
    inner.draft["pricing"] = serde_json::to_value(&next).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    Ok(next)
}

/// Read-only usage aggregation. since accepts all, hours, or days; by accepts
/// agent, upstream, model, pool, status, hour, day, or empty. Return empty=true
/// rather than an error when the metrics database does not exist.
#[tauri::command]
fn get_stats(
    state: State<'_, AppStateManaged>,
    since: String,
    by: Option<String>,
    agent_id: Option<String>,
    source: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StatsView, String> {
    let db = {
        let inner = state.0.lock().unwrap();
        inner.data_dir().join("metrics.sqlite")
    };
    if !db.exists() {
        return Ok(StatsView {
            total: AggView::zero(),
            groups: vec![],
            by,
            empty: true,
        });
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        });
    let cutoff = stats::cutoff_from_since(&since, now_ms)?;
    let group = match by.as_deref() {
        None | Some("") => None,
        Some("agent") => Some(stats::GroupBy::Agent),
        Some("upstream") => Some(stats::GroupBy::Upstream),
        Some("model") => Some(stats::GroupBy::Model),
        Some("pool") => Some(stats::GroupBy::Pool),
        Some("status") => Some(stats::GroupBy::Status),
        Some("hour") => Some(stats::GroupBy::Hour),
        Some("day") => Some(stats::GroupBy::Day),
        Some("engine") => Some(stats::GroupBy::Engine),
        Some("fallback") => Some(stats::GroupBy::Fallback),
        Some(other) => return Err(format!("未知分组 `{other}`")),
    };
    let report = stats::collect_filtered(
        &db,
        cutoff,
        None,
        group,
        stats::StatsFilter {
            agent_id: agent_id.as_deref(),
            source: source.as_deref(),
            upstream: upstream.as_deref(),
            model: model.as_deref(),
        },
    )?;
    Ok(StatsView {
        total: AggView::from(&report.total),
        groups: report
            .groups
            .iter()
            .map(|(k, a)| (k.clone(), AggView::from(a)))
            .collect(),
        by,
        empty: false,
    })
}

/// Return the five most recent body-free Request Receipts for the home page.
/// Return an empty array if the metrics database does not exist, including when
/// metrics are disabled. The read layer enforces the five-item limit.
#[tauri::command]
fn get_recent_receipts(
    state: State<'_, AppStateManaged>,
    limit: usize,
) -> Result<Vec<ReceiptView>, String> {
    let db = {
        let inner = state.0.lock().unwrap();
        inner.data_dir().join("metrics.sqlite")
    };
    SqliteStore::recent_receipts(&db, limit)
}

/// Read the complete body-free Request Receipt ledger with pagination for the usage page.
#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps the dashboard filters to named command fields"
)]
fn get_request_receipts(
    state: State<'_, AppStateManaged>,
    since: String,
    agent_id: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
    status: Option<String>,
    page: usize,
    page_size: usize,
) -> Result<ReceiptPageView, String> {
    let (data_dir, now_ms) = {
        let inner = state.0.lock().unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        (inner.data_dir(), now_ms)
    };
    let since_ms = stats::cutoff_from_since(&since, now_ms)?;
    let bounded_page_size = page_size.clamp(1, 50);
    let bounded_page = page.max(1);
    let offset = bounded_page
        .saturating_sub(1)
        .saturating_mul(bounded_page_size);
    let result = SqliteStore::receipt_page(
        &data_dir.join("metrics.sqlite"),
        &ReceiptQuery {
            since_ms,
            agent_id,
            upstream,
            model,
            status,
        },
        bounded_page_size,
        offset,
    )?;
    let mut plaintext_by_request_id = BTreeMap::new();
    let mut plaintext_errors_by_request_id = BTreeMap::new();
    match BodyLog::open(&data_dir) {
        Ok(body_log) => {
            for receipt in &result.items {
                if !valid_request_id(&receipt.request_id) {
                    continue;
                }
                match body_log.read(&receipt.request_id) {
                    Ok(Some(exchange)) => {
                        plaintext_by_request_id.insert(receipt.request_id.clone(), exchange);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        plaintext_errors_by_request_id.insert(receipt.request_id.clone(), error);
                    }
                }
            }
        }
        Err(error) => {
            for receipt in &result.items {
                if valid_request_id(&receipt.request_id) {
                    plaintext_errors_by_request_id
                        .insert(receipt.request_id.clone(), error.clone());
                }
            }
        }
    }
    Ok(ReceiptPageView {
        items: result.items,
        plaintext_by_request_id,
        plaintext_errors_by_request_id,
        total: result.total,
        page: bounded_page,
        page_size: bounded_page_size,
    })
}

/// Convert the draft's rules, hints, heuristic tiers, and fallback into a read-only routing-table view with no API calls.
#[tauri::command]
fn get_router_table(state: State<'_, AppStateManaged>) -> RouterTableView {
    let inner = state.0.lock().unwrap();
    let r = &inner.draft["router"];

    let rules = r["rules"].as_array().cloned().unwrap_or_default();
    let hint_routes = r["hint_routes"].as_array().cloned().unwrap_or_default();
    let threshold = r["heuristic"]["threshold"].as_u64().map(|v| v as u32);

    let bands = r["heuristic"]["bands"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|b| {
                    let pool = b["pool"].as_str().unwrap_or_default().to_string();
                    let (upstream, model) = inner.pool_member(&pool);
                    BandView {
                        at_least: b["at_least"].as_u64().unwrap_or(0) as u32,
                        pool,
                        upstream,
                        model,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let pools = r["pools"]
        .as_object()
        .map(|obj| {
            obj.keys()
                .map(|pool| {
                    let (upstream, model) = inner.pool_member(pool);
                    PoolView {
                        pool: pool.clone(),
                        upstream,
                        model,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    RouterTableView {
        default_pool: r["default_pool"].as_str().unwrap_or_default().to_string(),
        assumed_context_window: r["assumed_context_window"].as_u64().unwrap_or(0),
        threshold,
        rules,
        hint_routes,
        bands,
        pools,
    }
}

/// Discover the plugin directory and reuse core render_list() for a monospace
/// listing shared with CLI `plugin list`. This works even with incomplete tiers.
#[tauri::command]
fn get_plugins(state: State<'_, AppStateManaged>) -> Result<PluginsView, String> {
    let (plugins_cfg, data_dir) = {
        let inner = state.0.lock().unwrap();
        let cfg: PluginsConfig = serde_json::from_value(inner.draft["plugins"].clone())
            .map_err(|e| format!("plugins 配置不合法: {e}"))?;
        (cfg, inner.data_dir())
    };
    let receipts = Receipts::load(&data_dir)?;
    let registry = PluginRegistry::discover(&plugins_cfg, &receipts)?;
    let approved = south_approved_dialects(&registry);
    {
        let mut inner = state.0.lock().unwrap();
        let current_plugins_dir = inner.draft["plugins"]["dir"].as_str().map(Path::new);
        if current_plugins_dir == Some(plugins_cfg.dir.as_path()) && inner.data_dir() == data_dir {
            inner.south_approved_dialects = approved;
        }
    }
    Ok(PluginsView {
        dir: plugins_cfg.dir.display().to_string(),
        agent: plugins_cfg.effective_agents().join(", "),
        dialects: registry
            .provider_dialects()
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        listing: registry.render_list(),
    })
}

fn desktop_updater<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoint = official_update_manifest_endpoint()?
        .parse()
        .map_err(|error| format!("更新地址无效：{error}"))?;
    app.updater_builder()
        .pubkey(OFFICIAL_PUBLIC_KEY)
        .endpoints(vec![endpoint])
        .map_err(|error| format!("更新地址配置失败：{error}"))?
        .build()
        .map_err(|error| format!("更新器初始化失败：{error}"))
}

#[cfg(target_os = "windows")]
fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    Some(desktop_update::WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
}

#[cfg(target_os = "macos")]
fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    Some(desktop_update::MACOS_ONLY_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
}

/// Check the signed desktop update channel without changing the installed app.
#[tauri::command]
async fn check_desktop_update(
    app: AppHandle,
    operation: State<'_, DesktopUpdateOperation>,
) -> Result<DesktopUpdateView, String> {
    let current = app.package_info().version.to_string();
    let _lease = operation.try_begin()?;
    if let Some(message) = desktop_update_platform_unsupported_message() {
        return Ok(DesktopUpdateView::unsupported(&current, message));
    }

    Ok(
        desktop_update::check_with(&current, OFFICIAL_PUBLIC_KEY, || async {
            let update = desktop_updater(&app)?
                .check()
                .await
                .map_err(|error| format!("暂时无法检查更新，请稍后重试：{error}"))?;
            Ok(update.map(|update| DesktopUpdateCandidate {
                version: update.version.to_string(),
                notes: update.body,
                pub_date: update.date.map(|date| date.to_string()),
            }))
        })
        .await,
    )
}

async fn prepare_gateway_for_desktop_update(
    app: AppHandle,
) -> Result<bool, desktop_update::DesktopUpdatePrepareFailure<bool>> {
    let Some(state) = app.try_state::<AppStateManaged>() else {
        return Ok(false);
    };
    let was_active = {
        let inner = state.0.lock().unwrap();
        matches!(
            inner.server,
            ServerLifecycle::Starting { .. }
                | ServerLifecycle::Applying { .. }
                | ServerLifecycle::Running { .. }
        )
    };
    begin_serve_stop(app.clone(), state.inner());

    let wait_result = tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let stopped = {
                let state = app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                matches!(
                    inner.server,
                    ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. }
                )
            };
            if stopped {
                return Ok(was_active);
            }
            if Instant::now() >= deadline {
                return Err(desktop_update::DesktopUpdatePrepareFailure::new(
                    "update_gateway_stop_timeout: 等待本地网关安全停止超时",
                    was_active,
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await;
    match wait_result {
        Ok(result) => result,
        Err(error) => Err(desktop_update::DesktopUpdatePrepareFailure::new(
            format!("等待本地网关停止任务失败：{error}"),
            was_active,
        )),
    }
}

async fn restore_gateway_after_failed_update(
    app: AppHandle,
    was_active: bool,
) -> Result<(), String> {
    restore_gateway_after_failed_update_with(app, was_active, prepare_server).await
}

async fn restore_gateway_after_failed_update_with<R, F>(
    app: AppHandle<R>,
    was_active: bool,
    prepare: F,
) -> Result<(), String>
where
    R: Runtime,
    F: FnOnce(ClientConfig) -> Result<PreparedServer, StartFailure> + Send + 'static,
{
    if !was_active {
        return Ok(());
    }
    if app.try_state::<AppStateManaged>().is_none() {
        return Ok(());
    }
    let wait_app = app.clone();
    let needs_start = tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let action = {
                let state = wait_app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                match inner.server {
                    ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. } => Some(true),
                    ServerLifecycle::Starting { .. }
                    | ServerLifecycle::Applying { .. }
                    | ServerLifecycle::Running { .. } => Some(false),
                    ServerLifecycle::Stopping { .. } => None,
                }
            };
            if let Some(needs_start) = action {
                return Ok(needs_start);
            }
            if Instant::now() >= deadline {
                return Err(
                    "update_gateway_restore_timeout: 等待本地网关停止完成后恢复超时".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .map_err(|error| format!("等待本地网关恢复任务失败：{error}"))??;
    if !needs_start {
        return Ok(());
    }
    let state = app.state::<AppStateManaged>();
    let expected_generation = {
        let inner = state.0.lock().unwrap();
        inner
            .server
            .generation()
            .checked_add(1)
            .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_owned())?
    };
    begin_serve_start(app.clone(), state.inner(), prepare)?;

    let wait_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let outcome = {
                let state = wait_app.state::<AppStateManaged>();
                let inner = state.0.lock().unwrap();
                let actual_generation = inner.server.generation();
                if actual_generation != expected_generation {
                    Some(Err(format!(
                        "update_gateway_restore_interrupted: 恢复目标 generation {expected_generation} 已被 {actual_generation} 取代"
                    )))
                } else {
                    match &inner.server {
                        ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => None,
                        ServerLifecycle::Running { server, .. } => {
                            if !server.is_task_alive() {
                                Some(Err(
                                    "update_gateway_restore_start_failed: 恢复后的代理任务已退出"
                                        .to_owned(),
                                ))
                            } else if server.listener_reachable() {
                                Some(Ok(()))
                            } else {
                                None
                            }
                        }
                        ServerLifecycle::Failed { error, .. } => Some(Err(format!(
                            "update_gateway_restore_start_failed: {error}"
                        ))),
                        ServerLifecycle::Stopped { .. } | ServerLifecycle::Stopping { .. } => {
                            Some(Err(
                                "update_gateway_restore_interrupted: 恢复启动被停止".to_owned(),
                            ))
                        }
                    }
                }
            };
            if let Some(outcome) = outcome {
                return outcome;
            }
            if Instant::now() >= deadline {
                return Err(
                    "update_gateway_restore_start_timeout: 等待本地网关恢复运行超时".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .map_err(|error| format!("等待本地网关恢复启动任务失败：{error}"))?
}

#[tauri::command]
async fn install_desktop_update_and_restart(
    app: AppHandle,
    operation: State<'_, DesktopUpdateOperation>,
    expected_version: String,
) -> Result<bool, String> {
    let _lease = operation.try_begin()?;
    if let Some(message) = desktop_update_platform_unsupported_message() {
        return Err(message.to_owned());
    }
    if OFFICIAL_PUBLIC_KEY.trim().is_empty() {
        return Err("当前构建没有内置官方更新公钥，不能在 App 内安装更新。".to_owned());
    }

    let updater = desktop_updater(&app)?;
    let progress_app = app.clone();
    let prepare_app = app.clone();
    let recover_app = app.clone();
    let installed = desktop_update::install_with(
        OFFICIAL_PUBLIC_KEY,
        &expected_version,
        || async move {
            updater
                .check()
                .await
                .map_err(|error| format!("暂时无法检查更新，请稍后重试：{error}"))
                .map(|update| update.map(|update| (update.version.to_string(), update)))
        },
        move |update| async move {
            let mut downloaded = 0_u64;
            let bytes = update
                .download(
                    move |chunk_length, total| {
                        downloaded = downloaded.saturating_add(chunk_length as u64);
                        let _ = progress_app
                            .emit(PROGRESS_EVENT, DesktopUpdateProgress { downloaded, total });
                    },
                    || {},
                )
                .await
                .map_err(|error| format!("更新包下载或签名校验失败，当前版本未被替换：{error}"))?;
            Ok((update, bytes))
        },
        move || prepare_gateway_for_desktop_update(prepare_app),
        |update, bytes, _was_active| {
            update
                .install(bytes)
                .map_err(|error| format!("更新安装失败，当前版本未被替换：{error}"))
        },
        move |was_active| restore_gateway_after_failed_update(recover_app, was_active),
    )
    .await?;
    if !installed {
        return Ok(false);
    }

    #[cfg(target_os = "windows")]
    return Ok(true);

    #[cfg(not(target_os = "windows"))]
    app.restart()
}

/// Minimal recovery control plane. These commands depend only on application
/// paths and the filesystem; they never require the business metrics DB to
/// open successfully.
#[tauri::command]
fn get_recovery_state(paths: State<'_, DesktopPaths>) -> RecoveryState {
    recovery::inspect_recovery_state(&paths.data_dir)
}

#[tauri::command]
fn get_recovery_diagnostics(paths: State<'_, DesktopPaths>) -> Result<DiagnosticPreview, String> {
    recovery::diagnostic_preview(&paths.config_file, &paths.data_dir)
}

#[tauri::command]
fn record_frontend_diagnostic(
    paths: State<'_, DesktopPaths>,
    event: FrontendDiagnosticInput,
) -> Result<FrontendDiagnosticRecord, String> {
    recovery::append_frontend_event(&recovery::diagnostic_log_path(&paths.data_dir), event)
}

#[tauri::command]
fn export_recovery_bundle(
    paths: State<'_, DesktopPaths>,
    confirmed: bool,
) -> Result<String, String> {
    recovery::export_bundle(&paths.config_file, &paths.data_dir, confirmed)
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn open_recovery_folder(paths: State<'_, DesktopPaths>) -> Result<String, String> {
    std::fs::create_dir_all(&paths.data_dir)
        .map_err(|error| format!("{}: {error}", paths.data_dir.display()))?;
    tauri_plugin_opener::open_path(&paths.data_dir, None::<&str>)
        .map_err(|error| format!("打开自救目录失败：{error}"))?;
    Ok(paths.data_dir.display().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(desktop_shell::handle_window_event)
        .setup(|app| {
            let desktop_paths = DesktopPaths::from_app_roots(
                app.path().app_config_dir()?,
                app.path().app_data_dir()?,
            );
            desktop_paths.create_writable_dirs().map_err(|error| {
                std::io::Error::other(format!(
                    "初始化桌面应用目录失败（配置：{}，数据：{}，插件：{}）：{error}",
                    desktop_paths.config_file.display(),
                    desktop_paths.data_dir.display(),
                    desktop_paths.plugins_dir.display()
                ))
            })?;

            // The recovery control plane is available before any business
            // state. In safe mode we intentionally do not manage AppState or
            // Agent command state, so normal read/write IPC cannot be invoked
            // behind the recovery shell.
            app.manage(desktop_paths.clone());
            app.manage(DesktopUpdateOperation::default());
            if recovery::inspect_recovery_state(&desktop_paths.data_dir).mode == RecoveryMode::Safe
            {
                #[cfg(target_os = "macos")]
                desktop_shell::install(
                    app.handle(),
                    desktop_shell::ProxyMenuMode::RecoverySafe,
                    |_app| {
                        desktop_shell::ProxyMenuSnapshot::new(
                            0,
                            desktop_shell::ProxyMenuPhase::Stopped,
                            "不可用（恢复安全模式不读取配置）",
                        )
                    },
                    |_app| desktop_shell::AgentMenuSnapshot::default(),
                    |_app, _action, _expected_generation| {},
                )?;
                return Ok(());
            }

            // Reuse existing config after complete CLI validation and defaulting.
            // Damaged config enters read-only protection and is never silently
            // replaced by an empty template. Upgrade legacy single-OpenAI inbound
            // config only in memory.
            let (draft, saved, load_error) = load_draft_state(
                &desktop_paths.config_file,
                &desktop_paths.data_dir,
                &desktop_paths.plugins_dir,
            );
            let mut inner = AppInner::new_with_saved(
                desktop_paths.config_file.clone(),
                draft,
                saved,
                load_error,
            );
            if inner.load_error.is_none()
                && seed_builtin_pricing(&mut inner.draft).map_err(std::io::Error::other)?
            {
                inner.observe_draft().map_err(std::io::Error::other)?;
                inner.save_draft().map_err(std::io::Error::other)?;
            }
            let pricing = draft_price_table(&inner).map_err(std::io::Error::other)?;
            if let Err(error) = SqliteStore::backfill_unknown_costs(
                &desktop_paths.data_dir.join("metrics.sqlite"),
                &pricing,
            ) {
                eprintln!("历史未知成本回填失败：{error}");
            }
            #[cfg(target_os = "macos")]
            let read_only = inner.load_error.is_some();
            app.manage(AppStateManaged(Mutex::new(inner)));
            app.manage(ModelTestStreamState::default());

            // Agent command state must exist before the native menu is built so
            // its initial snapshot and every later refresh share one authority.
            let paths = AgentIntegrationPaths {
                snapshot_root: desktop_paths.agent_data_root.join("snapshots"),
                ownership_root: desktop_paths.agent_data_root.join("ownership"),
            };
            let agent_commands = AgentCommandState::new(paths.clone()).map_err(|message| {
                std::io::Error::other(format!("初始化 Agent IPC 失败：{message}"))
            })?;
            app.manage(paths);
            app.manage(agent_commands);
            app.manage(CursorTunnelState::default());
            #[cfg(target_os = "macos")]
            desktop_shell::install(
                app.handle(),
                if read_only {
                    desktop_shell::ProxyMenuMode::ConfigReadOnly
                } else {
                    desktop_shell::ProxyMenuMode::Normal
                },
                |app| {
                    let state = app.state::<AppStateManaged>();
                    let inner = state.0.lock().unwrap();
                    desktop_shell_snapshot(&inner)
                },
                |app| {
                    let Some(agents) = app.try_state::<AgentCommandState>() else {
                        return desktop_shell::AgentMenuSnapshot::default();
                    };
                    desktop_shell::agent_menu_snapshot(
                        agents.managed_agent_menu_entries().into_iter().map(
                            |(agent_id, display_name, order)| {
                                desktop_shell::AgentMenuEntry::new(agent_id, display_name, order)
                            },
                        ),
                    )
                },
                |app, action, expected_generation| {
                    let Some(state) = app.try_state::<AppStateManaged>() else {
                        return;
                    };
                    match action {
                        desktop_shell::ProxyMenuAction::Start => {
                            match begin_serve_start_if_generation(
                                app.clone(),
                                state.inner(),
                                expected_generation,
                                prepare_server,
                            ) {
                                Ok(true) => {}
                                Ok(false) => desktop_shell::update_proxy_menu(app),
                                Err(error) => {
                                    publish_status_menu_start_error(app, state.inner(), error);
                                }
                            }
                        }
                        desktop_shell::ProxyMenuAction::Stop => {
                            if begin_serve_stop_if_generation(
                                app.clone(),
                                state.inner(),
                                expected_generation,
                            )
                            .is_none()
                            {
                                desktop_shell::update_proxy_menu(app);
                            }
                        }
                        desktop_shell::ProxyMenuAction::None => {}
                    }
                },
            )?;

            #[cfg(target_os = "macos")]
            {
                // Native state must remain truthful even when the WebView is
                // hidden or its JavaScript timers are throttled. Each request
                // is resolved against the authoritative supervisor only when
                // its main-thread refresh executes.
                let status_app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        if status_app.try_state::<AppStateManaged>().is_none() {
                            break;
                        }
                        desktop_shell::update_proxy_menu(&status_app);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_dock_theme_icon,
            get_state,
            get_runtime_state,
            preview_provider_endpoints,
            list_free_provider_presets,
            add_free_provider,
            add_provider,
            add_provider_with_credential,
            add_managed_enterprise_route,
            set_local_routing,
            set_routing_mode,
            set_direct_route,
            set_quota_accounts,
            set_quota_plan,
            get_quota_snapshot,
            edit_provider,
            edit_provider_with_credential,
            discover_provider_models,
            verify_enterprise_route,
            test_provider,
            test_model_chat_stream,
            cancel_model_test_chat,
            set_provider_model_vision,
            set_provider_model_limits,
            update_provider_models,
            preview_provider_removal,
            remove_provider,
            restore_provider,
            set_tier,
            add_keyword,
            remove_keyword,
            set_agent_route_mode,
            set_agent_tier,
            save_home_route_as_profile,
            mount_agent_profile,
            delete_profile,
            save_config,
            save_agent_routes,
            restart_agent_route,
            apply_home_route_to_all_agents,
            serve_start,
            ensure_serve_running,
            serve_stop,
            list_agent_registry,
            scan_agents,
            get_cached_agent_views,
            plan_agent_connection,
            get_cursor_provider_status,
            configure_cursor_provider,
            restore_cursor_provider,
            apply_agent_plan,
            plan_agent_disconnect,
            force_forget_agent,
            list_agent_snapshots,
            get_agent_drift,
            plan_snapshot_restore,
            apply_snapshot_restore,
            set_settings,
            get_egress,
            get_stats,
            get_agent_budgets,
            set_agent_budget,
            remove_agent_budget,
            get_price_table,
            list_public_provider_models,
            suggest_model_price,
            import_model_prices_for_provider,
            set_model_price,
            remove_model_price,
            get_recent_receipts,
            get_request_receipts,
            get_router_table,
            get_plugins,
            check_desktop_update,
            install_desktop_update_and_restart,
            get_recovery_state,
            get_recovery_diagnostics,
            record_frontend_diagnostic,
            export_recovery_bundle,
            open_recovery_folder,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        #[cfg(not(target_os = "macos"))]
        let _ = (app, &event);
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                desktop_shell::restore_main_window(app);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};
    use tauri::Manager;

    #[test]
    fn desktop_update_runtime_support_is_macos_only() {
        #[cfg(target_os = "macos")]
        assert_eq!(desktop_update_platform_unsupported_message(), None);
        #[cfg(target_os = "windows")]
        assert_eq!(
            desktop_update_platform_unsupported_message(),
            Some(desktop_update::WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(
            desktop_update_platform_unsupported_message(),
            Some(desktop_update::MACOS_ONLY_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
        );
    }

    #[test]
    fn south_selector_reports_each_static_ineligibility_without_exposing_secrets() {
        let eligible_draft = json!({
            "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
            "egress": {"mode": "direct"}
        });
        let eligible = json!({
            "provider": "openai-compatible",
            "base_url": "https://provider.example/v1",
            "auth": {"slot": "provider_api_key", "store": true}
        });
        assert_eq!(
            south_v1_unavailable_reason(&eligible_draft, &eligible, true),
            None
        );
        assert_eq!(
            south_v1_unavailable_reason(&eligible_draft, &eligible, false),
            Some("provider_package")
        );

        let cases = [
            (
                json!({"provider": "anthropic", "auth": {"store": true}}),
                eligible_draft.clone(),
                "provider_package",
            ),
            (
                json!({
                    "provider": "openai-compatible",
                    "api_dialect": "anthropic-native",
                    "auth": {"store": true}
                }),
                eligible_draft.clone(),
                "api_dialect",
            ),
            (
                eligible.clone(),
                json!({
                    "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
                    "egress": {"mode": "proxy", "proxy": "http://secret.example"}
                }),
                "egress",
            ),
            (
                json!({
                    "provider": "openai-compatible",
                    "auth": {"slot": "provider_api_key", "file": "/private/key"}
                }),
                eligible_draft,
                "auth",
            ),
        ];

        for (upstream, draft, expected) in cases {
            assert_eq!(
                south_v1_unavailable_reason(&draft, &upstream, true),
                Some(expected)
            );
        }
    }

    #[test]
    fn header_auth_selector_is_independent_from_the_legacy_south_modes() {
        let draft = json!({
            "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
            "egress": {"mode": "direct"}
        });
        let azure = json!({
            "provider": "azure-openai-v1",
            "base_url": "https://fixture.openai.azure.com/openai/v1",
            "auth": {"slot": "provider_api_key", "store": true}
        });

        assert_eq!(
            south_v1_unavailable_reason(&draft, &azure, true),
            Some("provider_package"),
            "the old South selector must remain Bearer-only"
        );
        assert_eq!(
            south_header_auth_v1_unavailable_reason(&draft, &azure, true),
            None,
            "the new cumulative selector accepts the exact Azure dialect"
        );

        let unknown = json!({
            "provider": "future-header-provider",
            "auth": {"slot": "provider_api_key", "store": true}
        });
        assert_eq!(
            south_header_auth_v1_unavailable_reason(&draft, &unknown, true),
            Some("provider_package")
        );
    }

    #[test]
    fn prepare_desktop_draft_preserves_omitted_optional_maps() {
        let source = template(
            std::path::Path::new("/tmp/token-station-data"),
            std::path::Path::new("/tmp/plugins"),
        );
        assert_eq!(source["routing"]["mode"], json!("direct"));
        assert_eq!(source["router"]["routing_mode"], json!("tiered"));
        assert!(source["router"].get("direct_target").is_none());
        assert!(source.get("agent_routes").is_none());
        assert!(source.get("profiles").is_none());

        let prepared = prepare_desktop_draft(source, std::path::Path::new("/tmp"));

        assert!(prepared.get("agent_routes").is_none());
        assert!(prepared.get("profiles").is_none());
        serde_json::from_value::<ClientConfig>(prepared)
            .expect("desktop preparation must preserve the ClientConfig shape");
    }

    #[test]
    fn free_provider_catalog_exposes_only_reviewed_free_models() {
        let presets = list_free_provider_presets();
        assert_eq!(presets.len(), 13);
        let nvidia = presets
            .iter()
            .find(|preset| preset.id == "nvidia")
            .expect("NVIDIA is included in the reviewed free catalog");
        assert_eq!(nvidia.upstream_name, "nvidia_free");
        assert_eq!(nvidia.base_url, "https://integrate.api.nvidia.com/v1");
        assert!(!nvidia.models.is_empty());
        assert!(presets
            .iter()
            .all(|preset| preset.upstream_name.ends_with("_free")));
        assert!(presets
            .iter()
            .flat_map(|preset| preset.models)
            .all(|model| {
                model.tool == CapabilityState::Unknown
                    && model.json_schema == CapabilityState::Unknown
            }));
        assert!(["gemini", "hugging_face"].iter().all(|id| {
            presets
                .iter()
                .find(|preset| preset.id == *id)
                .is_some_and(|preset| {
                    preset.overage_policy
                        == free_provider_catalog::OveragePolicy::UserMustEnableGuard
                })
        }));
    }

    #[test]
    fn provider_brand_id_uses_curated_identity_and_not_the_editable_upstream_name() {
        assert_eq!(
            provider_brand_id("renamed-account", "https://api.deepseek.com/v1", "paid"),
            Some("deepseek")
        );
        assert_eq!(
            provider_brand_id("nvidia_free", "https://integrate.api.nvidia.com/v1", "free"),
            Some("nvidia")
        );
        assert_eq!(
            provider_brand_id("deepseek", "https://proxy.example.test/v1", "paid"),
            None,
            "a custom endpoint must not inherit a logo from its editable name"
        );
        let root = scratch_home("provider-brand-view");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["renamed-account"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.deepseek.com/v1",
            "models": [{"model": "deepseek-chat"}]
        });
        let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();
        assert_eq!(view.providers[0].brand_id, Some("deepseek"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn free_provider_command_rejects_forged_models_before_network_or_keychain() {
        let root = scratch_home("free-provider-forged-model");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));

        let error = match tauri::async_runtime::block_on(add_free_provider(
            app.state(),
            "nvidia".to_owned(),
            vec!["paid/model".to_owned()],
            "not-a-real-key".to_owned(),
            true,
        )) {
            Err(error) => error,
            Ok(_) => panic!("a model outside the backend allowlist is rejected"),
        };
        assert!(error.contains("免费目录"), "{error}");
        assert!(get_state(app.state()).providers.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn guarded_trial_provider_requires_explicit_quota_protection_confirmation() {
        let root = scratch_home("free-provider-guard");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));

        let error = match tauri::async_runtime::block_on(add_free_provider(
            app.state(),
            "alibaba_model_studio".to_owned(),
            vec!["qwen-turbo".to_owned()],
            "not-a-real-key".to_owned(),
            false,
        )) {
            Err(error) => error,
            Ok(_) => panic!("trial providers cannot continue without the free-quota guard"),
        };
        assert!(error.contains("免费额度保护"), "{error}");
        assert!(get_state(app.state()).providers.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn catalog_managed_free_providers_reject_every_generic_mutator() {
        let root = scratch_home("free-provider-generic-mutation");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["nvidia_free"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://integrate.api.nvidia.com/v1",
            "access_tier": "free",
            "auth": { "slot": "provider_api_key", "store": true },
            "models": [{
                "model": "openai/gpt-oss-120b",
                "tool_state": "unknown",
                "vision_state": "unknown",
                "json_schema_state": "unknown",
                "context_window": 131072
            }]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let edit_error = match edit_provider(
            app.state(),
            "nvidia_free".to_owned(),
            "https://attacker.invalid/v1".to_owned(),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("a generic edit cannot retarget a free credential"),
        };
        assert!(edit_error.contains("内置目录管理"), "{edit_error}");

        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let discovery_error = prepare_discovery_credential(
            &inner,
            "nvidia_free",
            "https://integrate.api.nvidia.com/v1",
            Some("renderer-supplied-key"),
        )
        .expect_err("generic discovery cannot use a free identity");
        assert!(
            discovery_error.contains("内置目录管理"),
            "{discovery_error}"
        );
        let models_error =
            replace_provider_models(&mut inner, "nvidia_free", vec!["paid/model".to_owned()])
                .expect_err("generic model replacement cannot change a free allowlist");
        assert!(models_error.contains("内置目录管理"), "{models_error}");
        let vision_error =
            replace_provider_model_vision(&mut inner, "nvidia_free", "openai/gpt-oss-120b", true)
                .expect_err("generic capability edits cannot change a free allowlist");
        assert!(vision_error.contains("内置目录管理"), "{vision_error}");
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn archived_free_provider_requires_catalog_revalidation_instead_of_restore() {
        let root = scratch_home("free-provider-restore");
        let draft = template_for_test(&root);
        let data_dir = root.join("token-station-data");
        provider_tombstones::archive(
            &data_dir,
            "nvidia_free",
            &json!({
                "provider": "openai-compatible",
                "base_url": "https://integrate.api.nvidia.com/v1",
                "access_tier": "free"
            }),
        )
        .unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let error = match restore_provider(app.state(), "nvidia_free".to_owned()) {
            Err(error) => error,
            Ok(_) => panic!("free provider tombstones cannot bypass catalog revalidation"),
        };
        assert!(error.contains("免费目录重新验证"), "{error}");
        assert!(provider_tombstones::contains(&data_dir, "nvidia_free").unwrap());
        assert!(get_state(app.state()).deleted_providers.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prepare_desktop_draft_backfills_missing_builtin_agent_adapters() {
        // Legacy agents snapshots omit the later agent-gemini adapter.
        let draft = json!({
            "plugins": { "agents": ["agent-openai", "agent-anthropic", "agent-openai-responses"] }
        });
        let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
        let agents: Vec<String> = out["plugins"]["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        // Include every desktop_agents() built-in adapter, including agent-gemini, while preserving existing entries.
        assert!(agents.contains(&"agent-openai".to_string()));
        assert!(
            agents.contains(&"agent-gemini".to_string()),
            "agent-gemini 应被补齐,实际 ={agents:?}"
        );
        for adapter in desktop_agents() {
            assert!(
                agents.iter().any(|a| a == adapter),
                "缺 {adapter}:{agents:?}"
            );
        }
    }

    #[test]
    fn prepare_desktop_draft_prunes_dangling_agent_route_and_profile_references() {
        // Only upstream `live` with model `keep` remains. agent_routes and profiles
        // still reference removed provider `gone` and removed model `dropped`.
        let draft = json!({
            "plugins": {"agents": desktop_agents()},
            "upstreams": { "live": { "models": [{ "model": "keep" }] } },
            "routing": {
                "mode": "direct",
                "direct_target": { "upstream": "live", "model": "dropped" }
            },
            "router": {
                "quota_accounts": [
                    { "upstream": "gone", "model": "whatever" },
                    { "upstream": "live", "model": "keep" },
                    { "upstream": "live", "model": "dropped" }
                ]
            },
            "agent_routes": {
                "opencode": {
                    "mode": "custom",
                    "direct_target": { "upstream": "gone", "model": "whatever" },
                    "custom_route": {
                        "high": { "upstream": "gone", "model": "whatever" },
                        "mid": { "upstream": "live", "model": "dropped" },
                        "low": { "upstream": "live", "model": "keep" }
                    }
                }
            },
            "profiles": {
                "团队默认": {
                    "high": { "upstream": "gone", "model": "x" },
                    "mid": { "upstream": "live", "model": "keep" },
                    "low": { "upstream": "live", "model": "dropped" }
                }
            }
        });

        let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));

        let route = &out["agent_routes"]["opencode"]["custom_route"];
        // A removed provider clears the entire tier.
        assert!(route["high"]["upstream"].is_null());
        assert!(route["high"]["model"].is_null());
        // A removed model clears only the model and keeps the provider.
        assert_eq!(route["mid"]["upstream"], json!("live"));
        assert!(route["mid"]["model"].is_null());
        // Preserve targets that remain valid.
        assert_eq!(route["low"]["upstream"], json!("live"));
        assert_eq!(route["low"]["model"], json!("keep"));

        let profile = &out["profiles"]["团队默认"];
        assert!(profile["high"]["upstream"].is_null());
        assert_eq!(profile["mid"]["model"], json!("keep"));
        assert_eq!(profile["low"]["upstream"], json!("live"));
        assert!(profile["low"]["model"].is_null());
        assert_eq!(out["routing"]["direct_target"]["upstream"], json!("live"));
        assert!(out["routing"]["direct_target"]["model"].is_null());
        assert!(out["router"].get("direct_target").is_none());
        assert!(out["agent_routes"]["opencode"]["direct_target"].is_object());
        assert!(out["agent_routes"]["opencode"]["direct_target"]["upstream"].is_null());
        assert!(out["agent_routes"]["opencode"]["direct_target"]["model"].is_null());
        assert_eq!(
            out["router"]["quota_accounts"],
            json!([{ "upstream": "live", "model": "keep" }])
        );
    }

    #[test]
    fn known_context_window_reads_size_markers_then_family_defaults() {
        // Prefer explicit size markers supplied in model names.
        assert_eq!(known_context_window("glm-5.2[1m]"), 1_000_000);
        assert_eq!(known_context_window("moonshot-v1-128k"), 128_000);
        assert_eq!(known_context_window("qwen-turbo-1m"), 1_000_000);
        assert_eq!(known_context_window("gpt-4-32k"), 32_000);
        // Fall back to the family default when no marker exists.
        assert_eq!(known_context_window("gemini-2.5-pro"), 1_000_000);
        assert_eq!(known_context_window("claude-opus-4-8"), 200_000);
        // Unknown families and version numbers use the 128k fallback without false inference.
        assert_eq!(known_context_window("deepseek-v4-pro"), 128_000);
        assert_eq!(known_context_window("glm-4.6"), 128_000);
        assert_eq!(known_context_window("some-obscure-model"), 128_000);
    }

    #[test]
    fn desktop_preparation_backfills_exact_kimi_models_from_builtin_limits() {
        let draft = json!({
            "upstreams": {
                "kimi": {
                    "provider": "openai-compatible",
                    "base_url": "https://api.moonshot.cn/v1/",
                    "models": [
                        {"model": "kimi-k2.6", "context_window": 128000},
                        {"model": "kimi-k3", "context_window": 128000}
                    ]
                }
            }
        });

        let prepared = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
        let models = prepared["upstreams"]["kimi"]["models"].as_array().unwrap();
        assert_eq!(models[0]["context_window"], json!(262_144));
        assert_eq!(models[0]["max_output_tokens"], json!(262_144));
        assert_eq!(
            models[0]["x-token-station-context-window-source"],
            json!("builtin_preset")
        );
        assert_eq!(models[1]["context_window"], json!(1_048_576));
        assert_eq!(models[1]["max_output_tokens"], json!(131_072));
        assert_eq!(
            models[1]["x-token-station-max-output-tokens-source"],
            json!("builtin_preset")
        );
    }

    #[test]
    fn builtin_limits_do_not_match_unofficial_endpoints_similar_ids_or_operator_values() {
        let draft = json!({
            "upstreams": {
                "gateway": {
                    "provider": "openai-compatible",
                    "base_url": "https://gateway.example/v1",
                    "models": [{"model": "kimi-k3", "context_window": 128000}]
                },
                "kimi": {
                    "provider": "openai-compatible",
                    "base_url": "https://api.moonshot.cn/v1",
                    "models": [
                        {"model": "kimi-k3-preview", "context_window": 128000},
                        {
                            "model": "kimi-k3",
                            "context_window": 64000,
                            "max_output_tokens": 8000,
                            "x-token-station-context-window-source": "operator",
                            "x-token-station-max-output-tokens-source": "operator"
                        }
                    ]
                }
            }
        });

        let prepared = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
        assert!(prepared["upstreams"]["gateway"]["models"][0]
            .get("max_output_tokens")
            .is_none());
        assert!(prepared["upstreams"]["kimi"]["models"][0]
            .get("max_output_tokens")
            .is_none());
        assert_eq!(
            prepared["upstreams"]["kimi"]["models"][1]["context_window"],
            json!(64_000)
        );
        assert_eq!(
            prepared["upstreams"]["kimi"]["models"][1]["max_output_tokens"],
            json!(8_000)
        );
    }

    #[test]
    fn prepare_desktop_draft_keeps_free_capabilities_fail_closed() {
        let root = scratch_home("free-capability-migration");
        let draft = json!({
            "plugins": {"agents": desktop_agents(), "dir": root.join("plugins")},
            "data": {"dir": root.join("data")},
            "upstreams": {
                "nvidia_free": {
                    "access_tier": "free",
                    "models": [{
                        "model": "openai/gpt-oss-120b",
                        "tool": false,
                        "json_schema": false,
                        "tool_state": "unknown",
                        "json_schema_state": "unknown"
                    }]
                }
            }
        });

        let migrated = prepare_desktop_draft(draft, &root);
        let model = &migrated["upstreams"]["nvidia_free"]["models"][0];
        assert_eq!(model["tool_state"], "unknown");
        assert_eq!(model["json_schema_state"], "unknown");
        assert_eq!(model["tool"], false);
        assert_eq!(model["json_schema"], false);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prepare_desktop_draft_upgrades_unknown_tool_capability_but_keeps_unsupported() {
        let draft = json!({
            "upstreams": {
                "deepseek": { "models": [
                    { "model": "a", "tool_state": "unknown", "json_schema_state": "unknown", "vision_state": "unknown" },
                    { "model": "b", "tool_state": "unsupported", "json_schema_state": "verified", "vision_state": "unknown" },
                ]}
            }
        });
        let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
        let models = out["upstreams"]["deepseek"]["models"].as_array().unwrap();
        // Promote tools and structured output from unknown to declared while keeping vision unknown.
        assert_eq!(models[0]["tool_state"], json!("declared"));
        assert_eq!(models[0]["json_schema_state"], json!("declared"));
        assert_eq!(models[0]["vision_state"], json!("unknown"));
        // Do not overwrite explicit operator-set unsupported or verified states.
        assert_eq!(models[1]["tool_state"], json!("unsupported"));
        assert_eq!(models[1]["json_schema_state"], json!("verified"));
    }

    fn scratch_home(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "token-station-desktop-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("scratch home is writable");
        path
    }

    fn template_for_test(root: &std::path::Path) -> Value {
        let mut draft = template(&root.join("token-station-data"), &root.join("plugins"));
        draft
            .as_object_mut()
            .expect("config fixture is an object")
            .remove("routing");
        draft
    }

    #[test]
    fn state_snapshot_uses_cached_south_eligibility() {
        let root = scratch_home("cached-south-eligibility");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.example.test/v1",
            "auth": { "slot": "provider_api_key", "store": true },
            "models": [{ "model": "gpt-test" }],
            "provider_call": "south_v1_buffered"
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        inner
            .south_approved_dialects
            .insert("openai-compatible".to_owned());
        let invalid_package = root.join("plugins/broken");
        std::fs::create_dir_all(&invalid_package).expect("plugin fixture directory is writable");
        std::fs::write(invalid_package.join("manifest.json"), "not JSON")
            .expect("plugin fixture is writable");

        let view = inner.snapshot();

        assert_eq!(view.providers.len(), 1);
        assert!(view.providers[0].south_v1_available);
        assert_eq!(view.providers[0].south_v1_unavailable_reason, None);
        std::fs::remove_dir_all(root).ok();
    }

    fn gateway_template_for_test(root: &std::path::Path) -> Value {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("plugins-dist");
        let mut draft = template(&root.join("token-station-data"), &plugins_dir);
        draft
            .as_object_mut()
            .expect("config fixture is an object")
            .remove("routing");
        draft
    }

    fn published_agent_route_fixture(root: &std::path::Path) -> (Value, RunningServer) {
        let mut draft = gateway_template_for_test(root);
        draft["server"]["listen"] = json!("127.0.0.1:0");
        draft["server"]["auth"] = json!(false);
        draft["data"]["metrics"] = json!(false);
        draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        draft["router"]["pools"] = json!({
            TIER_LOW: [{"upstream": "local", "model": "small"}]
        });
        draft["router"]["default_pool"] = json!(TIER_LOW);
        let config: ClientConfig = serde_json::from_value(draft.clone()).unwrap();
        let running = prepare_server(config)
            .unwrap()
            .bind()
            .unwrap()
            .publish(7)
            .unwrap();
        (draft, running)
    }

    fn manage_test_agent_state<R: Runtime>(app: &tauri::App<R>, root: &std::path::Path) {
        let paths = AgentIntegrationPaths {
            snapshot_root: root.join("agent-data/snapshots"),
            ownership_root: root.join("agent-data/ownership"),
        };
        assert!(app.manage(paths.clone()));
        assert!(app.manage(AgentCommandState::new(paths).expect("Agent command state initializes")));
    }

    fn serve_model_catalog(
        responses: Vec<(u16, &'static str)>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("model catalog fixture binds");
        listener
            .set_nonblocking(true)
            .expect("model catalog fixture is nonblocking");
        let address = listener
            .local_addr()
            .expect("model catalog fixture has an address");
        let worker = std::thread::spawn(move || {
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "model catalog discovery request did not arrive before deadline"
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("model catalog fixture accept failed: {error}"),
                    }
                };
                stream
                    .set_nonblocking(false)
                    .expect("accepted model catalog socket is blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("model catalog fixture read is bounded");
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .expect("model catalog fixture write is bounded");
                let mut request = [0u8; 2048];
                let read = stream
                    .read(&mut request)
                    .expect("model catalog fixture reads the request");
                assert!(read > 0, "model catalog request must not be empty");
                let response = format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("model catalog fixture responds");
            }
        });
        (format!("http://{address}"), worker)
    }

    fn serve_chat_completion(
        marker: &'static str,
        requests: usize,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("chat fixture binds");
        let address = listener.local_addr().expect("chat fixture has an address");
        let worker = std::thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("chat request arrives");
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).expect("chat fixture reads request");
                    assert!(read > 0, "chat request ended before its declared body");
                    request.extend_from_slice(&chunk[..read]);
                    let Some(header_end) =
                        request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                assert!(
                    String::from_utf8_lossy(&request).contains("/v1/chat/completions"),
                    "gateway must call the configured chat endpoint"
                );
                let streaming = String::from_utf8_lossy(&request).contains(r#""stream":true"#);
                let (content_type, body) = if streaming {
                    (
                        "text/event-stream",
                        format!(
                            "data: {}\n\ndata: [DONE]\n\n",
                            json!({
                                "id": format!("fixture-{marker}"),
                                "object": "chat.completion.chunk",
                                "created": 1,
                                "model": "small",
                                "choices": [{
                                    "index": 0,
                                    "delta": {"content": marker},
                                    "finish_reason": null
                                }]
                            })
                        ),
                    )
                } else {
                    (
                        "application/json",
                        json!({
                            "id": format!("fixture-{marker}"),
                            "object": "chat.completion",
                            "created": 1,
                            "model": "small",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": marker},
                                "finish_reason": "stop"
                            }],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                        })
                        .to_string(),
                    )
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("chat fixture responds");
            }
        });
        (format!("http://{address}/v1"), worker)
    }

    fn chat_through_proxy(listen: &str) -> String {
        let body = r#"{"model":"auto","messages":[{"role":"user","content":"ping"}]}"#;
        let mut stream = std::net::TcpStream::connect(listen).expect("proxy listener is reachable");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(
            stream,
            "POST /v1/chat/completions HTTP/1.1\r\nhost: {listen}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("proxy request writes");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("proxy response reads");
        response
    }

    fn wait_for_serve_phase<R: Runtime>(app: &tauri::App<R>, expected: ServePhase) -> StateView {
        wait_for_serve_phase_with_timeout(app, expected, Duration::from_secs(60))
    }

    fn wait_for_serve_phase_with_timeout<R: Runtime>(
        app: &tauri::App<R>,
        expected: ServePhase,
        timeout: Duration,
    ) -> StateView {
        let deadline = Instant::now() + timeout;
        loop {
            let state = get_state(app.state());
            if state.serve.phase == expected {
                return state;
            }
            assert!(
                expected == ServePhase::Error || state.serve.phase != ServePhase::Error,
                "serve phase entered Error before {expected:?}; error={:?}",
                state.serve.error
            );
            assert!(
                Instant::now() < deadline,
                "serve phase did not reach {expected:?}; current={:?}, error={:?}",
                state.serve.phase,
                state.serve.error
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_receipts(path: &std::path::Path, expected: usize) -> Vec<ReceiptView> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let receipts = SqliteStore::recent_receipts(path, 5).expect("receipts read");
            if receipts.len() >= expected {
                return receipts;
            }
            assert!(
                Instant::now() < deadline,
                "receipt count did not reach {expected}; current={}",
                receipts.len()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn desktop_shell_distinguishes_live_application_from_listener_handoff() {
        assert_eq!(
            desktop_shell_applying_phase(true, true),
            desktop_shell::ProxyMenuPhase::Applying
        );
        assert_eq!(
            desktop_shell_applying_phase(true, false),
            desktop_shell::ProxyMenuPhase::Switching
        );
        assert_eq!(
            desktop_shell_applying_phase(false, true),
            desktop_shell::ProxyMenuPhase::Switching
        );
        assert_eq!(
            desktop_shell_applying_phase(false, false),
            desktop_shell::ProxyMenuPhase::Switching
        );
    }

    #[test]
    fn status_menu_actions_require_the_same_generation_and_lifecycle_action() {
        assert!(menu_action_expectation_matches(
            7,
            7,
            desktop_shell::ProxyMenuAction::Start,
            desktop_shell::ProxyMenuAction::Start,
        ));
        assert!(menu_action_expectation_matches(
            7,
            7,
            desktop_shell::ProxyMenuAction::Stop,
            desktop_shell::ProxyMenuAction::Stop,
        ));
        assert!(!menu_action_expectation_matches(
            7,
            8,
            desktop_shell::ProxyMenuAction::Start,
            desktop_shell::ProxyMenuAction::Start,
        ));
        assert!(!menu_action_expectation_matches(
            7,
            7,
            desktop_shell::ProxyMenuAction::Start,
            desktop_shell::ProxyMenuAction::Stop,
        ));
    }

    #[test]
    fn the_desktop_template_enables_every_supported_inbound_protocol() {
        let root = PathBuf::from("/tmp/token-station-desktop-test");
        let draft = template_for_test(&root);

        assert_eq!(draft["plugins"]["agents"], json!(desktop_agents()));
    }

    #[test]
    fn desktop_paths_stay_inside_tauri_roots_and_create_writable_directories() {
        let root = scratch_home("tauri-paths");
        let config_root = root.join("config");
        let data_root = root.join("data");
        let paths = DesktopPaths::from_app_roots(config_root.clone(), data_root.clone());

        assert_eq!(paths.config_file, config_root.join("token-station.json"));
        assert_eq!(paths.data_dir, data_root.join("token-station-data"));
        assert_eq!(paths.plugins_dir, data_root.join("plugins"));
        assert_eq!(paths.agent_data_root, data_root.join("agent-integration"));

        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(&paths.config_file, b"{}").unwrap();
        let legacy_cache = paths.data_dir.join("model-catalog-cache.json");
        std::fs::write(&legacy_cache, b"{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config_root, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&paths.data_dir, std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::fs::set_permissions(&paths.config_file, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            std::fs::set_permissions(&legacy_cache, std::fs::Permissions::from_mode(0o644))
                .unwrap();
        }
        paths.create_writable_dirs().unwrap();
        assert!(config_root.is_dir());
        assert!(paths.data_dir.is_dir());
        assert!(paths.plugins_dir.is_dir());
        assert!(paths.agent_data_root.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&config_root)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&paths.data_dir)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&paths.config_file)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&legacy_cache)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let draft = template(&paths.data_dir, &paths.plugins_dir);
        assert_eq!(draft["data"]["dir"], json!(paths.data_dir));
        assert_eq!(draft["plugins"]["dir"], json!(paths.plugins_dir));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(feature = "bundled-plugins")]
    #[test]
    fn desktop_bundled_plugins_load_without_an_external_plugin_directory() {
        let root = scratch_home("bundled-plugins");
        let output = root.join("installed-self-test.json");
        run_installed_self_test(&output).expect("the installed-artifact self-test passes");
        let report: Value = serde_json::from_slice(
            &std::fs::read(&output).expect("the self-test report was written"),
        )
        .expect("the self-test report is JSON");
        assert_eq!(report["passed"], json!(true));
        // The set, not a count. This assertion used to read `Some(5)`, and it
        // went stale the moment `provider-anthropic` joined the bundle — the
        // desktop crate is excluded from the workspace, so
        // `cargo test --workspace` never ran it and only the desktop build
        // gate noticed. `official_package_set.rs` checks that every consumer
        // *names* each package; a bare integer is not a name, so it could not
        // see this one. Naming them puts this test back under that gate.
        let reported: Vec<&str> = report["plugins"]
            .as_array()
            .expect("the report lists the bundled plugins")
            .iter()
            .map(|plugin| plugin["id"].as_str().expect("a plugin id is a string"))
            .collect();
        assert_eq!(reported, BUNDLED_PLUGIN_IDS.to_vec());
        assert_eq!(report["storage"]["credential_read"], json!(false));
        assert_eq!(report["gateway"]["loadable"], json!(true));
        assert!(report["gateway"]["provider_dialects"]
            .as_array()
            .is_some_and(|dialects| dialects.iter().any(|dialect| dialect == "azure-openai-v1")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn enterprise_verification_never_persists_the_discovered_catalog() {
        const CATALOG: &str = r#"{"data":[{"id":"private-enterprise-model"}]}"#;
        let root = scratch_home("enterprise-verification-isolation");
        let data_dir = root.join("token-station-data");
        let (base_url, server) = serve_model_catalog(vec![(200, CATALOG)]);
        let mut draft = template_for_test(&root);
        draft["data"]["dir"] = json!(data_dir.clone());
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let result = tauri::async_runtime::block_on(verify_enterprise_route(
            app.state(),
            "enterprise_main".to_owned(),
            base_url,
            "secret-key".to_owned(),
        ))
        .expect("live enterprise verification succeeds");

        assert_eq!(result.source, "live");
        assert_eq!(result.models, ["private-enterprise-model"]);
        assert!(!data_dir.join("model-catalog-cache.json").exists());
        assert!(get_state(app.state()).providers.is_empty());
        server.join().expect("model catalog fixture exits");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn repeated_model_discovery_only_updates_the_catalog_cache() {
        const CATALOG: &str = r#"{"data":[{"id":"model-b"},{"id":"model-a"}]}"#;

        let root = scratch_home("discovery-isolation");
        let data_dir = root.join("token-station-data");
        let config_path = root.join("token-station.json");
        let (base_url, server) = serve_model_catalog(vec![
            (200, CATALOG),
            (200, CATALOG),
            (200, CATALOG),
            (503, r#"{"error":"offline"}"#),
        ]);
        let mut draft = template_for_test(&root);
        draft["data"]["dir"] = json!(data_dir.clone());
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": base_url,
            "models": [{"model": "configured-model"}]
        });
        let expected_draft = draft.clone();
        // Compact JSON is intentionally different from the normal pretty save
        // format, so even a semantically identical rewrite fails this check.
        let expected_config =
            serde_json::to_vec(&expected_draft).expect("saved config fixture serializes");
        std::fs::write(&config_path, &expected_config).expect("saved config fixture writes");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            draft,
            None,
        )))));
        let agent_paths = AgentIntegrationPaths {
            snapshot_root: root.join("agent-data/snapshots"),
            ownership_root: root.join("agent-data/ownership"),
        };
        assert!(app.manage(agent_paths.clone()));
        assert!(app
            .manage(AgentCommandState::new(agent_paths).expect("Agent command state initializes")));
        let initial_state = get_state(app.state());
        assert_eq!(initial_state.draft_revision, initial_state.saved_revision);
        assert!(!initial_state.config_dirty);

        for _ in 0..3 {
            let result = tauri::async_runtime::block_on(discover_provider_models(
                app.state(),
                "fixture".to_owned(),
                base_url.clone(),
                None,
            ))
            .expect("live discovery succeeds");
            assert_eq!(result.source, "live");
            assert_eq!(result.models, ["model-a", "model-b"]);
            assert_eq!(
                app.state::<AppStateManaged>()
                    .inner()
                    .0
                    .lock()
                    .unwrap()
                    .draft,
                expected_draft
            );
            assert_eq!(
                std::fs::read(&config_path).expect("saved config remains readable"),
                expected_config
            );
            let state = get_state(app.state());
            assert_eq!(state.draft_revision, initial_state.draft_revision);
            assert_eq!(state.saved_revision, initial_state.saved_revision);
            assert!(!state.config_dirty);
        }

        let cached = tauri::async_runtime::block_on(discover_provider_models(
            app.state(),
            "fixture".to_owned(),
            base_url,
            None,
        ))
        .expect("offline discovery falls back to its cache");
        assert_eq!(cached.source, "cache");
        assert_eq!(cached.models, ["model-a", "model-b"]);
        assert_eq!(
            app.state::<AppStateManaged>()
                .inner()
                .0
                .lock()
                .unwrap()
                .draft,
            expected_draft
        );
        assert_eq!(
            std::fs::read(&config_path).expect("saved config remains readable"),
            expected_config
        );
        let cached_state = get_state(app.state());
        assert_eq!(cached_state.draft_revision, initial_state.draft_revision);
        assert_eq!(cached_state.saved_revision, initial_state.saved_revision);
        assert!(!cached_state.config_dirty);
        assert!(data_dir.join("model-catalog-cache.json").is_file());
        server.join().expect("model catalog fixture exits");

        {
            let managed = app.state::<AppStateManaged>();
            let mut inner = managed.0.lock().unwrap();
            inner.draft["upstreams"]["fixture"]["models"] = json!([{"model": "model-a"}]);
            inner.observe_draft().unwrap();
        }
        let explicitly_edited = get_state(app.state());
        assert!(explicitly_edited.draft_revision > initial_state.draft_revision);
        assert_eq!(
            explicitly_edited.saved_revision,
            initial_state.saved_revision
        );
        assert!(explicitly_edited.config_dirty);

        let warning_root = scratch_home("discovery-cache-warning");
        let warning_data = warning_root.join("data");
        std::fs::create_dir_all(warning_data.join("model-catalog-cache.json"))
            .expect("directory fixture blocks the cache rename");
        let warning_config = warning_root.join("token-station.json");
        let (warning_base, warning_server) = serve_model_catalog(vec![(200, CATALOG)]);
        let mut warning_draft = template_for_test(&warning_root);
        warning_draft["data"]["dir"] = json!(warning_data);
        warning_draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": warning_base,
            "models": [{"model": "configured-model"}]
        });
        let expected_warning_draft = warning_draft.clone();
        let expected_warning_config =
            serde_json::to_vec(&expected_warning_draft).expect("warning config fixture serializes");
        std::fs::write(&warning_config, &expected_warning_config)
            .expect("warning config fixture writes");
        let warning_app = tauri::test::mock_app();
        assert!(warning_app.manage(AppStateManaged(Mutex::new(AppInner::new(
            warning_config.clone(),
            warning_draft,
            None,
        )))));
        let warning_initial_state = get_state(warning_app.state());

        let warning = tauri::async_runtime::block_on(discover_provider_models(
            warning_app.state(),
            "fixture".to_owned(),
            warning_base,
            None,
        ))
        .expect("cache failure remains a live discovery result");
        assert_eq!(warning.source, "live");
        assert_eq!(warning.models, ["model-a", "model-b"]);
        assert!(warning
            .warning
            .as_deref()
            .is_some_and(|message| message.contains("保存模型缓存失败")));
        assert_eq!(
            warning_app
                .state::<AppStateManaged>()
                .inner()
                .0
                .lock()
                .unwrap()
                .draft,
            expected_warning_draft
        );
        assert_eq!(
            std::fs::read(&warning_config).expect("warning config remains readable"),
            expected_warning_config
        );
        let warning_state = get_state(warning_app.state());
        assert_eq!(
            warning_state.draft_revision,
            warning_initial_state.draft_revision
        );
        assert_eq!(
            warning_state.saved_revision,
            warning_initial_state.saved_revision
        );
        assert!(!warning_state.config_dirty);
        warning_server.join().expect("warning fixture exits");

        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(warning_root).ok();
    }

    #[test]
    fn remote_http_discovery_fails_before_network_access_even_without_credentials() {
        let root = scratch_home("credentialed-http-discovery");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));

        let error = tauri::async_runtime::block_on(discover_provider_models(
            app.state(),
            "remote_http".to_owned(),
            "http://192.0.2.1/v1".to_owned(),
            None,
        ))
        .expect_err("a remote plaintext endpoint must fail before the request starts");

        assert!(error.contains("must use HTTPS"), "{error}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_legacy_chat_only_config_is_migrated_in_memory_with_absolute_runtime_paths() {
        let root = scratch_home("legacy");
        let mut draft = template_for_test(&root);
        draft["plugins"].as_object_mut().unwrap().remove("agents");
        draft["plugins"]["agent"] = json!("agent-openai");
        draft["plugins"]["dir"] = json!("plugins-dist");
        draft["data"]["dir"] = json!("token-station-data");

        let saved = draft.clone();
        let prepared = prepare_desktop_draft(draft, &root);

        assert_eq!(prepared["plugins"]["agents"], json!(desktop_agents()));
        assert!(prepared["plugins"].get("agent").is_none());
        assert_eq!(prepared["plugins"]["dir"], json!(root.join("plugins-dist")));
        assert_eq!(
            prepared["data"]["dir"],
            json!(root.join("token-station-data"))
        );
        let inner =
            AppInner::new_with_saved(root.join("token-station.json"), prepared, saved, None);
        assert!(inner.config_state.is_dirty());
        assert_ne!(
            inner.config_state.draft_revision(),
            inner.config_state.saved_revision()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_desktop_v1_agent_list_is_migrated_to_every_supported_inbound_protocol() {
        let root = scratch_home("desktop-v1-agents");
        let mut draft = template_for_test(&root);
        draft["plugins"]["agents"] = json!(["agent-openai"]);

        let prepared = prepare_desktop_draft(draft, &root);

        assert_eq!(prepared["plugins"]["agents"], json!(desktop_agents()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_broken_existing_config_enters_read_only_protection_without_overwrite() {
        let root = scratch_home("broken-config");
        let path = root.join("token-station.json");
        let original = b"{ definitely not json";
        std::fs::write(&path, original).unwrap();

        let (_draft, error) = load_draft(&path, &root);

        assert!(error.as_deref().is_some_and(|e| e.contains("只读保护")));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dangling_direct_and_quota_references_load_as_an_editable_dirty_draft() {
        let root = scratch_home("dangling-direct-startup-repair");
        let path = root.join("token-station.json");
        let mut source = template_for_test(&root);
        source["upstreams"]["live"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "keep"}]
        });
        source["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "live", "model": "dropped"}
        });
        source["router"]["quota_accounts"] = json!([
            {"upstream": "gone", "model": "missing"},
            {"upstream": "live", "model": "keep"},
            {"upstream": "live", "model": "dropped"}
        ]);
        source["agent_routes"] = json!({
            "codex": {
                "mode": "inherit",
                "routing_mode": "direct",
                "direct_target": {"upstream": "gone", "model": "missing"}
            }
        });
        // The recovery path must apply the exact same legacy load migration as
        // ClientConfig::load before auditing the known dangling references.
        source["concurrency"] = json!({
            "global": 0,
            "per_agent": 0,
            "per_provider": 0
        });
        let original = serde_json::to_vec(&source).expect("dangling fixture serializes");
        std::fs::write(&path, &original).expect("dangling fixture writes");

        let (draft, saved, load_error) = load_draft_state(
            &path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );

        assert_eq!(load_error, None);
        assert_eq!(draft["routing"]["mode"], json!("direct"));
        assert_eq!(draft["routing"]["direct_target"]["upstream"], json!("live"));
        assert!(draft["routing"]["direct_target"]["model"].is_null());
        assert!(draft["agent_routes"]["codex"]["direct_target"].is_object());
        assert!(draft["agent_routes"]["codex"]["direct_target"]["upstream"].is_null());
        assert!(draft["agent_routes"]["codex"]["direct_target"]["model"].is_null());
        assert_eq!(
            draft["router"]["quota_accounts"],
            json!([{"upstream": "live", "model": "keep"}])
        );
        for field in ["global", "per_agent", "per_provider"] {
            assert!(draft["concurrency"][field].as_u64().unwrap_or_default() > 0);
            assert!(saved["concurrency"][field].as_u64().unwrap_or_default() > 0);
        }
        assert_eq!(
            saved["routing"]["direct_target"],
            json!({"upstream": "live", "model": "dropped"})
        );
        assert_eq!(
            std::fs::read(&path).expect("startup repair never rewrites source"),
            original
        );

        let inner = AppInner::new_with_saved(path, draft, saved, load_error);
        assert!(inner.ensure_editable().is_ok());
        assert!(inner.config_state.is_dirty());
        assert!(inner.materialize().is_err());
        let wire = serde_json::to_value(inner.snapshot()).expect("StateView serializes");
        assert_eq!(
            wire["direct_target"],
            json!({"upstream": "live", "model": null})
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dangling_agent_direct_target_never_silently_inherits_a_valid_home_target() {
        let root = scratch_home("dangling-agent-direct-does-not-inherit-home");
        let path = root.join("token-station.json");
        let mut source = template_for_test(&root);
        source["upstreams"]["live"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "keep"}]
        });
        source["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "live", "model": "keep"}
        });
        source["agent_routes"] = json!({
            "codex": {
                "mode": "inherit",
                "routing_mode": "direct",
                "direct_target": {"upstream": "gone", "model": "missing"}
            }
        });
        let original = serde_json::to_vec(&source).expect("dangling fixture serializes");
        std::fs::write(&path, &original).expect("dangling fixture writes");

        let (draft, saved, load_error) = load_draft_state(
            &path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );

        assert_eq!(load_error, None);
        assert!(draft["agent_routes"]["codex"]["direct_target"].is_object());
        assert!(draft["agent_routes"]["codex"]["direct_target"]["upstream"].is_null());
        assert!(draft["agent_routes"]["codex"]["direct_target"]["model"].is_null());
        let inner = AppInner::new_with_saved(path.clone(), draft, saved, load_error);
        let view = inner.snapshot();
        assert_eq!(
            view.direct_target.as_ref().unwrap().model.as_deref(),
            Some("keep")
        );
        assert!(view.agent_routes["codex"].direct_target.is_none());
        assert!(view.agent_routes["codex"].config_error.is_some());
        let wire = serde_json::to_value(&view).expect("StateView serializes");
        assert_eq!(
            wire["direct_target"],
            json!({"upstream": "live", "model": "keep"})
        );
        assert!(wire["agent_routes"]["codex"]["direct_target"].is_null());
        assert_eq!(
            std::fs::read(&path).expect("startup repair remains zero-write"),
            original
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dangling_direct_recovery_keeps_unrelated_semantic_damage_read_only() {
        let root = scratch_home("dangling-direct-with-unrelated-damage");
        let path = root.join("token-station.json");
        let mut source = template_for_test(&root);
        source["server"]["listen"] = json!("0.0.0.0:8787");
        source["upstreams"]["live"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "keep"}]
        });
        source["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "live", "model": "dropped"}
        });
        let original = serde_json::to_vec(&source).expect("damaged fixture serializes");
        std::fs::write(&path, &original).expect("damaged fixture writes");

        let (_draft, _saved, load_error) = load_draft_state(
            &path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );

        let error = load_error.expect("unrelated semantic damage remains protected");
        assert!(error.contains("只读保护"), "{error}");
        assert_eq!(
            std::fs::read(&path).expect("damaged source remains untouched"),
            original
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn loading_config_without_agent_routes_or_profiles_stays_materializable() {
        let root = scratch_home("omitted-routing-maps");
        let path = root.join("token-station.json");
        let source = template_for_test(&root);
        assert!(source.get("agent_routes").is_none());
        assert!(source.get("profiles").is_none());
        let original = serde_json::to_vec(&source).expect("config fixture serializes");
        std::fs::write(&path, &original).expect("config fixture writes");

        let (draft, saved, error) = load_draft_state(
            &path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );

        assert_eq!(error, None);
        assert!(saved.get("agent_routes").is_none());
        assert!(saved.get("profiles").is_none());
        assert!(draft.get("agent_routes").is_none());
        assert!(draft.get("profiles").is_none());
        serde_json::from_value::<ClientConfig>(draft)
            .expect("loaded desktop draft remains structurally valid");
        assert_eq!(
            std::fs::read(&path).expect("source config remains readable"),
            original
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn loading_legacy_null_optional_maps_recovers_without_read_only_lockout() {
        let root = scratch_home("null-optional-routing-maps");
        let path = root.join("token-station.json");
        let mut source = template_for_test(&root);
        source["agent_routes"] = Value::Null;
        source["profiles"] = Value::Null;
        source["agent_budgets"] = Value::Null;
        std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();

        let (draft, saved, error) = load_draft_state(
            &path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );

        assert_eq!(error, None);
        assert!(saved.get("agent_routes").is_none());
        assert!(saved.get("profiles").is_none());
        assert!(saved.get("agent_budgets").is_none());
        serde_json::from_value::<ClientConfig>(draft)
            .expect("legacy null optional maps recover to empty maps");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_desktop_legacy_zero_concurrency_config_loads_writable_without_rewriting_source() {
        let root = scratch_home("legacy-zero-concurrency");
        let path = root.join("token-station.json");
        let mut legacy = template_for_test(&root);
        legacy["server"]["auth"] = json!(false);
        legacy["concurrency"] = json!({
            "global": 0,
            "per_agent": 0,
            "per_provider": 0
        });
        let original = serde_json::to_vec(&legacy).expect("legacy fixture serializes");
        std::fs::write(&path, &original).expect("legacy fixture writes");

        let (draft, error) = load_draft(&path, &root);

        assert_eq!(error, None);
        assert_eq!(draft["concurrency"]["global"], json!(64));
        assert_eq!(draft["concurrency"]["per_agent"], json!(16));
        assert_eq!(draft["concurrency"]["per_provider"], json!(16));
        assert_eq!(draft["server"]["auth"], json!(false));
        assert_eq!(
            std::fs::read(&path).expect("legacy source remains readable"),
            original
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_vision_declaration_updates_the_public_state() {
        let root = scratch_home("model-vision");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{
                "model": "vision-model",
                "vision": false,
                "vision_state": "unknown",
                "context_window": 128000
            }]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let declared = set_provider_model_vision(
            app.state(),
            "provider".to_owned(),
            "vision-model".to_owned(),
            true,
        )
        .expect("a configured model can be declared vision-capable");
        let model = &declared.providers[0].model_capabilities[0];
        assert_eq!(model.vision, CapabilityState::Declared);

        let unsupported = set_provider_model_vision(
            app.state(),
            "provider".to_owned(),
            "vision-model".to_owned(),
            false,
        )
        .expect("an operator can explicitly disable vision routing");
        let model = &unsupported.providers[0].model_capabilities[0];
        assert_eq!(model.vision, CapabilityState::Unsupported);

        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("token-station.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["upstreams"]["provider"]["models"][0]["vision_state"],
            json!("unsupported")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_limits_require_a_positive_output_within_context_and_persist_atomically() {
        let root = scratch_home("model-limits");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{
                "model": "bounded-model",
                "tool": true,
                "context_window": 128000
            }]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let before = inner.draft.clone();

        let error =
            replace_provider_model_limits(&mut inner, "provider", "bounded-model", 128_000, 0)
                .expect_err("a missing maximum output remains unproven");
        assert!(error.contains("大于 0"), "{error}");
        assert_eq!(inner.draft, before);

        let error = replace_provider_model_limits(
            &mut inner,
            "provider",
            "bounded-model",
            128_000,
            128_001,
        )
        .expect_err("output cannot exceed the context window");
        assert!(error.contains("不能大于"), "{error}");
        assert_eq!(inner.draft, before);

        replace_provider_model_limits(&mut inner, "provider", "bounded-model", 128_000, 32_768)
            .expect("operator-confirmed limits persist");
        let model = &inner.draft["upstreams"]["provider"]["models"][0];
        assert_eq!(model["context_window"], json!(128_000));
        assert_eq!(model["max_output_tokens"], json!(32_768));
        assert_eq!(
            model[CONTEXT_WINDOW_SOURCE_KEY],
            json!(LIMIT_SOURCE_OPERATOR)
        );
        assert_eq!(
            model[MAX_OUTPUT_TOKENS_SOURCE_KEY],
            json!(LIMIT_SOURCE_OPERATOR)
        );
        assert_eq!(model["tool"], json!(true));

        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("token-station.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["upstreams"]["provider"]["models"][0]["max_output_tokens"],
            json!(32_768)
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn trusted_catalog_vision_facts_update_configured_models() {
        let root = scratch_home("catalog-vision");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["openrouter"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://openrouter.ai/api/v1",
            "models": [
                {"model": "vision-model", "vision": false, "context_window": 128000},
                {"model": "text-model", "vision": true, "vision_state": "declared", "context_window": 128000}
            ]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let catalog = vec![
            model_catalog::CatalogModelView {
                model: "vision-model".to_owned(),
                tool: CapabilityState::Unknown,
                vision: CapabilityState::Verified,
                json_schema: CapabilityState::Unknown,
                context_window: Some(257_550),
                max_output_tokens: Some(32_768),
                cost: Some(model_catalog::CatalogCostView {
                    input: Some(0.2),
                    output: Some(0.6),
                    cache_read: Some(0.04),
                    cache_write: None,
                }),
                source: model_catalog::CatalogSource::Live,
                last_seen_ms: Some(42),
                catalog_state: model_catalog::CatalogState::Active,
            },
            model_catalog::CatalogModelView {
                model: "text-model".to_owned(),
                tool: CapabilityState::Unknown,
                vision: CapabilityState::Unsupported,
                json_schema: CapabilityState::Unknown,
                context_window: None,
                max_output_tokens: None,
                cost: None,
                source: model_catalog::CatalogSource::Live,
                last_seen_ms: Some(42),
                catalog_state: model_catalog::CatalogState::Active,
            },
        ];

        assert!(
            apply_discovered_model_capabilities(&mut inner, "openrouter", &catalog)
                .expect("trusted catalog facts apply")
        );

        let models = inner.draft["upstreams"]["openrouter"]["models"]
            .as_array()
            .unwrap();
        assert_eq!(models[0]["vision"], json!(true));
        assert_eq!(models[0]["vision_state"], json!("verified"));
        assert_eq!(
            models[0]["context_window"],
            json!(128000),
            "a non-zero legacy value without a default source remains operator-owned"
        );
        assert_eq!(models[0]["max_output_tokens"], json!(32768));
        assert_eq!(
            models[0]["catalog_cost"],
            json!({"input": 0.2, "output": 0.6, "cache_read": 0.04})
        );
        assert_eq!(models[1]["vision"], json!(false));
        assert_eq!(models[1]["vision_state"], json!("unsupported"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn catalog_output_larger_than_the_existing_context_is_ignored() {
        let root = scratch_home("catalog-invalid-output");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "bounded", "context_window": 32000}]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let catalog = vec![model_catalog::CatalogModelView {
            model: "bounded".to_owned(),
            tool: CapabilityState::Unknown,
            vision: CapabilityState::Unknown,
            json_schema: CapabilityState::Unknown,
            context_window: None,
            max_output_tokens: Some(64_000),
            cost: None,
            source: model_catalog::CatalogSource::Live,
            last_seen_ms: Some(42),
            catalog_state: model_catalog::CatalogState::Active,
        }];

        assert!(!apply_discovered_model_capabilities(&mut inner, "provider", &catalog).unwrap());
        assert!(inner.draft["upstreams"]["provider"]["models"][0]
            .get("max_output_tokens")
            .is_none());
        inner
            .materialize()
            .expect("catalog refresh keeps the draft valid");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_context_replaces_preset_and_clears_an_incompatible_preset_output() {
        let root = scratch_home("catalog-provider-over-preset");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["kimi"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.moonshot.cn/v1",
            "models": [{
                "model": "kimi-k3",
                "context_window": 1048576,
                "max_output_tokens": 131072,
                "x-token-station-context-window-source": "builtin_preset",
                "x-token-station-max-output-tokens-source": "builtin_preset"
            }]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let catalog = vec![model_catalog::CatalogModelView {
            model: "kimi-k3".to_owned(),
            tool: CapabilityState::Unknown,
            vision: CapabilityState::Unknown,
            json_schema: CapabilityState::Unknown,
            context_window: Some(64_000),
            max_output_tokens: None,
            cost: None,
            source: model_catalog::CatalogSource::Live,
            last_seen_ms: Some(42),
            catalog_state: model_catalog::CatalogState::Active,
        }];

        assert!(apply_discovered_model_capabilities(&mut inner, "kimi", &catalog).unwrap());
        let model = &inner.draft["upstreams"]["kimi"]["models"][0];
        assert_eq!(model["context_window"], json!(64_000));
        assert_eq!(
            model["x-token-station-context-window-source"],
            json!("provider")
        );
        assert!(model.get("max_output_tokens").is_none());
        assert!(model
            .get("x-token-station-max-output-tokens-source")
            .is_none());
        inner
            .materialize()
            .expect("conflicting metadata remains valid");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn catalog_refresh_fills_unknown_metadata_without_overwriting_operator_values() {
        let root = scratch_home("catalog-metadata-ownership");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [
                {"model": "operator-owned", "context_window": 64000, "max_output_tokens": 8000},
                {"model": "unknown", "context_window": 0}
            ]
        });
        draft["pricing"] = json!({
            "version": 4,
            "models": {
                "provider/operator-owned": {
                    "input_per_mtok": 900000,
                    "output_per_mtok": 1800000
                }
            }
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let catalog_cost = model_catalog::CatalogCostView {
            input: Some(0.2),
            output: Some(0.6),
            cache_read: Some(0.02),
            cache_write: Some(0.2),
        };
        let catalog_price = catalog_cost_to_model_price(&catalog_cost).unwrap();
        let catalog = ["operator-owned", "unknown"]
            .into_iter()
            .map(|model| model_catalog::CatalogModelView {
                model: model.to_owned(),
                tool: CapabilityState::Unknown,
                vision: CapabilityState::Unknown,
                json_schema: CapabilityState::Unknown,
                context_window: Some(128_000),
                max_output_tokens: Some(32_000),
                cost: Some(catalog_cost.clone()),
                source: model_catalog::CatalogSource::Live,
                last_seen_ms: Some(42),
                catalog_state: model_catalog::CatalogState::Active,
            })
            .collect::<Vec<_>>();

        assert!(apply_discovered_model_capabilities(&mut inner, "provider", &catalog).unwrap());

        let models = inner.draft["upstreams"]["provider"]["models"]
            .as_array()
            .unwrap();
        assert_eq!(models[0]["context_window"], json!(64_000));
        assert_eq!(models[0]["max_output_tokens"], json!(8_000));
        assert_eq!(models[1]["context_window"], json!(128_000));
        assert_eq!(models[1]["max_output_tokens"], json!(32_000));
        let pricing = draft_price_table(&inner).unwrap();
        assert_eq!(
            pricing.version, 5,
            "only the previously unknown price is added"
        );
        assert_eq!(
            pricing.models["provider/operator-owned"].input_per_mtok,
            900_000
        );
        assert_eq!(pricing.models["provider/unknown"], catalog_price);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn catalog_cost_requires_every_billed_token_class() {
        let partial = model_catalog::CatalogCostView {
            input: Some(0.2),
            output: Some(0.6),
            cache_read: Some(0.02),
            cache_write: None,
        };
        assert!(catalog_cost_to_model_price(&partial).is_none());
    }

    #[test]
    fn provider_model_updates_preserve_metadata_and_protect_routing_references() {
        let root = scratch_home("model-update");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["moonshot"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.moonshot.cn/v1",
            "models": [
                {
                    "model": "moonshot-v1-8k",
                    "tool": false,
                    "context_window": 8192
                }
            ]
        });
        draft["router"]["pools"][TIER_LOW] =
            json!([{ "upstream": "moonshot", "model": "moonshot-v1-8k" }]);
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        inner.rebuild_routing();

        let error = replace_provider_models(&mut inner, "moonshot", vec!["kimi-k2.6".to_owned()])
            .expect_err("the routed model cannot be removed");
        assert!(error.contains("下档"), "{error}");

        replace_provider_models(
            &mut inner,
            "moonshot",
            vec![
                "moonshot-v1-8k".to_owned(),
                "kimi-k2.6".to_owned(),
                "kimi-k2.6".to_owned(),
            ],
        )
        .expect("retaining the routed model is valid");
        let models = inner.draft["upstreams"]["moonshot"]["models"]
            .as_array()
            .unwrap();
        assert_eq!(models.len(), 2);
        let retained = models
            .iter()
            .find(|model| model["model"] == json!("moonshot-v1-8k"))
            .unwrap();
        assert_eq!(retained["tool"], json!(false));
        assert_eq!(retained["context_window"], json!(8192));
        let preset = models
            .iter()
            .find(|model| model["model"] == json!("kimi-k2.6"))
            .unwrap();
        assert_eq!(preset["context_window"], json!(262_144));
        assert_eq!(preset["max_output_tokens"], json!(262_144));
        assert_eq!(
            preset[MAX_OUTPUT_TOKENS_SOURCE_KEY],
            json!(LIMIT_SOURCE_BUILTIN_PRESET)
        );
        assert!(std::fs::read_to_string(&inner.config_path)
            .unwrap()
            .contains("kimi-k2.6"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_mutations_protect_home_agent_direct_and_quota_references() {
        let root = scratch_home("provider-direct-quota-references");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [
                {"model": "home-direct"},
                {"model": "agent-direct"},
                {"model": "quota"},
                {"model": "keep"}
            ]
        });
        draft["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "provider", "model": "home-direct"}
        });
        draft["router"]["quota_accounts"] = json!([{"upstream": "provider", "model": "quota"}]);
        draft["agent_routes"]["codex"] = json!({
            "mode": "inherit",
            "direct_target": {"upstream": "provider", "model": "agent-direct"}
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);

        let references = provider_references(&inner, "provider");
        assert!(
            references.contains(&"主页/单独路由".to_owned()),
            "{references:?}"
        );
        assert!(
            references.contains(&"Agent/codex/单独路由".to_owned()),
            "{references:?}"
        );
        assert!(
            references.contains(&"主页/额度优先#1".to_owned()),
            "{references:?}"
        );

        let before = inner.draft.clone();
        let error = replace_provider_models(&mut inner, "provider", vec!["keep".to_owned()])
            .expect_err("every direct and quota target protects its model");
        assert!(
            error.contains("单独路由") && error.contains("额度优先"),
            "{error}"
        );
        assert_eq!(inner.draft, before);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_updates_respect_broken_config_read_only_protection() {
        let root = scratch_home("model-update-read-only");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "keep"}]
        });
        let before = draft.clone();
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            draft,
            Some("只读保护".to_owned()),
        );

        let error = replace_provider_models(&mut inner, "provider", vec!["replacement".to_owned()])
            .expect_err("read-only protection blocks model writes");
        assert!(error.contains("只读保护"), "{error}");
        assert_eq!(inner.draft, before);
        assert!(!inner.config_path.exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_updates_protect_inactive_agent_route_drafts() {
        let root = scratch_home("model-update-agent-route");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("home".into()))
            .unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        for slot in ["high", "mid", "low"] {
            set_agent_tier(
                app.state(),
                "codex".to_owned(),
                slot.to_owned(),
                Some("provider".to_owned()),
                Some("agent".to_owned()),
            )
            .unwrap();
        }
        save_agent_routes(app.state()).unwrap();
        set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();

        let error = match update_provider_models(
            app.state(),
            "provider".to_owned(),
            vec!["home".to_owned()],
        ) {
            Ok(_) => panic!("inactive custom drafts still protect their model references"),
            Err(error) => error,
        };
        assert!(error.contains("codex/high"), "{error}");
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(
            inner.draft["upstreams"]["provider"]["models"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_updates_protect_unsaved_agent_route_editors() {
        let root = scratch_home("model-update-agent-editor");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            "high".to_owned(),
            Some("provider".to_owned()),
            Some("agent".to_owned()),
        )
        .unwrap();

        let error = match update_provider_models(
            app.state(),
            "provider".to_owned(),
            vec!["home".to_owned()],
        ) {
            Ok(_) => panic!("an unsaved Agent editor must protect its selected model"),
            Err(error) => error,
        };

        assert!(error.contains("codex/high"), "{error}");
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(
            inner.draft["upstreams"]["provider"]["models"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            inner.agent_route_drafts["codex"]["high"].model.as_deref(),
            Some("agent")
        );
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_removal_reports_unsaved_agent_route_editor_references() {
        let root = scratch_home("provider-removal-agent-editor");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "agent"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            "high".to_owned(),
            Some("provider".to_owned()),
            Some("agent".to_owned()),
        )
        .unwrap();

        let preview = preview_provider_removal(app.state(), "provider".to_owned()).unwrap();
        assert!(!preview.can_remove);
        assert_eq!(preview.references, ["Agent/codex/high"]);
        let error = match remove_provider(app.state(), "provider".to_owned()) {
            Ok(_) => panic!("an editor reference must block Provider removal"),
            Err(error) => error,
        };
        assert!(error.contains("Agent/codex/high"), "{error}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tier_keywords_write_valid_rules_dedupe_and_require_a_configured_pool() {
        let root = scratch_home("tier-keywords");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "m"}]
        });

        // Unconfigured tiers cannot accept keywords because the rule would target an empty pool.
        let error = inner
            .add_tier_keyword("low", "提交git")
            .expect_err("adding to an unconfigured tier is refused");
        assert!(error.contains("先"), "{error}");

        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("m".into()))
            .unwrap();

        inner.add_tier_keyword("low", "提交git").unwrap();
        // Deduplicate case-insensitively.
        let dup = inner
            .add_tier_keyword("low", "提交GIT")
            .expect_err("case-insensitive duplicate is refused");
        assert!(dup.contains("已在"), "{dup}");

        // The keyword enters the low-tier rule targeting tier_low, and the full config validates.
        let keywords = inner.home_keywords();
        assert_eq!(keywords["low"], vec!["提交git".to_string()]);
        let config = inner
            .materialize()
            .expect("keyword rule keeps config valid");
        let rule = config
            .router
            .rules
            .iter()
            .find(|rule| rule.id == KW_RULE_LOW)
            .expect("low keyword rule exists");
        assert_eq!(rule.route_to, TIER_LOW);
        assert_eq!(rule.matcher.keywords_any, vec!["提交git".to_string()]);

        // Remove case-insensitively and delete the rule when its list empties instead of leaving empty keywords_any.
        inner.remove_tier_keyword("low", "提交GIT").unwrap();
        assert!(inner.home_keywords()["low"].is_empty());
        assert!(inner.materialize().unwrap().router.rules.is_empty());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn clearing_a_tier_drops_its_keyword_rule_so_the_config_stays_valid() {
        let root = scratch_home("tier-keywords-clear");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "m"}]
        });
        // Keep another fallback tier so clearing the only tier does not empty pools.
        inner
            .set_tier_value(TIER_HIGH, Some("provider".into()), Some("m".into()))
            .unwrap();
        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("m".into()))
            .unwrap();
        inner.add_tier_keyword("low", "翻译").unwrap();
        assert!(inner
            .materialize()
            .unwrap()
            .router
            .rules
            .iter()
            .any(|rule| rule.id == KW_RULE_LOW));

        // Clearing the low tier must also remove its keyword rule or route_to would target an empty pool.
        inner.set_tier_value(TIER_LOW, None, None).unwrap();
        let config = inner
            .materialize()
            .expect("clearing a tier leaves a valid config, not a dangling rule");
        assert!(config
            .router
            .rules
            .iter()
            .all(|rule| rule.id != KW_RULE_LOW));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stored_discovery_credentials_cannot_be_redirected_to_another_base_url() {
        let root = scratch_home("model-discovery-url-binding");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://trusted.example/v1",
            "auth": {"slot": "provider_api_key", "store": true},
            "models": [{"model": "model"}]
        });
        let inner = AppInner::new(root.join("token-station.json"), draft, None);

        let error =
            prepare_discovery_credential(&inner, "provider", "https://attacker.example/v1", None)
                .expect_err("a stored credential is bound to its configured URL");
        assert!(error.contains("Base URL 必须与供应商配置一致"), "{error}");

        let one_time = prepare_discovery_credential(
            &inner,
            "new-provider",
            "https://new.example/v1",
            Some("one-time-secret"),
        )
        .expect("an explicit one-time key is accepted");
        assert_eq!(
            one_time,
            DiscoveryCredential::Explicit(Some("one-time-secret".to_owned()))
        );

        let stored =
            prepare_discovery_credential(&inner, "provider", "https://trusted.example/v1", None)
                .expect("stored credentials are prepared without resolving the keyring");
        assert_eq!(
            stored,
            DiscoveryCredential::Stored {
                provider: "provider".to_owned(),
                slot: "provider_api_key".to_owned(),
            }
        );

        let openrouter = prepare_discovery_credential(
            &inner,
            "openrouter",
            "https://openrouter.ai/api/v1",
            None,
        )
        .expect("OpenRouter's public catalog needs no stored credential");
        assert_eq!(openrouter, DiscoveryCredential::Explicit(None));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn azure_model_discovery_fails_before_resolving_any_credential() {
        let root = scratch_home("azure-model-discovery");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["azure"] = json!({
            "provider": "azure-openai-v1",
            "base_url": "https://fixture.openai.azure.com/openai/v1",
            "auth": {"slot": "provider_api_key", "store": true},
            "models": [{"model": "deployment-fixture"}]
        });
        let inner = AppInner::new(root.join("token-station.json"), draft, None);

        for api_key in [None, Some("one-time-secret")] {
            let error = prepare_discovery_credential(
                &inner,
                "azure",
                "https://fixture.openai.azure.com/openai/v1",
                api_key,
            )
            .expect_err("Azure deployments are configured manually, never fetched with Bearer");
            assert_eq!(error, "model_catalog_azure_deployment_manual");
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tier_updates_refuse_unknown_provider_model_and_partial_values() {
        let root = scratch_home("tiers-invalid");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["deepseek"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.deepseek.com",
            "models": [{"model": "deepseek-chat"}]
        });

        assert!(inner
            .set_tier_value(TIER_HIGH, Some("missing".into()), Some("model".into()))
            .unwrap_err()
            .contains("未知供应商"));
        assert!(inner
            .set_tier_value(
                TIER_HIGH,
                Some("deepseek".into()),
                Some("missing-model".into())
            )
            .unwrap_err()
            .contains("未配置模型"));
        assert!(inner
            .set_tier_value(TIER_HIGH, Some("deepseek".into()), None)
            .unwrap_err()
            .contains("同时提供"));
        assert!(inner.draft["router"]["pools"]
            .as_object()
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn entering_an_incomplete_agent_route_keeps_the_global_config_valid_and_clean() {
        let root = scratch_home("agent-route-editor-isolation");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));
        let before = get_state(app.state());

        let editing =
            set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();

        assert_eq!(editing.agent_routes["codex"].mode, "custom");
        assert_eq!(
            editing.agent_routes["codex"].config_error.as_deref(),
            Some("Agent `codex` 的 high 档缺少供应商和模型")
        );
        assert_eq!(editing.config_error, None);
        assert_eq!(editing.draft_revision, before.draft_revision);
        assert_eq!(editing.saved_revision, before.saved_revision);
        assert_eq!(editing.config_dirty, before.config_dirty);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejecting_an_agent_route_target_keeps_editor_and_global_state_unchanged() {
        let root = scratch_home("agent-route-target-rollback");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));
        let before = get_state(app.state());

        let error = match set_agent_tier(
            app.state(),
            "codex".to_owned(),
            "high".to_owned(),
            Some("missing-provider".to_owned()),
            Some("missing-model".to_owned()),
        ) {
            Ok(_) => panic!("an unknown provider must be rejected"),
            Err(error) => error,
        };

        assert_eq!(error, "未知供应商 `missing-provider`");
        let after = get_state(app.state());
        assert_eq!(after.agent_routes["codex"].mode, "inherit");
        assert_eq!(after.agent_routes["codex"].config_error, None);
        assert_eq!(after.config_error, None);
        assert_eq!(after.draft_revision, before.draft_revision);
        assert_eq!(after.saved_revision, before.saved_revision);
        assert_eq!(after.config_dirty, before.config_dirty);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn incomplete_agent_editor_does_not_block_configuring_and_saving_home() {
        let root = scratch_home("agent-route-home-save");
        let config_path = root.join("token-station.json");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            template_for_test(&root),
            None,
        )))));

        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        add_provider(
            app.state(),
            "provider".to_owned(),
            "https://example.com/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
        )
        .unwrap();
        for slot in ["high", "mid", "low"] {
            set_tier(
                app.state(),
                slot.to_owned(),
                Some("provider".to_owned()),
                Some("model".to_owned()),
            )
            .unwrap();
        }

        let saved = save_config(app.state()).unwrap();

        assert_eq!(saved.config_error, None);
        assert!(!saved.config_dirty);
        assert_eq!(saved.agent_routes["codex"].mode, "custom");
        assert_eq!(
            saved.agent_routes["codex"].config_error.as_deref(),
            Some("Agent `codex` 的 high 档缺少供应商和模型")
        );
        assert!(saved
            .tiers
            .values()
            .all(|tier| tier.upstream.as_deref() == Some("provider")
                && tier.model.as_deref() == Some("model")));
        let config = ClientConfig::load(&config_path).unwrap();
        assert!(
            !config.agent_routes.contains_key("codex"),
            "an incomplete editor must not enter the saved ClientConfig"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn saving_an_incomplete_agent_editor_fails_without_touching_config_state_or_disk() {
        let root = scratch_home("agent-route-incomplete-save");
        let config_path = root.join("token-station.json");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            template_for_test(&root),
            None,
        )))));
        let editing =
            set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();

        let error = match save_agent_routes(app.state()) {
            Ok(_) => panic!("an incomplete Agent editor cannot be saved"),
            Err(error) => error,
        };

        assert_eq!(error, "Agent `codex` 的 high 档缺少供应商和模型");
        let after = get_state(app.state());
        assert_eq!(after.agent_routes["codex"].mode, "custom");
        assert_eq!(after.config_error, None);
        assert_eq!(after.draft_revision, editing.draft_revision);
        assert_eq!(after.saved_revision, editing.saved_revision);
        assert_eq!(after.config_dirty, editing.config_dirty);
        assert!(!config_path.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn completing_and_saving_an_agent_editor_commits_one_valid_custom_route() {
        let root = scratch_home("agent-route-complete-save");
        let config_path = root.join("token-station.json");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            template_for_test(&root),
            None,
        )))));
        add_provider(
            app.state(),
            "provider".to_owned(),
            "https://example.com/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
        )
        .unwrap();
        let before =
            set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        for slot in ["high", "mid", "low"] {
            let editing = set_agent_tier(
                app.state(),
                "codex".to_owned(),
                slot.to_owned(),
                Some("provider".to_owned()),
                Some("model".to_owned()),
            )
            .unwrap();
            assert_eq!(editing.config_error, None);
            assert_eq!(editing.draft_revision, before.draft_revision);
        }

        let saved = save_agent_routes(app.state()).unwrap();

        assert_eq!(saved.agent_routes["codex"].mode, "custom");
        assert_eq!(saved.agent_routes["codex"].config_error, None);
        assert!(!saved.config_dirty);
        assert!(saved.draft_revision > before.draft_revision);
        assert_eq!(saved.draft_revision, saved.saved_revision);
        let config = ClientConfig::load(&config_path).unwrap();
        let route = config.agent_routes["codex"].custom_route.as_ref().unwrap();
        for target in [&route.high, &route.mid, &route.low] {
            assert_eq!(target.upstream.as_str(), "provider");
            assert_eq!(target.model, "model");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_route_drafts_seed_from_home_validate_targets_and_preserve_complete_profiles() {
        let root = scratch_home("agent-route-draft");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(pool, Some("provider".into()), Some("home".into()))
                .unwrap();
        }
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        let editing =
            set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        assert_eq!(
            editing.agent_routes["codex"].tiers["high"].model.as_deref(),
            Some("home")
        );
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            "high".to_owned(),
            Some("provider".to_owned()),
            Some("agent".to_owned()),
        )
        .unwrap();
        let unknown = match set_agent_tier(
            app.state(),
            "future-agent".to_owned(),
            "high".to_owned(),
            Some("provider".to_owned()),
            Some("agent".to_owned()),
        ) {
            Ok(_) => panic!("an unknown Agent must be rejected"),
            Err(error) => error,
        };
        assert!(unknown.contains("未知 Agent"), "{unknown}");
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert!(
                !inner
                    .materialize()
                    .unwrap()
                    .agent_routes
                    .contains_key("codex"),
                "editor state stays outside ClientConfig until save"
            );
        }
        save_agent_routes(app.state()).unwrap();
        let config = ClientConfig::load(&root.join("token-station.json")).unwrap();
        assert_eq!(
            config.agent_routes["codex"]
                .custom_route
                .as_ref()
                .unwrap()
                .high
                .model,
            "agent"
        );

        let inherited =
            set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();
        assert_eq!(inherited.agent_routes["codex"].mode, "inherit");
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_object());
        assert!(inner.materialize().is_ok());
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restoring_one_agent_to_home_clears_every_routing_override() {
        let root = scratch_home("agent-restore-clears-routing-overrides");
        let config_path = root.join("token-station.json");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            draft,
            None,
        )))));
        set_direct_route(app.state(), "provider".to_owned(), "home".to_owned(), None).unwrap();
        set_direct_route(
            app.state(),
            "provider".to_owned(),
            "agent".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();

        let restored =
            set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();

        assert_eq!(restored.agent_routes["codex"].routing_mode, "tiered");
        assert_eq!(
            restored.agent_routes["codex"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("home")
        );
        save_agent_routes(app.state()).unwrap();
        let saved = ClientConfig::load(&config_path).unwrap();
        assert!(saved.agent_routes["codex"].routing_mode.is_none());
        assert!(saved.agent_routes["codex"].direct_target.is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn returning_an_incomplete_agent_draft_to_inherit_cannot_poison_home_config() {
        let root = scratch_home("agent-route-incomplete");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "model"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("model".into()))
            .unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        let editing =
            set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        assert!(editing.agent_routes["codex"].config_error.is_some());
        assert_eq!(editing.config_error, None);
        let inherited =
            set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();
        assert_eq!(inherited.agent_routes["codex"].mode, "inherit");
        assert_eq!(inherited.config_error, None);
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_null());
        assert!(inner.materialize().is_ok());
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn applying_direct_agent_route_ignores_and_preserves_incomplete_tier_draft() {
        let root = scratch_home("agent-direct-with-incomplete-tier-draft");
        let config_path = root.join("token-station.json");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "direct"}, {"model": "tier"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            draft,
            None,
        )))));
        manage_test_agent_state(&app, &root);

        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            "high".to_owned(),
            Some("provider".to_owned()),
            Some("tier".to_owned()),
        )
        .unwrap();
        set_direct_route(
            app.state(),
            "provider".to_owned(),
            "direct".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();

        let applied = restart_agent_route(app.state(), app.state(), "codex".to_owned())
            .expect("Direct apply must not promote an unrelated incomplete tier draft");

        assert_eq!(applied.agent_routes["codex"].routing_mode, "direct");
        assert_eq!(
            applied.agent_routes["codex"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("direct")
        );
        let saved = ClientConfig::load(&config_path).expect("applied Direct route persists");
        let saved_route = &saved.agent_routes["codex"];
        assert_eq!(saved_route.routing_mode, Some(HostRoutingMode::Direct));
        assert_eq!(saved_route.direct_target.as_ref().unwrap().model, "direct");
        assert!(saved_route.custom_route.is_none());
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            let tier_draft = &inner.agent_route_drafts["codex"];
            assert_eq!(tier_draft["high"].model.as_deref(), Some("tier"));
            assert!(tier_draft["mid"].model.is_none());
            assert!(tier_draft["low"].model.is_none());
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restarting_one_agent_route_rejects_an_apply_already_in_progress_before_saving() {
        let root = scratch_home("agent-route-during-apply");
        let config_path = root.join("token-station.json");
        let mut draft = gateway_template_for_test(&root);
        draft["server"]["listen"] = json!("127.0.0.1:0");
        draft["server"]["auth"] = json!(false);
        draft["data"]["metrics"] = json!(false);
        draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        draft["router"]["pools"] = json!({
            TIER_LOW: [{"upstream": "local", "model": "small"}]
        });
        draft["router"]["default_pool"] = json!(TIER_LOW);
        let config: ClientConfig = serde_json::from_value(draft.clone()).unwrap();
        let running = prepare_server(config)
            .unwrap()
            .bind()
            .unwrap()
            .publish(7)
            .unwrap();
        let mut inner = AppInner::new(config_path.clone(), draft, None);
        inner.server = ServerLifecycle::Applying {
            generation: 8,
            revision: 2,
            old: running,
        };
        let saved_before = inner.config_state.saved_revision();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        let error = match restart_agent_route(app.state(), app.state(), "opencode".to_owned()) {
            Ok(_) => panic!("an already frozen apply cannot accept another Agent route revision"),
            Err(error) => error,
        };

        assert!(error.contains("apply_in_progress"), "{error}");
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        assert_eq!(inner.config_state.saved_revision(), saved_before);
        let lifecycle = std::mem::replace(
            &mut inner.server,
            ServerLifecycle::Stopped { generation: 9 },
        );
        drop(inner);
        let ServerLifecycle::Applying { old, .. } = lifecycle else {
            panic!("the rejected command must leave the applying runtime in place");
        };
        old.drain_and_shutdown();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restarting_one_agent_route_rejects_draft_only_targets_before_saving() {
        let root = scratch_home("agent-route-draft-only-target");
        let config_path = root.join("token-station.json");
        let (serving_draft, running) = published_agent_route_fixture(&root);
        let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
        serving_config.save(&config_path).unwrap();
        let persisted_before = std::fs::read(&config_path).unwrap();
        let mut latest_draft = serving_draft.clone();
        latest_draft["upstreams"]["draft_only"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://draft-only.example/v1",
            "models": [{"model": "new-model"}]
        });
        latest_draft["agent_routes"]["opencode"] = json!({
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "draft_only", "model": "new-model"}
        });
        let mut inner =
            AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
        inner.server = ServerLifecycle::Running {
            generation: 7,
            server: running,
            apply_error: None,
        };
        let saved_before = inner.config_state.saved_revision();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        let outcome = restart_agent_route(app.state(), app.state(), "opencode".to_owned());
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let saved_after = inner.config_state.saved_revision();
        let override_after = match &inner.server {
            ServerLifecycle::Running { server, .. } => server
                .agent_router_override("opencode")
                .map(|router| router.cloned()),
            _ => panic!("a rejected Agent route must leave the proxy running"),
        };
        let lifecycle = std::mem::replace(
            &mut inner.server,
            ServerLifecycle::Stopped { generation: 8 },
        );
        drop(inner);
        let ServerLifecycle::Running { server, .. } = lifecycle else {
            unreachable!()
        };
        server.drain_and_shutdown();

        let error = match outcome {
            Err(error) => error,
            Ok(_) => panic!("a draft-only target cannot be installed into the old Gateway"),
        };
        assert!(error.contains("draft_only/new-model"), "{error}");
        assert!(error.contains("全量应用"), "{error}");
        assert_eq!(saved_after, saved_before);
        assert_eq!(std::fs::read(&config_path).unwrap(), persisted_before);
        assert_eq!(override_after, None);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restarting_one_agent_route_prepares_the_router_before_saving() {
        let root = scratch_home("agent-route-invalid-router");
        let config_path = root.join("token-station.json");
        let (serving_draft, running) = published_agent_route_fixture(&root);
        let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
        serving_config.save(&config_path).unwrap();
        let persisted_before = std::fs::read(&config_path).unwrap();
        let mut latest_draft = serving_draft.clone();
        latest_draft["router"]["assumed_context_window"] = json!(0);
        latest_draft["agent_routes"]["opencode"] = json!({
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "local", "model": "small"}
        });
        let mut inner =
            AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
        inner.server = ServerLifecycle::Running {
            generation: 7,
            server: running,
            apply_error: None,
        };
        let saved_before = inner.config_state.saved_revision();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        let outcome = restart_agent_route(app.state(), app.state(), "opencode".to_owned());
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let saved_after = inner.config_state.saved_revision();
        let override_after = match &inner.server {
            ServerLifecycle::Running { server, .. } => server
                .agent_router_override("opencode")
                .map(|router| router.cloned()),
            _ => panic!("an invalid Agent router must leave the proxy running"),
        };
        let lifecycle = std::mem::replace(
            &mut inner.server,
            ServerLifecycle::Stopped { generation: 8 },
        );
        drop(inner);
        let ServerLifecycle::Running { server, .. } = lifecycle else {
            unreachable!()
        };
        server.drain_and_shutdown();

        let error = match outcome {
            Err(error) => error,
            Ok(_) => panic!("an invalid router must fail during the prepare phase"),
        };
        assert!(error.contains("assumed_context_window"), "{error}");
        assert_eq!(saved_after, saved_before);
        assert_eq!(std::fs::read(&config_path).unwrap(), persisted_before);
        assert_eq!(override_after, None);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restarting_one_agent_route_commits_and_installs_one_prevalidated_plan() {
        let root = scratch_home("agent-route-prevalidated-success");
        let config_path = root.join("token-station.json");
        let (serving_draft, running) = published_agent_route_fixture(&root);
        let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
        serving_config.save(&config_path).unwrap();
        let mut latest_draft = serving_draft.clone();
        latest_draft["agent_routes"]["opencode"] = json!({
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "local", "model": "small"}
        });
        let mut inner =
            AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
        inner.server = ServerLifecycle::Running {
            generation: 7,
            server: running,
            apply_error: None,
        };
        let saved_before = inner.config_state.saved_revision();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        let applied = restart_agent_route(app.state(), app.state(), "opencode".to_owned())
            .expect("a target in the serving snapshot can be prepared, saved, and installed");
        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        assert!(inner.config_state.saved_revision() > saved_before);
        let installed = match &inner.server {
            ServerLifecycle::Running { server, .. } => server
                .agent_router_override("opencode")
                .and_then(|router| router)
                .cloned()
                .expect("the committed router is recorded on the running instance"),
            _ => panic!("the successful Agent reload must leave the proxy running"),
        };
        let persisted = ClientConfig::load(&config_path).unwrap();
        assert_eq!(
            Some(installed),
            persisted.custom_router_for_agent("opencode").unwrap()
        );
        assert_eq!(
            applied.agent_routes["opencode"]
                .direct_target
                .as_ref()
                .and_then(|target| target.model.as_deref()),
            Some("small")
        );
        let lifecycle = std::mem::replace(
            &mut inner.server,
            ServerLifecycle::Stopped { generation: 8 },
        );
        drop(inner);
        let ServerLifecycle::Running { server, .. } = lifecycle else {
            unreachable!()
        };
        server.drain_and_shutdown();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_route_commands_save_one_profile_and_apply_home_without_deleting_its_draft() {
        let root = scratch_home("agent-route-commands");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "model"}]
        });
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(pool, Some("provider".into()), Some("model".into()))
                .unwrap();
        }
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        let custom =
            set_agent_route_mode(app.state(), "codex".to_string(), "custom".to_string()).unwrap();
        assert_eq!(custom.agent_routes["codex"].mode, "custom");
        save_agent_routes(app.state()).unwrap();
        for agent_id in ["codex", "opencode"] {
            set_direct_route(
                app.state(),
                "provider".to_owned(),
                "model".to_owned(),
                Some(agent_id.to_owned()),
            )
            .unwrap();
            set_routing_mode(app.state(), "direct".to_owned(), Some(agent_id.to_owned())).unwrap();
        }
        let inherited = apply_home_route_to_all_agents(app.state(), app.state()).unwrap();
        assert!(inherited
            .agent_routes
            .values()
            .all(|profile| profile.mode == "inherit"));
        assert!(inherited
            .agent_routes
            .values()
            .all(|route| route.routing_mode == "tiered" && route.direct_target.is_none()));
        let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
        assert!(saved.agent_routes["codex"].custom_route.is_some());
        for agent_id in ["codex", "opencode"] {
            assert!(saved.agent_routes[agent_id].routing_mode.is_none());
            assert!(saved.agent_routes[agent_id].direct_target.is_none());
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn applying_home_routes_replaces_running_agent_overrides() {
        let root = scratch_home("agent-routes-apply-home-running");
        let config_path = root.join("token-station.json");
        let (serving_draft, mut running) = published_agent_route_fixture(&root);
        let mut custom_draft = serving_draft.clone();
        custom_draft["agent_routes"]["opencode"] = json!({
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "local", "model": "small"}
        });
        let custom_config: ClientConfig = serde_json::from_value(custom_draft.clone()).unwrap();
        custom_config.save(&config_path).unwrap();
        let custom_router = custom_config
            .custom_router_for_agent("opencode")
            .unwrap()
            .expect("the fixture has one custom Agent router");
        let prepared = running
            .prepare_agent_router_reload("opencode", Some(custom_router))
            .unwrap();
        running.install_prevalidated_agent_router(prepared);

        let mut inner =
            AppInner::new_with_saved(config_path.clone(), custom_draft, serving_draft, None);
        inner.server = ServerLifecycle::Running {
            generation: 7,
            server: running,
            apply_error: None,
        };
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        manage_test_agent_state(&app, &root);

        apply_home_route_to_all_agents(app.state(), app.state()).unwrap();

        let state = app.state::<AppStateManaged>();
        let mut inner = state.0.lock().unwrap();
        let override_after = match &inner.server {
            ServerLifecycle::Running { server, .. } => server
                .agent_router_override("opencode")
                .map(|router| router.cloned()),
            _ => panic!("applying Home routes must leave the proxy running"),
        };
        assert_eq!(override_after, Some(None));
        let persisted = ClientConfig::load(&config_path).unwrap();
        assert_eq!(persisted.custom_router_for_agent("opencode").unwrap(), None);
        let lifecycle = std::mem::replace(
            &mut inner.server,
            ServerLifecycle::Stopped { generation: 8 },
        );
        drop(inner);
        let ServerLifecycle::Running { server, .. } = lifecycle else {
            unreachable!()
        };
        server.drain_and_shutdown();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn named_profiles_are_draft_only_until_saved_and_can_be_shared_by_agents() {
        let root = scratch_home("named-agent-profile");
        let config_path = root.join("token-station.json");
        let mut inner = AppInner::new(config_path.clone(), template_for_test(&root), None);
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "shared"}]
        });
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(pool, Some("provider".into()), Some("shared".into()))
                .unwrap();
        }
        inner.observe_draft().unwrap();
        let before_revision = inner.config_state.draft_revision();
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        let missing =
            match mount_agent_profile(app.state(), "codex".to_string(), "missing".to_string()) {
                Ok(_) => panic!("an unknown profile cannot be mounted"),
                Err(error) => error,
            };
        assert!(missing.contains("不存在"), "{missing}");

        let profiled = save_home_route_as_profile(app.state(), "daily".to_string()).unwrap();
        assert_eq!(profiled.profiles, vec!["daily"]);
        assert!(profiled.config_dirty);
        assert!(profiled.draft_revision > before_revision);
        assert!(
            !config_path.exists(),
            "creating a profile must not bypass save"
        );

        for agent_id in ["codex", "opencode"] {
            let mounted =
                mount_agent_profile(app.state(), agent_id.to_string(), "daily".to_string())
                    .unwrap();
            assert_eq!(mounted.agent_routes[agent_id].mode, "profile");
            assert_eq!(
                mounted.agent_routes[agent_id].profile.as_deref(),
                Some("daily")
            );
            assert_eq!(
                mounted.agent_routes[agent_id].tiers["high"]
                    .model
                    .as_deref(),
                Some("shared")
            );
        }

        let error = match delete_profile(app.state(), "daily".to_string()) {
            Ok(_) => panic!("mounted profiles cannot be deleted"),
            Err(error) => error,
        };
        assert!(
            error.contains("codex") && error.contains("opencode"),
            "{error}"
        );

        {
            let managed = app.state::<AppStateManaged>();
            let mut inner = managed.0.lock().unwrap();
            inner.draft["upstreams"]["provider"]["models"] =
                json!([{"model": "shared"}, {"model": "updated"}]);
            for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
                inner
                    .set_tier_value(pool, Some("provider".into()), Some("updated".into()))
                    .unwrap();
            }
            inner.observe_draft().unwrap();
        }
        let updated = save_home_route_as_profile(app.state(), "daily".to_string()).unwrap();
        assert_eq!(updated.profiles, vec!["daily"]);
        assert_eq!(
            updated.agent_routes["codex"].tiers["high"].model.as_deref(),
            Some("updated")
        );

        save_agent_routes(app.state()).unwrap();
        let saved = ClientConfig::load(&config_path).unwrap();
        assert!(saved.profiles.contains_key("daily"));
        for agent_id in ["codex", "opencode"] {
            let router = saved
                .custom_router_for_agent(agent_id)
                .unwrap()
                .expect("mounted profile materializes");
            assert_eq!(router.pools[TIER_HIGH][0].model, "updated");
        }

        for agent_id in ["codex", "opencode"] {
            set_agent_route_mode(app.state(), agent_id.to_string(), "inherit".to_string()).unwrap();
        }
        let deleted = delete_profile(app.state(), "daily".to_string()).unwrap();
        assert!(deleted.profiles.is_empty());
        assert!(deleted
            .agent_routes
            .values()
            .all(|route| route.profile.is_none()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn one_two_and_three_tiers_always_end_with_a_zero_score_fallback() {
        let root = scratch_home("tiers-valid");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [
                {"model": "high"},
                {"model": "mid"},
                {"model": "low"}
            ]
        });

        for (pool, model) in [(TIER_HIGH, "high"), (TIER_MID, "mid"), (TIER_LOW, "low")] {
            inner
                .set_tier_value(pool, Some("provider".into()), Some(model.into()))
                .unwrap();
            let bands = inner.draft["router"]["heuristic"]["bands"]
                .as_array()
                .unwrap();
            assert_eq!(bands.last().unwrap()["at_least"], json!(0));
        }
        assert_eq!(inner.draft["router"]["default_pool"], json!(TIER_LOW));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_serve_running_starts_a_stopped_proxy_and_waits_until_reachable() {
        let root = scratch_home("ensure-stopped");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        inner.draft["server"]["listen"] = json!("127.0.0.1:0");
        inner.draft["server"]["auth"] = json!(false);
        inner.draft["data"]["metrics"] = json!(false);
        inner.draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
            .unwrap();
        // Build dependency-heavy runtime state before the bounded lifecycle
        // window. Coverage instrumentation can make this preparation exceed
        // the production readiness timeout on a loaded runner.
        let first_prepared = prepare_server(inner.materialize().unwrap()).unwrap();
        let recovered_prepared = prepare_server(inner.materialize().unwrap()).unwrap();
        let app = tauri::test::mock_app();
        manage_test_agent_state(&app, &root);
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        let ready = tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| Ok(first_prepared),
            Duration::from_secs(30),
        ))
        .unwrap();

        assert_eq!(ready.serve.phase, ServePhase::Running);
        assert_eq!(ready.serve.app_runtime, AppRuntime::Running);
        assert!(ready.serve.listener_reachable);
        let instance_id = ready.serve.instance_id.clone();
        let running_revision = ready.serve.running_revision;
        let prepare_calls = Arc::new(AtomicUsize::new(0));
        let calls_in_prepare = Arc::clone(&prepare_calls);
        let idempotent = tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                calls_in_prepare.fetch_add(1, Ordering::SeqCst);
                Err(StartFailure::new("duplicate", "must not restart"))
            },
            Duration::from_secs(1),
        ))
        .unwrap();
        assert_eq!(idempotent.serve.instance_id, instance_id);
        assert_eq!(idempotent.serve.running_revision, running_revision);
        assert_eq!(prepare_calls.load(Ordering::SeqCst), 0);
        begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        wait_for_serve_phase(&app, ServePhase::Stopped);

        {
            let state = app.state::<AppStateManaged>();
            state.0.lock().unwrap().server = ServerLifecycle::Failed {
                generation: 9,
                listen: "127.0.0.1:0".to_owned(),
                error: "previous fixture failure".to_owned(),
            };
        }
        let recovered = tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| Ok(recovered_prepared),
            Duration::from_secs(30),
        ))
        .unwrap();
        assert!(recovered.serve.listener_reachable);
        begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        wait_for_serve_phase(&app, ServePhase::Stopped);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_serve_running_joins_starting_and_rejects_a_failed_apply() {
        let root = scratch_home("ensure-join");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        inner.draft["server"]["listen"] = json!("127.0.0.1:0");
        inner.draft["server"]["auth"] = json!(false);
        inner.draft["data"]["metrics"] = json!(false);
        inner.draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
            .unwrap();
        // This test owns the Starting/Applying join contract. Build the real
        // Gateway before its bounded lifecycle window so parallel Wasm cold
        // starts cannot turn a join assertion into a machine-speed benchmark.
        let prepared = prepare_server(inner.materialize().unwrap()).unwrap();
        let app = tauri::test::mock_app();
        manage_test_agent_state(&app, &root);
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(prepared)
            },
        )
        .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("fixture is Starting");
        let (joined, ()) = tauri::async_runtime::block_on(async {
            tokio::join! {
                biased;
                ensure_serve_running_with(
                    app.handle().clone(),
                    app.state::<AppStateManaged>().inner(),
                    |_config| panic!("joining Starting must not prepare another runtime"),
                    Duration::from_secs(30),
                ),
                async move { release_tx.send(()).unwrap() },
            }
        });
        let joined = joined.unwrap();
        assert!(joined.serve.listener_reachable);

        let (apply_started_tx, apply_started_rx) = mpsc::channel();
        let (apply_release_tx, apply_release_rx) = mpsc::channel();
        begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                apply_started_tx.send(()).unwrap();
                apply_release_rx.recv().unwrap();
                Err(StartFailure::new("apply_fixture", "candidate rejected"))
            },
        )
        .unwrap();
        apply_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("fixture is Applying");
        let (outcome, ()) = tauri::async_runtime::block_on(async {
            tokio::join! {
                biased;
                ensure_serve_running_with(
                    app.handle().clone(),
                    app.state::<AppStateManaged>().inner(),
                    |_config| panic!("joining Applying must not prepare another runtime"),
                    Duration::from_secs(2),
                ),
                async move { apply_release_tx.send(()).unwrap() },
            }
        });
        let error = match outcome {
            Ok(_) => {
                panic!("a failed apply must not authorize Agent connection through the old runtime")
            }
            Err(error) => error,
        };
        assert!(
            error.contains("ensure_serve_running_start_failed"),
            "{error}"
        );
        assert!(error.contains("apply_fixture"), "{error}");

        begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        wait_for_serve_phase(&app, ServePhase::Stopped);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_serve_running_fails_closed_for_stopping_timeout_failure_and_generation_change() {
        let root = scratch_home("ensure-failures");
        let app = tauri::test::mock_app();
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.server = ServerLifecycle::Stopping {
            generation: 4,
            listen: "127.0.0.1:8787".to_owned(),
            draining: true,
        };
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        let stopping = match tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| panic!("Stopping must fail before preparation"),
            Duration::from_millis(50),
        )) {
            Ok(_) => panic!("Stopping is not automatically reversed"),
            Err(error) => error,
        };
        assert!(
            stopping.contains("ensure_serve_running_stopping"),
            "{stopping}"
        );

        {
            let state = app.state::<AppStateManaged>();
            state.0.lock().unwrap().server = ServerLifecycle::Starting {
                generation: 5,
                listen: "127.0.0.1:8787".to_owned(),
                revision: 1,
            };
        }
        let timeout = match tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| panic!("joining Starting must not prepare"),
            Duration::from_millis(30),
        )) {
            Ok(_) => panic!("an unfinished generation is bounded"),
            Err(error) => error,
        };
        assert!(
            timeout.contains("ensure_serve_running_timeout"),
            "{timeout}"
        );

        {
            let state = app.state::<AppStateManaged>();
            state.0.lock().unwrap().server = ServerLifecycle::Starting {
                generation: 6,
                listen: "127.0.0.1:8787".to_owned(),
                revision: 2,
            };
        }
        let fail_app = app.handle().clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            fail_app.state::<AppStateManaged>().0.lock().unwrap().server =
                ServerLifecycle::Failed {
                    generation: 6,
                    listen: "127.0.0.1:8787".to_owned(),
                    error: "fixture failed".to_owned(),
                };
        });
        let failed = match tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| panic!("joining Starting must not prepare"),
            Duration::from_secs(1),
        )) {
            Ok(_) => panic!("the lifecycle failure is returned"),
            Err(error) => error,
        };
        assert!(failed.contains("fixture failed"), "{failed}");

        {
            let state = app.state::<AppStateManaged>();
            state.0.lock().unwrap().server = ServerLifecycle::Starting {
                generation: 7,
                listen: "127.0.0.1:8787".to_owned(),
                revision: 3,
            };
        }
        let replace_app = app.handle().clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            replace_app
                .state::<AppStateManaged>()
                .0
                .lock()
                .unwrap()
                .server = ServerLifecycle::Stopped { generation: 8 };
        });
        let interrupted = match tauri::async_runtime::block_on(ensure_serve_running_with(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| panic!("joining Starting must not prepare"),
            Duration::from_secs(1),
        )) {
            Ok(_) => panic!("a replacement generation invalidates the wait"),
            Err(error) => error,
        };
        assert!(
            interrupted.contains("ensure_serve_running_interrupted"),
            "{interrupted}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn startup_preparation_is_single_flight_lock_free_and_cancellable() {
        let root = scratch_home("nonblocking-start");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        inner.draft["data"]["dir"] = json!(root.join("data"));
        inner.draft["server"]["listen"] = json!("127.0.0.1:0");
        inner.draft["server"]["auth"] = json!(false);
        inner.draft["data"]["metrics"] = json!(false);
        inner.draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
            .unwrap();

        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let prepare_calls = Arc::new(AtomicUsize::new(0));
        let calls_in_task = Arc::clone(&prepare_calls);

        let starting = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                calls_in_task.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err(StartFailure::new("test_gate", "cancelled fixture"))
            },
        )
        .unwrap();
        assert_eq!(starting.serve.phase, ServePhase::Starting);
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("preparer starts in the background");

        // get_state acquires the same AppInner mutex while preparation is blocked.
        let visible = get_state(app.state());
        assert_eq!(visible.serve.phase, ServePhase::Starting);

        let duplicate_calls = Arc::new(AtomicUsize::new(0));
        let duplicate_calls_in_task = Arc::clone(&duplicate_calls);
        let duplicate = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                duplicate_calls_in_task.fetch_add(1, Ordering::SeqCst);
                Err(StartFailure::new("duplicate", "must not run"))
            },
        )
        .err()
        .expect("a concurrent apply is rejected explicitly");
        assert!(duplicate.contains("apply_in_progress"));
        assert_eq!(duplicate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(prepare_calls.load(Ordering::SeqCst), 1);

        let stopping =
            begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        assert_eq!(stopping.serve.phase, ServePhase::Stopping);
        release_tx.send(()).unwrap();
        let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
        assert_eq!(stopped.serve.app_runtime, AppRuntime::Stopped);
        assert!(stopped.serve.error.is_none());

        let retrying = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| Err(StartFailure::new("gateway_init", "fixture failure")),
        )
        .unwrap();
        assert_eq!(retrying.serve.phase, ServePhase::Starting);
        let failed = wait_for_serve_phase(&app, ServePhase::Error);
        assert!(failed
            .serve
            .error
            .as_deref()
            .is_some_and(|error| error.contains("gateway_init: fixture failure")));

        let panicking = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| panic!("fixture preparation panic"),
        )
        .unwrap();
        assert_eq!(panicking.serve.phase, ServePhase::Starting);
        let panicked = wait_for_serve_phase(&app, ServePhase::Error);
        assert!(panicked
            .serve
            .error
            .as_deref()
            .is_some_and(|error| error.contains("startup_task: 后台启动任务异常退出")));

        let (retry_started_tx, retry_started_rx) = mpsc::channel();
        let (retry_release_tx, retry_release_rx) = mpsc::channel();
        let retry = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            move |_config| {
                retry_started_tx.send(()).unwrap();
                retry_release_rx.recv().unwrap();
                Err(StartFailure::new("test_gate", "retry cancelled"))
            },
        )
        .unwrap();
        assert_eq!(retry.serve.phase, ServePhase::Starting);
        retry_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("failed lifecycle can start a fresh generation");
        let retry_stopping =
            begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        assert_eq!(retry_stopping.serve.phase, ServePhase::Stopping);
        retry_release_tx.send(()).unwrap();
        wait_for_serve_phase(&app, ServePhase::Stopped);

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn desktop_update_recovery_waits_for_the_restart_result() {
        let root = scratch_home("desktop-update-recovery");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        inner.draft["data"]["dir"] = json!(root.join("data"));
        inner.draft["server"]["listen"] = json!("127.0.0.1:0");
        inner.draft["server"]["auth"] = json!(false);
        inner.draft["data"]["metrics"] = json!(false);
        inner.draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{"model": "small"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
            .unwrap();
        inner.server = ServerLifecycle::Failed {
            generation: 7,
            listen: "127.0.0.1:0".to_owned(),
            error: "previous stop failure".to_owned(),
        };

        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        let error = tauri::async_runtime::block_on(restore_gateway_after_failed_update_with(
            app.handle().clone(),
            true,
            |_config| Err(StartFailure::new("restore_fixture", "restart failed")),
        ))
        .expect_err("an asynchronous restart failure must be returned to the updater");

        assert!(error.contains("update_gateway_restore_start_failed"));
        assert!(error.contains("restore_fixture: restart failed"));
        let failed = get_state(app.state());
        assert_eq!(failed.serve.phase, ServePhase::Error);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn save_and_apply_hands_new_requests_to_the_new_revision() {
        let root = scratch_home("live-apply");
        let listen = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().to_string()
        };
        let (upstream_a, fixture_a) = serve_chat_completion("revision-a", 1);
        let (upstream_b, fixture_b) = serve_chat_completion("revision-b", 2);
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        inner.draft["server"]["listen"] = json!(listen.clone());
        inner.draft["server"]["auth"] = json!(false);
        inner.draft["data"]["metrics"] = json!(true);
        inner.draft["data"]["dir"] = json!(root.join("data"));
        inner.draft["pricing"] = json!({
            "version": 1,
            "models": {
                "small": { "input_per_mtok": 1_000_000, "output_per_mtok": 2_000_000 }
            }
        });
        let metrics_path = root.join("data/metrics.sqlite");
        inner.draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": upstream_a,
            "models": [{"model": "small"}]
        });
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(pool, Some("fixture".into()), Some("small".into()))
                .unwrap();
        }
        inner.observe_draft().unwrap();
        let app = tauri::test::mock_app();
        manage_test_agent_state(&app, &root);
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            prepare_server,
        )
        .unwrap();
        let first =
            wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
        let revision_a = first.serve.running_revision.unwrap();
        let instance_a = first.serve.instance_id.clone().unwrap();
        assert_eq!(revision_a, first.saved_revision);
        assert!(chat_through_proxy(&listen).contains("revision-a"));
        let first_receipts = wait_for_receipts(&metrics_path, 1);
        assert_eq!(first_receipts[0].running_revision, Some(revision_a));
        assert_eq!(first_receipts[0].cost_micros, Some(3));
        assert_eq!(first_receipts[0].price_version, Some(1));
        fixture_a.join().unwrap();

        save_home_route_as_profile(app.state(), "shared".to_string()).unwrap();
        let mounted =
            mount_agent_profile(app.state(), "codex".to_string(), "shared".to_string()).unwrap();
        assert!(mounted.config_dirty);
        assert_eq!(mounted.serve.running_revision, Some(revision_a));

        let price_v2 = set_model_price(
            app.state(),
            "small".to_string(),
            2_000_000,
            4_000_000,
            0,
            0,
            None,
            1,
        )
        .unwrap();
        assert_eq!(price_v2.version, 2);

        edit_provider(app.state(), "fixture".to_owned(), upstream_b, None).unwrap();
        update_provider_models(
            app.state(),
            "fixture".to_owned(),
            vec!["small".to_owned(), "extra".to_owned()],
        )
        .unwrap();
        let applying = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            prepare_server,
        )
        .unwrap();
        assert_eq!(applying.serve.app_runtime, AppRuntime::Running);
        assert_eq!(applying.serve.running_revision, Some(revision_a));
        let second =
            wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
        assert!(second.serve.running_revision.unwrap() > revision_a);
        assert_eq!(second.serve.running_revision, Some(second.saved_revision));
        assert_ne!(
            second.serve.instance_id.as_deref(),
            Some(instance_a.as_str())
        );
        assert!(chat_through_proxy(&listen).contains("revision-b"));
        let second_revision = second.serve.running_revision.unwrap();
        let second_receipts = wait_for_receipts(&metrics_path, 2);
        assert_eq!(second_receipts[0].running_revision, Some(second_revision));
        assert_eq!(second_receipts[0].cost_micros, Some(6));
        assert_eq!(second_receipts[0].price_version, Some(2));
        assert_eq!(second_receipts[1].cost_micros, Some(3));
        assert_eq!(second_receipts[1].price_version, Some(1));
        let ipc_receipts = get_recent_receipts(app.state(), 5).expect("receipt IPC reads");
        assert_eq!(
            ipc_receipts, second_receipts,
            "IPC uses the fixed store view"
        );

        edit_provider(
            app.state(),
            "fixture".to_owned(),
            "http://127.0.0.1:1/v1".to_owned(),
            None,
        )
        .unwrap();
        save_home_route_as_profile(app.state(), "candidate".to_string()).unwrap();
        let candidate =
            mount_agent_profile(app.state(), "opencode".to_string(), "candidate".to_string())
                .unwrap();
        assert_eq!(candidate.serve.running_revision, Some(second_revision));
        begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            |_config| {
                Err(StartFailure::new(
                    "gateway_init",
                    "preflight fixture failure",
                ))
            },
        )
        .unwrap();
        let failed_apply = wait_for_serve_phase(&app, ServePhase::Running);
        assert_eq!(
            failed_apply.serve.running_revision,
            second.serve.running_revision
        );
        assert!(failed_apply.saved_revision > failed_apply.serve.running_revision.unwrap());
        assert!(failed_apply
            .serve
            .error
            .as_deref()
            .is_some_and(|error| error.contains("已保存尚未应用")));
        assert!(chat_through_proxy(&listen).contains("revision-b"));
        let failed_apply_receipts = wait_for_receipts(&metrics_path, 3);
        assert_eq!(
            failed_apply_receipts[0].running_revision,
            Some(second_revision),
            "a failed apply keeps serving and receipting the published revision"
        );
        fixture_b.join().unwrap();

        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            let ServerLifecycle::Running { server, .. } = &inner.server else {
                panic!("fixture server must still be published");
            };
            server.abort_task();
        }
        let exited =
            wait_for_serve_phase_with_timeout(&app, ServePhase::Error, Duration::from_secs(1));
        assert_eq!(exited.serve.app_runtime, AppRuntime::Stopped);
        assert!(!exited.serve.listener_reachable);
        assert_eq!(exited.serve.running_revision, None);
        assert_eq!(exited.serve.instance_id, None);

        begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        wait_for_serve_phase(&app, ServePhase::Stopped);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_endpoint_preview_uses_the_protocol_resolver() {
        for input in [
            "https://api.example.com",
            "https://api.example.com/v1",
            "https://api.example.com/v1/chat/completions",
        ] {
            let preview = preview_provider_endpoints(input.to_owned()).unwrap();
            assert_eq!(preview.chat, "https://api.example.com/v1/chat/completions");
            assert_eq!(preview.responses, "https://api.example.com/v1/responses");
            assert_eq!(preview.messages, "https://api.example.com/v1/messages");
            assert!(!preview.loopback);
        }

        let local = preview_provider_endpoints("http://127.0.0.1:11434/v1".to_owned()).unwrap();
        assert!(local.loopback);
    }

    #[test]
    fn invalid_proxy_settings_are_transactional_and_field_scoped() {
        let root = scratch_home("settings-transaction");
        let config_path = root.join("token-station.json");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            template_for_test(&root),
            None,
        )))));
        add_provider(
            app.state(),
            "local".to_owned(),
            "http://127.0.0.1:11434/v1".to_owned(),
            vec!["local-model".to_owned()],
            None,
            true,
        )
        .expect("baseline provider is valid");
        set_tier(
            app.state(),
            "low".to_owned(),
            Some("local".to_owned()),
            Some("local-model".to_owned()),
        )
        .expect("baseline route is valid");
        let before = save_config(app.state()).expect("baseline config saves");
        let before_disk = std::fs::read(&config_path).expect("baseline config is on disk");

        let error = match set_settings(
            app.state(),
            false,
            false,
            "http".to_owned(),
            "ftp://invalid.example".to_owned(),
            vec!["localhost".to_owned()],
            String::new(),
            String::new(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("an unsupported proxy scheme is rejected"),
        };

        assert_eq!(error.field, "egress_proxy_url", "{}", error.message);
        assert_eq!(error.reason_code, "invalid_proxy_url");
        let after = get_state(app.state());
        assert_eq!(after.draft_revision, before.draft_revision);
        assert_eq!(after.saved_revision, before.saved_revision);
        assert_eq!(after.settings.auth, before.settings.auth);
        assert_eq!(after.settings.metrics, before.settings.metrics);
        assert_eq!(
            std::fs::read(&config_path).expect("saved config remains readable"),
            before_disk,
            "a rejected settings edit must not mutate the authoritative file"
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn managed_enterprise_provider_and_direct_target_are_one_draft_mutation() {
        let root = scratch_home("managed-enterprise-route");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));

        let view = add_provider_impl(
            app.state(),
            "enterprise_main".to_owned(),
            "https://enterprise.example.com/v1".to_owned(),
            vec!["auto".to_owned()],
            None,
            false,
            "env",
            Some("ENTERPRISE_API_KEY"),
            "openai-compatible",
            true,
        )
        .expect("the managed provider and Direct target are valid together");

        assert_eq!(view.routing_mode, "direct");
        let target = view.direct_target.expect("the Direct target is complete");
        assert_eq!(target.upstream, "enterprise_main");
        assert_eq!(target.model.as_deref(), Some("auto"));
        let provider = view
            .providers
            .iter()
            .find(|provider| provider.name == "enterprise_main")
            .expect("the managed provider is visible");
        assert_eq!(
            provider.model_capabilities[0].vision,
            CapabilityState::Declared
        );

        let managed = app.state::<AppStateManaged>();
        let inner = managed.0.lock().unwrap();
        let upstream = &inner.draft["upstreams"]["enterprise_main"];
        assert_eq!(upstream["managed_route"], json!(true));
        assert_eq!(
            upstream["models"][0]["supported_parameters"],
            json!(["reasoning_effort"])
        );
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn managed_enterprise_route_rollback_restores_present_and_absent_routing() {
        let previous_router = json!({ "routing_mode": "quota-first" });
        let expected_routing = json!({
            "mode": "quota-first",
            "direct_target": { "upstream": "original", "model": "stable" }
        });
        let previous_routing = Some(expected_routing.clone());
        let mut draft = json!({
            "router": { "routing_mode": "tiered" },
            "routing": { "mode": "direct" }
        });

        restore_managed_route_mutation(&mut draft, &previous_routing, &previous_router);
        assert_eq!(draft["router"], previous_router);
        assert_eq!(draft["routing"], expected_routing);

        let mut draft_without_previous_routing = json!({
            "router": { "routing_mode": "tiered" },
            "routing": { "mode": "direct" }
        });
        restore_managed_route_mutation(
            &mut draft_without_previous_routing,
            &None,
            &json!({ "routing_mode": "tiered" }),
        );
        assert!(draft_without_previous_routing.get("routing").is_none());
    }

    #[test]
    fn provider_credentials_default_to_store_and_advanced_sources_save_only_references() {
        let root = scratch_home("credential-sources");
        let config_path = root.join("token-station.json");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            config_path.clone(),
            template_for_test(&root),
            None,
        )))));

        let env_view = add_provider_with_credential(
            app.state(),
            "deepseek_env".to_owned(),
            "https://api.deepseek.com/v1".to_owned(),
            vec!["deepseek-chat".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("DEEPSEEK_API_KEY".to_owned()),
            None,
        )
        .expect("an environment credential reference is accepted");
        let env_provider = env_view
            .providers
            .iter()
            .find(|provider| provider.name == "deepseek_env")
            .expect("environment provider is visible");
        assert_eq!(env_provider.credential_source, "env");
        assert_eq!(env_provider.credential_reference, "DEEPSEEK_API_KEY");

        let credential_file = root.join("credentials").join("deepseek.key");
        let file_view = add_provider_with_credential(
            app.state(),
            "deepseek_file".to_owned(),
            "https://api.deepseek.com/v1".to_owned(),
            vec!["deepseek-reasoner".to_owned()],
            None,
            false,
            "file".to_owned(),
            Some(credential_file.to_string_lossy().into_owned()),
            None,
        )
        .expect("an absolute credential file reference is accepted");
        let file_provider = file_view
            .providers
            .iter()
            .find(|provider| provider.name == "deepseek_file")
            .expect("file provider is visible");
        assert_eq!(file_provider.credential_source, "file");
        assert_eq!(
            file_provider.credential_reference,
            credential_file.to_string_lossy()
        );

        let plaintext_error = match add_provider_with_credential(
            app.state(),
            "forbidden_plaintext".to_owned(),
            "https://api.example.com/v1".to_owned(),
            vec!["model".to_owned()],
            Some("must-not-be-saved".to_owned()),
            false,
            "env".to_owned(),
            Some("EXAMPLE_API_KEY".to_owned()),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("env/file sources cannot accept plaintext API keys"),
        };
        assert!(plaintext_error.contains("不能同时提交 API Key 明文"));
        let invalid_env = match add_provider_with_credential(
            app.state(),
            "bad_env".to_owned(),
            "https://api.example.com/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("1INVALID".to_owned()),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid environment names are rejected"),
        };
        assert!(invalid_env.contains("不能以数字开头"));
        let invalid_file = match add_provider_with_credential(
            app.state(),
            "bad_file".to_owned(),
            "https://api.example.com/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
            "file".to_owned(),
            Some("relative.key".to_owned()),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("relative credential files are rejected"),
        };
        assert!(invalid_file.contains("绝对路径"));

        set_tier(
            app.state(),
            "low".to_owned(),
            Some("deepseek_env".to_owned()),
            Some("deepseek-chat".to_owned()),
        )
        .expect("credential test has a valid route");
        save_config(app.state()).expect("credential references save");
        let saved = std::fs::read_to_string(config_path).expect("saved config is readable");
        assert!(saved.contains("DEEPSEEK_API_KEY"));
        assert!(saved.contains(&credential_file.to_string_lossy().replace('\\', "\\\\")));
        assert!(!saved.contains("must-not-be-saved"));
        assert!(!root
            .join("token-station-data")
            .join(secrets::SECRETS_FILE)
            .exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_creation_persists_only_the_closed_dialect_catalog() {
        let root = scratch_home("provider-dialect");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        )))));

        let view = add_provider_with_credential(
            app.state(),
            "azure".to_owned(),
            "https://fixture.openai.azure.com/openai/v1".to_owned(),
            vec!["deployment-fixture".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("AZURE_OPENAI_API_KEY".to_owned()),
            Some("azure-openai-v1".to_owned()),
        )
        .expect("the Azure OpenAI v1 dialect is accepted");
        let provider = view
            .providers
            .iter()
            .find(|provider| provider.name == "azure")
            .expect("the Azure provider is visible");
        assert_eq!(provider.provider, "azure-openai-v1");

        let before = get_state(app.state()).draft_revision;
        let error = match add_provider_with_credential(
            app.state(),
            "azure_wrong_base".to_owned(),
            "https://fixture.openai.azure.com/v1".to_owned(),
            vec!["deployment-fixture".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("AZURE_OPENAI_API_KEY".to_owned()),
            Some("azure-openai-v1".to_owned()),
        ) {
            Err(error) => error,
            Ok(_) => panic!("Azure OpenAI v1 requires the exact /openai/v1 API root"),
        };
        assert!(error.contains("/openai/v1"), "{error}");
        let after_wrong_base = get_state(app.state());
        assert_eq!(after_wrong_base.draft_revision, before);
        assert!(after_wrong_base
            .providers
            .iter()
            .all(|provider| provider.name != "azure_wrong_base"));

        let error = match edit_provider_with_credential(
            app.state(),
            "azure".to_owned(),
            "https://fixture.openai.azure.com/v1".to_owned(),
            None,
            "env".to_owned(),
            Some("AZURE_OPENAI_API_KEY".to_owned()),
            Some("legacy".to_owned()),
        ) {
            Err(error) => error,
            Ok(_) => panic!("editing Azure must preserve the exact /openai/v1 API root"),
        };
        assert!(error.contains("/openai/v1"), "{error}");
        let after_wrong_edit = get_state(app.state());
        assert_eq!(after_wrong_edit.draft_revision, before);
        assert_eq!(
            after_wrong_edit
                .providers
                .iter()
                .find(|provider| provider.name == "azure")
                .map(|provider| provider.base_url.as_str()),
            Some("https://fixture.openai.azure.com/openai/v1")
        );

        let error = match add_provider_with_credential(
            app.state(),
            "unknown".to_owned(),
            "https://provider.example/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("UNKNOWN_API_KEY".to_owned()),
            Some("future-header-provider".to_owned()),
        ) {
            Err(error) => error,
            Ok(_) => panic!("unknown provider dialects must fail closed"),
        };
        assert!(error.contains("Provider dialect"), "{error}");
        let after = get_state(app.state());
        assert_eq!(after.draft_revision, before);
        assert!(after
            .providers
            .iter()
            .all(|provider| provider.name != "unknown"));

        let error = match add_provider_with_credential(
            app.state(),
            "remote_http".to_owned(),
            "http://192.0.2.1/v1".to_owned(),
            vec!["model".to_owned()],
            None,
            false,
            "env".to_owned(),
            Some("REMOTE_HTTP_API_KEY".to_owned()),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("desktop creation must reject credentialed remote HTTP"),
        };
        assert!(error.contains("must use HTTPS"), "{error}");
        let after_http = get_state(app.state());
        assert_eq!(after_http.draft_revision, before);
        assert!(after_http
            .providers
            .iter()
            .all(|provider| provider.name != "remote_http"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn changing_provider_identity_clears_only_that_providers_scoped_prices() {
        let root = scratch_home("provider-identity-pricing");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://old.example/v1",
            "models": [{"model": "shared", "context_window": 128000}]
        });
        draft["pricing"] = json!({
            "version": 7,
            "models": {
                "fixture/shared": {"input_per_mtok": 200000, "output_per_mtok": 600000},
                "other/shared": {"input_per_mtok": 900000, "output_per_mtok": 1200000},
                "shared": {"input_per_mtok": 100000, "output_per_mtok": 300000}
            }
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        edit_provider(
            app.state(),
            "fixture".to_owned(),
            "https://new.example/v1".to_owned(),
            None,
        )
        .expect("a provider identity may be edited");

        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let pricing = draft_price_table(&inner).unwrap();
        assert_eq!(pricing.version, 8);
        assert!(!pricing.models.contains_key("fixture/shared"));
        assert!(pricing.models.contains_key("other/shared"));
        assert!(pricing.models.contains_key("shared"));
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn saving_unchanged_provider_credentials_preserves_scoped_price() {
        let root = scratch_home("provider-identity-noop");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://same.example/v1",
            "auth": {"slot": "provider_api_key", "env": "FIXTURE_API_KEY"},
            "models": [{"model": "shared", "context_window": 128000}]
        });
        draft["pricing"] = json!({
            "version": 7,
            "models": {
                "fixture/shared": {"input_per_mtok": 200000, "output_per_mtok": 600000}
            }
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));
        edit_provider_with_credential(
            app.state(),
            "fixture".to_owned(),
            "https://same.example/v1".to_owned(),
            None,
            "env".to_owned(),
            Some("FIXTURE_API_KEY".to_owned()),
            Some("south_v1_buffered_streaming".to_owned()),
        )
        .expect("submitting unchanged provider details is a no-op identity update");

        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let pricing = draft_price_table(&inner).unwrap();
        assert_eq!(pricing.version, 7);
        assert!(pricing.models.contains_key("fixture/shared"));
        assert_eq!(
            inner.draft["upstreams"]["fixture"]["provider_call"],
            json!("south_v1_buffered_streaming")
        );
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_identity_cleanup_does_not_apply_the_provider_call_engine() {
        let root = scratch_home("provider-engine-rollback");
        let data_dir = root.join("data");
        std::fs::create_dir_all(&data_dir).expect("data directory exists");
        let cache_target = root.join("catalog-target.json");
        std::fs::write(
            &cache_target,
            r#"{
                "version": 3,
                "providers": {
                    "fixture": {
                        "base_url": "https://old.example/v1",
                        "revision": 1,
                        "models": [],
                        "fetched_at_ms": 0
                    }
                }
            }"#,
        )
        .expect("catalog target writes");
        std::os::unix::fs::symlink(&cache_target, data_dir.join("model-catalog-cache.json"))
            .expect("catalog symlink writes");

        let mut draft = template_for_test(&root);
        draft["data"]["dir"] = json!(data_dir);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://old.example/v1",
            "auth": {"slot": "provider_api_key", "env": "FIXTURE_API_KEY"},
            "models": [{"model": "shared", "context_window": 128000}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let error = match edit_provider_with_credential(
            app.state(),
            "fixture".to_owned(),
            "https://new.example/v1".to_owned(),
            None,
            "env".to_owned(),
            Some("FIXTURE_API_KEY".to_owned()),
            Some("south_v1_buffered".to_owned()),
        ) {
            Err(error) => error,
            Ok(_) => panic!("a catalog symlink makes identity cleanup fail closed"),
        };
        assert!(error.contains("保存模型缓存失败"), "{error}");

        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(
            inner.draft["upstreams"]["fixture"]
                .get("provider_call")
                .is_none(),
            "a failed save must preserve the previous engine"
        );
        assert_eq!(
            inner.draft["upstreams"]["fixture"]["base_url"],
            json!("https://old.example/v1")
        );
        drop(inner);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn desktop_commands_cover_provider_routing_settings_server_and_read_only_views() {
        let root = scratch_home("command-lifecycle");
        let mut draft = gateway_template_for_test(&root);
        draft["data"]["dir"] = json!(root.join("data"));
        draft["server"]["listen"] = json!("127.0.0.1:0");
        let app = tauri::test::mock_app();
        manage_test_agent_state(&app, &root);
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let initial = get_state(app.state());
        assert_eq!(initial.serve.app_runtime, AppRuntime::Stopped);
        assert!(initial.providers.is_empty());
        assert_eq!(initial.settings.listen, "127.0.0.1:0");

        for (name, url, models) in [
            ("local", "http://127.0.0.1:11434/v1", vec!["small", "large"]),
            ("spare", "http://127.0.0.1:11435/v1", vec!["backup"]),
        ] {
            let view = add_provider(
                app.state(),
                name.to_string(),
                url.to_string(),
                models.into_iter().map(str::to_string).collect(),
                None,
                name == "local",
            )
            .unwrap();
            let provider = view
                .providers
                .iter()
                .find(|provider| provider.name == name)
                .expect("the added provider is visible");
            // OpenAI-compatible chat declares tools and structured output by default; keep vision Unknown.
            assert_eq!(
                provider.model_capabilities[0].tool,
                CapabilityState::Declared
            );
            assert_eq!(
                provider.model_capabilities[0].vision,
                CapabilityState::Unknown
            );
            assert_eq!(
                provider.model_capabilities[0].json_schema,
                CapabilityState::Declared
            );
        }
        let duplicate = add_provider(
            app.state(),
            "local".to_owned(),
            "http://127.0.0.1:9999/v1".to_owned(),
            vec!["replacement".to_owned()],
            None,
            false,
        )
        .err()
        .expect("重复名称不能绕过 Provider 编辑流程");
        assert!(duplicate.contains("已存在"));
        let unchanged = get_state(app.state());
        let local = unchanged
            .providers
            .iter()
            .find(|provider| provider.name == "local")
            .unwrap();
        assert_eq!(local.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(local.models, ["small", "large"]);
        assert!(add_provider(
            app.state(),
            " ".to_string(),
            "http://127.0.0.1/v1".to_string(),
            vec!["m".to_string()],
            None,
            false,
        )
        .err()
        .expect("blank provider is rejected")
        .contains("不能为空"));
        assert!(add_provider(
            app.state(),
            "empty".to_string(),
            "http://127.0.0.1/v1".to_string(),
            vec![" ".to_string()],
            None,
            false,
        )
        .err()
        .expect("blank model set is rejected")
        .contains("至少填一个"));
        let provider_count = get_state(app.state()).providers.len();
        let invalid_name = match add_provider(
            app.state(),
            "minimax-cn".to_string(),
            "https://api.minimaxi.com/v1".to_string(),
            vec!["MiniMax-M3".to_string()],
            None,
            false,
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid upstream reference names must be rejected before mutation"),
        };
        assert!(invalid_name.contains("upstream reference name"));
        assert_eq!(get_state(app.state()).providers.len(), provider_count);

        set_tier(
            app.state(),
            "high".to_string(),
            Some("local".to_string()),
            Some("large".to_string()),
        )
        .unwrap();
        set_tier(
            app.state(),
            "low".to_string(),
            Some("local".to_string()),
            Some("small".to_string()),
        )
        .unwrap();
        assert!(set_tier(app.state(), "invalid".to_string(), None, None)
            .err()
            .expect("invalid tier is rejected")
            .contains("未知档位"));

        let saved = save_config(app.state()).unwrap();
        assert!(saved.config_error.is_none());
        assert!(root.join("token-station.json").is_file());
        let router = get_router_table(app.state());
        assert_eq!(router.default_pool, TIER_LOW);
        assert_eq!(router.threshold, Some(CUT_MID));
        assert_eq!(router.bands.len(), 2);
        assert_eq!(router.pools.len(), 2);
        assert_eq!(router.bands[0].upstream.as_deref(), Some("local"));

        update_provider_models(
            app.state(),
            "local".to_string(),
            vec![
                "large".to_string(),
                "small".to_string(),
                "extra".to_string(),
            ],
        )
        .unwrap();
        let configured = set_settings(
            app.state(),
            false,
            false,
            "direct".to_string(),
            String::new(),
            Vec::new(),
            String::new(),
            String::new(),
        )
        .unwrap();
        assert!(!configured.settings.auth);
        assert!(!configured.settings.metrics);

        let plugins = get_plugins(app.state()).unwrap();
        assert!(plugins.agent.contains("agent-openai"));
        assert!(plugins
            .dialects
            .iter()
            .any(|dialect| dialect == "openai-compatible"));
        assert!(plugins.listing.contains("provider-openai-compatible"));

        let empty_stats =
            get_stats(app.state(), "all".to_string(), None, None, None, None, None).unwrap();
        assert!(empty_stats.empty);
        assert_eq!(empty_stats.total.requests, 0);

        let started = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            prepare_server,
        )
        .unwrap();
        assert_eq!(started.serve.phase, ServePhase::Starting);
        let duplicate = begin_serve_start(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            prepare_server,
        )
        .err()
        .expect("a concurrent apply is rejected explicitly");
        assert!(duplicate.contains("apply_in_progress"));
        // Coverage instrumentation makes Wasmtime's first compilation much
        // slower on a cold Linux runner; this remains a bounded integration test.
        let running =
            wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
        assert_eq!(running.serve.app_runtime, AppRuntime::Running);
        assert!(running.serve.listener_reachable);
        assert!(running.serve.virtual_key.is_none());
        assert!(root.join("data").join("requests.log").exists());
        let stopping =
            begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        assert_eq!(stopping.serve.phase, ServePhase::Stopping);
        let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
        assert_eq!(stopped.serve.app_runtime, AppRuntime::Stopped);

        let impact = preview_provider_removal(app.state(), "local".to_string()).unwrap();
        assert!(!impact.can_remove);
        assert!(impact
            .references
            .iter()
            .any(|item| item.contains("主页/上档")));
        assert!(remove_provider(app.state(), "local".to_string())
            .err()
            .expect("被引用的 Provider 必须拒绝删除")
            .contains("仍被引用"));
        set_tier(app.state(), "high".to_string(), None, None).unwrap();
        set_tier(app.state(), "low".to_string(), None, None).unwrap();
        assert!(
            preview_provider_removal(app.state(), "local".to_string())
                .unwrap()
                .can_remove
        );

        let catalog_path = root.join("data").join("model-catalog-cache.json");
        crate::agent_integration::safe_fs::write_atomic_private(
            &catalog_path,
            &serde_json::to_vec_pretty(&json!({
                "version": 3,
                "providers": {
                    "local": {
                        "base_url": "http://127.0.0.1:11434/v1",
                        "revision": 7,
                        "models": [{
                            "model": "old-account-private-model",
                            "tool": "unknown",
                            "vision": "unknown",
                            "json_schema": "unknown",
                            "source": "live",
                            "last_seen_ms": 1,
                            "catalog_state": "active"
                        }],
                        "fetched_at_ms": 1
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let removed = remove_provider(app.state(), "local".to_string()).unwrap();
        assert_eq!(removed.providers.len(), 1);
        assert_eq!(removed.deleted_providers, ["local"]);
        assert!(removed.tiers.values().all(|tier| tier.upstream.is_none()));
        assert!(
            !std::fs::read_to_string(&catalog_path)
                .unwrap()
                .contains("old-account-private-model"),
            "deletion invalidates the old Provider identity's trusted catalog"
        );
        let Err(readd_error) = add_provider(
            app.state(),
            "local".to_owned(),
            "http://127.0.0.1:11434/v1".to_owned(),
            vec!["replacement".to_owned()],
            None,
            false,
        ) else {
            panic!("a tombstoned Provider name must be restored, never silently replaced")
        };
        assert!(readd_error.contains("请先恢复"), "{readd_error}");
        let restored = restore_provider(app.state(), "local".to_string()).unwrap();
        assert_eq!(restored.providers.len(), 2);
        assert!(restored.deleted_providers.is_empty());
        let restored_local = restored
            .providers
            .iter()
            .find(|provider| provider.name == "local")
            .unwrap();
        assert_eq!(restored_local.catalog_revision, 0);
        assert!(restored_local
            .catalog
            .iter()
            .all(|model| model.source == model_catalog::CatalogSource::Configured));
        assert!(save_config(app.state())
            .err()
            .expect("empty routing config is rejected")
            .contains("至少配置一档"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_budget_commands_persist_display_only_thresholds_and_report_zero_without_a_store() {
        let root = scratch_home("agent-budget-commands");
        let inner = AppInner::new(
            root.join("token-station.json"),
            gateway_template_for_test(&root),
            None,
        );
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        let statuses = set_agent_budget(
            app.state(),
            "codex".to_string(),
            1_000_000,
            80,
            Some(1_000),
            Some(2_000),
            7,
        )
        .unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].agent_id, "codex");
        assert_eq!(statuses[0].used_micros, 0);
        assert!(!statuses[0].routing_affected);
        let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
        assert_eq!(saved.agent_budgets["codex"].limit_micros, 1_000_000);

        assert!(set_agent_budget(
            app.state(),
            "unknown-agent".to_string(),
            1,
            80,
            None,
            None,
            7,
        )
        .is_err());
        assert!(remove_agent_budget(app.state(), "codex".to_string())
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn model_price_edits_append_versions_and_never_revalue_historical_receipts() {
        use token_station_metrics::{CostKind, Recorder, RequestRecord};

        let root = scratch_home("model-price-editor");
        let mut draft = gateway_template_for_test(&root);
        draft["pricing"] = json!({ "version": 0, "models": {} });
        let data_dir = PathBuf::from(draft["data"]["dir"].as_str().unwrap());
        std::fs::create_dir_all(&data_dir).unwrap();
        let store = SqliteStore::open(&data_dir.join("metrics.sqlite")).unwrap();
        let mut historical = RequestRecord::begin(1, "openai-responses");
        historical.request_id = "historical-v7".to_string();
        historical.requested_model = "model-a".to_string();
        historical.status = 200;
        historical.cost_kind = CostKind::Estimated;
        historical.cost_micros = Some(111);
        historical.price_version = Some(7);
        store.record(&historical);
        drop(store);

        let inner = AppInner::new(root.join("token-station.json"), draft, None);
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        assert_eq!(get_price_table(app.state()).unwrap().version, 0);
        let v1 = set_model_price(
            app.state(),
            "model-a".to_string(),
            1_000_000,
            2_000_000,
            300_000,
            4_000_000,
            Some(5_000_000),
            0,
        )
        .unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v1.models["model-a"].reasoning_per_mtok, Some(5_000_000));
        assert!(
            set_model_price(app.state(), "model-a".to_string(), 9, 9, 9, 9, None, 0,)
                .unwrap_err()
                .contains("版本冲突")
        );

        let v2 = set_model_price(
            app.state(),
            "model-a".to_string(),
            2_000_000,
            3_000_000,
            300_000,
            4_000_000,
            None,
            1,
        )
        .unwrap();
        assert_eq!(v2.version, 2);
        let v3 = remove_model_price(app.state(), "model-a".to_string(), 2).unwrap();
        assert_eq!(v3.version, 3);
        assert!(v3.models.is_empty());

        let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
        assert_eq!(saved.pricing.version, 3);
        assert!(saved.pricing.models.is_empty());
        let receipts = SqliteStore::recent_receipts(&data_dir.join("metrics.sqlite"), 5).unwrap();
        assert_eq!(receipts[0].cost_micros, Some(111));
        assert_eq!(receipts[0].price_version, Some(7));
        let source_filtered = get_stats(
            app.state(),
            "all".to_string(),
            None,
            None,
            Some("openai-responses".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(source_filtered.total.requests, 1);
        let agent_filtered = get_stats(
            app.state(),
            "all".to_string(),
            None,
            Some("codex".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(agent_filtered.total.requests, 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn public_price_batch_scopes_models_preserves_manual_values_and_bumps_once() {
        use token_station_metrics::{
            CostKind, RecordedDecidedBy, Recorder, RequestRecord, RoutingRecord,
        };
        use token_station_protocol::Usage;
        use token_station_router_core::RequestFeatures;

        let root = scratch_home("public-price-batch");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.example.test/v1",
            "models": [{"model": "model-a"}, {"model": "model-b"}, {"model": "missing"}]
        });
        draft["pricing"] = json!({
            "version": 4,
            "models": {
                "fixture/model-a": {
                    "input_per_mtok": 99,
                    "output_per_mtok": 199
                }
            }
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let suggestion = |requested_model_id: &str, input_per_mtok| RequestedModelPriceSuggestion {
            requested_model_id: requested_model_id.to_owned(),
            suggestion: ModelPriceSuggestionView {
                model_id: requested_model_id.to_owned(),
                display_name: requested_model_id.to_owned(),
                provider_id: "fixture-catalog".to_owned(),
                provider_name: "Fixture".to_owned(),
                source: "models.dev".to_owned(),
                catalog_source: "cache".to_owned(),
                fetched_at_ms: 1,
                input_per_mtok,
                output_per_mtok: input_per_mtok * 2,
                cache_read_per_mtok: 0,
                cache_write_per_mtok: 0,
                reasoning_per_mtok: None,
            },
        };
        let requested = BTreeSet::from([
            "missing".to_owned(),
            "model-a".to_owned(),
            "model-b".to_owned(),
        ]);

        let mut stale = vec![suggestion("model-b", 2)];
        stale[0].suggestion.catalog_source = "stale_cache".to_owned();
        assert!(ensure_automatic_price_suggestions_fresh(&stale)
            .unwrap_err()
            .contains("cached prices are advisory only"));

        let result = apply_public_model_prices(
            &mut inner,
            "fixture",
            &requested,
            vec![suggestion("model-a", 1), suggestion("model-b", 2)],
        )
        .unwrap();

        assert_eq!(result, (1, 1, vec!["missing".to_owned()], 5));
        let pricing = draft_price_table(&inner).unwrap();
        assert_eq!(pricing.version, 5);
        assert_eq!(pricing.models["fixture/model-a"].input_per_mtok, 99);
        assert_eq!(pricing.models["fixture/model-b"].input_per_mtok, 2);
        assert!(!pricing.models.contains_key("model-b"));

        let data_dir = inner.data_dir();
        std::fs::create_dir_all(&data_dir).unwrap();
        let db = data_dir.join("metrics.sqlite");
        let store = SqliteStore::open(&db).unwrap();
        let mut unknown = RequestRecord::begin(1, "openai-responses");
        unknown.request_id = "auto-price-backfill".to_owned();
        unknown.requested_model = "model-b".to_owned();
        unknown.status = 200;
        unknown.routing = Some(RoutingRecord {
            upstream: "fixture".to_owned(),
            model: "model-b".to_owned(),
            pool: "main".to_owned(),
            decided_by: RecordedDecidedBy::Default,
            fallbacks: 0,
            features: RequestFeatures::default(),
        });
        unknown.usage = Some(Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        });
        unknown.cost_kind = CostKind::Unknown;
        store.record(&unknown);
        drop(store);

        inner
            .save_draft()
            .expect("saving an automatically imported price also backfills receipts");
        let receipts = SqliteStore::recent_receipts(&db, 5).unwrap();
        assert_eq!(receipts[0].cost_kind, CostKind::Estimated);
        assert_eq!(receipts[0].cost_micros, Some(2));
        assert_eq!(receipts[0].price_version, Some(5));

        let unconfigured = BTreeSet::from(["model-outside-provider".to_owned()]);
        let error = apply_public_model_prices(
            &mut inner,
            "fixture",
            &unconfigured,
            vec![suggestion("model-outside-provider", 3)],
        )
        .unwrap_err();
        assert!(error.contains("is not configured for Provider"), "{error}");
        assert_eq!(draft_price_table(&inner).unwrap().version, 5);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn public_price_import_rejects_a_changed_target_snapshot() {
        let root = scratch_home("public-price-stale-target");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://old.example/v1",
            "models": [{"model": "model-a"}]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
        let target = capture_price_import_target(&inner, "fixture").unwrap();
        let original = inner.draft["upstreams"]["fixture"].clone();

        inner.draft["upstreams"]["fixture"]["base_url"] = json!("https://new.example/v1");
        inner.observe_draft().unwrap();
        inner.draft["upstreams"]["fixture"] = original;
        inner.observe_draft().unwrap();

        let restored = capture_price_import_target(&inner, "fixture").unwrap();
        assert_eq!(restored.upstream, target.upstream);
        assert_eq!(restored.price_version, target.price_version);
        assert!(restored.upstream_epoch > target.upstream_epoch);

        let error = ensure_price_import_target_unchanged(&inner, "fixture", &target)
            .expect_err("an ABA edit must still invalidate the old Provider identity");
        assert!(
            error.contains("changed while public prices were loading"),
            "{error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn public_price_import_rejects_a_secret_only_identity_rotation() {
        let root = scratch_home("public-price-secret-rotation");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://same.example/v1",
            "auth": {"slot": "provider_api_key", "store": true},
            "models": [{"model": "model-a"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));
        let target = {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            capture_price_import_target(&inner, "fixture").unwrap()
        };

        edit_provider(
            app.state(),
            "fixture".to_owned(),
            "https://same.example/v1".to_owned(),
            Some("rotated-secret".to_owned()),
        )
        .expect("a stored credential may be rotated without changing its descriptor");

        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let rotated = capture_price_import_target(&inner, "fixture").unwrap();
        assert_eq!(rotated.upstream, target.upstream);
        assert_eq!(rotated.price_version, target.price_version);
        assert!(rotated.upstream_epoch > target.upstream_epoch);
        ensure_price_import_target_unchanged(&inner, "fixture", &target)
            .expect_err("a key rotation must invalidate an in-flight price import");
        drop(inner);
        secrets::store_remove(&root, "fixture", "provider_api_key").ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_discovery_targets_reject_identity_aba_absent_add_and_older_generations() {
        let root = scratch_home("provider-discovery-targets");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://a.example/v1",
            "models": [{"model": "model-a"}]
        });
        let mut inner = AppInner::new(root.join("token-station.json"), draft, None);

        let original = inner.draft["upstreams"]["fixture"].clone();
        let aba_target = begin_provider_discovery_target(&mut inner, "fixture");
        inner.draft["upstreams"]["fixture"]["base_url"] = json!("https://b.example/v1");
        inner.observe_draft().unwrap();
        inner.draft["upstreams"]["fixture"] = original;
        inner.observe_draft().unwrap();
        ensure_provider_discovery_target_unchanged(&inner, "fixture", &aba_target)
            .expect_err("an A-B-A edit must invalidate discovery");

        let older = begin_provider_discovery_target(&mut inner, "fixture");
        let latest = begin_provider_discovery_target(&mut inner, "fixture");
        ensure_provider_discovery_target_unchanged(&inner, "fixture", &older)
            .expect_err("only the latest same-identity discovery may commit");
        ensure_provider_discovery_target_unchanged(&inner, "fixture", &latest)
            .expect("the latest same-identity discovery remains current");

        let absent = begin_provider_discovery_target(&mut inner, "new-provider");
        inner.draft["upstreams"]["new-provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://new.example/v1",
            "models": [{"model": "model-a"}]
        });
        inner.observe_draft().unwrap();
        ensure_provider_discovery_target_unchanged(&inner, "new-provider", &absent)
            .expect_err("adding a same-name Provider must invalidate prior discovery");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_model_ids_are_bounded_normalized_and_unique() {
        assert_eq!(
            normalize_provider_model_ids(vec![
                " model-b ".to_owned(),
                "model-a".to_owned(),
                "model-b".to_owned(),
            ])
            .unwrap(),
            vec!["model-b".to_owned(), "model-a".to_owned()]
        );
        assert!(normalize_provider_model_ids(vec!["bad\nmodel".to_owned()]).is_err());
        assert!(normalize_provider_model_ids(vec![
            "x".repeat(model_catalog::MAX_MODEL_ID_BYTES + 1)
        ])
        .is_err());
        assert!(normalize_provider_model_ids(vec![
            "same".to_owned();
            model_catalog::MAX_MODELS_PER_PROVIDER + 1
        ])
        .is_err());
    }

    #[test]
    fn provider_transport_rejects_remote_http_and_proxied_loopback_credentials() {
        let direct = EgressConfig::default();
        let loopback = ProviderEndpoint::try_new("http://127.0.0.1:11434/v1").unwrap();
        ensure_credential_transport(&loopback, &direct)
            .expect("direct loopback credentials stay on the device");

        let proxied: EgressConfig = serde_json::from_value(json!({
            "mode": "http",
            "proxy_url": "http://proxy.example.test:8080"
        }))
        .unwrap();
        assert!(ensure_credential_transport(&loopback, &proxied)
            .unwrap_err()
            .contains("must bypass"));

        let bypassed: EgressConfig = serde_json::from_value(json!({
            "mode": "http",
            "proxy_url": "http://proxy.example.test:8080",
            "no_proxy": ["127.0.0.1"]
        }))
        .unwrap();
        ensure_credential_transport(&loopback, &bypassed)
            .expect("an exact proxy bypass keeps loopback credentials local");

        let remote = ProviderEndpoint::try_new("http://192.0.2.1/v1").unwrap();
        assert!(ensure_credential_transport(&remote, &direct)
            .unwrap_err()
            .contains("must use HTTPS"));
    }

    #[test]
    fn provider_health_uses_the_configured_production_engine() {
        let draft = json!({
            "plugins": {"providers": {
                "openai-compatible": "provider-openai-compatible",
                "azure-openai-v1": "provider-openai-compatible"
            }},
            "egress": {"mode": "direct"}
        });
        let eligible = json!({
            "provider": "openai-compatible",
            "provider_call": "south_v1_buffered_streaming",
            "auth": {"env": "PROVIDER_API_KEY"}
        });
        assert!(provider_health_uses_south(&draft, &eligible, true));

        // No engine named: the South default applies, as it does for traffic.
        let defaulted = json!({
            "provider": "openai-compatible",
            "auth": {"env": "PROVIDER_API_KEY"}
        });
        assert!(provider_health_uses_south(&draft, &defaulted, true));
        let explicit_legacy = json!({
            "provider": "openai-compatible",
            "provider_call": "legacy",
            "auth": {"env": "PROVIDER_API_KEY"}
        });
        assert!(!provider_health_uses_south(&draft, &explicit_legacy, true));

        let mut proxied = draft.clone();
        proxied["egress"] = json!({
            "mode": "http",
            "proxy_url": "http://proxy.example.test:8080"
        });
        assert!(!provider_health_uses_south(&proxied, &eligible, true));

        let mut native = eligible.clone();
        native["api_dialect"] = json!("anthropic-native");
        assert!(!provider_health_uses_south(&draft, &native, true));
        assert!(!provider_health_uses_south(&draft, &eligible, false));

        let azure_header = json!({
            "provider": "azure-openai-v1",
            "provider_call": "south_v1_buffered_streaming_header_auth",
            "auth": {"store": true}
        });
        assert!(provider_health_uses_south(&draft, &azure_header, true));
        let mut azure_legacy_south = azure_header;
        azure_legacy_south["provider_call"] = json!("south_v1_buffered_streaming");
        assert!(!provider_health_uses_south(
            &draft,
            &azure_legacy_south,
            true
        ));
    }

    #[test]
    fn public_price_import_derives_its_catalog_namespace_from_provider_identity() {
        let root = scratch_home("public-price-provider-mapping");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["glm_cn"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://open.bigmodel.cn/api/paas/v4",
            "models": [{"model": "glm-5.2"}]
        });
        draft["upstreams"]["custom"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://custom.example/v1",
            "models": [{"model": "custom-model"}]
        });
        let inner = AppInner::new(root.join("token-station.json"), draft, None);

        assert_eq!(
            configured_public_price_provider_id(&inner, "glm_cn").unwrap(),
            "zhipuai"
        );
        assert!(configured_public_price_provider_id(&inner, "custom")
            .unwrap_err()
            .contains("no authoritative"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_empty_price_table_receives_builtin_catalog_once() {
        let mut draft = json!({ "pricing": { "version": 0, "models": {} } });

        assert!(seed_builtin_pricing(&mut draft).unwrap());
        let table: PriceTable = serde_json::from_value(draft["pricing"].clone()).unwrap();
        assert_eq!(table.version, 1);
        assert!(table.models.contains_key("deepseek-v4-pro"));
        assert!(!seed_builtin_pricing(&mut draft).unwrap());
    }

    #[test]
    fn local_only_routing_flags_local_providers_and_toggles_the_switch() {
        let root = scratch_home("local-only-routing");
        let inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        // One local provider marked local and one cloud provider.
        add_provider(
            app.state(),
            "ollama".to_owned(),
            "http://127.0.0.1:11434/v1".to_owned(),
            vec!["llama3".to_owned()],
            None,
            true,
        )
        .unwrap();
        let view = add_provider(
            app.state(),
            "openai".to_owned(),
            "https://api.openai.com/v1".to_owned(),
            vec!["gpt-5".to_owned()],
            None,
            false,
        )
        .unwrap();

        let ollama = view.providers.iter().find(|p| p.name == "ollama").unwrap();
        assert!(
            ollama.local,
            "the local provider is flagged for local_only routing"
        );
        let openai = view.providers.iter().find(|p| p.name == "openai").unwrap();
        assert!(!openai.local, "an ordinary cloud provider is not flagged");
        assert!(!view.local_only, "local_only is off until asked for");
        assert!(!view.allow_cloud_fallback);

        // Enable local-only routing with cloud fallback.
        let on = set_local_routing(app.state(), true, true).unwrap();
        assert!(on.local_only);
        assert!(on.allow_cloud_fallback);

        // Disabling clears both keys and returns the config to clean default-equivalent state.
        let off = set_local_routing(app.state(), false, false).unwrap();
        assert!(!off.local_only);
        assert!(!off.allow_cloud_fallback);
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert!(inner.draft["router"].get("local_only").is_none());
            assert!(inner.draft["router"].get("allow_cloud_fallback").is_none());
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn historical_home_mode_without_top_level_routing_remains_visible() {
        let root = scratch_home("historical-home-routing-mode");
        let mut draft = template_for_test(&root);
        draft["router"]["routing_mode"] = json!("quota_first");

        let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();

        assert_eq!(view.routing_mode, "quota_first");
        assert!(view
            .agent_routes
            .values()
            .all(|route| route.routing_mode == "quota_first"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn routing_mode_switches_per_agent_without_touching_home_or_siblings() {
        let root = scratch_home("per-agent-routing-mode");
        let inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        // Home default is tiered, and every Agent inherits it.
        let view = get_state(app.state());
        assert_eq!(view.routing_mode, "tiered");
        assert!(
            view.agent_routes
                .values()
                .all(|r| r.routing_mode == "tiered"),
            "every Agent inherits the tiered home default"
        );

        // Flip Home to quota-first: the Home view flips, and Agents that never
        // overrode follow it.
        let view = set_routing_mode(app.state(), "quota_first".to_owned(), None).unwrap();
        assert_eq!(view.routing_mode, "quota_first");
        assert!(
            view.agent_routes
                .values()
                .all(|r| r.routing_mode == "quota_first"),
            "un-overridden Agents follow the new Home default"
        );
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.draft["routing"]["mode"], json!("quota_first"));
            assert_eq!(inner.draft["router"]["routing_mode"], json!("quota_first"));
        }

        // Pin one Agent back to tiered while Home stays quota-first. Only that
        // Agent changes; Home and its siblings keep quota-first.
        let view =
            set_routing_mode(app.state(), "tiered".to_owned(), Some("codex".to_owned())).unwrap();
        assert_eq!(view.routing_mode, "quota_first", "Home is untouched");
        assert_eq!(view.agent_routes["codex"].routing_mode, "tiered");
        assert!(
            view.agent_routes
                .iter()
                .filter(|(id, _)| id.as_str() != "codex")
                .all(|(_, r)| r.routing_mode == "quota_first"),
            "sibling Agents are untouched by the per-Agent switch"
        );
        // The Agent's mode is written explicitly (not cleared), so it stays
        // pinned independent of Home — the whole point of a per-Agent switch.
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(
                inner.draft["agent_routes"]["codex"]["routing_mode"].as_str(),
                Some("tiered")
            );
        }

        // Flipping Home back to tiered leaves the pinned Agent alone (still an
        // explicit "tiered" override), and un-pinned siblings follow Home.
        let view = set_routing_mode(app.state(), "tiered".to_owned(), None).unwrap();
        assert_eq!(view.routing_mode, "tiered");
        assert_eq!(view.agent_routes["codex"].routing_mode, "tiered");
        assert!(
            view.agent_routes
                .iter()
                .filter(|(id, _)| id.as_str() != "codex")
                .all(|(_, r)| r.routing_mode == "tiered"),
            "un-pinned siblings track the tiered Home again"
        );
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.draft["routing"]["mode"], json!("tiered"));
            assert_eq!(inner.draft["router"]["routing_mode"], json!("tiered"));
        }

        // Switching the pinned Agent to quota-first writes the explicit value.
        let view = set_routing_mode(
            app.state(),
            "quota_first".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        assert_eq!(view.agent_routes["codex"].routing_mode, "quota_first");
        assert_eq!(view.routing_mode, "tiered", "Home stays tiered");

        let direct = set_routing_mode(app.state(), "direct".to_owned(), None).unwrap();
        assert_eq!(direct.routing_mode, "direct");
        assert_eq!(direct.agent_routes["codex"].routing_mode, "quota_first");
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.draft["routing"]["mode"], json!("direct"));
            assert_eq!(inner.draft["router"]["routing_mode"], json!("tiered"));
        }

        // Unknown Agent is rejected.
        assert!(set_routing_mode(
            app.state(),
            "quota_first".to_owned(),
            Some("nope".to_owned())
        )
        .is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn direct_route_targets_are_validated_and_isolated_between_home_and_agents() {
        let root = scratch_home("direct-route-targets");
        let mut draft = template_for_test(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let home =
            set_direct_route(app.state(), "provider".to_owned(), "home".to_owned(), None).unwrap();
        let home_target = home.direct_target.expect("Home target is public state");
        assert_eq!(
            (home_target.upstream.as_str(), home_target.model.as_deref()),
            ("provider", Some("home"))
        );
        assert!(home.agent_routes.values().all(|route| route
            .direct_target
            .as_ref()
            .is_some_and(|target| target.model.as_deref() == Some("home"))));
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.draft["routing"]["mode"], json!("tiered"));
            assert_eq!(
                inner.draft["routing"]["direct_target"],
                json!({"upstream": "provider", "model": "home"})
            );
            assert!(inner.draft["router"].get("direct_target").is_none());
        }

        let codex = set_direct_route(
            app.state(),
            "provider".to_owned(),
            "agent".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        assert_eq!(
            codex.direct_target.as_ref().unwrap().model.as_deref(),
            Some("home")
        );
        assert_eq!(
            codex.agent_routes["codex"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("agent")
        );
        assert_eq!(
            codex.agent_routes["opencode"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("home")
        );
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(
                inner.draft["agent_routes"]["codex"]["direct_target"],
                json!({"upstream": "provider", "model": "agent"})
            );
            assert_eq!(inner.draft["routing"]["direct_target"]["model"], "home");
        }

        let before_revision = codex.draft_revision;
        let error = match set_direct_route(
            app.state(),
            "provider".to_owned(),
            "missing".to_owned(),
            Some("codex".to_owned()),
        ) {
            Ok(_) => panic!("an unmanaged model is rejected transactionally"),
            Err(error) => error,
        };
        assert!(error.contains("未配置模型"), "{error}");
        let unchanged = get_state(app.state());
        assert_eq!(unchanged.draft_revision, before_revision);
        assert_eq!(
            unchanged.agent_routes["codex"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("agent")
        );
        assert!(!root.join("token-station.json").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_explicit_incomplete_agent_direct_target_does_not_inherit_home() {
        let root = scratch_home("agent-incomplete-direct-target");
        let mut draft = template_for_test(&root);
        draft["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "provider", "model": "home"}
        });
        draft["agent_routes"]["codex"] = json!({
            "mode": "inherit",
            "direct_target": {"upstream": "provider", "model": null}
        });

        let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();

        assert_eq!(
            view.direct_target.as_ref().unwrap().model.as_deref(),
            Some("home")
        );
        let wire = serde_json::to_value(&view).expect("StateView serializes");
        assert_eq!(
            wire["agent_routes"]["codex"]["direct_target"],
            json!({"upstream": "provider", "model": null}),
            "a known Agent provider must remain selected while its model is incomplete"
        );
        assert!(view.agent_routes["codex"].config_error.is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn direct_config_saves_without_a_dummy_tier_pool() {
        let root = scratch_home("direct-save");
        let mut draft = template_for_test(&root);
        draft["routing"] = json!({"mode": "direct"});
        draft["router"]["routing_mode"] = json!("tiered");
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "selected"}]
        });
        draft["routing"]["direct_target"] = json!({"upstream": "provider", "model": "selected"});
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));

        let saved = save_config(app.state()).expect("direct mode needs no synthetic tier pool");

        assert_eq!(saved.routing_mode, "direct");
        assert_eq!(
            saved.direct_target.as_ref().unwrap().model.as_deref(),
            Some("selected")
        );
        assert!(root.join("token-station.json").exists());
        let persisted: Value = serde_json::from_slice(
            &std::fs::read(root.join("token-station.json")).expect("saved config is readable"),
        )
        .expect("saved config remains JSON");
        assert_eq!(persisted["routing"]["mode"], json!("direct"));
        assert_eq!(persisted["router"]["routing_mode"], json!("tiered"));
        assert!(persisted["router"].get("direct_target").is_none());
        let config_path = root.join("token-station.json");
        let (reloaded_draft, reloaded_saved, load_error) = load_draft_state(
            &config_path,
            &root.join("token-station-data"),
            &root.join("plugins"),
        );
        assert!(load_error.is_none());
        assert_eq!(reloaded_draft["routing"], persisted["routing"]);
        let reloaded =
            AppInner::new_with_saved(config_path, reloaded_draft, reloaded_saved, load_error)
                .snapshot();
        assert!(!reloaded.config_dirty);
        assert_eq!(reloaded.routing_mode, "direct");
        assert_eq!(
            reloaded.direct_target.as_ref().unwrap().model.as_deref(),
            Some("selected")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn saving_agent_tier_edits_preserves_its_direct_target_and_routing_mode() {
        let root = scratch_home("agent-direct-preserved");
        let mut inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "selected"}]
        });
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(
                    pool,
                    Some("provider".to_owned()),
                    Some("selected".to_owned()),
                )
                .unwrap();
        }
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        set_direct_route(
            app.state(),
            "provider".to_owned(),
            "selected".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
        for slot in ["high", "mid", "low"] {
            set_agent_tier(
                app.state(),
                "codex".to_owned(),
                slot.to_owned(),
                Some("provider".to_owned()),
                Some("selected".to_owned()),
            )
            .unwrap();
        }

        let saved = save_agent_routes(app.state()).unwrap();

        assert_eq!(saved.agent_routes["codex"].routing_mode, "direct");
        assert_eq!(
            saved.agent_routes["codex"]
                .direct_target
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("selected")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn quota_accounts_persist_validate_dedupe_and_reject_invalid_input() {
        let root = scratch_home("quota-accounts");
        let inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));

        add_provider(
            app.state(),
            "deepseek".to_owned(),
            "https://api.deepseek.com/v1".to_owned(),
            vec!["deepseek-v4-flash".to_owned(), "deepseek-v4-pro".to_owned()],
            None,
            false,
        )
        .unwrap();
        add_provider(
            app.state(),
            "ollama".to_owned(),
            "http://127.0.0.1:11434/v1".to_owned(),
            vec!["qwen2.5".to_owned()],
            None,
            true,
        )
        .unwrap();

        // Two valid picks, then an exact duplicate of the first (→ collapsed).
        // Order is preserved as the operator's priority.
        let arg = |upstream: &str, model: &str| QuotaAccountArg {
            upstream: upstream.to_owned(),
            model: model.to_owned(),
        };
        let view = set_quota_accounts(
            app.state(),
            vec![
                arg("deepseek", "deepseek-v4-flash"),
                arg("ollama", "qwen2.5"),
                arg("deepseek", "deepseek-v4-flash"),
            ],
        )
        .unwrap();
        assert_eq!(view.quota_accounts.len(), 2);
        assert_eq!(view.quota_accounts[0].upstream, "deepseek");
        assert_eq!(view.quota_accounts[0].model, "deepseek-v4-flash");
        assert_eq!(view.quota_accounts[1].upstream, "ollama");
        assert_eq!(view.quota_accounts[1].model, "qwen2.5");

        // The list lands verbatim under router.quota_accounts (the router-core
        // key that drives quota routing).
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            let stored = inner.draft["router"]["quota_accounts"].as_array().unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0]["upstream"], json!("deepseek"));
            assert_eq!(stored[0]["model"], json!("deepseek-v4-flash"));
        }

        // An incomplete row is rejected as a whole; the command must not
        // silently reinterpret the visible editor state or touch prior state.
        assert!(set_quota_accounts(app.state(), vec![arg("deepseek", "")]).is_err());
        assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

        // An account referencing a model the provider never declared is rejected,
        // and the previously saved list is left intact.
        assert!(set_quota_accounts(app.state(), vec![arg("deepseek", "ghost-model")]).is_err());
        assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

        // Empty selection is not a valid quota route and leaves prior state intact.
        assert!(set_quota_accounts(app.state(), vec![]).is_err());
        assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn quota_plan_declares_a_window_validates_and_clears() {
        let root = scratch_home("quota-plan");
        let inner = AppInner::new(
            root.join("token-station.json"),
            template_for_test(&root),
            None,
        );
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(inner))));
        add_provider(
            app.state(),
            "deepseek".to_owned(),
            "https://api.deepseek.com/v1".to_owned(),
            vec!["deepseek-v4-flash".to_owned()],
            None,
            false,
        )
        .unwrap();

        // Declare a 5h / 1,000,000-token plan with a 60/min rate limit.
        let view = set_quota_plan(
            app.state(),
            "deepseek".to_owned(),
            18_000_000,
            1_000_000,
            "tokens".to_owned(),
            Some(60),
        )
        .unwrap();
        let plan = view
            .providers
            .iter()
            .find(|p| p.name == "deepseek")
            .unwrap()
            .quota_plan
            .as_ref()
            .unwrap();
        assert_eq!(plan.len_ms, 18_000_000);
        assert_eq!(plan.limit, 1_000_000);
        assert_eq!(plan.unit, "tokens");
        assert_eq!(plan.rate_limit_per_min, Some(60));
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert_eq!(
                inner.draft["upstreams"]["deepseek"]["quota_plan"]["windows"][0]["limit"],
                json!(1_000_000)
            );
        }

        // Unknown provider and unknown unit are rejected.
        assert!(set_quota_plan(
            app.state(),
            "nope".to_owned(),
            1,
            1,
            "tokens".to_owned(),
            None
        )
        .is_err());
        assert!(set_quota_plan(
            app.state(),
            "deepseek".to_owned(),
            1,
            1,
            "credits".to_owned(),
            None
        )
        .is_err());

        // A zero limit clears the plan entirely.
        let cleared = set_quota_plan(
            app.state(),
            "deepseek".to_owned(),
            18_000_000,
            0,
            "tokens".to_owned(),
            None,
        )
        .unwrap();
        assert!(cleared
            .providers
            .iter()
            .find(|p| p.name == "deepseek")
            .unwrap()
            .quota_plan
            .is_none());
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert!(inner.draft["upstreams"]["deepseek"]
                .get("quota_plan")
                .is_none());
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn desktop_helpers_cover_empty_absolute_and_legacy_display_shapes() {
        let root = scratch_home("helper-shapes");
        let missing = root.join("missing.json");
        let (draft, error) = load_draft(&missing, &root);
        assert!(error.is_none());
        assert_eq!(draft["server"]["auth"], json!(true));

        let absolute = root.join("already-absolute");
        let mut shapes = template_for_test(&root);
        shapes["plugins"]["dir"] = json!(absolute.clone());
        shapes["data"]["dir"] = json!(42);
        let shapes = prepare_desktop_draft(shapes, &root);
        assert_eq!(shapes["plugins"]["dir"], json!(absolute));
        assert_eq!(shapes["data"]["dir"], json!(42));

        assert_eq!(agents_display(&json!({"agent": "legacy"})), "legacy");
        assert_eq!(agents_display(&json!({"agents": [1, null]})), "");
        assert_eq!(pool_key("high").unwrap(), TIER_HIGH);
        assert_eq!(pool_key("mid").unwrap(), TIER_MID);
        assert_eq!(pool_key("low").unwrap(), TIER_LOW);

        let mut inner = AppInner::new(
            root.join("token-station.json"),
            json!({
                "server": {}, "data": {}, "plugins": {}, "upstreams": [],
                "router": {"pools": [], "rules": null, "hint_routes": null}
            }),
            None,
        );
        assert!(inner.upstreams().is_empty());
        assert_eq!(inner.pool_member("missing"), (None, None));
        inner.rebuild_routing();
        assert!(inner.draft["router"]["heuristic"].is_null());
        assert_eq!(inner.serve_view().listen, "127.0.0.1:8787");
        assert!(inner.config_error().is_some());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn model_test_messages_enforce_roles_order_and_size_bounds() {
        let valid = vec![
            ModelTestMessage {
                role: "user".to_owned(),
                content: "hello".to_owned(),
            },
            ModelTestMessage {
                role: "assistant".to_owned(),
                content: "hi".to_owned(),
            },
            ModelTestMessage {
                role: "user".to_owned(),
                content: "status".to_owned(),
            },
        ];
        assert!(validate_model_test_messages(&valid).is_ok());

        let wrong_role = [ModelTestMessage {
            role: "system".to_owned(),
            content: "hidden".to_owned(),
        }];
        assert!(validate_model_test_messages(&wrong_role).is_err());

        let assistant_last = [ModelTestMessage {
            role: "assistant".to_owned(),
            content: "done".to_owned(),
        }];
        assert!(validate_model_test_messages(&assistant_last).is_err());

        let assistant_first = [
            ModelTestMessage {
                role: "assistant".to_owned(),
                content: "orphan".to_owned(),
            },
            ModelTestMessage {
                role: "user".to_owned(),
                content: "question".to_owned(),
            },
        ];
        assert!(validate_model_test_messages(&assistant_first).is_err());

        let consecutive_users = [
            ModelTestMessage {
                role: "user".to_owned(),
                content: "first".to_owned(),
            },
            ModelTestMessage {
                role: "user".to_owned(),
                content: "second".to_owned(),
            },
        ];
        assert!(validate_model_test_messages(&consecutive_users).is_err());

        let oversized = [ModelTestMessage {
            role: "user".to_owned(),
            content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES + 1),
        }];
        assert!(validate_model_test_messages(&oversized).is_err());

        let too_many = (0..=MODEL_TEST_MAX_MESSAGES)
            .map(|_| ModelTestMessage {
                role: "user".to_owned(),
                content: "x".to_owned(),
            })
            .collect::<Vec<_>>();
        assert!(validate_model_test_messages(&too_many).is_err());

        let empty = [ModelTestMessage {
            role: "user".to_owned(),
            content: "  ".to_owned(),
        }];
        assert!(validate_model_test_messages(&empty).is_err());

        let total_too_large = [
            ModelTestMessage {
                role: "user".to_owned(),
                content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
            },
            ModelTestMessage {
                role: "assistant".to_owned(),
                content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
            },
            ModelTestMessage {
                role: "user".to_owned(),
                content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
            },
            ModelTestMessage {
                role: "assistant".to_owned(),
                content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
            },
            ModelTestMessage {
                role: "user".to_owned(),
                content: "x".to_owned(),
            },
        ];
        assert!(validate_model_test_messages(&total_too_large).is_err());
    }

    #[test]
    fn model_test_reply_extracts_text_and_keeps_provider_errors_value_free() {
        let reply =
            extract_model_test_reply(200, r#"{"choices":[{"message":{"content":"connected"}}]}"#)
                .unwrap();
        assert_eq!(reply, "connected");

        let multipart = extract_model_test_reply(
            200,
            r#"{"choices":[{"message":{"content":[{"text":"part "},{"text":"two"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(multipart, "part two");

        let error = extract_model_test_reply(
            401,
            r#"{"error":{"code":"invalid_api_key","message":"prompt and secret must not escape"}}"#,
        )
        .unwrap_err();
        assert!(error.contains("authentication failed"));
        assert!(!error.contains("invalid_api_key"));
        assert!(!error.contains("prompt"));
        assert!(!error.contains("secret"));

        let oversized = format!(
            r#"{{"choices":[{{"message":{{"content":"{}"}}}}]}}"#,
            "x".repeat(MODEL_TEST_MAX_RESPONSE_BYTES + 1)
        );
        assert!(extract_model_test_reply(200, &oversized)
            .unwrap_err()
            .contains("response limit"));

        let oversized_envelope = format!(
            r#"{{"id":"{}","choices":[{{"message":{{"content":"ok"}}}}]}}"#,
            "x".repeat(MODEL_TEST_MAX_STREAM_BYTES)
        );
        assert!(extract_model_test_reply(200, &oversized_envelope)
            .unwrap_err()
            .contains("wire response limit"));
    }

    #[test]
    fn model_test_sse_decoder_handles_split_frames_and_utf8_boundaries() {
        let wire = "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n".as_bytes();
        let chinese = "你".as_bytes();
        let utf8_start = wire
            .windows(chinese.len())
            .position(|window| window == chinese)
            .expect("fixture contains a multibyte delta");
        let mut decoder = ModelTestSseDecoder::default();

        assert!(decoder.push(&wire[..utf8_start + 1]).unwrap().is_empty());
        let frames = decoder.push(&wire[utf8_start + 1..wire.len() - 1]).unwrap();
        assert!(
            frames.is_empty(),
            "an incomplete SSE delimiter must remain buffered"
        );
        let frames = decoder.push(&wire[wire.len() - 1..]).unwrap();

        assert_eq!(frames.len(), 1);
        assert_eq!(
            model_test_stream_delta(&frames[0]).unwrap(),
            Some("你".to_owned())
        );
        assert!(decoder.finish().unwrap().is_empty());

        let mut trailing = ModelTestSseDecoder::default();
        assert!(trailing.push(b"data: tail").unwrap().is_empty());
        assert_eq!(trailing.finish().unwrap(), ["data: tail"]);

        assert_eq!(
            find_model_test_sse_boundary(b"a\n\nb\r\n\r\n"),
            Some((1, 2))
        );
        assert_eq!(
            find_model_test_sse_boundary(b"a\r\n\r\nb\n\n"),
            Some((1, 4))
        );
        assert_eq!(find_model_test_sse_boundary(b"a\r\n\r\n"), Some((1, 4)));
    }

    #[test]
    fn model_test_sse_delta_ignores_metadata_and_recognizes_completion() {
        assert_eq!(
            model_test_stream_delta(
                "event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}"
            )
            .unwrap(),
            Some("hel".to_owned())
        );
        assert_eq!(model_test_stream_delta("data: [DONE]").unwrap(), None);
        assert_eq!(model_test_stream_delta(": keepalive").unwrap(), None);

        assert_eq!(
            model_test_stream_delta(
                "data: {\"choices\":[{\"delta\":{\"content\":[{\"text\":\"part \"},{\"text\":\"two\"}]}}]}"
            )
            .unwrap(),
            Some("part two".to_owned())
        );
        assert_eq!(
            model_test_stream_delta("data: {\"choices\":[{\"text\":\"legacy\"}]}").unwrap(),
            Some("legacy".to_owned())
        );

        let coded_error = model_test_stream_delta(
            "data: {\"error\":{\"code\":\"stream_rejected\",\"message\":\"secret\"}}",
        )
        .unwrap_err();
        assert!(!coded_error.contains("stream_rejected"));
        assert!(!coded_error.contains("secret"));
        assert_eq!(
            model_test_stream_delta("data: {\"error\":{\"message\":\"secret\"}}").unwrap_err(),
            "The Provider returned a stream error"
        );
    }

    #[test]
    fn model_test_output_budget_rejects_many_individually_valid_deltas() {
        let mut budget = ModelTestOutputBudget::default();
        for _ in 0..MODEL_TEST_MAX_STREAM_EVENTS {
            budget.accept("x").unwrap();
        }
        assert!(budget.accept("x").unwrap_err().contains("event limit"));

        let mut byte_budget = ModelTestOutputBudget::default();
        byte_budget
            .accept(&"x".repeat(MODEL_TEST_MAX_RESPONSE_BYTES))
            .unwrap();
        assert!(byte_budget
            .accept("x")
            .unwrap_err()
            .contains("response limit"));

        let mut wire_budget = ModelTestOutputBudget::default();
        wire_budget
            .accept_wire(MODEL_TEST_MAX_STREAM_BYTES)
            .unwrap();
        assert!(wire_budget
            .accept_wire(1)
            .unwrap_err()
            .contains("wire response limit"));
    }

    #[test]
    fn model_test_command_uses_the_home_route_and_cleans_registration() {
        let root = scratch_home("model-test-command");
        let (upstream, fixture) = serve_chat_completion("model-test-ok", 1);
        let mut draft = gateway_template_for_test(&root);
        draft["upstreams"]["fixture"] = json!({
            "provider": "openai-compatible",
            "base_url": upstream,
            "models": [{"model": "small"}]
        });
        draft["routing"] = json!({
            "mode": "direct",
            "direct_target": {"upstream": "fixture", "model": "small"}
        });
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
            root.join("token-station.json"),
            draft,
            None,
        )))));
        assert!(app.manage(ModelTestStreamState::default()));

        let reply = tauri::async_runtime::block_on(run_model_test_chat(
            app.handle().clone(),
            app.state::<AppStateManaged>().inner(),
            app.state::<ModelTestStreamState>().inner(),
            vec![ModelTestMessage {
                role: "user".to_owned(),
                content: "ping".to_owned(),
            }],
            "model-test-command".to_owned(),
        ))
        .unwrap();

        assert_eq!(reply.content, "model-test-ok");
        assert!(app
            .state::<ModelTestStreamState>()
            .0
            .lock()
            .unwrap()
            .active
            .is_empty());
        fixture.join().unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn model_test_request_ids_are_bounded_correlation_values() {
        assert!(validate_model_test_request_id("model-test-1729-aBc_0").is_ok());
        assert!(validate_model_test_request_id("").is_err());
        assert!(validate_model_test_request_id("contains spaces").is_err());
        assert!(validate_model_test_request_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn model_test_cancel_before_registration_still_stops_the_request() {
        let mut registry = ModelTestStreamRegistry::default();
        registry.cancel("model-test-race".to_owned());

        let error = registry
            .register("model-test-race", CancelToken::root())
            .unwrap_err();

        assert_eq!(error, "Model test cancelled");
        assert!(!registry.active.contains_key("model-test-race"));
        assert!(!registry.pending_cancellations.contains("model-test-race"));
    }
}
