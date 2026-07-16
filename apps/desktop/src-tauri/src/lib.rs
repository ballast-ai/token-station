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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use token_station_cli::config::{ClientConfig, PluginsConfig};
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::plugins::{PluginRegistry, Receipts};
use token_station_cli::store::SqliteStore;
use token_station_cli::{secrets, server, stats, upgrade, virtual_key};
use token_station_metrics::Recorder;

/// Pool names for the three tier slots shown as the panel's high, middle, and low rows.
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// Tier thresholds mapping heuristic scores to tiers. Bands descend strictly by
/// at_least, with a final zero fallback. Evaluation will calibrate these defaults later.
const CUT_HIGH: u32 = 55;
const CUT_MID: u32 = 22;

/// Running serve instance. Stop it by shutting down this runtime; the listener then releases the port.
struct RunningServer {
    runtime: tokio::runtime::Runtime,
    listen: String,
    virtual_key: Option<String>,
}

/// Global backend state protected by one lock; commands are short transactions.
struct AppInner {
    /// Actual token-station.json configuration path.
    config_path: PathBuf,
    /// Editable draft. Partial states are valid and are validated only when saved.
    draft: Value,
    /// Currently running serve instance, if any.
    server: Option<RunningServer>,
}

pub struct AppStateManaged(Mutex<AppInner>);

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
fn template(root: &PathBuf) -> Value {
    json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:8787", "auth": true },
        "data": { "dir": root.join("token-station-data"), "metrics": true },
        "plugins": {
            "dir": root.join("plugins-dist"),
            "agents": ["agent-openai"],
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
    has_auth: bool,
}

#[derive(Serialize)]
struct TierView {
    upstream: Option<String>,
    model: Option<String>,
}

#[derive(Serialize)]
struct ServeView {
    running: bool,
    listen: String,
    virtual_key: Option<String>,
}

#[derive(Serialize)]
struct StateView {
    providers: Vec<ProviderView>,
    tiers: std::collections::BTreeMap<String, TierView>,
    serve: ServeView,
    /// Whether the draft materializes as a valid config and can be saved or started.
    config_error: Option<String>,
    /// Settings page read model: switches and read-only environment information.
    settings: SettingsView,
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

    /// Rebuild tier-pool references, heuristic bands, and default from configured
    /// tiers. Include only tiers with a selected upstream-model pair.
    fn rebuild_routing(&mut self) {
        // Collect configured tiers from high to low.
        let present: Vec<(&str, u32)> = [
            (TIER_HIGH, CUT_HIGH),
            (TIER_MID, CUT_MID),
            (TIER_LOW, 0u32),
        ]
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

    fn config_error(&self) -> Option<String> {
        self.materialize().err()
    }

    fn serve_view(&self) -> ServeView {
        match &self.server {
            Some(s) => ServeView {
                running: true,
                listen: s.listen.clone(),
                virtual_key: s.virtual_key.clone(),
            },
            None => ServeView {
                running: false,
                listen: self.draft["server"]["listen"]
                    .as_str()
                    .unwrap_or("127.0.0.1:8787")
                    .to_string(),
                virtual_key: None,
            },
        }
    }

    fn snapshot(&self) -> StateView {
        let mut tiers = std::collections::BTreeMap::new();
        tiers.insert("high".to_string(), self.tier(TIER_HIGH));
        tiers.insert("mid".to_string(), self.tier(TIER_MID));
        tiers.insert("low".to_string(), self.tier(TIER_LOW));
        StateView {
            providers: self.upstreams(),
            tiers,
            serve: self.serve_view(),
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
}

fn pool_key(slot: &str) -> Result<&'static str, String> {
    match slot {
        "high" => Ok(TIER_HIGH),
        "mid" => Ok(TIER_MID),
        "low" => Ok(TIER_LOW),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

// ---- Tauri commands ---------------------------------------------------------------

#[tauri::command]
fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
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
    let mut inner = state.0.lock().unwrap();

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

#[tauri::command]
fn remove_provider(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
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

    match (upstream, model) {
        (Some(u), Some(m)) => {
            inner.draft["router"]["pools"][pool] = json!([{ "upstream": u, "model": m }]);
        }
        _ => {
            if let Some(pools) = inner.draft["router"]["pools"].as_object_mut() {
                pools.remove(pool);
            }
        }
    }
    inner.rebuild_routing();
    Ok(inner.snapshot())
}

/// Validate and write atomically. Return validation errors without writing, matching config edit semantics.
#[tauri::command]
fn save_config(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let inner = state.0.lock().unwrap();
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
    Ok(inner.snapshot())
}

#[tauri::command]
fn serve_start(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    if inner.server.is_some() {
        return Ok(inner.snapshot());
    }
    let config = inner.materialize()?;

    // recorder: always write file logs and gate the metrics database by its setting. Neither contains prompt content.
    let mut sinks: Vec<Box<dyn Recorder>> = vec![Box::new(FileLog::open(&config.data.dir)?)];
    if config.data.metrics {
        sinks.push(Box::new(SqliteStore::open(
            &config.data.dir.join("metrics.sqlite"),
        )?));
    }
    let gateway = Arc::new(Gateway::new(&config, Arc::new(Recorders(sinks)))?);

    let key = if config.server.auth {
        let (key, _created) = virtual_key::load_or_create(&config.data.dir)?;
        Some(key)
    } else {
        None
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    let listen = config.server.listen.clone();
    let listener = runtime
        .block_on(async { tokio::net::TcpListener::bind(&listen).await })
        .map_err(|e| format!("绑定 {listen} 失败: {e}"))?;

    let app_state = server::AppState {
        gateway,
        virtual_key: key.clone().map(Arc::from),
    };
    runtime.spawn(async move {
        let _ = server::serve(app_state, listener).await;
    });

    inner.server = Some(RunningServer {
        runtime,
        listen,
        virtual_key: key,
    });
    Ok(inner.snapshot())
}

#[tauri::command]
fn serve_stop(state: State<'_, AppStateManaged>) -> StateView {
    let mut inner = state.0.lock().unwrap();
    if let Some(s) = inner.server.take() {
        s.runtime.shutdown_background();
    }
    inner.snapshot()
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "读不到 HOME".to_string())
}

/// Back up the original file for reversibility, then return whether it already existed.
fn backup(path: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(path) {
        let bak = path.with_extension(format!(
            "{}.token-station.bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
        ));
        let _ = std::fs::write(bak, text);
    }
}

/// Claude Code: write the env block to `~/.claude/settings.json` (embedded key; CC reads it directly, with no
/// manual export). CC uses the Anthropic protocol; end-to-end operation also requires the agent-anthropic adapter.
/// Determine whether the `plugins` configuration includes an inbound adapter that supports Anthropic. Inspect the `agents` list
/// and the two adapter names in the deprecated single `agent` string. Do not inspect providers to avoid false positives. agent-anthropic
/// Once it enters the configuration, the CC safety gate unlocks automatically.
fn anthropic_inbound_ready(plugins: &Value) -> bool {
    let hits = |v: &Value| v.as_str().is_some_and(|s| s.contains("anthropic"));
    let in_list = plugins["agents"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(hits));
    in_list || hits(&plugins["agent"])
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

fn connect_cc(base: &str, token: &str, anthropic_inbound_ready: bool) -> Result<String, String> {
    // Security gate: CC uses the Anthropic protocol. If the gateway inbound adapter does not support Anthropic
    // (agent-anthropic is unavailable), connection only points ~/.claude/settings.json to a
    // A proxy that cannot answer Anthropic requests also stops a running Claude Code instance, including development
    // this token-station session). Reject before readiness and do not touch settings.json.
    if !anthropic_inbound_ready {
        return Err(
            "暂不能接入 Claude Code:网关入站适配器(plugins.agent)还不支持 Anthropic \
             协议,agent-anthropic 尚未就位。现在接入会把 ~/.claude/settings.json 指向一个\
             无法应答 Anthropic 请求的代理,反而掐断你正在运行的 Claude Code。等 agent-anthropic \
             入站适配器配好后再接。(Codex / opencode 走 OpenAI 协议,现在即可正常接入。)"
                .to_string(),
        );
    }
    let dir = home_dir()?.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.claude 失败: {e}"))?;
    let path = dir.join("settings.json");
    backup(&path);

    let mut settings: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }
    {
        let obj = settings.as_object_mut().unwrap();
        let env = obj.entry("env").or_insert_with(|| json!({}));
        if !env.is_object() {
            *env = json!({});
        }
        let env = env.as_object_mut().unwrap();
        env.insert("ANTHROPIC_BASE_URL".into(), json!(base));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(token));
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings).unwrap())
        .map_err(|e| format!("写 settings.json 失败: {e}"))?;
    Ok(format!(
        "Claude Code 已指向 {base}(~/.claude/settings.json,已备份)。\
         注意:原生 Anthropic 入站解析需 agent-anthropic 适配器就位后方可端到端生效。"
    ))
}

/// Codex: write `~/.codex/config.toml` and add a model_provider that points to this proxy
/// (`wire_api = "chat"` because the gateway only provides /v1/chat/completions). The Codex key uses
/// environment variable, so return a one-line export instruction.
fn connect_codex(openai_base: &str) -> Result<String, String> {
    let dir = home_dir()?.join(".codex");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.codex 失败: {e}"))?;
    let path = dir.join("config.toml");
    backup(&path);

    let mut doc: toml::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let root = doc
        .as_table_mut()
        .ok_or_else(|| "config.toml 顶层不是表".to_string())?;

    root.insert("model".into(), toml::Value::String("auto".into()));
    root.insert(
        "model_provider".into(),
        toml::Value::String("tokenstation".into()),
    );

    let mut provider = toml::map::Map::new();
    provider.insert("name".into(), toml::Value::String("token-station".into()));
    provider.insert(
        "base_url".into(),
        toml::Value::String(openai_base.to_string()),
    );
    provider.insert("wire_api".into(), toml::Value::String("chat".into()));
    provider.insert(
        "env_key".into(),
        toml::Value::String("TOKENSTATION_KEY".into()),
    );

    let providers = root
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if let Some(t) = providers.as_table_mut() {
        t.insert("tokenstation".into(), toml::Value::Table(provider));
    }

    let text = toml::to_string_pretty(&doc).map_err(|e| format!("序列化 config.toml 失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写 config.toml 失败: {e}"))?;
    Ok(format!(
        "Codex 已指向 {openai_base}(~/.codex/config.toml,已备份)。\
         Codex 的 key 走环境变量,请在启动 Codex 的终端执行一次:\
         export TOKENSTATION_KEY=<面板上的虚拟 Key>"
    ))
}

/// opencode: write `~/.config/opencode/opencode.json` and add an OpenAI-compatible custom
/// provider (embedded apiKey; no export required).
fn connect_opencode(openai_base: &str, token: &str) -> Result<String, String> {
    let dir = home_dir()?.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.config/opencode 失败: {e}"))?;
    let path = dir.join("opencode.json");
    backup(&path);

    let mut cfg: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !cfg.is_object() {
        cfg = json!({});
    }
    {
        let obj = cfg.as_object_mut().unwrap();
        let providers = obj.entry("provider").or_insert_with(|| json!({}));
        if !providers.is_object() {
            *providers = json!({});
        }
        providers.as_object_mut().unwrap().insert(
            "tokenstation".into(),
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "token-station",
                "options": { "baseURL": openai_base, "apiKey": token },
                "models": { "auto": { "name": "auto (智能路由)" } }
            }),
        );
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap())
        .map_err(|e| format!("写 opencode.json 失败: {e}"))?;
    Ok(format!(
        "opencode 已加入 token-station provider(~/.config/opencode/opencode.json,已备份)。\
         在 opencode 里选模型 tokenstation/auto 即可。"
    ))
}

/// Connect an agent. Each agent writes its own configuration file, so they do not conflict and can connect and run at the same time.
#[tauri::command]
fn connect_agent(state: State<'_, AppStateManaged>, kind: String) -> Result<String, String> {
    let (listen, token, anthropic_inbound_ready) = {
        let inner = state.0.lock().unwrap();
        let sv = inner.serve_view();
        if !sv.running {
            return Err("请先启动代理(serve)再接入 agent".into());
        }
        // Check whether inbound adapters include an Anthropic-capable adapter. Support both plugins.agent and
        // The plugins.agents list after match_inbound. Check adapter names only in these two locations, not the complete
        // plugins. This prevents packages named anthropic-* under providers from being accepted incorrectly.
        let ready = anthropic_inbound_ready(&inner.draft["plugins"]);
        (sv.listen, sv.virtual_key.clone().unwrap_or_default(), ready)
    };
    let anthropic_base = format!("http://{listen}");
    let openai_base = format!("http://{listen}/v1");

    match kind.as_str() {
        "cc" => connect_cc(&anthropic_base, &token, anthropic_inbound_ready),
        "codex" => connect_codex(&openai_base),
        "opencode" => connect_opencode(&openai_base, &token),
        other => Err(format!("未知 agent `{other}`")),
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

    // Reuse existing configuration for v1 users. Otherwise, start a template draft.
    let draft = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| template(&root));

    let managed = AppStateManaged(Mutex::new(AppInner {
        config_path,
        draft,
        server: None,
    }));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(managed)
        .invoke_handler(tauri::generate_handler![
            get_state,
            add_provider,
            remove_provider,
            set_tier,
            save_config,
            serve_start,
            serve_stop,
            connect_agent,
            set_settings,
            get_stats,
            get_router_table,
            get_plugins,
            check_upgrade,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
