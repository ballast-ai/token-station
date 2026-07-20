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
mod model_catalog;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{Manager, State};

use token_station_cli::config::{ClientConfig, PluginsConfig};
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::plugins::{PluginRegistry, Receipts};
use token_station_cli::store::SqliteStore;
use token_station_cli::{secrets, server, stats, upgrade, virtual_key};
use token_station_metrics::Recorder;

use agent_integration::commands::{
    apply_agent_plan, apply_snapshot_restore, list_agent_registry, list_agent_snapshots,
    plan_agent_connection, plan_agent_disconnect, plan_snapshot_restore, scan_agents,
    AgentCommandState,
};
use model_catalog::ModelDiscoveryView;

/// 三个档位槽的池名——面板上/中/下三行对应这三个 `router.pools` 键。
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// 分档切点(启发式分数 → 档)。band 从高到低,`at_least` 严格递减,末档 0 兜底。
/// 这些默认值将来由评测中心校准替换;现在给个能跑的合理值。
const CUT_HIGH: u32 = 55;
const CUT_MID: u32 = 22;

/// 桌面端对外承诺的三种入站协议。顺序同时是 `match_inbound` 优先级；三者路径
/// 互斥，因此保持最通用的 Chat Completions 在前不会吞掉 Messages/Responses。
const DESKTOP_AGENTS: [&str; 3] = ["agent-openai", "agent-anthropic", "agent-openai-responses"];

/// 运行中的 serve 实例。停止 = 关掉这个 runtime,监听器随之释放端口。
struct RunningServer {
    runtime: tokio::runtime::Runtime,
    listen: String,
    virtual_key: Option<String>,
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
    /// 当前运行的 serve(若有)。
    server: Option<RunningServer>,
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

// ---- 路径锚点(开发期锚到仓库根;打包时另行处理)-------------------------------

/// 仓库根:`apps/desktop/src-tauri` 往上三级。开发期 `tauri dev` 的 CWD 不稳定,
/// 所以配置/插件/数据目录一律用这个绝对锚点,serve 才能找到 `plugins-dist`。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 全新配置模板。空 upstreams / 空 pools——作为 `ClientConfig` 非法,但作为草稿
/// 合法,直到用户至少配好一档。绝对路径让 serve 在任何 CWD 下都能找到插件。
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

/// 把 CLI 时代的单 Chat 入站配置升级为桌面端三入站草稿，并把相对运行目录锚到
/// 配置文件所在的仓库根。只改内存草稿；用户点击保存前不触碰原文件。
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

/// 既有配置必须完整通过 CLI 的读取/默认值填充/结构校验。失败时返回安全模板用于
/// 展示，同时携带只读错误闸，后续保存/启动均会拒绝，避免覆盖损坏文件。
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
    /// 草稿能否物化成合法配置(能否保存/启动)。
    config_error: Option<String>,
    /// 设置页读取面:开关 + 只读环境信息。
    settings: SettingsView,
}

/// 设置页视图:两个可写开关(server.auth / data.metrics)+ 只读环境信息。
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

/// 用量页视图。`empty=true` 表示指标库还没建(serve 从未在 metrics 开启下跑过),
/// 前端据此展示引导而非空表。
#[derive(Serialize)]
struct StatsView {
    total: AggView,
    groups: Vec<(String, AggView)>,
    by: Option<String>,
    empty: bool,
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
            }
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_string()),
        }
        self.rebuild_routing();
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

// ---- Tauri 命令 ---------------------------------------------------------------

#[tauri::command]
fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
}

/// 新增/更新一个供应商(= 一个 openai-compatible 上游)。有 key 就存进系统钥匙串。
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
    // 有 key → keychain,auth 指向 slot;没 key(如本地 Ollama)→ 省略 auth。
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

/// 获取厂商当前模型目录。网络请求在 blocking worker 上执行，不阻塞 Tauri UI。
/// 使用已保存 Key 时强制请求 URL 与供应商配置一致，避免凭证被转发到任意地址。
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
    // 清掉任何引用它的档。
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
    Ok(inner.snapshot())
}

/// 校验 + 原子写盘。校验不过原样报错,不写盘(复刻 config edit 语义)。
#[tauri::command]
fn save_config(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let inner = state.0.lock().unwrap();
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
    Ok(inner.snapshot())
}

#[tauri::command]
fn serve_start(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if inner.server.is_some() {
        return Ok(inner.snapshot());
    }
    let config = inner.materialize()?;

    // recorder:文件日志始终写,指标库按开关。二者都不含 prompt 内容。
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
        admin: Arc::new(token_station_cli::admin::AdminContext {
            data_dir: config.data.dir.clone(),
            router: config.router.clone(),
            plugins: config.plugins.clone(),
        }),
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

/// Claude Code:写 `~/.claude/settings.json` 的 env 块(key 内嵌,CC 直接读,无需
/// 手动 export)。CC 走 Anthropic 协议——端到端还需 agent-anthropic 适配器就位。
/// 判断 `plugins` 配置里是否已挂上能讲 Anthropic 的入站适配器。看 `agents` 列表
/// 与废弃的单串 `agent` 两处适配器名——不看 providers,避免误判。agent-anthropic
/// 一旦进配置,CC 安全闸就据此自动解封。
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

/// 用量页:只读聚合指标库。`since` = all / <N>h / <N>d;`by` = upstream/model/pool/status
/// 或空。指标库还没建时返回 `empty=true`,不当错误报。
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let root = repo_root();
    let config_path = root.join("token-station.json");

    // 有现成配置就经 CLI 的完整校验与默认值填充后沿用；损坏配置进入只读保护，
    // 绝不以空模板静默覆盖。旧版单 OpenAI 入站只在内存中升级为桌面三入站。
    let (draft, load_error) = load_draft(&config_path, &root);

    let managed = AppStateManaged(Mutex::new(AppInner {
        config_path,
        draft,
        load_error,
        server: None,
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
            add_provider,
            discover_provider_models,
            update_provider_models,
            remove_provider,
            set_tier,
            save_config,
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
            server: None,
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
            server: None,
        };

        let error = replace_provider_models(&mut inner, "provider", vec!["replacement".to_owned()])
            .expect_err("read-only protection blocks model writes");
        assert!(error.contains("只读保护"), "{error}");
        assert_eq!(inner.draft, before);
        assert!(!inner.config_path.exists());

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
            server: None,
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
            server: None,
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
    fn one_two_and_three_tiers_always_end_with_a_zero_score_fallback() {
        let root = scratch_home("tiers-valid");
        let mut inner = AppInner {
            config_path: root.join("token-station.json"),
            draft: template(&root),
            load_error: None,
            server: None,
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
            server: None,
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

        let started = serve_start(app.state()).unwrap();
        assert!(started.serve.running);
        assert!(started.serve.virtual_key.is_none());
        assert!(root.join("data").join("requests.log").exists());
        let duplicate = serve_start(app.state()).unwrap();
        assert!(duplicate.serve.running);
        let stopped = serve_stop(app.state());
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
            server: None,
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
