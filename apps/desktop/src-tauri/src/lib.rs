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

mod model_catalog;

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

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "读不到 HOME".to_string())
}

/// 读取一个可选的 JSON 对象配置。文件存在但不可读、JSON 损坏或顶层非对象时必须
/// 原样报错，不能回退到空对象后覆盖用户配置。
fn read_json_object(path: &std::path::Path, label: &str) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| format!("{label} 不是合法 JSON（{}）：{error}", path.display()))?;
            if value.is_object() {
                Ok(value)
            } else {
                Err(format!("{label} 顶层必须是对象（{}）", path.display()))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(format!("读取 {label} 失败（{}）：{error}", path.display())),
    }
}

fn backup_path(path: &std::path::Path) -> PathBuf {
    path.with_extension(format!(
        "{}.token-station.bak",
        path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
    ))
}

/// 先可靠备份，再以同目录临时文件 + rename 原子替换配置。
fn write_config(path: &std::path::Path, rendered: &str, label: &str) -> Result<(), String> {
    match std::fs::read(path) {
        Ok(original) => {
            let backup = backup_path(path);
            std::fs::write(&backup, original)
                .map_err(|error| format!("备份 {label} 失败（{}）：{error}", backup.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "读取 {label} 以备份失败（{}）：{error}",
                path.display()
            ))
        }
    }
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("{label} 路径没有文件名：{}", path.display()))?;
    let temporary = path.with_file_name(format!(".{file_name}.token-station.tmp"));
    std::fs::write(&temporary, rendered).map_err(|error| {
        format!(
            "写 {label} 临时文件失败（{}）：{error}",
            temporary.display()
        )
    })?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("替换 {label} 失败（{}）：{error}", path.display()))
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

fn connect_cc_at(
    home: &std::path::Path,
    base: &str,
    token: &str,
    anthropic_inbound_ready: bool,
) -> Result<String, String> {
    // 安全闸:CC 走 Anthropic 协议,若网关入站适配器还不支持 Anthropic
    // (agent-anthropic 未就位),接入只会把 ~/.claude/settings.json 指向一个
    // 应答不了 Anthropic 请求的代理——连带掐断*正在运行*的 Claude Code(包括开发
    // token-station 用的这个会话)。所以在就位前直接拒绝,绝不碰 settings.json。
    if !anthropic_inbound_ready {
        return Err(
            "暂不能接入 Claude Code:网关入站适配器(plugins.agent)还不支持 Anthropic \
             协议,agent-anthropic 尚未就位。现在接入会把 ~/.claude/settings.json 指向一个\
             无法应答 Anthropic 请求的代理,反而掐断你正在运行的 Claude Code。等 agent-anthropic \
             入站适配器配好后再接。(Codex / opencode 走 OpenAI 协议,现在即可正常接入。)"
                .to_string(),
        );
    }
    let dir = home.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.claude 失败: {e}"))?;
    let path = dir.join("settings.json");
    let mut settings = read_json_object(&path, "Claude Code settings.json")?;
    {
        let obj = settings.as_object_mut().unwrap();
        let env = obj.entry("env").or_insert_with(|| json!({}));
        let env = env
            .as_object_mut()
            .ok_or_else(|| "Claude Code settings.json 的 env 必须是对象".to_string())?;
        env.insert("ANTHROPIC_BASE_URL".into(), json!(base));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(token));
        env.insert("MAX_THINKING_TOKENS".into(), json!("0"));
        env.insert("CLAUDE_CODE_DISABLE_THINKING".into(), json!("1"));
        env.insert("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING".into(), json!("1"));
        env.insert("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".into(), json!("1"));
        env.insert(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            json!("1"),
        );
    }
    let rendered = serde_json::to_string_pretty(&settings)
        .map_err(|error| format!("序列化 settings.json 失败：{error}"))?;
    write_config(&path, &rendered, "Claude Code settings.json")?;
    Ok(format!(
        "Claude Code 已指向 {base}(~/.claude/settings.json,已备份)。\
         已关闭当前 Canonical IR 暂不支持的 thinking/beta；\
         使用 /v1/messages，经 agent-anthropic 入站适配器转发。"
    ))
}

fn connect_cc(base: &str, token: &str, anthropic_inbound_ready: bool) -> Result<String, String> {
    connect_cc_at(&home_dir()?, base, token, anthropic_inbound_ready)
}

/// Codex:写 `~/.codex/config.toml`,加一个指向本代理的 model_provider
/// (`wire_api = "responses"`,对应网关 `/v1/responses`)。Codex 的 key 走
/// 环境变量,故返回一行 export 提示。
fn connect_codex_at(
    home: &std::path::Path,
    openai_base: &str,
    responses_inbound_ready: bool,
) -> Result<String, String> {
    if !responses_inbound_ready {
        return Err(
            "暂不能接入 Codex：网关未加载 agent-openai-responses，/v1/responses \
             无入站适配器。本次未修改 ~/.codex/config.toml。"
                .to_string(),
        );
    }
    let dir = home.join(".codex");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.codex 失败: {e}"))?;
    let path = dir.join("config.toml");
    let mut doc: toml::Value = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| format!("Codex config.toml 不合法（{}）：{error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => {
            return Err(format!(
                "读取 Codex config.toml 失败（{}）：{error}",
                path.display()
            ))
        }
    };
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
    provider.insert("wire_api".into(), toml::Value::String("responses".into()));
    provider.insert(
        "env_key".into(),
        toml::Value::String("TOKENSTATION_KEY".into()),
    );
    provider.insert("requires_openai_auth".into(), toml::Value::Boolean(false));
    provider.insert("request_max_retries".into(), toml::Value::Integer(0));
    provider.insert("stream_max_retries".into(), toml::Value::Integer(0));

    let providers = root
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let providers = providers
        .as_table_mut()
        .ok_or_else(|| "Codex config.toml 的 model_providers 必须是表".to_string())?;
    providers.insert("tokenstation".into(), toml::Value::Table(provider));

    let text = toml::to_string_pretty(&doc).map_err(|e| format!("序列化 config.toml 失败: {e}"))?;
    write_config(&path, &text, "Codex config.toml")?;
    Ok(format!(
        "Codex 已通过 Responses API 指向 {openai_base}(~/.codex/config.toml,已备份)。\
         Codex 的 key 走环境变量,请在启动 Codex 的终端执行一次:\
         export TOKENSTATION_KEY=<面板上的虚拟 Key>"
    ))
}

fn connect_codex(openai_base: &str, responses_inbound_ready: bool) -> Result<String, String> {
    connect_codex_at(&home_dir()?, openai_base, responses_inbound_ready)
}

/// opencode:写 `~/.config/opencode/opencode.json`,加一个 openai-compatible 自定义
/// provider(key 内嵌 apiKey,无需 export)。
fn connect_opencode_at(
    home: &std::path::Path,
    openai_base: &str,
    token: &str,
) -> Result<String, String> {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 ~/.config/opencode 失败: {e}"))?;
    let path = dir.join("opencode.json");
    let mut cfg = read_json_object(&path, "OpenCode opencode.json")?;
    {
        let obj = cfg.as_object_mut().unwrap();
        let providers = obj.entry("provider").or_insert_with(|| json!({}));
        let providers = providers
            .as_object_mut()
            .ok_or_else(|| "OpenCode opencode.json 的 provider 必须是对象".to_string())?;
        providers.insert(
            "tokenstation".into(),
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "token-station",
                "options": { "baseURL": openai_base, "apiKey": token },
                "models": { "auto": { "name": "auto (智能路由)" } }
            }),
        );
    }
    let rendered = serde_json::to_string_pretty(&cfg)
        .map_err(|error| format!("序列化 opencode.json 失败：{error}"))?;
    write_config(&path, &rendered, "OpenCode opencode.json")?;
    Ok(
        "opencode 已加入 token-station provider(~/.config/opencode/opencode.json,已备份)。\
         在 opencode 里选模型 tokenstation/auto 即可。"
            .to_string(),
    )
}

fn connect_opencode(openai_base: &str, token: &str) -> Result<String, String> {
    connect_opencode_at(&home_dir()?, openai_base, token)
}

/// 接入某个 agent。所有 agent 各写各的配置文件,互不冲突,可同时接入、同时运行。
#[tauri::command]
fn connect_agent(state: State<'_, AppStateManaged>, kind: String) -> Result<String, String> {
    let (listen, token, anthropic_inbound_ready, responses_inbound_ready) = {
        let inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        let sv = inner.serve_view();
        if !sv.running {
            return Err("请先启动代理(serve)再接入 agent".into());
        }
        // 入站适配器是否含能讲 Anthropic 的适配器:兼容单串 plugins.agent 与
        // match_inbound 后的 plugins.agents 列表。只看这两处适配器名(不看整个
        // plugins,避免 providers 里叫 anthropic-* 的包误判放行)。
        let anthropic_ready = anthropic_inbound_ready(&inner.draft["plugins"]);
        let responses_ready = responses_inbound_ready(&inner.draft["plugins"]);
        let client_token = sv
            .virtual_key
            .clone()
            .unwrap_or_else(|| "token-station-no-auth".to_string());
        (sv.listen, client_token, anthropic_ready, responses_ready)
    };
    let anthropic_base = format!("http://{listen}");
    let openai_base = format!("http://{listen}/v1");

    match kind.as_str() {
        "cc" => connect_cc(&anthropic_base, &token, anthropic_inbound_ready),
        "codex" => connect_codex(&openai_base, responses_inbound_ready),
        "opencode" => connect_opencode(&openai_base, &token),
        other => Err(format!("未知 agent `{other}`")),
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "agents": ["agent-anthropic-proxy", "agent-openai-responses-beta"]
        });
        assert!(!anthropic_inbound_ready(&plugins));
        assert!(!responses_inbound_ready(&plugins));

        let plugins = json!({
            "agents": ["agent-anthropic", "agent-openai-responses"]
        });
        assert!(anthropic_inbound_ready(&plugins));
        assert!(responses_inbound_ready(&plugins));
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
    fn codex_connection_uses_responses_and_preserves_the_existing_config() {
        let home = scratch_home("codex");
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "[features]\napps = false\n";
        std::fs::write(&path, original).unwrap();

        connect_codex_at(&home, "http://127.0.0.1:8787/v1", true).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let config: toml::Value = toml::from_str(&text).unwrap();
        let provider = &config["model_providers"]["tokenstation"];
        assert_eq!(provider["wire_api"].as_str(), Some("responses"));
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(false));
        assert_eq!(provider["request_max_retries"].as_integer(), Some(0));
        assert_eq!(provider["stream_max_retries"].as_integer(), Some(0));
        assert_eq!(config["features"]["apps"].as_bool(), Some(false));
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).unwrap(),
            original
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn an_invalid_codex_config_is_never_replaced() {
        let home = scratch_home("codex-invalid");
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "this = [is not valid";
        std::fs::write(&path, original).unwrap();

        let error =
            connect_codex_at(&home, "http://127.0.0.1:8787/v1", true).unwrap_err();

        assert!(error.contains("不合法"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!backup_path(&path).exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn codex_connection_refuses_before_writing_when_responses_inbound_is_missing() {
        let home = scratch_home("codex-missing-responses");
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "model = \"existing\"\nmodel_provider = \"existing-provider\"\n";
        std::fs::write(&path, original).unwrap();

        let error =
            connect_codex_at(&home, "http://127.0.0.1:8787/v1", false).unwrap_err();

        assert!(error.contains("agent-openai-responses"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!backup_path(&path).exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn claude_connection_preserves_other_settings_and_creates_a_recovery_backup() {
        let home = scratch_home("claude");
        let dir = home.join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = r#"{"permissions":{"allow":["Read"]},"env":{"KEEP":"yes"}}"#;
        std::fs::write(&path, original).unwrap();

        connect_cc_at(&home, "http://127.0.0.1:8787", "local-test-key", true).unwrap();

        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["permissions"]["allow"], json!(["Read"]));
        assert_eq!(settings["env"]["KEEP"], json!("yes"));
        assert_eq!(
            settings["env"]["ANTHROPIC_BASE_URL"],
            json!("http://127.0.0.1:8787")
        );
        assert_eq!(
            settings["env"]["ANTHROPIC_AUTH_TOKEN"],
            json!("local-test-key")
        );
        assert_eq!(settings["env"]["MAX_THINKING_TOKENS"], json!("0"));
        assert_eq!(settings["env"]["CLAUDE_CODE_DISABLE_THINKING"], json!("1"));
        assert_eq!(
            settings["env"]["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"],
            json!("1")
        );
        assert_eq!(
            settings["env"]["CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS"],
            json!("1")
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).unwrap(),
            original
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn claude_safety_gate_and_invalid_json_leave_settings_untouched() {
        let home = scratch_home("claude-invalid");
        let dir = home.join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = "not-json";
        std::fs::write(&path, original).unwrap();

        let gated = connect_cc_at(&home, "http://127.0.0.1:8787", "key", false).unwrap_err();
        assert!(gated.contains("暂不能接入"));
        let invalid = connect_cc_at(&home, "http://127.0.0.1:8787", "key", true).unwrap_err();
        assert!(invalid.contains("不是合法 JSON"), "{invalid}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!backup_path(&path).exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn opencode_connection_preserves_other_providers_and_is_idempotent() {
        let home = scratch_home("opencode");
        let dir = home.join(".config/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        std::fs::write(&path, r#"{"provider":{"existing":{"name":"keep"}}}"#).unwrap();

        connect_opencode_at(&home, "http://127.0.0.1:8787/v1", "local-key").unwrap();
        connect_opencode_at(&home, "http://127.0.0.1:8787/v1", "local-key").unwrap();

        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["provider"]["existing"]["name"], json!("keep"));
        assert_eq!(
            config["provider"]["tokenstation"]["options"]["baseURL"],
            json!("http://127.0.0.1:8787/v1")
        );
        assert_eq!(config["provider"].as_object().unwrap().len(), 2);
        std::fs::remove_dir_all(home).ok();
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
}
