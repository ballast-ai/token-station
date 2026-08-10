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
pub mod desktop_update;
mod free_provider_catalog;
mod model_catalog;
mod pricing_catalog;
mod provider_tombstones;
mod recovery;
mod serve_lifecycle;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::UpdaterExt;
use zeroize::Zeroizing;

use token_station_cli::budget::{AgentBudget, BudgetStatus};
use token_station_cli::config::{ClientConfig, EgressConfig, PluginsConfig};
use token_station_cli::gateway::{FeatureLayer, Gateway, HealthLayer, StageStatus};
use token_station_cli::plugins::{PluginRegistry, Receipts};
use token_station_cli::pricing::{ModelPrice, PriceTable};
use token_station_cli::{
    secrets, stats,
    store::{ReceiptQuery, SqliteStore},
    upgrade,
};
use token_station_metrics::ReceiptView;
use token_station_protocol::{CapabilityState, ModelCapability, ProviderApi, ProviderEndpoint};
use token_station_router_core::UpstreamRef;

use agent_integration::commands::{
    apply_agent_plan, apply_snapshot_restore, configure_cursor_provider, force_forget_agent,
    get_agent_drift, list_agent_registry, list_agent_snapshots, plan_agent_connection,
    plan_agent_disconnect, plan_snapshot_restore, runtime_from_app, scan_agents, AgentCommandState,
};
use agent_integration::registry::AgentRegistry;
use agent_integration::types::AdmissionStatus;
use config_state::ConfigState;
use desktop_update::{
    DesktopUpdateCandidate, DesktopUpdateOperation, DesktopUpdateProgress, DesktopUpdateView,
    LATEST_JSON_URL, OFFICIAL_PUBLIC_KEY, PROGRESS_EVENT,
};
use model_catalog::ModelDiscoveryView;
use pricing_catalog::ModelPriceSuggestionView;
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
}

pub struct AppStateManaged(Mutex<AppInner>);

struct FreeProviderValidationGuard<'a> {
    inner: &'a Mutex<AppInner>,
    upstream: String,
}

impl Drop for FreeProviderValidationGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_free_providers.remove(&self.upstream);
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

/// New-config template. Empty upstreams and pools are invalid ClientConfig but a
/// valid draft until the user configures one tier. Tauri injects runtime directories.
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
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {},
        "pricing": pricing,
        "router": {
            "version": 1,
            "pools": {},
            "rules": [],
            "hint_routes": [],
            "default_pool": "",
            "assumed_context_window": 8192
        }
    })
}

const BUNDLED_PLUGIN_IDS: [&str; 5] = [
    "agent-openai",
    "agent-anthropic",
    "agent-openai-responses",
    "agent-gemini",
    "provider-openai-compatible",
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
            let manifest = &package.manifest;
            let kind = serde_json::to_value(manifest.kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned());
            let capabilities = manifest
                .capabilities
                .iter()
                .filter_map(|capability| {
                    serde_json::to_value(capability)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                })
                .collect();
            plugins.push(InstalledPluginSelfTest {
                id: manifest.name.clone(),
                version: manifest.version.clone(),
                kind,
                source: "builtin",
                protocols: manifest.agent_protocols.clone(),
                agent_tools: manifest.agent_tools.clone(),
                providers: manifest.providers.clone(),
                capabilities,
                loadable: true,
            });
        }
        if registry.provider_binding("openai-compatible").is_none() {
            return Err("builtin provider dialect `openai-compatible` is not bound".to_owned());
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

        if let Some(agent_routes) = draft["agent_routes"].as_object_mut() {
            for route in agent_routes.values_mut() {
                for slot in ["high", "mid", "low"] {
                    if route["custom_route"][slot].is_object() {
                        prune_dangling_tier(&mut route["custom_route"][slot], &valid);
                    }
                }
            }
        }
        if let Some(profiles) = draft["profiles"].as_object_mut() {
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
    provider: String,
    base_url: String,
    models: Vec<String>,
    model_capabilities: Vec<ModelCapabilityView>,
    catalog_revision: u64,
    catalog: Vec<model_catalog::CatalogModelView>,
    has_auth: bool,
    credential_source: String,
    credential_reference: String,
    /// This upstream runs on the local machine; `local_only` routing keeps to it.
    local: bool,
    access_tier: String,
    /// The declared quota plan (window + limit + unit) used for local estimation
    /// in quota-first mode, if the user set one. `None` ⇒ non-windowed / metered.
    quota_plan: Option<QuotaPlanView>,
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
    /// Routing mode: tiered intelligent routing by default, or quota_first.
    routing_mode: String,
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
        Self {
            config_path,
            draft,
            load_error,
            config_state,
            agent_route_drafts: BTreeMap::new(),
            server: ServerLifecycle::stopped(),
            pending_free_providers: BTreeSet::new(),
            pending_provider_keys: BTreeMap::new(),
        }
    }

    fn observe_draft(&mut self) -> Result<(), String> {
        let draft = self.draft.clone();
        if let Err(error) = self.config_state.observe_draft(&draft) {
            self.draft = self.config_state.draft().clone();
            return Err(error);
        }
        Ok(())
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
                self.draft = previous;
                return Err(error);
            }
        };
        if let Err(error) = self.materialize() {
            self.draft = previous;
            return Err(error);
        }
        let candidate = self.draft.clone();
        self.draft = previous;
        let mut candidate_state = self.config_state.clone();
        candidate_state.observe_draft(&candidate)?;
        self.draft = candidate;
        self.config_state = candidate_state;
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
                        }
                    })
                    .collect();
                let base_url = up["base_url"].as_str().unwrap_or_default().to_string();
                let (catalog_revision, catalog) = model_catalog::catalog_for_provider(
                    &self.data_dir(),
                    name,
                    &base_url,
                    &configured_capabilities,
                );
                ProviderView {
                    name: name.clone(),
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
                    local: up.get("local").and_then(Value::as_bool).unwrap_or(false),
                    access_tier: up
                        .get("access_tier")
                        .and_then(Value::as_str)
                        .unwrap_or("paid")
                        .to_owned(),
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
        let home_mode = self.draft["router"]["routing_mode"]
            .as_str()
            .unwrap_or("tiered");
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
                let config_error = if mode == "custom" || mode == "profile" {
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
                    },
                )
            })
            .collect()
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
            routing_mode: self.draft["router"]["routing_mode"]
                .as_str()
                .unwrap_or("tiered")
                .to_string(),
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
                inner.draft["agent_routes"][agent_id] = json!({
                    "mode": "custom",
                    "custom_route": route,
                });
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
fn set_dock_theme_icon(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let icon_bytes = dock_icon_bytes(&theme)?;

    #[cfg(target_os = "macos")]
    {
        app.run_on_main_thread(move || {
            use objc2::{AnyThread, MainThreadMarker};
            use objc2_app_kit::{NSApp, NSImage};
            use objc2_foundation::NSData;

            let Some(main_thread) = MainThreadMarker::new() else {
                return;
            };
            let data = NSData::with_bytes(icon_bytes);
            let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
                return;
            };

            // AppKit requires application icon updates on the main thread.
            unsafe { NSApp(main_thread).setApplicationIconImage(Some(&image)) };
        })
        .map_err(|error| format!("failed to schedule Dock icon update: {error}"))?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (app, icon_bytes);

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
            assert!(dock_icon_bytes(theme).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }
}

#[tauri::command]
fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
}

#[tauri::command]
fn get_runtime_state(
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
) -> ServeView {
    // Agent config inspection is file I/O and therefore intentionally outside
    // the App lock. Revalidate the immutable instance identity afterwards so
    // a concurrent publish can never combine old Agent facts with a new
    // running_revision/instance_id.
    for _ in 0..3 {
        let Ok(runtime) = runtime_from_app(state.inner()) else {
            return state.0.lock().unwrap().serve_view();
        };
        let identity = runtime.instance_id().to_owned();
        let agent_connected = agents.any_connected_to(&runtime).unwrap_or(false);
        let mut view = state.0.lock().unwrap().serve_view();
        if view.instance_id.as_deref() == Some(identity.as_str()) {
            view.agent_connected = agent_connected;
            return view;
        }
    }
    // Continuous handoffs are rare; if all snapshots raced, return a truthful
    // current runtime view with the conservative independent Agent fact.
    state.0.lock().unwrap().serve_view()
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
        let egress: EgressConfig = serde_json::from_value(inner.draft["egress"].clone())
            .map_err(|error| format!("出站配置不合法：{error}"))?;
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
    add_provider_impl(state, name, base_url, models, api_key, local, source, None)
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
) -> Result<StateView, String> {
    add_provider_impl(
        state,
        name,
        base_url,
        models,
        api_key,
        local,
        credential_source.trim(),
        credential_reference.as_deref(),
    )
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
) -> Result<StateView, String> {
    if name.trim().is_empty() {
        return Err("供应商名不能为空".into());
    }
    let name = name.trim().to_string();
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let base_url = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?
        .as_str();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if inner.draft["upstreams"].get(&name).is_some() {
        return Err(format!("供应商 `{name}` 已存在，请在 Provider 详情中编辑"));
    }
    let data_dir = inner.data_dir();
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err(format!(
            "Provider 回收站中已有 `{name}`，请先恢复它，再在详情中编辑"
        ));
    }

    let model_objs: Vec<Value> = models
        .iter()
        .filter(|m| !m.trim().is_empty())
        .map(|m| {
            json!({
                // OpenAI Chat Completions includes tools and structured output,
                // and catalog entries are compatible chat providers, so declare
                // support by default. Leaving these unknown would fail closed and
                // reject every tool-using Agent. Keep vision unknown because it
                // varies by model; unsupported requests can degrade at runtime.
                "model": m,
                "tool": true,
                "vision": false,
                "json_schema": true,
                "tool_state": "declared",
                "vision_state": "unknown",
                "json_schema_state": "declared",
                "context_window": known_context_window(m)
            })
        })
        .collect();
    if model_objs.is_empty() {
        return Err("至少填一个模型名".into());
    }
    // A previous interrupted removal may have left only derived catalog data.
    // New Provider identity must never inherit it, even with the same name/URL.
    model_catalog::remove_provider(&data_dir, &name)?;

    let mut up = json!({
        "provider": "openai-compatible",
        "base_url": base_url,
        "models": model_objs,
    });
    // Write the local key only when marked local, preserving ordinary cloud
    // provider configs in line with serde skip_serializing_if. local_only uses it
    // to keep traffic on the machine.
    if local {
        up["local"] = json!(true);
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

    inner.draft["upstreams"][&name] = up;
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(&name);
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

/// Switch between tiered (difficulty-based) and quota-first (allowance-draining)
/// routing. The two are mutually exclusive top-level modes; the config carries
/// only `quota_first` explicitly (tiered is the serde default, so it is cleared
/// rather than written, keeping the document minimal).
#[tauri::command]
fn set_routing_mode(
    state: State<'_, AppStateManaged>,
    mode: String,
    agent_id: Option<String>,
) -> Result<StateView, String> {
    if mode != "tiered" && mode != "quota_first" {
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

    let previous = inner.draft["router"].clone();
    if mode == "quota_first" {
        inner.draft["router"]["routing_mode"] = json!("quota_first");
    } else if let Some(router) = inner.draft["router"].as_object_mut() {
        router.remove("routing_mode");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Persists the quota-first rotation list (ordered upstream+model accounts). The
/// order is the operator's priority; it lands verbatim in
/// `router.quota_accounts` and drives `Router::route_quota_first`. Incomplete
/// rows (a provider picked but no model yet) are dropped, complete ones are
/// validated against the configured catalog, and exact duplicates are collapsed
/// while keeping first-seen order.
#[tauri::command]
fn set_quota_accounts(
    state: State<'_, AppStateManaged>,
    accounts: Vec<QuotaAccountArg>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    let mut seen = std::collections::BTreeSet::new();
    let mut clean: Vec<Value> = Vec::new();
    for account in &accounts {
        let upstream = account.upstream.trim();
        let model = account.model.trim();
        if upstream.is_empty() || model.is_empty() {
            continue;
        }
        inner.validate_route_target(upstream, model)?;
        if seen.insert((upstream.to_owned(), model.to_owned())) {
            clean.push(json!({ "upstream": upstream, "model": model }));
        }
    }

    let previous = inner.draft["router"].clone();
    if clean.is_empty() {
        if let Some(router) = inner.draft["router"].as_object_mut() {
            router.remove("quota_accounts");
        }
    } else {
        inner.draft["router"]["quota_accounts"] = Value::Array(clean);
    }
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
    edit_provider_impl(state, name, base_url, api_key, None, None)
}

#[tauri::command]
fn edit_provider_with_credential(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: String,
    credential_reference: Option<String>,
) -> Result<StateView, String> {
    edit_provider_impl(
        state,
        name,
        base_url,
        api_key,
        Some(credential_source.trim()),
        credential_reference.as_deref(),
    )
}

fn edit_provider_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: Option<&str>,
    credential_reference: Option<&str>,
) -> Result<StateView, String> {
    let name = name.trim().to_owned();
    let base_url = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?
        .as_str();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(&inner, &name)?;
    let previous = inner.draft["upstreams"]
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
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
    let identity_changed = previous["base_url"].as_str() != Some(base_url.as_str())
        || api_key.is_some()
        || auth.is_some();
    if identity_changed {
        // A URL or credential change may select a different Provider account.
        // Invalidate first: losing derived cache on a later rollback is safe;
        // presenting the old account's catalog as trusted is not.
        model_catalog::remove_provider(&inner.data_dir(), &name)?;
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
        return Err(error);
    }
    if let Some(key) = api_key {
        if let Err(key_error) =
            secrets::store_set(&inner.data_dir(), &name, "provider_api_key", &key)
        {
            inner.draft["upstreams"][&name] = previous;
            return match inner.observe_draft() {
                Ok(()) => Err(key_error),
                Err(rollback_error) => Err(format!(
                    "{key_error}；同时回滚 Provider 草稿失败：{rollback_error}"
                )),
            };
        }
    }
    Ok(inner.snapshot())
}

#[derive(Debug, PartialEq, Eq)]
enum DiscoveryCredential {
    Explicit(Option<String>),
    Stored { provider: String, slot: String },
}

fn prepare_discovery_credential(
    inner: &AppInner,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<DiscoveryCredential, String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
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
    let facts: std::collections::BTreeMap<&str, CapabilityState> = catalog
        .iter()
        .filter(|model| model.catalog_state == model_catalog::CatalogState::Active)
        .filter_map(|model| {
            (model.vision != CapabilityState::Unknown)
                .then_some((model.model.as_str(), model.vision))
        })
        .collect();
    if facts.is_empty() {
        return Ok(false);
    }

    inner.ensure_editable()?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str() else {
            continue;
        };
        let Some(state) = facts.get(model).copied() else {
            continue;
        };
        let (supported, serialized) = match state {
            CapabilityState::Verified => (true, "verified"),
            CapabilityState::Unsupported => (false, "unsupported"),
            CapabilityState::Declared | CapabilityState::Unknown => continue,
        };
        if capability["vision"].as_bool() != Some(supported)
            || capability["vision_state"].as_str() != Some(serialized)
        {
            capability["vision"] = json!(supported);
            capability["vision_state"] = json!(serialized);
            changed = true;
        }
    }
    if !changed {
        return Ok(false);
    }

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
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
    let name = name.trim().to_owned();
    let base_url = base_url.trim().trim_end_matches('/').to_owned();
    if name.is_empty() {
        return Err("请先填写供应商名称".to_owned());
    }
    let base_url = ProviderEndpoint::try_new(&base_url)
        .map_err(|error| format!("Base URL 不合法：{error}"))?
        .as_str();

    let (data_dir, credential, egress, egress_secrets) = {
        let inner = state.0.lock().unwrap();
        let credential =
            prepare_discovery_credential(&inner, &name, &base_url, api_key.as_deref())?;
        let config = inner.materialize()?;
        (
            inner.data_dir(),
            credential,
            config.egress.clone(),
            secrets::SecretStore::from_config(&config, &inner.data_dir()),
        )
    };

    let task_name = name.clone();
    let task_base_url = base_url.clone();
    let mut result = tauri::async_runtime::spawn_blocking(move || {
        let resolved_key = match credential {
            DiscoveryCredential::Explicit(key) => key,
            DiscoveryCredential::Stored { provider, slot } => {
                Some(egress_secrets.resolve(&provider, &slot)?)
            }
        };
        model_catalog::discover_with_cache_egress(
            &data_dir,
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
    result.capabilities_updated =
        apply_discovered_model_capabilities(&mut inner, &name, &result.catalog)?;
    Ok(result)
}

#[tauri::command]
async fn test_provider(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<Vec<ProviderTestResult>, String> {
    let (config, name) = {
        let inner = state.0.lock().unwrap();
        let name = name.trim().to_owned();
        if inner.draft["upstreams"].get(&name).is_none() {
            return Err(format!("供应商 `{name}` 不存在"));
        }
        (inner.materialize()?, name)
    };
    tauri::async_runtime::spawn_blocking(move || {
        let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
        let gateway = Gateway::new(&config, recorder)?;
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

/// Update an existing provider's model set while protecting models referenced by routing tiers.
fn replace_provider_models(
    inner: &mut AppInner,
    name: &str,
    models: Vec<String>,
) -> Result<(), String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
    let mut normalized: Vec<String> = models
        .into_iter()
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    if normalized.is_empty() {
        return Err("至少保留一个模型".to_owned());
    }

    let upstream = inner.draft["upstreams"]
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
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
                json!({
                    // As in add_provider, OpenAI-compatible chat declares tools and structured output by default.
                    "model": model,
                    "tool": true,
                    "vision": false,
                    "json_schema": true,
                    "tool_state": "declared",
                    "vision_state": "unknown",
                    "json_schema_state": "declared",
                    "context_window": known_context_window(&model)
                })
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
    inner.draft["upstreams"][name]["models"] = json!(model_objects);
    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
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

fn provider_references(inner: &AppInner, name: &str) -> Vec<String> {
    let mut references = Vec::new();
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
    agent_id: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    if !supported_agent_ids().contains(&agent_id) {
        return Err(format!("未知 Agent：{agent_id}"));
    }
    // Persist the draft exactly as "save custom routing" does.
    inner.promote_agent_route_drafts()?;
    inner.save_draft()?;
    inner.agent_route_drafts.clear();

    // Materialize this one Agent's router from the saved config and hot-swap it
    // on the running gateway. `None` ⇒ the Agent inherits Home, so the reload
    // clears its per-Agent router.
    let config = inner.materialize()?;
    let router = config.custom_router_for_agent(&agent_id)?;
    if let ServerLifecycle::Running { server, .. } = &inner.server {
        server
            .reload_agent_router(&agent_id, router)
            .map_err(|error| format!("热重启 Agent 路由失败：{error}"))?;
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn apply_home_route_to_all_agents(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| {
        for agent_id in supported_agent_ids() {
            candidate.set_agent_inherit_value(&agent_id);
        }
        Ok(())
    })?;
    inner.save_draft()?;
    inner.agent_route_drafts.clear();
    Ok(inner.snapshot())
}

/// Validate and write atomically. Return validation errors without writing, matching config edit semantics.
#[tauri::command]
fn save_config(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if inner.draft["router"]["pools"]
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
    let _ = app.emit(SERVE_STATE_CHANGED_EVENT, view.clone());
}

fn complete_serve_start<R: Runtime>(
    app: &AppHandle<R>,
    generation: u64,
    result: Result<PreparedServer, StartFailure>,
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
    let view = {
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
                Ok(server) => ServerLifecycle::Running {
                    generation,
                    server,
                    apply_error: None,
                },
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
    if let Some(prepared) = discard {
        prepared.discard();
    }
    if let Some(old) = retire {
        tauri::async_runtime::spawn_blocking(move || old.drain_and_shutdown());
    }
    if let Some(view) = view {
        emit_serve_state(app, &view);
    }
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
    let (config, generation, snapshot, serve_view) = {
        let mut inner = state.0.lock().unwrap();
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
        (config, generation, snapshot, serve_view)
    };

    emit_serve_state(&app, &serve_view);
    let completion_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(move || prepare(config))
            .await
            .unwrap_or_else(|error| Err(StartFailure::new("startup_task", error)));
        let _ = tauri::async_runtime::spawn_blocking(move || {
            complete_serve_start(&completion_app, generation, result);
        })
        .await;
    });
    Ok(snapshot)
}

#[tauri::command]
fn serve_start(app: AppHandle, state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    begin_serve_start(app, state.inner(), prepare_server)
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

fn begin_serve_stop<R: Runtime>(app: AppHandle<R>, state: &AppStateManaged) -> StateView {
    let (generation, snapshot, serve_view, running) = {
        let mut inner = state.0.lock().unwrap();
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
    snapshot
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

#[tauri::command]
fn get_price_table(state: State<'_, AppStateManaged>) -> Result<PriceTable, String> {
    draft_price_table(&state.0.lock().unwrap())
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
        let config = inner.materialize()?;
        (
            inner.data_dir(),
            config.egress.clone(),
            secrets::SecretStore::from_config(&config, &inner.data_dir()),
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
    let db = inner.data_dir().join("metrics.sqlite");
    SqliteStore::backfill_unknown_costs(&db, &next)?;
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
    let (db, now_ms) = {
        let inner = state.0.lock().unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        (inner.data_dir().join("metrics.sqlite"), now_ms)
    };
    let since_ms = stats::cutoff_from_since(&since, now_ms)?;
    let bounded_page_size = page_size.clamp(1, 50);
    let bounded_page = page.max(1);
    let offset = bounded_page
        .saturating_sub(1)
        .saturating_mul(bounded_page_size);
    let result = SqliteStore::receipt_page(
        &db,
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
    Ok(ReceiptPageView {
        items: result.items,
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
    let endpoint = LATEST_JSON_URL
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

#[cfg(not(target_os = "windows"))]
fn desktop_update_platform_unsupported_message() -> Option<&'static str> {
    None
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

async fn prepare_gateway_for_desktop_update(app: AppHandle) -> Result<bool, String> {
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

    tauri::async_runtime::spawn_blocking(move || {
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
                return Err("update_gateway_stop_timeout: 等待本地网关安全停止超时".to_owned());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .map_err(|error| format!("等待本地网关停止任务失败：{error}"))?
}

async fn restore_gateway_after_failed_update(
    app: AppHandle,
    was_active: bool,
) -> Result<(), String> {
    if !was_active {
        return Ok(());
    }
    let Some(state) = app.try_state::<AppStateManaged>() else {
        return Ok(());
    };
    begin_serve_start(app.clone(), state.inner(), prepare_server).map(|_| ())
}

#[tauri::command]
async fn install_desktop_update_and_restart(
    app: AppHandle,
    operation: State<'_, DesktopUpdateOperation>,
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
        || async move {
            updater
                .check()
                .await
                .map_err(|error| format!("暂时无法检查更新，请稍后重试：{error}"))
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            app.manage(AppStateManaged(Mutex::new(inner)));

            let paths = AgentIntegrationPaths {
                snapshot_root: desktop_paths.agent_data_root.join("snapshots"),
                ownership_root: desktop_paths.agent_data_root.join("ownership"),
            };
            let agent_commands = AgentCommandState::new(paths.clone()).map_err(|message| {
                std::io::Error::other(format!("初始化 Agent IPC 失败：{message}"))
            })?;
            app.manage(paths);
            app.manage(agent_commands);
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
            set_local_routing,
            set_routing_mode,
            set_quota_accounts,
            set_quota_plan,
            get_quota_snapshot,
            edit_provider,
            edit_provider_with_credential,
            discover_provider_models,
            test_provider,
            set_provider_model_vision,
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
            serve_stop,
            list_agent_registry,
            scan_agents,
            plan_agent_connection,
            configure_cursor_provider,
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
            suggest_model_price,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
            "agent_routes": {
                "opencode": {
                    "mode": "custom",
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
        template(&root.join("token-station-data"), &root.join("plugins"))
    }

    fn gateway_template_for_test(root: &std::path::Path) -> Value {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("plugins-dist");
        template(&root.join("token-station-data"), &plugins_dir)
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
                let body = json!({
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
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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
        assert_eq!(report["plugins"].as_array().map(Vec::len), Some(5));
        assert_eq!(report["storage"]["credential_read"], json!(false));
        assert_eq!(report["gateway"]["loadable"], json!(true));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn repeated_model_discovery_only_updates_the_catalog_cache() {
        const CATALOG: &str = r#"{"data":[{"id":"model-b"},{"id":"model-a"}]}"#;

        let root = scratch_home("discovery-isolation");
        let data_dir = root.join("data");
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
                source: model_catalog::CatalogSource::Live,
                last_seen_ms: Some(42),
                catalog_state: model_catalog::CatalogState::Active,
            },
            model_catalog::CatalogModelView {
                model: "text-model".to_owned(),
                tool: CapabilityState::Unknown,
                vision: CapabilityState::Unsupported,
                json_schema: CapabilityState::Unknown,
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
        assert_eq!(models[1]["vision"], json!(false));
        assert_eq!(models[1]["vision_state"], json!("unsupported"));
        std::fs::remove_dir_all(root).ok();
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
        assert!(std::fs::read_to_string(&inner.config_path)
            .unwrap()
            .contains("kimi-k2.6"));

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

        let custom =
            set_agent_route_mode(app.state(), "codex".to_string(), "custom".to_string()).unwrap();
        assert_eq!(custom.agent_routes["codex"].mode, "custom");
        save_agent_routes(app.state()).unwrap();
        let inherited = apply_home_route_to_all_agents(app.state()).unwrap();
        assert!(inherited
            .agent_routes
            .values()
            .all(|profile| profile.mode == "inherit"));
        let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
        assert!(saved.agent_routes["codex"].custom_route.is_some());
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
    fn desktop_commands_cover_provider_routing_settings_server_and_read_only_views() {
        let root = scratch_home("command-lifecycle");
        let mut draft = gateway_template_for_test(&root);
        draft["data"]["dir"] = json!(root.join("data"));
        draft["server"]["listen"] = json!("127.0.0.1:0");
        let app = tauri::test::mock_app();
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
                "version": 2,
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

        // Switching the pinned Agent to quota-first writes the explicit value.
        let view = set_routing_mode(
            app.state(),
            "quota_first".to_owned(),
            Some("codex".to_owned()),
        )
        .unwrap();
        assert_eq!(view.agent_routes["codex"].routing_mode, "quota_first");
        assert_eq!(view.routing_mode, "tiered", "Home stays tiered");

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
    fn quota_accounts_persist_validate_dedupe_and_drop_incomplete_rows() {
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

        // A valid pick, an incomplete row (no model → dropped), another valid
        // pick, then an exact duplicate of the first (→ collapsed). Order is
        // preserved as the operator's priority.
        let arg = |upstream: &str, model: &str| QuotaAccountArg {
            upstream: upstream.to_owned(),
            model: model.to_owned(),
        };
        let view = set_quota_accounts(
            app.state(),
            vec![
                arg("deepseek", "deepseek-v4-flash"),
                arg("deepseek", ""),
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

        // An account referencing a model the provider never declared is rejected,
        // and the previously saved list is left intact.
        assert!(set_quota_accounts(app.state(), vec![arg("deepseek", "ghost-model")]).is_err());
        assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

        // Clearing removes the key entirely (tiered configs stay minimal).
        let cleared = set_quota_accounts(app.state(), vec![]).unwrap();
        assert!(cleared.quota_accounts.is_empty());
        {
            let state = app.state::<AppStateManaged>();
            let inner = state.0.lock().unwrap();
            assert!(inner.draft["router"].get("quota_accounts").is_none());
        }

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
}
