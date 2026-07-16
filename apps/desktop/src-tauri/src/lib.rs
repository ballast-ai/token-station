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

/// 三个档位槽的池名——面板上/中/下三行对应这三个 `router.pools` 键。
const TIER_HIGH: &str = "tier_high";
const TIER_MID: &str = "tier_mid";
const TIER_LOW: &str = "tier_low";

/// 分档切点(启发式分数 → 档)。band 从高到低,`at_least` 严格递减,末档 0 兜底。
/// 这些默认值将来由评测中心校准替换;现在给个能跑的合理值。
const CUT_HIGH: u32 = 55;
const CUT_MID: u32 = 22;

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

#[tauri::command]
fn remove_provider(state: State<'_, AppStateManaged>, name: String) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
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

/// 校验 + 原子写盘。校验不过原样报错,不写盘(复刻 config edit 语义)。
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

/// 备份一份原文件(可逆),再返回是否已存在。
fn backup(path: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(path) {
        let bak = path.with_extension(format!(
            "{}.token-station.bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
        ));
        let _ = std::fs::write(bak, text);
    }
}

/// Claude Code:写 `~/.claude/settings.json` 的 env 块(key 内嵌,CC 直接读,无需
/// 手动 export)。CC 走 Anthropic 协议——端到端还需 agent-anthropic 适配器就位。
/// 判断 `plugins` 配置里是否已挂上能讲 Anthropic 的入站适配器。看 `agents` 列表
/// 与废弃的单串 `agent` 两处适配器名——不看 providers,避免误判。agent-anthropic
/// 一旦进配置,CC 安全闸就据此自动解封。
fn anthropic_inbound_ready(plugins: &Value) -> bool {
    let hits = |v: &Value| v.as_str().is_some_and(|s| s.contains("anthropic"));
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

fn connect_cc(base: &str, token: &str, anthropic_inbound_ready: bool) -> Result<String, String> {
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

/// Codex:写 `~/.codex/config.toml`,加一个指向本代理的 model_provider
/// (`wire_api = "chat"`,因为网关只有 /v1/chat/completions)。Codex 的 key 走
/// 环境变量,故返回一行 export 提示。
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

/// opencode:写 `~/.config/opencode/opencode.json`,加一个 openai-compatible 自定义
/// provider(key 内嵌 apiKey,无需 export)。
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

/// 接入某个 agent。所有 agent 各写各的配置文件,互不冲突,可同时接入、同时运行。
#[tauri::command]
fn connect_agent(state: State<'_, AppStateManaged>, kind: String) -> Result<String, String> {
    let (listen, token, anthropic_inbound_ready) = {
        let inner = state.0.lock().unwrap();
        let sv = inner.serve_view();
        if !sv.running {
            return Err("请先启动代理(serve)再接入 agent".into());
        }
        // 入站适配器是否含能讲 Anthropic 的适配器:兼容单串 plugins.agent 与
        // match_inbound 后的 plugins.agents 列表。只看这两处适配器名(不看整个
        // plugins,避免 providers 里叫 anthropic-* 的包误判放行)。
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

    // 有现成配置就沿用(复用 v1 用户的配置);否则起模板草稿。
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
