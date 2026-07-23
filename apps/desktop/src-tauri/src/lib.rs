//! token-station 桌面客户端后端。
//!
//! 这里**不重写任何路由/网关逻辑**:它把 `token-station-cli` 当库调,复用同一套
//! `Gateway` / `ClientConfig` / `server::serve` / keychain。GUI 只是这套内核的一层
//! 面板。三档路由面板 = 往 `router.pools` 的 tier_high/tier_mid/tier_low 三个池里
//! 各填一个 (供应商, 模型),再由 heuristic `bands` 自动分档。
//!
//! 部分(未填满)的三档状态在 RouterConfig 校验下是非法的,所以草稿以
//! `serde_json::Value` 承接,只有在「保存」或「启动」时才物化成 `ClientConfig`
//! 走校验——校验不过就把错误原样报给用户,绝不写盘。

pub mod agent_integration;
mod config_state;
mod model_catalog;
mod provider_tombstones;
mod recovery;
mod serve_lifecycle;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use token_station_cli::budget::{AgentBudget, BudgetStatus};
use token_station_cli::config::{ClientConfig, PluginsConfig};
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
    apply_agent_plan, apply_snapshot_restore, get_agent_drift, list_agent_registry,
    list_agent_snapshots, plan_agent_connection, plan_agent_disconnect, plan_snapshot_restore,
    runtime_from_app, scan_agents, AgentCommandState,
};
use agent_integration::registry::AgentRegistry;
use agent_integration::types::AdmissionStatus;
use config_state::ConfigState;
use model_catalog::ModelDiscoveryView;
use recovery::{
    DiagnosticPreview, FrontendDiagnosticInput, FrontendDiagnosticRecord, RecoveryMode,
    RecoveryState,
};
use serve_lifecycle::{prepare_server, PreparedServer, RunningServer, StartFailure};

/// 三个档位槽的池名——面板上/中/下三行对应这三个 `router.pools` 键。
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// 每档一条「关键词覆盖」规则的稳定 id。用户在某档加的关键词落进对应规则的
/// `keywords_any`——命中即走该档,压过复杂度分档(router-core 第 1 层最高优先级)。
/// id 稳定,因为它同时是决策记录/审计里 `路由命中规则 ID` 的取值。
const KW_RULE_HIGH: &str = "kw-high";
const KW_RULE_MID: &str = "kw-mid";
const KW_RULE_LOW: &str = "kw-low";

/// UI 档位槽(high/mid/low)→(池名, 关键词规则 id)。规则顺序即优先级:
/// 高→中→低,同一句话若同时命中两档的词,向上升档(更安全)。
fn tier_pool_and_rule(slot: &str) -> Result<(&'static str, &'static str), String> {
    match slot {
        "high" => Ok((TIER_HIGH, KW_RULE_HIGH)),
        "mid" => Ok((TIER_MID, KW_RULE_MID)),
        "low" => Ok((TIER_LOW, KW_RULE_LOW)),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

/// 从高到低的三档(UI 槽名, 池名, 关键词规则 id),写回 `router.rules` 时按此定序。
const TIER_ORDER: [(&str, &str, &str); 3] = [
    ("high", TIER_HIGH, KW_RULE_HIGH),
    ("mid", TIER_MID, KW_RULE_MID),
    ("low", TIER_LOW, KW_RULE_LOW),
];

/// 分档切点(启发式分数 → 档)。band 从高到低,`at_least` 严格递减,末档 0 兜底。
/// 这些默认值将来由评测中心校准替换;现在给个能跑的合理值。
const CUT_HIGH: u32 = 55;
const CUT_MID: u32 = 22;

/// 桌面端所需入站适配器由 Connector 能力声明推导。重复 adapter 去重且保留
/// build-time Connector registry 的稳定顺序；新增 Connector 不再要求修改此处。
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

/// 后端全局状态。用一把锁保护;命令都是短事务。
struct AppInner {
    /// 真实配置文件路径(`token-station.json`)。
    config_path: PathBuf,
    /// 可编辑草稿。部分状态合法,保存时才校验。
    draft: Value,
    /// 启动时既有配置无法读取/校验时保留错误。此时展示安全模板但禁止写盘，
    /// 防止一次“保存”静默覆盖用户原文件。
    load_error: Option<String>,
    /// 可编辑、已保存配置的持久化身份；运行态版本由 Runtime Supervisor 持有。
    config_state: ConfigState,
    /// 代理服务的权威生命周期状态。
    server: ServerLifecycle,
}

pub struct AppStateManaged(Mutex<AppInner>);

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
            std::fs::create_dir_all(path)?;
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

/// 全新配置模板。空 upstreams / 空 pools——作为 `ClientConfig` 非法,但作为草稿
/// 合法,直到用户至少配好一档。运行目录由 Tauri 的应用目录显式注入。
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

/// 把 CLI 时代的单 Chat 入站配置升级为桌面端三入站草稿，并把相对运行目录锚到
/// 配置文件所在目录。只改内存草稿；用户点击保存前不触碰原文件。
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
    draft
}

/// 既有配置必须完整通过 CLI 的读取/默认值填充/结构校验。失败时返回安全模板用于
/// 展示，同时携带只读错误闸，后续保存/启动均会拒绝，避免覆盖损坏文件。
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

/// 满 10 维的启发式权重:让内容驱动的自动分档真正生效(短中文难题也能升档)。
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

// ---- 前端视图类型 -------------------------------------------------------------

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
    /// This upstream runs on the local machine; `local_only` routing keeps to it.
    local: bool,
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
    /// 三档(high/mid/low)各自的用户关键词库。「用户在控制路由」的抓手,直接
    /// 落进 `router.rules` 的 `keywords_any`。
    keywords: std::collections::BTreeMap<String, Vec<String>>,
    /// 「只走本地」:锁定路由只用标了 local 的供应商,请求不出本机。
    local_only: bool,
    /// `local_only` 下,本地无可用时是否许可退到云(默认关=严格本地)。
    allow_cloud_fallback: bool,
    serve: ServeView,
    draft_revision: u64,
    saved_revision: u64,
    config_dirty: bool,
    /// 草稿能否物化成合法配置(能否保存/启动)。
    config_error: Option<String>,
    /// 设置页读取面:开关、出站策略 + 只读环境信息。
    settings: SettingsView,
}

/// 设置页视图:代理开关、出站策略与只读环境信息。
#[derive(Serialize)]
struct SettingsView {
    listen: String,
    auth: bool,
    metrics: bool,
    data_dir: String,
    plugins_dir: String,
    agent: String,
    version: String,
    egress_mode: String,
    egress_proxy_url: String,
    egress_no_proxy: Vec<String>,
    egress_auth_username: String,
    egress_auth_slot: String,
}

// ---- 子页面视图类型(#5 全能力子页面)----------------------------------------

/// 一档(某档位/分组)的用量聚合,`stats::Aggregate` 的可序列化镜像。
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

/// 用量页视图。`empty=true` 表示指标库还没建(serve 从未在 metrics 开启下跑过),
/// 前端据此展示引导而非空表。
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

/// 四层路由表可视化。层序:rules(1) → hint_routes(2) → heuristic bands(3)
/// → default_pool(4 兜底)。全部纯读草稿,零 API。
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

/// heuristic 一条 band:分数 ≥ at_least 落到 pool,并解出该 pool 当前的 (供应商, 模型)。
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

/// 插件页视图。listing 复用内核 `render_list()` 的等宽文本(与 CLI `plugin list` 同源)。
#[derive(Serialize)]
struct PluginsView {
    dir: String,
    agent: String,
    dialects: Vec<String>,
    listing: String,
}

/// 更新检查视图。只做匿名版本检查(内核 `upgrade::check`),不自替换二进制。
#[derive(Serialize)]
struct UpgradeView {
    current: String,
    latest_tag: String,
    html_url: String,
    newer: bool,
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
            server: ServerLifecycle::stopped(),
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

    fn save_draft(&mut self) -> Result<u64, String> {
        self.ensure_editable()?;
        let config = self.materialize()?;
        let draft = self.draft.clone();
        let revision = self.config_state.prepare_save(&draft)?;
        config
            .save(&self.config_path)
            .map_err(|error| format!("写配置失败: {error}"))?;
        if let Err(error) = self.config_state.finish_save(&draft) {
            // 配置已经原子提交成功。pending journal 会在下次启动时自动晋升，
            // 不能把“状态尾写失败”误报成配置保存失败。
            eprintln!("configuration saved but revision finalization failed: {error}");
        }
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
                    local: up.get("local").and_then(Value::as_bool).unwrap_or(false),
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

    /// 某档的池是否已配置(有非空成员)。加关键词前必须成立,否则规则会指向空池。
    fn pool_present(&self, pool: &str) -> bool {
        self.draft["router"]["pools"][pool]
            .as_array()
            .is_some_and(|members| !members.is_empty())
    }

    /// 读某条关键词规则(按 id)当前的 `keywords_any`。
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

    /// 三档(high/mid/low)各自的关键词库,给前端展示。
    fn home_keywords(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        TIER_ORDER
            .iter()
            .map(|(slot, _pool, rule_id)| ((*slot).to_string(), self.rule_keywords(rule_id)))
            .collect()
    }

    /// 当前三档的关键词映射(槽名 → 词表),作为写回前的快照来源。
    fn keyword_map(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        self.home_keywords()
    }

    /// 用给定的三档关键词映射,重写 `router.rules`。规则顺序=优先级(高→中→低),
    /// 只为「有关键词且池已配置」的档发规则;非关键词规则(操作员手写的其它规则)
    /// 原样保留在后面。空词表或未配置的档不发规则,避免指向不存在的池。
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
        // 保留任何非本模块管理的既有规则(id 不在三档之列),接在后面。
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

    /// 关键词归一:去首尾空白。用于去重(大小写不敏感,与内核 `keywords_any`
    /// 的匹配一致)与存储(保留用户原样大小写用于展示)。
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

    /// 清空某池时同步删掉它的关键词规则(否则规则 `route_to` 会指向空池,保存失败)。
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
            _ => return self.tier(pool_key(slot).expect("known UI tier slot")),
        };
        TierView {
            upstream: target["upstream"].as_str().map(str::to_string),
            model: target["model"].as_str().map(str::to_string),
        }
    }

    fn agent_profile(&self, agent_id: &str) -> Option<String> {
        (self.agent_route_mode(agent_id) == "profile")
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
        supported_agent_ids()
            .into_iter()
            .map(|agent_id| {
                let mode = self.agent_route_mode(&agent_id).to_string();
                let tiers = ["high", "mid", "low"]
                    .into_iter()
                    .map(|slot| (slot.to_string(), self.agent_tier(&agent_id, slot)))
                    .collect();
                let config_error = if mode == "custom" || mode == "profile" {
                    ["high", "mid", "low"].into_iter().find_map(|slot| {
                        let tier = self.agent_tier(&agent_id, slot);
                        (tier.upstream.is_none() || tier.model.is_none())
                            .then(|| format!("{slot} 档尚未配置供应商和模型"))
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
                    },
                )
            })
            .collect()
    }

    /// 根据当前已配置的档位,重建 pools 的档池引用 + heuristic bands + default。
    /// 只把「已选好 (upstream, model)」的档纳入路由。
    fn rebuild_routing(&mut self) {
        // 收集已配置的档(从高到低)。
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
            // 一档都没有:清空启发式/默认,保存时会因空池报错提示用户。
            self.draft["router"]["heuristic"] = Value::Null;
            self.draft["router"]["default_pool"] = json!("");
            return;
        }

        // bands:present 已按高→低;末档强制 at_least=0 兜底,不漏请求。
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

    /// 把草稿物化成 `ClientConfig`(校验)。失败返回人类可读错误。
    fn materialize(&self) -> Result<ClientConfig, String> {
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
            version: upgrade::CURRENT_VERSION.to_string(),
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

    /// 数据目录(草稿里的绝对路径)。stats / receipts / plugins 都锚到它。
    fn data_dir(&self) -> PathBuf {
        PathBuf::from(self.draft["data"]["dir"].as_str().unwrap_or_default())
    }

    /// 解出某个 pool 当前第一个成员的 (供应商, 模型)——路由表/band 展示用。
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
                // 池没了,它的关键词规则会指向空池,保存必失败——同步删掉。
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
        if mode != "profile" {
            if let Some(route) = self.draft["agent_routes"][agent_id].as_object_mut() {
                route.remove("profile");
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
        .filter(|descriptor| descriptor.admission == AdmissionStatus::Supported)
        .map(|descriptor| descriptor.agent_id.clone())
        .collect()
}

// ---- Tauri 命令 ---------------------------------------------------------------

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

/// 保存前预览三种入站协议最终会命中的 Provider URL。
#[tauri::command]
fn preview_provider_endpoints(base_url: String) -> Result<ProviderEndpointPreview, String> {
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    Ok(ProviderEndpointPreview {
        chat: endpoint.resolve(ProviderApi::ChatCompletions),
        responses: endpoint.resolve(ProviderApi::Responses),
        messages: endpoint.resolve(ProviderApi::Messages),
    })
}

/// 新增一个供应商(= 一个 openai-compatible 上游)。有 key 就存进系统钥匙串。
#[tauri::command]
fn add_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
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
                "model": m,
                "tool": false,
                "vision": false,
                "json_schema": false,
                "tool_state": "unknown",
                "vision_state": "unknown",
                "json_schema_state": "unknown",
                "context_window": 128000
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
    // 只在标了本地时写 local 键,让普通云供应商的配置保持原样(与 serde 的
    // skip_serializing_if 对齐)。local_only 路由据此把流量锁在本机。
    if local {
        up["local"] = json!(true);
    }
    // 有 key → keychain,auth 指向 slot;没 key(如本地 Ollama)→ 省略 auth。
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    if api_key.is_some() {
        up["auth"] = json!({ "slot": "provider_api_key", "keyring": true });
    }

    inner.draft["upstreams"][&name] = up;
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(&name);
        return Err(error);
    }
    if let Some(key) = api_key {
        if let Err(key_error) = secrets::keyring_set(&name, "provider_api_key", &key) {
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

/// 设置「只走本地」及其云兜底许可。写进 home `router`,agent inherit 自动跟随。
/// 关掉时把两个键一并移除,让普通配置保持原样(与 serde default=false 对齐)。
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
        // 「只走本地」关掉后,云许可无意义,一并清除,避免残留误导。
        router.remove("local_only");
        router.remove("allow_cloud_fallback");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn edit_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<StateView, String> {
    let name = name.trim().to_owned();
    let base_url = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?
        .as_str();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft["upstreams"]
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    let identity_changed =
        previous["base_url"].as_str() != Some(base_url.as_str()) || api_key.is_some();
    if identity_changed {
        // A URL or credential change may select a different Provider account.
        // Invalidate first: losing derived cache on a later rollback is safe;
        // presenting the old account's catalog as trusted is not.
        model_catalog::remove_provider(&inner.data_dir(), &name)?;
    }
    inner.draft["upstreams"][&name]["base_url"] = json!(base_url);
    if api_key.is_some() {
        inner.draft["upstreams"][&name]["auth"] =
            json!({ "slot": "provider_api_key", "keyring": true });
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&name] = previous;
        return Err(error);
    }
    if let Some(key) = api_key {
        if let Err(key_error) = secrets::keyring_set(&name, "provider_api_key", &key) {
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

/// 获取厂商当前模型目录。网络请求在 blocking worker 上执行，不阻塞 Tauri UI。
/// 使用已保存 Key 时强制请求 URL 与供应商配置一致，避免凭证被转发到任意地址。
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
            secrets::SecretStore::from_config(&config),
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

/// 更新一个已添加供应商的模型集合，并保护三档仍在使用的模型引用。
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
            existing.get(&model).cloned().unwrap_or_else(|| {
                json!({
                    "model": model,
                    "tool": false,
                    "vision": false,
                    "json_schema": false,
                    "tool_state": "unknown",
                    "vision_state": "unknown",
                    "json_schema_state": "unknown",
                    "context_window": 128000
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
    references.sort();
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
    inner.draft["upstreams"]
        .as_object_mut()
        .expect("upstreams is an object")
        .remove(name);
    inner.rebuild_routing();
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][name] = provider;
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

/// 设置某一档 = (供应商, 模型)。传 null 清空该档。
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
    inner.observe_draft()?;
    Ok(inner.snapshot())
}

/// 往某档(high/mid/low)的关键词库加一个词。命中即强制走该档(router-core 第 1 层)。
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

/// 从某档关键词库删除一个词。
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
    inner.observe_draft()?;
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
    inner.observe_draft()?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn save_home_route_as_profile(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.save_home_route_as_profile_value(&name)?;
    if let Err(error) = inner.observe_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn mount_agent_profile(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    profile: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.mount_agent_profile_value(&agent_id, &profile)?;
    if let Err(error) = inner.observe_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn delete_profile(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft.clone();
    inner.delete_profile_value(&name)?;
    if let Err(error) = inner.observe_draft() {
        inner.draft = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
fn save_agent_routes(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.save_draft()?;
    Ok(inner.snapshot())
}

#[tauri::command]
fn apply_home_route_to_all_agents(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    for agent_id in supported_agent_ids() {
        inner.set_agent_route_mode_value(&agent_id, "inherit");
    }
    inner.observe_draft()?;
    inner.save_draft()?;
    Ok(inner.snapshot())
}

/// 校验 + 原子写盘。校验不过原样报错,不写盘(复刻 config edit 语义)。
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

/// Claude Code:写 `~/.claude/settings.json` 的 env 块(key 内嵌,CC 直接读,无需
/// 手动 export)。CC 走 Anthropic 协议——端到端还需 agent-anthropic 适配器就位。
/// 判断 `plugins` 配置里是否已挂上能讲 Anthropic 的入站适配器。看 `agents` 列表
/// 与废弃的单串 `agent` 两处适配器名——不看 providers,避免误判。agent-anthropic
/// 一旦进配置,CC 安全闸就据此自动解封。
pub(crate) fn inbound_adapter_ready(plugins: &Value, expected: &str) -> bool {
    let hits = |value: &Value| value.as_str() == Some(expected);
    let in_list = plugins["agents"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(hits));
    in_list || hits(&plugins["agent"])
}

/// 入站适配器的展示串:优先 `agents` 列表(逗号连接),否则回退单串 `agent`。
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

// ---- 子页面命令(#5)----------------------------------------------------------

/// 设置页:切换 server.auth / data.metrics 两个开关。能物化就落盘(复刻 config set),
/// 否则只改草稿等完整保存。注意:改这两项对*正在运行*的 serve 不生效,需重启代理。
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
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.draft["server"]["auth"] = json!(auth);
    inner.draft["data"]["metrics"] = json!(metrics);
    inner.draft["egress"] = if egress_mode == "direct" {
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
                "credential": { "slot": egress_auth_slot, "keyring": true }
            });
        }
        egress
    };
    inner.observe_draft()?;
    if inner.materialize().is_ok() {
        inner.save_draft()?;
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

/// 用量页:只读聚合指标库。`since` = all / <N>h / <N>d;`by` = agent/upstream/model/pool/status/hour/day
/// 或空。指标库还没建时返回 `empty=true`,不当错误报。
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

/// 主页：最近五条无正文 Request Receipt。指标库尚未创建（含关闭
/// metrics）时返回空数组；读取层自身硬限制最多五条。
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

/// 用量页：分页读取完整的无正文 Request Receipt 台账。
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

/// 路由表页:把草稿里的四层路由(规则/提示/启发式档/兜底)整理成可视化视图。纯读,零 API。
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

/// 插件页:发现插件目录 + 复用内核 `render_list()` 的等宽清单(与 CLI `plugin list` 同源)。
/// 不依赖完整配置,tiers 没配好也能看。
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

/// 关于/更新页:匿名版本检查(内核唯一的合法外联)。只比对版本 + 给发布页链接,
/// 不自替换二进制——桌面 app 的自更新走各自渠道,这里只做「有没有新版」。
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
            if recovery::inspect_recovery_state(&desktop_paths.data_dir).mode == RecoveryMode::Safe
            {
                return Ok(());
            }

            // 有现成配置就经 CLI 的完整校验与默认值填充后沿用；损坏配置进入只读
            // 保护，绝不以空模板静默覆盖。旧版单 OpenAI 入站只在内存中升级。
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
            get_state,
            get_runtime_state,
            preview_provider_endpoints,
            add_provider,
            set_local_routing,
            edit_provider,
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
            apply_home_route_to_all_agents,
            serve_start,
            serve_stop,
            list_agent_registry,
            scan_agents,
            plan_agent_connection,
            apply_agent_plan,
            plan_agent_disconnect,
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
            set_model_price,
            remove_model_price,
            get_recent_receipts,
            get_request_receipts,
            get_router_table,
            get_plugins,
            check_upgrade,
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

        paths.create_writable_dirs().unwrap();
        assert!(config_root.is_dir());
        assert!(paths.data_dir.is_dir());
        assert!(paths.plugins_dir.is_dir());
        assert!(paths.agent_data_root.is_dir());

        let draft = template(&paths.data_dir, &paths.plugins_dir);
        assert_eq!(draft["data"]["dir"], json!(paths.data_dir));
        assert_eq!(draft["plugins"]["dir"], json!(paths.plugins_dir));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(feature = "bundled-plugins")]
    #[test]
    fn desktop_bundled_plugins_load_without_an_external_plugin_directory() {
        let root = scratch_home("bundled-plugins");
        let missing_plugins = root.join("intentionally-missing-plugins");
        let plugins: PluginsConfig = serde_json::from_value(
            template(&root.join("data"), &missing_plugins)["plugins"].clone(),
        )
        .unwrap();
        let receipts = Receipts::load(&root.join("data")).unwrap();
        let registry = PluginRegistry::discover(&plugins, &receipts).unwrap();

        for package in [
            "agent-openai",
            "agent-anthropic",
            "agent-openai-responses",
            "agent-gemini",
            "provider-openai-compatible",
        ] {
            let package = registry
                .package(package)
                .expect("official package is builtin");
            assert!(matches!(
                package.source,
                token_station_cli::plugins::PackageSource::Builtin { .. }
            ));
        }
        assert!(registry.provider_binding("openai-compatible").is_some());
        assert!(!missing_plugins.exists());
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
    fn inbound_readiness_requires_exact_adapter_names() {
        let plugins = json!({
            "agents": [
                "agent-anthropic-proxy",
                "agent-openai-responses-beta",
                "agent-openai-compatible"
            ]
        });
        assert!(!inbound_adapter_ready(&plugins, "agent-anthropic"));
        assert!(!inbound_adapter_ready(&plugins, "agent-openai-responses"));
        assert!(!inbound_adapter_ready(&plugins, "agent-openai"));

        let plugins = json!({
            "agents": ["agent-anthropic", "agent-openai-responses", "agent-openai"]
        });
        assert!(inbound_adapter_ready(&plugins, "agent-anthropic"));
        assert!(inbound_adapter_ready(&plugins, "agent-openai-responses"));
        assert!(inbound_adapter_ready(&plugins, "agent-openai"));
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

        // 未配置的档不能加词(否则规则会指向空池,保存失败)。
        let error = inner
            .add_tier_keyword("low", "提交git")
            .expect_err("adding to an unconfigured tier is refused");
        assert!(error.contains("先"), "{error}");

        inner
            .set_tier_value(TIER_LOW, Some("provider".into()), Some("m".into()))
            .unwrap();

        inner.add_tier_keyword("low", "提交git").unwrap();
        // 大小写不敏感去重。
        let dup = inner
            .add_tier_keyword("low", "提交GIT")
            .expect_err("case-insensitive duplicate is refused");
        assert!(dup.contains("已在"), "{dup}");

        // 词进了 low 档的规则,指向 tier_low,且整份配置能通过内核校验。
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

        // 删词(大小写不敏感);词表空后规则整条移除,不留空 keywords_any。
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
        // 需要另一档兜底,否则清空唯一的档会让 pools 变空。
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

        // 清空 low 档:其关键词规则必须同步消失,否则 route_to 指向空池、校验失败。
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
            "auth": {"slot": "provider_api_key", "keyring": true},
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
        }
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
            assert_eq!(
                provider.model_capabilities[0].tool,
                CapabilityState::Unknown
            );
            assert_eq!(
                provider.model_capabilities[0].vision,
                CapabilityState::Unknown
            );
            assert_eq!(
                provider.model_capabilities[0].json_schema,
                CapabilityState::Unknown
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
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            &catalog_path,
            serde_json::to_vec_pretty(&json!({
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

        // 一个本地供应商(标 local)和一个云供应商。
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

        // 打开「只走本地」+ 云兜底许可。
        let on = set_local_routing(app.state(), true, true).unwrap();
        assert!(on.local_only);
        assert!(on.allow_cloud_fallback);

        // 关掉后两个键都被清除,配置回到与默认一致的干净状态。
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
