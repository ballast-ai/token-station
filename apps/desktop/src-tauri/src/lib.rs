//! Token Station desktop backend.
//!
//! This does not rewrite routing or gateway logic. It uses `token-station-cli` as a library and reuses the same
//! `Gateway` / `ClientConfig` / `server::serve` / keychain. The GUI is one layer over this core
//! panel. The three-tier routing panel writes to tier_high, tier_mid, and tier_low pools in `router.pools`
//! Enter one (provider, model) pair for each tier. Heuristic `bands` then classify requests automatically.
//!
//! Partially configured tiers are invalid under RouterConfig validation, so the
//! draft remains a serde_json::Value and materializes as ClientConfig only when
//! saving or starting. Failed validation is reported to the user without writing.

pub mod agent_integration;
mod model_catalog;
mod serve_lifecycle;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use token_station_cli::config::{ClientConfig, PluginsConfig, KNOWN_AGENT_IDS};
use token_station_cli::plugins::{PluginRegistry, Receipts};
use token_station_cli::{secrets, stats, upgrade};
use token_station_router_core::UpstreamRef;

use agent_integration::commands::{
    apply_agent_plan, apply_snapshot_restore, list_agent_registry, list_agent_snapshots,
    plan_agent_connection, plan_agent_disconnect, plan_snapshot_restore, scan_agents,
    AgentCommandState,
};
use model_catalog::ModelDiscoveryView;
use serve_lifecycle::{prepare_server, PreparedServer, RunningServer, StartFailure};

/// Pool names for the three tier slots shown as the panel's high, middle, and low rows.
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// Stable id for each tier’s “keyword override” rule. Keywords added to a tier enter the corresponding rule’s
/// `keywords_any`—a match selects this tier and overrides complexity tiers (highest priority in router-core layer 1).
/// The id is stable because audit and decision records also use it as the `matched routing rule ID`.
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

/// The three inbound protocols promised by the desktop app. Their order is also the `match_inbound` priority; their paths
/// are mutually exclusive, so putting the general Chat Completions first does not consume Messages or Responses.
const DESKTOP_AGENTS: [&str; 3] = ["agent-openai", "agent-anthropic", "agent-openai-responses"];

const SERVE_STATE_CHANGED_EVENT: &str = "serve-state-changed";

enum ServerLifecycle {
    Stopped {
        generation: u64,
    },
    Starting {
        generation: u64,
        listen: String,
    },
    Stopping {
        generation: u64,
        listen: String,
    },
    Running {
        generation: u64,
        server: RunningServer,
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
    /// Editable draft. Partial states are valid and are validated only when saved.
    draft: Value,
    /// Preserve startup read or validation errors. Show a safe template but block
    /// writes so Save cannot silently overwrite the user's original file.
    load_error: Option<String>,
    /// Authoritative proxy-service lifecycle state.
    server: ServerLifecycle,
    /// Hash of the configuration saved to disk. A different draft hash means unsaved changes exist.
    saved_config_hash: Option<u64>,
    /// Hash of the configuration used when the current server started; None when stopped. Equal to
    /// Only `saved_config_hash` proves that the running configuration is the saved configuration; otherwise, it is “saved but not applied.”
    running_config_hash: Option<u64>,
}

/// Content hash of one configuration, stable within the process and used to compare draft, saved, and running states.
fn config_hash(config: &ClientConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Serialize as canonical JSON before hashing. Stable field order is sufficient to detect changes.
    serde_json::to_string(config)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

pub struct AppStateManaged(Mutex<AppInner>);

/// OS application cache root injected by Tauri. Agent compatibility data is
/// kept outside the repository and is not created until a valid signed remote
/// catalog is actually accepted.
#[derive(Clone)]
pub struct AgentIntegrationPaths {
    pub compatibility_cache_dir: PathBuf,
    pub snapshot_root: PathBuf,
    pub ownership_root: PathBuf,
}

// ---- Path anchor (repository root during development. Packaging handles it separately) -------------------------------

/// Repository root: three levels above `apps/desktop/src-tauri`. The CWD for `tauri dev` is unstable,
/// Therefore, anchor configuration, plugin, and data directories to this absolute path so serve can find `plugins-dist`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// New-config template. Empty upstreams and pools are invalid ClientConfig but a
/// remains valid until the user configures at least one tier. Absolute paths let serve find plugins from any CWD.
fn template(root: &std::path::Path) -> Value {
    json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:8787", "auth": true },
        "data": { "dir": root.join("token-station-data"), "metrics": true },
        "plugins": {
            "dir": root.join("plugins-dist"),
            "agents": DESKTOP_AGENTS,
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {},
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

/// Upgrade the CLI-era single Chat inbound configuration to the desktop three-inbound draft, and anchor relative runtime directories to
/// Repository root that contains the configuration file. Change only the in-memory draft; do not touch the original file until the user saves.
fn prepare_desktop_draft(mut draft: Value, root: &std::path::Path) -> Value {
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
        draft["plugins"]["agents"] = json!(DESKTOP_AGENTS);
    }

    fn anchor(path: &mut Value, root: &std::path::Path) {
        let Some(raw) = path.as_str() else {
            return;
        };
        let value = PathBuf::from(raw);
        if value.is_relative() {
            *path = json!(root.join(value));
        }
    }
    anchor(&mut draft["plugins"]["dir"], root);
    anchor(&mut draft["data"]["dir"], root);
    draft
}

/// Existing configurations must pass the CLI read, default filling, and structural validation flow. On failure, return a safe template for
/// for display, with a read-only error gate that rejects later save and start operations to prevent overwriting the damaged file.
fn load_draft(config_path: &std::path::Path, root: &std::path::Path) -> (Value, Option<String>) {
    if !config_path.exists() {
        return (template(root), None);
    }
    match ClientConfig::load(config_path) {
        Ok(config) => {
            let draft = serde_json::to_value(config).expect("ClientConfig always serializes");
            (prepare_desktop_draft(draft, root), None)
        }
        Err(error) => (
            template(root),
            Some(format!(
                "现有配置无法读取，已进入只读保护；请先修复或移走 {}：{error}",
                config_path.display()
            )),
        ),
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
    /// The same models with their declared four-state capabilities, so the UI
    /// can show what each can do (tools, vision, JSON schema, context window)
    /// before a request is ever routed. `models` stays as the flat id list that
    /// existing views rely on.
    model_details: Vec<ModelCapabilityView>,
    has_auth: bool,
}

/// A model's declared capabilities, flattened for the UI. Mirrors the four
/// dimensions the router gates on; `context_window` of 0 means "unknown".
#[derive(Serialize)]
struct ModelCapabilityView {
    model: String,
    tool: bool,
    vision: bool,
    json_schema: bool,
    context_window: u32,
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
    /// The mounted profile name, when `mode == "profile"`.
    profile: Option<String>,
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

#[derive(Clone, Debug, Serialize)]
struct ServeView {
    phase: ServePhase,
    running: bool,
    listen: String,
    virtual_key: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct StateView {
    providers: Vec<ProviderView>,
    tiers: std::collections::BTreeMap<String, TierView>,
    /// User keyword set for each tier (high/mid/low). A match forces that tier; this is the requested
    /// A direct control for user routing; stored in `router.rules` as `keywords_any`.
    keywords: std::collections::BTreeMap<String, Vec<String>>,
    agent_routes: std::collections::BTreeMap<String, AgentRouteView>,
    serve: ServeView,
    /// Whether the draft materializes as a valid config and can be saved or started.
    config_error: Option<String>,
    /// Settings page read model: switches and read-only environment information.
    settings: SettingsView,
    /// The draft differs from the configuration saved to disk, so unsaved changes exist.
    dirty: bool,
    /// The running proxy uses the saved configuration (true when stopped). false means the saved configuration has not been applied.
    applied: bool,
    /// Configured named profiles (profile names) available for Agent mounts.
    profiles: Vec<String>,
}

/// Settings page view: two writable switches (server.auth / data.metrics) and read-only environment information.
#[derive(Serialize)]
struct SettingsView {
    listen: String,
    auth: bool,
    metrics: bool,
    data_dir: String,
    plugins_dir: String,
    agent: String,
    version: String,
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
    cost_micros: Option<i64>,
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
            cost_micros: None,
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
            cost_micros: a.cost_micros,
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

/// Update-check view. Perform only an anonymous version check through core `upgrade::check`; do not replace the binary.
#[derive(Serialize)]
struct UpgradeView {
    current: String,
    latest_tag: String,
    html_url: String,
    newer: bool,
}

// ---- helpers ------------------------------------------------------------------

impl AppInner {
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
            .map(|(name, up)| ProviderView {
                name: name.clone(),
                provider: up["provider"].as_str().unwrap_or_default().to_string(),
                base_url: up["base_url"].as_str().unwrap_or_default().to_string(),
                models: up["models"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m["model"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                model_details: up["models"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                let model = m["model"].as_str()?.to_string();
                                Some(ModelCapabilityView {
                                    model,
                                    tool: m["tool"].as_bool().unwrap_or(false),
                                    vision: m["vision"].as_bool().unwrap_or(false),
                                    json_schema: m["json_schema"].as_bool().unwrap_or(false),
                                    context_window: m["context_window"]
                                        .as_u64()
                                        .and_then(|w| u32::try_from(w).ok())
                                        .unwrap_or(0),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                has_auth: up.get("auth").map(|a| !a.is_null()).unwrap_or(false),
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

    /// Whether a pool is configured (exists and is non-empty). Keyword rules may reference only existing pools; otherwise, saving
    /// validation then fails with `UnknownPool`.
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

    /// Rewrite `router.rules` from the given three-tier keyword mapping. Rule order is priority (high → mid → low),
    /// Emit rules only for tiers that have both keywords and a configured pool. Preserve non-keyword rules written by the operator
    /// Preserve them unchanged afterward. Do not emit rules for empty keyword sets or unconfigured tiers, to avoid references to missing pools.
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
                let is_managed = rule["id"]
                    .as_str()
                    .is_some_and(|id| managed.contains(&id));
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

    fn agent_tier(&self, agent_id: &str, slot: &str) -> TierView {
        let target = match self.agent_route_mode(agent_id) {
            "custom" => &self.draft["agent_routes"][agent_id]["custom_route"][slot],
            "profile" => {
                let name = self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .unwrap_or_default();
                &self.draft["profiles"][name][slot]
            }
            // inherit (or unknown): the home tier.
            _ => return self.tier(pool_key(slot).expect("known UI tier slot")),
        };
        TierView {
            upstream: target["upstream"].as_str().map(str::to_string),
            model: target["model"].as_str().map(str::to_string),
        }
    }

    /// The profile this Agent mounts, when in profile mode.
    fn agent_profile(&self, agent_id: &str) -> Option<String> {
        (self.agent_route_mode(agent_id) == "profile")
            .then(|| {
                self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .map(str::to_string)
            })
            .flatten()
    }

    /// Names of every configured profile.
    fn profile_names(&self) -> Vec<String> {
        self.draft["profiles"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn agent_routes_view(&self) -> std::collections::BTreeMap<String, AgentRouteView> {
        KNOWN_AGENT_IDS
            .iter()
            .map(|agent_id| {
                let mode = self.agent_route_mode(agent_id).to_string();
                let tiers = ["high", "mid", "low"]
                    .into_iter()
                    .map(|slot| (slot.to_string(), self.agent_tier(agent_id, slot)))
                    .collect();
                let config_error = if mode == "custom" {
                    ["high", "mid", "low"].into_iter().find_map(|slot| {
                        let tier = self.agent_tier(agent_id, slot);
                        (tier.upstream.is_none() || tier.model.is_none())
                            .then(|| format!("{slot} 档尚未配置供应商和模型"))
                    })
                } else {
                    None
                };
                let profile = self.agent_profile(agent_id);
                (
                    (*agent_id).to_string(),
                    AgentRouteView {
                        mode,
                        tiers,
                        config_error,
                        profile,
                    },
                )
            })
            .collect()
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
        serde_json::from_value::<ClientConfig>(self.draft.clone())
            .map_err(|e| format!("配置结构不合法: {e}"))
    }

    /// Hash of the materialized current draft; None when the draft is invalid.
    fn draft_config_hash(&self) -> Option<u64> {
        self.materialize().ok().map(|config| config_hash(&config))
    }

    /// A draft that differs from the saved state has unsaved changes. Treat an invalid draft as changed because it cannot be called “unchanged.”
    fn is_dirty(&self) -> bool {
        match (self.draft_config_hash(), self.saved_config_hash) {
            (Some(draft), Some(saved)) => draft != saved,
            // Conservatively report changes when the draft is invalid or no baseline was saved.
            _ => true,
        }
    }

    /// The running server uses the saved configuration. This matters only in the Running state:
    /// There is no “pending application” state when the service is not running.
    fn is_applied(&self) -> bool {
        if !matches!(self.server, ServerLifecycle::Running { .. }) {
            return true;
        }
        match (self.running_config_hash, self.saved_config_hash) {
            (Some(running), Some(saved)) => running == saved,
            _ => false,
        }
    }

    fn config_error(&self) -> Option<String> {
        self.load_error.clone().or_else(|| self.materialize().err())
    }

    fn serve_view(&self) -> ServeView {
        match &self.server {
            ServerLifecycle::Stopped { .. } => ServeView {
                phase: ServePhase::Stopped,
                running: false,
                listen: self.draft["server"]["listen"]
                    .as_str()
                    .unwrap_or("127.0.0.1:8787")
                    .to_string(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Starting { listen, .. } => ServeView {
                phase: ServePhase::Starting,
                running: false,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Stopping { listen, .. } => ServeView {
                phase: ServePhase::Stopping,
                running: false,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
            },
            ServerLifecycle::Running { server, .. } => ServeView {
                phase: ServePhase::Running,
                running: true,
                listen: server.listen().to_string(),
                virtual_key: server.virtual_key().map(str::to_string),
                error: None,
            },
            ServerLifecycle::Failed { listen, error, .. } => ServeView {
                phase: ServePhase::Error,
                running: false,
                listen: listen.clone(),
                virtual_key: None,
                error: Some(error.clone()),
            },
        }
    }

    fn snapshot(&self) -> StateView {
        StateView {
            providers: self.upstreams(),
            tiers: self.home_tiers(),
            keywords: self.home_keywords(),
            agent_routes: self.agent_routes_view(),
            serve: self.serve_view(),
            config_error: self.config_error(),
            settings: self.settings_view(),
            dirty: self.is_dirty(),
            applied: self.is_applied(),
            profiles: self.profile_names(),
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
            version: upgrade::CURRENT_VERSION.to_string(),
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

    fn seed_agent_custom_route(&mut self, agent_id: &str) {
        if self.draft["agent_routes"][agent_id]["custom_route"].is_object() {
            return;
        }
        let mut custom = serde_json::Map::new();
        for (slot, pool) in [("high", TIER_HIGH), ("mid", TIER_MID), ("low", TIER_LOW)] {
            let tier = self.tier(pool);
            custom.insert(
                slot.to_string(),
                match (tier.upstream, tier.model) {
                    (Some(upstream), Some(model)) => {
                        json!({ "upstream": upstream, "model": model })
                    }
                    _ => Value::Null,
                },
            );
        }
        self.draft["agent_routes"][agent_id]["custom_route"] = Value::Object(custom);
    }

    fn agent_custom_route_complete(&self, agent_id: &str) -> bool {
        ["high", "mid", "low"].into_iter().all(|slot| {
            let target = &self.draft["agent_routes"][agent_id]["custom_route"][slot];
            target["upstream"].as_str().is_some() && target["model"].as_str().is_some()
        })
    }

    fn set_agent_route_mode_value(&mut self, agent_id: &str, mode: &str) {
        if mode == "custom" {
            self.seed_agent_custom_route(agent_id);
        } else if !self.agent_custom_route_complete(agent_id) {
            // ClientConfig intentionally persists only executable routes. An
            // incomplete desktop draft may be edited in custom mode, but once
            // the user returns to inherit it must not poison an otherwise valid
            // home configuration.
            if let Some(route) = self.draft["agent_routes"][agent_id].as_object_mut() {
                route.remove("custom_route");
            }
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!(mode);
    }

    fn set_agent_tier_value(
        &mut self,
        agent_id: &str,
        slot: &str,
        upstream: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        pool_key(slot)?;
        self.seed_agent_custom_route(agent_id);
        match (upstream, model) {
            (Some(upstream), Some(model)) => {
                self.validate_route_target(&upstream, &model)?;
                self.draft["agent_routes"][agent_id]["custom_route"][slot] =
                    json!({ "upstream": upstream, "model": model });
            }
            (None, None) => {
                self.draft["agent_routes"][agent_id]["custom_route"][slot] = Value::Null;
            }
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_string()),
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!("custom");
        Ok(())
    }

    /// Save the current home-page tiers as a named profile. All three tiers must be configured.
    fn save_home_route_as_profile(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("策略组名称不能为空".to_string());
        }
        let mut tiers = serde_json::Map::new();
        for (slot, pool) in [("high", TIER_HIGH), ("mid", TIER_MID), ("low", TIER_LOW)] {
            let tier = self.tier(pool);
            match (tier.upstream, tier.model) {
                (Some(upstream), Some(model)) => {
                    tiers.insert(
                        slot.to_string(),
                        json!({ "upstream": upstream, "model": model }),
                    );
                }
                _ => return Err(format!("{slot} 档尚未配置，无法另存为策略组")),
            }
        }
        if !self.draft["profiles"].is_object() {
            self.draft["profiles"] = json!({});
        }
        self.draft["profiles"][name] = Value::Object(tiers);
        Ok(())
    }

    /// Mount a named profile on an Agent.
    fn set_agent_profile_value(&mut self, agent_id: &str, profile: &str) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        if !self.draft["profiles"][profile].is_object() {
            return Err(format!("策略组 `{profile}` 不存在"));
        }
        self.draft["agent_routes"][agent_id] = json!({ "mode": "profile", "profile": profile });
        Ok(())
    }

    /// Delete a profile. Reject the operation while an Agent mounts it to avoid a dangling mount.
    fn delete_profile_value(&mut self, name: &str) -> Result<(), String> {
        let mounted: Vec<String> = KNOWN_AGENT_IDS
            .iter()
            .filter(|id| self.agent_profile(id).as_deref() == Some(name))
            .map(|id| (*id).to_string())
            .collect();
        if !mounted.is_empty() {
            return Err(format!("策略组 `{name}` 仍被挂载:{}", mounted.join(", ")));
        }
        if let Some(map) = self.draft["profiles"].as_object_mut() {
            map.remove(name);
        }
        Ok(())
    }

    /// Write the current draft to disk and set it as the saved baseline. Named profiles are persistent library entries, like providers,
    /// Cannot exist only in the in-memory draft, or a backend restart would remove it. The caller rolls back the draft on failure.
    fn persist_draft(&mut self) -> Result<(), String> {
        let config = self.materialize()?;
        config
            .save(&self.config_path)
            .map_err(|e| format!("写配置失败: {e}"))?;
        self.saved_config_hash = Some(config_hash(&config));
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
    KNOWN_AGENT_IDS
        .contains(&agent_id)
        .then_some(())
        .ok_or_else(|| format!("未知 Agent `{agent_id}`"))
}

// ---- Tauri commands ---------------------------------------------------------------

#[tauri::command]
fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
}

/// The final request URLs a base URL resolves to, so the operator sees a
/// doubled `/v1/v1` (or an invalid URL) before saving, not as a 404 later.
#[derive(serde::Serialize)]
struct EndpointPreview {
    base: String,
    chat: String,
    responses: String,
    messages: String,
}

#[tauri::command]
fn preview_endpoint(base_url: String) -> Result<EndpointPreview, String> {
    let endpoint =
        token_station_protocol::ProviderEndpoint::try_new(&base_url).map_err(|e| e.to_string())?;
    let base = endpoint.as_str();
    let trimmed = base.trim_end_matches('/').to_owned();
    let join = |suffix: &str| format!("{trimmed}/{suffix}");
    Ok(EndpointPreview {
        chat: join("chat/completions"),
        responses: join("responses"),
        messages: join("messages"),
        base,
    })
}

/// Add or update a provider (an OpenAI-compatible upstream). Store its key in the system keychain when provided.
#[tauri::command]
fn add_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
) -> Result<StateView, String> {
    if name.trim().is_empty() {
        return Err("供应商名不能为空".into());
    }
    let name = name.trim().to_string();
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    let model_objs: Vec<Value> = models
        .iter()
        .filter(|m| !m.trim().is_empty())
        .map(|m| json!({ "model": m, "tool": true, "context_window": 128000 }))
        .collect();
    if model_objs.is_empty() {
        return Err("至少填一个模型名".into());
    }

    let mut up = json!({
        "provider": "openai-compatible",
        "base_url": base_url,
        "models": model_objs,
    });
    // Store a key in the keychain and point auth to its slot; omit auth when no key exists, as with local Ollama.
    if let Some(key) = api_key.as_ref().filter(|k| !k.trim().is_empty()) {
        secrets::keyring_set(&name, "provider_api_key", key.trim())?;
        up["auth"] = json!({ "slot": "provider_api_key", "keyring": true });
    }

    inner.draft["upstreams"][&name] = up;
    Ok(inner.snapshot())
}

fn resolve_discovery_key(
    inner: &AppInner,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Option<String>, String> {
    inner.ensure_editable()?;
    let explicit = api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    if explicit.is_some() {
        return Ok(explicit);
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
            Some(slot) => {
                let config = inner.materialize()?;
                secrets::SecretStore::from_config(&config)
                    .resolve(name, slot)
                    .map(Some)
            }
            None => Ok(None),
        };
    }
    Ok(None)
}

/// Fetch the provider’s current model catalog. Run the network request on a blocking worker so it does not block the Tauri UI.
/// When using a saved key, require the request URL to match the provider configuration so credentials cannot be forwarded to an arbitrary address.
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
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return Err("Base URL 必须使用 http:// 或 https://".to_owned());
    }

    let (data_dir, resolved_key) = {
        let inner = state.0.lock().unwrap();
        let resolved = resolve_discovery_key(&inner, &name, &base_url, api_key.as_deref())?;
        (inner.data_dir(), resolved)
    };

    tauri::async_runtime::spawn_blocking(move || {
        model_catalog::discover_with_cache(&data_dir, &name, &base_url, resolved_key.as_deref())
    })
    .await
    .map_err(|error| format!("模型目录任务异常结束：{error}"))?
}

/// Update an existing provider's model set while protecting models referenced by routing tiers.
fn replace_provider_models(
    inner: &mut AppInner,
    name: &str,
    models: Vec<String>,
) -> Result<(), String> {
    inner.ensure_editable()?;
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
    for agent_id in KNOWN_AGENT_IDS {
        for slot in ["high", "mid", "low"] {
            let target = &inner.draft["agent_routes"][agent_id]["custom_route"][slot];
            let refers_to_provider = target["upstream"].as_str() == Some(name);
            let retained = target["model"]
                .as_str()
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            if refers_to_provider && !retained {
                agent_blocked.push(format!("{agent_id}/{slot}"));
            }
        }
    }
    if !agent_blocked.is_empty() {
        return Err(format!(
            "不能移除 Agent 独立路由 {} 正在使用的模型，请先调整对应档位",
            agent_blocked.join("、")
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
            existing.get(&model).cloned().unwrap_or_else(
                || json!({ "model": model, "tool": true, "context_window": 128000 }),
            )
        })
        .collect();

    let previous = inner.draft["upstreams"][name]["models"].clone();
    inner.draft["upstreams"][name]["models"] = json!(model_objects);
    let save = inner.materialize().and_then(|config| {
        config
            .save(&inner.config_path)
            .map_err(|error| error.to_string())
    });
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
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

#[tauri::command]
fn remove_provider(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if let Some(obj) = inner.draft["upstreams"].as_object_mut() {
        obj.remove(&name);
    }
    // Remove any tier that references it.
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        let refers = inner.draft["router"]["pools"][pool]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|m| m["upstream"].as_str())
            .map(|u| u == name)
            .unwrap_or(false);
        if refers {
            if let Some(pools) = inner.draft["router"]["pools"].as_object_mut() {
                pools.remove(pool);
            }
        }
    }
    // Independent Agent routes store only provider and model references. If a provider is deleted, an independent draft that references it
    // It cannot be restored as valid configuration. Safely fall back to the home page and clear the invalid draft.
    for agent_id in KNOWN_AGENT_IDS {
        let refers = ["high", "mid", "low"].into_iter().any(|slot| {
            inner.draft["agent_routes"][agent_id]["custom_route"][slot]["upstream"].as_str()
                == Some(name.as_str())
        });
        if refers {
            inner.draft["agent_routes"][agent_id]["mode"] = json!("inherit");
            if let Some(route) = inner.draft["agent_routes"][agent_id].as_object_mut() {
                route.remove("custom_route");
            }
        }
    }
    inner.rebuild_routing();
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
    inner.ensure_editable()?;

    inner.set_tier_value(pool, upstream, model)?;
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
    inner.ensure_editable()?;
    inner.add_tier_keyword(&slot, &keyword)?;
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
    inner.ensure_editable()?;
    inner.remove_tier_keyword(&slot, &keyword)?;
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
    inner.set_agent_route_mode_value(&agent_id, &mode);
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
    inner.set_agent_tier_value(&agent_id, &slot, upstream, model)?;
    Ok(inner.snapshot())
}

/// Save the current home-page tiers as a named profile for reuse by multiple Agents.
#[tauri::command]
fn save_home_route_as_profile(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.save_home_route_as_profile(&name)?;
    if let Err(error) = inner.persist_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Mount a named profile on an Agent.
#[tauri::command]
fn mount_agent_profile(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    profile: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.set_agent_profile_value(&agent_id, &profile)?;
    if let Err(error) = inner.persist_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Delete a named profile; reject the operation while it is mounted.
#[tauri::command]
fn delete_profile(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.delete_profile_value(&name)?;
    if let Err(error) = inner.persist_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn save_agent_routes(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let config = inner.materialize()?;
    config
        .save(&inner.config_path)
        .map_err(|error| format!("写配置失败: {error}"))?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn apply_home_route_to_all_agents(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    for agent_id in KNOWN_AGENT_IDS {
        inner.set_agent_route_mode_value(agent_id, "inherit");
    }
    let config = inner.materialize()?;
    config
        .save(&inner.config_path)
        .map_err(|error| format!("写配置失败: {error}"))?;
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
    let config = inner.materialize()?;
    config
        .save(&inner.config_path)
        .map_err(|e| format!("写配置失败: {e}"))?;
    // The draft is now the saved baseline (so "unsaved" clears). If the proxy is
    // running, it still holds the old config — `applied` will read false until it
    // is restarted, which is the truth the save button used to hide.
    inner.saved_config_hash = Some(config_hash(&config));
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
    match result {
        Ok(prepared) => {
            let mut prepared = Some(prepared);
            let view = {
                let state = app.state::<AppStateManaged>();
                let mut inner = state.0.lock().unwrap();
                match &inner.server {
                    ServerLifecycle::Starting {
                        generation: current,
                        ..
                    } if *current == generation => {
                        let running = prepared
                            .take()
                            .expect("prepared server is present")
                            .publish();
                        inner.server = ServerLifecycle::Running {
                            generation,
                            server: running,
                        };
                        Some(inner.serve_view())
                    }
                    ServerLifecycle::Stopping {
                        generation: current,
                        ..
                    } if *current == generation => {
                        // Keep `Stopping` authoritative until the prepared listener
                        // is dropped outside the mutex below.
                        None
                    }
                    _ => None,
                }
            };
            if let Some(prepared) = prepared {
                prepared.discard();
                complete_serve_stop(app, generation);
            }
            if let Some(view) = view {
                emit_serve_state(app, &view);
            }
        }
        Err(failure) => {
            let view = {
                let state = app.state::<AppStateManaged>();
                let mut inner = state.0.lock().unwrap();
                match &inner.server {
                    ServerLifecycle::Starting {
                        generation: current,
                        listen,
                    } if *current == generation => {
                        inner.server = ServerLifecycle::Failed {
                            generation,
                            listen: listen.clone(),
                            error: failure.public_message(),
                        };
                        Some(inner.serve_view())
                    }
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
            ServerLifecycle::Starting { .. } | ServerLifecycle::Running { .. } => {
                return Ok(inner.snapshot());
            }
            ServerLifecycle::Stopping { .. } => {
                return Err(
                    "startup_cleanup_in_progress: 上一次代理正在停止，请稍后重试".to_string(),
                );
            }
            ServerLifecycle::Stopped { .. } | ServerLifecycle::Failed { .. } => {}
        }
        let config = inner.materialize()?;
        let generation = inner
            .server
            .generation()
            .checked_add(1)
            .ok_or_else(|| "代理启动 generation 已耗尽，请重启 App".to_string())?;
        let listen = config.server.listen.clone();
        inner.server = ServerLifecycle::Starting { generation, listen };
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
        complete_serve_start(&completion_app, generation, result);
    });
    Ok(snapshot)
}

#[tauri::command]
fn serve_start(app: AppHandle, state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    // Stamp the config the proxy is being started with, so `applied` can later
    // tell whether a subsequent save has diverged from what is running.
    {
        let mut inner = state.0.lock().unwrap();
        let hash = inner.draft_config_hash();
        inner.running_config_hash = hash;
    }
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
            ServerLifecycle::Running { server, .. } => {
                let listen = server.listen().to_string();
                inner.server = ServerLifecycle::Stopping { generation, listen };
                running = Some(server);
                true
            }
            ServerLifecycle::Starting { listen, .. } => {
                inner.server = ServerLifecycle::Stopping { generation, listen };
                true
            }
            ServerLifecycle::Stopping { listen, .. } => {
                inner.server = ServerLifecycle::Stopping { generation, listen };
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
            let _ = tauri::async_runtime::spawn_blocking(move || running.shutdown()).await;
            complete_serve_stop(&completion_app, generation);
        });
    }
    snapshot
}

#[tauri::command]
fn serve_stop(app: AppHandle, state: State<'_, AppStateManaged>) -> StateView {
    {
        let mut inner = state.0.lock().unwrap();
        inner.running_config_hash = None;
    }
    begin_serve_stop(app, state.inner())
}

/// Claude Code: write the env block to `~/.claude/settings.json` (embedded key; CC reads it directly, with no
/// manual export). CC uses the Anthropic protocol; end-to-end operation also requires the agent-anthropic adapter.
/// Determine whether the `plugins` configuration includes an inbound adapter that supports Anthropic. Inspect the `agents` list
/// and the two adapter names in the deprecated single `agent` string. Do not inspect providers to avoid false positives. agent-anthropic
/// Once it enters the configuration, the CC safety gate unlocks automatically.
fn inbound_adapter_ready(plugins: &Value, expected: &str) -> bool {
    let hits = |value: &Value| value.as_str() == Some(expected);
    let in_list = plugins["agents"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(hits));
    in_list || hits(&plugins["agent"])
}

fn anthropic_inbound_ready(plugins: &Value) -> bool {
    inbound_adapter_ready(plugins, "agent-anthropic")
}

fn responses_inbound_ready(plugins: &Value) -> bool {
    inbound_adapter_ready(plugins, "agent-openai-responses")
}

fn openai_inbound_ready(plugins: &Value) -> bool {
    inbound_adapter_ready(plugins, "agent-openai")
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

/// Settings page: toggle server.auth and data.metrics. If the draft can materialize, write it to disk, matching config set behavior;
/// Otherwise, change only the draft until a complete save. Note: these changes do not affect a running serve; restart the proxy.
#[tauri::command]
fn set_settings(
    state: State<'_, AppStateManaged>,
    auth: bool,
    metrics: bool,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.draft["server"]["auth"] = json!(auth);
    inner.draft["data"]["metrics"] = json!(metrics);
    if let Ok(config) = inner.materialize() {
        config
            .save(&inner.config_path)
            .map_err(|e| format!("写配置失败: {e}"))?;
    }
    Ok(inner.snapshot())
}

/// Usage page: read-only aggregate metrics database. `since` = all / <N>h / <N>d; `by` = upstream/model/pool/status
/// or empty. Return `empty=true` when the metrics database does not exist; do not report an error.
#[tauri::command]
fn get_stats(
    state: State<'_, AppStateManaged>,
    since: String,
    by: Option<String>,
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
    let cutoff = stats::parse_since(&since)?;
    let group = match by.as_deref() {
        None | Some("") => None,
        Some("upstream") => Some(stats::GroupBy::Upstream),
        Some("model") => Some(stats::GroupBy::Model),
        Some("pool") => Some(stats::GroupBy::Pool),
        Some("status") => Some(stats::GroupBy::Status),
        Some(other) => return Err(format!("未知分组 `{other}`")),
    };
    let report = stats::collect(&db, cutoff, group)?;
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

/// Recent N request receipts (requester / actual service / reason / terminal state / cost). Read-only metrics database; no body content.
#[tauri::command]
fn get_receipts(
    state: State<'_, AppStateManaged>,
    limit: u32,
) -> Result<Vec<stats::Receipt>, String> {
    let db = {
        let inner = state.0.lock().unwrap();
        inner.data_dir().join("metrics.sqlite")
    };
    stats::recent(&db, limit.min(200))
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

/// About/Updates page: anonymous version check, the core’s only permitted outbound connection. Compare versions and provide a release-page link only,
/// Does not replace its own binary. Desktop apps update through their own channels; this only reports whether a newer version exists.
#[tauri::command]
fn check_upgrade() -> Result<UpgradeView, String> {
    let release = upgrade::check(upgrade::DEFAULT_ENDPOINT)?;
    let newer = upgrade::is_newer(upgrade::CURRENT_VERSION, &release.tag_name);
    Ok(UpgradeView {
        current: upgrade::CURRENT_VERSION.to_string(),
        latest_tag: release.tag_name,
        html_url: release.html_url,
        newer,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let root = repo_root();
    let config_path = root.join("token-station.json");

    // Validate existing configuration and apply defaults through the CLI before reuse. Put damaged configuration into read-only protection,
    // Never silently overwrite with an empty template. Upgrade the legacy OpenAI-only inbound configuration in memory to three desktop inbounds.
    let (draft, load_error) = load_draft(&config_path, &root);

    // Migrate an existing metrics store up to the current schema at startup, so
    // the read-only receipts/stats views (which cannot migrate themselves) never
    // trip over an old version. Only an existing store is touched — nothing is
    // created before the proxy first serves.
    if let Some(dir) = draft["data"]["dir"].as_str() {
        let metrics = std::path::Path::new(dir).join("metrics.sqlite");
        if metrics.exists() {
            if let Err(error) = token_station_cli::store::SqliteStore::open(&metrics) {
                eprintln!("metrics migration on startup failed: {error}");
            }
        }
    }

    // The on-disk baseline: a clean load means the draft equals what's saved, so
    // nothing shows as "unsaved" until the user edits. A load error leaves no
    // trustworthy baseline (None → treated as dirty).
    let saved_config_hash = if load_error.is_none() {
        serde_json::from_value::<ClientConfig>(draft.clone())
            .ok()
            .map(|config| config_hash(&config))
    } else {
        None
    };

    let managed = AppStateManaged(Mutex::new(AppInner {
        config_path,
        draft,
        load_error,
        server: ServerLifecycle::stopped(),
        saved_config_hash,
        running_config_hash: None,
    }));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let compatibility_cache_dir = app.path().app_cache_dir()?.join("agent-compatibility");
            let agent_data_root = app.path().app_data_dir()?.join("agent-integration");
            let paths = AgentIntegrationPaths {
                compatibility_cache_dir,
                snapshot_root: agent_data_root.join("snapshots"),
                ownership_root: agent_data_root.join("ownership"),
            };
            let agent_commands = AgentCommandState::new(paths.clone()).map_err(|message| {
                std::io::Error::other(format!("初始化 Agent IPC 失败：{message}"))
            })?;
            app.manage(paths);
            app.manage(agent_commands);
            Ok(())
        })
        .manage(managed)
        .invoke_handler(tauri::generate_handler![
            get_state,
            preview_endpoint,
            add_provider,
            discover_provider_models,
            update_provider_models,
            remove_provider,
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
            apply_home_route_to_all_agents,
            serve_start,
            serve_stop,
            list_agent_registry,
            scan_agents,
            plan_agent_connection,
            apply_agent_plan,
            plan_agent_disconnect,
            list_agent_snapshots,
            plan_snapshot_restore,
            apply_snapshot_restore,
            set_settings,
            get_stats,
            get_receipts,
            get_router_table,
            get_plugins,
            check_upgrade,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};
    use tauri::Manager;

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

    #[test]
    fn the_desktop_template_enables_every_supported_inbound_protocol() {
        let root = PathBuf::from("/tmp/token-station-desktop-test");
        let draft = template(&root);

        assert_eq!(
            draft["plugins"]["agents"],
            json!(["agent-openai", "agent-anthropic", "agent-openai-responses"])
        );
    }

    #[test]
    fn a_legacy_chat_only_config_is_migrated_in_memory_with_absolute_runtime_paths() {
        let root = scratch_home("legacy");
        let mut draft = template(&root);
        draft["plugins"].as_object_mut().unwrap().remove("agents");
        draft["plugins"]["agent"] = json!("agent-openai");
        draft["plugins"]["dir"] = json!("plugins-dist");
        draft["data"]["dir"] = json!("token-station-data");

        let prepared = prepare_desktop_draft(draft, &root);

        assert_eq!(prepared["plugins"]["agents"], json!(DESKTOP_AGENTS));
        assert!(prepared["plugins"].get("agent").is_none());
        assert_eq!(prepared["plugins"]["dir"], json!(root.join("plugins-dist")));
        assert_eq!(
            prepared["data"]["dir"],
            json!(root.join("token-station-data"))
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_desktop_v1_agent_list_is_migrated_to_every_supported_inbound_protocol() {
        let root = scratch_home("desktop-v1-agents");
        let mut draft = template(&root);
        draft["plugins"]["agents"] = json!(["agent-openai"]);

        let prepared = prepare_desktop_draft(draft, &root);

        assert_eq!(prepared["plugins"]["agents"], json!(DESKTOP_AGENTS));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn inbound_readiness_requires_exact_adapter_names() {
        let plugins = json!({
            "agents": [
                "agent-anthropic-proxy",
                "agent-openai-responses-beta",
                "agent-openai-compatible"
            ]
        });
        assert!(!anthropic_inbound_ready(&plugins));
        assert!(!responses_inbound_ready(&plugins));
        assert!(!openai_inbound_ready(&plugins));

        let plugins = json!({
            "agents": ["agent-anthropic", "agent-openai-responses", "agent-openai"]
        });
        assert!(anthropic_inbound_ready(&plugins));
        assert!(responses_inbound_ready(&plugins));
        assert!(openai_inbound_ready(&plugins));
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
    fn provider_model_updates_preserve_metadata_and_protect_routing_references() {
        let root = scratch_home("model-update");
        let mut draft = template(&root);
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
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft,
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
        let mut draft = template(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "keep"}]
        });
        let before = draft.clone();
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft,
            load_error: Some("只读保护".to_owned()),
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };

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
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "home"}, {"model": "agent"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("home".into()))
            .unwrap();
        inner.set_agent_route_mode_value("codex", "custom");
        for slot in ["high", "mid", "low"] {
            inner
                .set_agent_tier_value("codex", slot, Some("provider".into()), Some("agent".into()))
                .unwrap();
        }
        inner.set_agent_route_mode_value("codex", "inherit");

        let error = replace_provider_models(&mut inner, "provider", vec!["home".to_owned()])
            .expect_err("inactive custom drafts still protect their model references");
        assert!(error.contains("codex/high"), "{error}");
        assert_eq!(
            inner.draft["upstreams"]["provider"]["models"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tier_keywords_write_valid_rules_dedupe_and_require_a_configured_pool() {
        let root = scratch_home("tier-keywords");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
        let config = inner.materialize().expect("keyword rule keeps config valid");
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
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
        assert!(config.router.rules.iter().all(|rule| rule.id != KW_RULE_LOW));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stored_discovery_credentials_cannot_be_redirected_to_another_base_url() {
        let root = scratch_home("model-discovery-url-binding");
        let mut draft = template(&root);
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://trusted.example/v1",
            "auth": {"slot": "provider_api_key", "keyring": true},
            "models": [{"model": "model"}]
        });
        let inner = AppInner {
            config_path: root.join("token-station.json"),
            draft,
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };

        let error = resolve_discovery_key(&inner, "provider", "https://attacker.example/v1", None)
            .expect_err("a stored credential is bound to its configured URL");
        assert!(error.contains("Base URL 必须与供应商配置一致"), "{error}");

        let one_time = resolve_discovery_key(
            &inner,
            "new-provider",
            "https://new.example/v1",
            Some("one-time-secret"),
        )
        .expect("an explicit one-time key is accepted");
        assert_eq!(one_time.as_deref(), Some("one-time-secret"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tier_updates_refuse_unknown_provider_model_and_partial_values() {
        let root = scratch_home("tiers-invalid");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
    fn agent_route_drafts_seed_from_home_validate_targets_and_preserve_complete_profiles() {
        let root = scratch_home("agent-route-draft");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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

        inner.set_agent_route_mode_value("codex", "custom");
        assert!(inner.agent_custom_route_complete("codex"));
        assert_eq!(
            inner.agent_tier("codex", "high").model.as_deref(),
            Some("home")
        );
        inner
            .set_agent_tier_value(
                "codex",
                "high",
                Some("provider".into()),
                Some("agent".into()),
            )
            .unwrap();
        assert!(inner
            .set_agent_tier_value(
                "future-agent",
                "high",
                Some("provider".into()),
                Some("agent".into()),
            )
            .unwrap_err()
            .contains("未知 Agent"));
        let config = inner
            .materialize()
            .expect("complete custom profile validates");
        assert_eq!(
            config.agent_routes["codex"]
                .custom_route
                .as_ref()
                .unwrap()
                .high
                .model,
            "agent"
        );

        inner.set_agent_route_mode_value("codex", "inherit");
        assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_object());
        assert!(inner.materialize().is_ok());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn returning_an_incomplete_agent_draft_to_inherit_cannot_poison_home_config() {
        let root = scratch_home("agent-route-incomplete");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
        inner.draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.com/v1",
            "models": [{"model": "model"}]
        });
        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("model".into()))
            .unwrap();

        inner.set_agent_route_mode_value("codex", "custom");
        assert!(!inner.agent_custom_route_complete("codex"));
        assert!(inner.materialize().is_err());
        inner.set_agent_route_mode_value("codex", "inherit");
        assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_null());
        assert!(inner.materialize().is_ok());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_route_commands_save_one_profile_and_apply_home_without_deleting_its_draft() {
        let root = scratch_home("agent-route-commands");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
    fn one_two_and_three_tiers_always_end_with_a_zero_score_fallback() {
        let root = scratch_home("tiers-valid");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&repo_root()),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
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
        .unwrap();
        assert_eq!(duplicate.serve.phase, ServePhase::Starting);
        assert_eq!(duplicate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(prepare_calls.load(Ordering::SeqCst), 1);

        let stopping =
            begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        assert_eq!(stopping.serve.phase, ServePhase::Stopping);
        release_tx.send(()).unwrap();
        let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
        assert!(!stopped.serve.running);
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
    fn desktop_commands_cover_provider_routing_settings_server_and_read_only_views() {
        let root = scratch_home("command-lifecycle");
        let mut draft = template(&repo_root());
        draft["data"]["dir"] = json!(root.join("data"));
        draft["server"]["listen"] = json!("127.0.0.1:0");
        let app = tauri::test::mock_app();
        assert!(app.manage(AppStateManaged(Mutex::new(AppInner {
            config_path: root.join("token-station.json"),
            draft,
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        }))));

        let initial = get_state(app.state());
        assert!(!initial.serve.running);
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
            )
            .unwrap();
            assert!(view.providers.iter().any(|provider| provider.name == name));
        }
        assert!(add_provider(
            app.state(),
            " ".to_string(),
            "http://127.0.0.1/v1".to_string(),
            vec!["m".to_string()],
            None,
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
        let configured = set_settings(app.state(), false, false).unwrap();
        assert!(!configured.settings.auth);
        assert!(!configured.settings.metrics);

        let plugins = get_plugins(app.state()).unwrap();
        assert!(plugins.agent.contains("agent-openai"));
        assert!(plugins
            .dialects
            .iter()
            .any(|dialect| dialect == "openai-compatible"));
        assert!(plugins.listing.contains("provider-openai-compatible"));

        let empty_stats = get_stats(app.state(), "all".to_string(), None).unwrap();
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
        .unwrap();
        assert!(matches!(
            duplicate.serve.phase,
            ServePhase::Starting | ServePhase::Running
        ));
        // Coverage instrumentation makes Wasmtime's first compilation much
        // slower on a cold Linux runner; this remains a bounded integration test.
        let running =
            wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
        assert!(running.serve.running);
        assert!(running.serve.virtual_key.is_none());
        assert!(root.join("data").join("requests.log").exists());
        let stopping =
            begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
        assert_eq!(stopping.serve.phase, ServePhase::Stopping);
        let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
        assert!(!stopped.serve.running);

        let removed = remove_provider(app.state(), "local".to_string()).unwrap();
        assert_eq!(removed.providers.len(), 1);
        assert!(removed.tiers.values().all(|tier| tier.upstream.is_none()));
        assert!(save_config(app.state())
            .err()
            .expect("empty routing config is rejected")
            .contains("至少配置一档"));

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
        let mut shapes = template(&root);
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

        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: json!({
                "server": {}, "data": {}, "plugins": {}, "upstreams": [],
                "router": {"pools": [], "rules": null, "hint_routes": null}
            }),
            load_error: None,
            server: ServerLifecycle::stopped(),
            saved_config_hash: None,
            running_config_hash: None,
        };
        assert!(inner.upstreams().is_empty());
        assert_eq!(inner.pool_member("missing"), (None, None));
        inner.rebuild_routing();
        assert!(inner.draft["router"]["heuristic"].is_null());
        assert_eq!(inner.serve_view().listen, "127.0.0.1:8787");
        assert!(inner.config_error().is_some());

        std::fs::remove_dir_all(root).ok();
    }
}
