# 桌面 app 设计与交接

- 部件:`apps/desktop`（Tauri + React 桌面客户端）
- 状态:v1 能力子页面 + 多入站编排 + 三类 Agent 接入已落地
- 读者:要维护 / 扩展这个桌面 app 的人。先读 [架构总览.md](架构总览.md) 了解内核。

Agent 接入的用户操作、配置改写、请求链路、恢复与扩展方法见
[桌面App-Agent接入机制.md](桌面App-Agent接入机制.md)。

---

## 1. 这是什么、为什么

token-station 的内核是一个本地回环 LLM 代理二进制（`apps/cli`）。桌面 app 不是它的
替代品,而是**套在同一套内核上的一层 GUI**:把「加供应商 → 配路由 → 起代理 →
接 agent」这条链路做成可点的界面,面向不想碰命令行的使用者。

**北极星是自动智能路由**——按请求复杂度把每个请求路由到「够用的最便宜档」,不降质
省 token。GUI 的主界面因此是**三档路由面板**,不做手动切换。

关键设计约束:**GUI 不重写任何路由 / 网关 / 协议逻辑。** Tauri 后端把
`token-station-cli` 当**库**直接调（复用 `Gateway` / `server::serve` /
`ClientConfig` / keychain / `stats` / `plugins` / `upgrade`)。`router-core` /
`gateway` / `plugin-runtime` 零重写,GUI 只是这套内核的一层面板。

---

## 2. 架构

```
┌─────────────────────────── 桌面 app ───────────────────────────┐
│  前端 React + TS + Vite（apps/desktop/src）                     │
│    App.tsx           顶栏 + 6 个 tab 的壳                        │
│    pages/*.tsx       路由表 / 用量 / 插件 / 设置 / 关于           │
│    api.ts            invoke<T>(command) 的类型化封装             │
│        │  Tauri IPC（invoke ↔ #[tauri::command]）               │
│  后端 Rust（apps/desktop/src-tauri/src/lib.rs）                 │
│    AppInner{ config_path, draft: Value, server }               │
│    #[tauri::command] 一组短事务,一把锁                          │
│        │  直接函数调用（不是子进程 / sidecar）                   │
│  内核库 token_station_cli                                       │
│    Gateway::new · server::serve · ClientConfig · stats ·        │
│    plugins::PluginRegistry · virtual_key · upgrade · secrets    │
└────────────────────────────────────────────────────────────────┘
```

- **前后端边界 = Tauri command。** 前端 `api.ts` 每个函数是一个 `invoke<T>("cmd", args)`;
  后端每个 `#[tauri::command]` 是一次持锁短事务。命令清单见 §5。
- **状态模型:草稿 + 物化。** 后端用 `serde_json::Value`（`draft`）承接可编辑配置——
  部分填好的三档在 `ClientConfig` 校验下是**非法**的,但作为草稿合法。只有「保存」或
  「启动」时才 `materialize()` 成 `ClientConfig` 走校验,**校验不过原样报错、绝不写盘**
  （复刻 CLI `config edit` 的「整份文件校验通过才应用」语义）。
- **运行中的 serve** 是一个后台 tokio runtime，持 `token_station_cli::server::serve`;
  「停止」= 关这个 runtime，监听器随之释放端口。

---

## 3. 主界面:三档路由面板

界面上/中/下三档,每档 =（供应商下拉 + 模型下拉）。这三档映射到内核的三个
`router.pools` 键:`tier_high` / `tier_mid` / `tier_low`（见 `lib.rs` 常量
`TIER_HIGH/MID/LOW`）。

- 用户只定「谁在上、谁在下」（顺序),**绝对能力交给路由**。
- 每次改档，`rebuild_routing()` 依当前已配好的档，重建 `pools` 引用 + heuristic
  `bands`（分数切点 `CUT_HIGH=55` / `CUT_MID=22`，末档 `at_least=0` 兜底）+
  `default_pool`。**只有「已选好 (供应商, 模型)」的档才纳入路由**，两档也能跑。
- 这些切点是能跑的合理默认值,**将来由评测中心校准替换**（护城河那条线）。

供应商 = 一个 `openai-compatible` 上游。Key 存系统钥匙串（`secrets::keyring_set`），
配置文件里只留指向 keychain 的 `auth.slot`，**明文 Key 不落盘**。

---

## 4. v1 全能力子页面（6 tab）

主页之外,把 CLI 的能力搬成子页面,全部接**真实内核 API**、不碰护城河:

| tab | 接的内核 | 说明 |
|---|---|---|
| **主页** | `pools` + heuristic | 三档路由面板 + 供应商增删 |
| **路由表** | 纯读 `draft.router` | 四层可视化:规则 → 提示 → 启发式分档 → 默认兜底;第 3 层把三档解成「分数≥X → 池 → 供应商·模型」 |
| **用量** | `stats::collect` | 只读本地 SQLite;总览卡 + 按 upstream/model/pool/status 分组 + 时间窗;库没建给引导（不当错误） |
| **插件** | `PluginRegistry::discover` + `render_list()` | 与 CLI `plugin list` 同源的等宽清单 + 插件目录 / 入站适配器 / 方言 |
| **设置** | `server.auth` / `data.metrics` 两开关 | 能物化就落盘;**改这两项对运行中的 serve 不生效,需重启代理**（界面已提示）|
| **关于** | `upgrade::check` | 匿名版本检查（内核唯一合法外联）;只比对 + 给发布页链接,**不自替换二进制** |

隐私红线在这里也守住:用量页读的指标库**结构上装不下 prompt**（列都是数字 / 闭合
枚举 / 运营者配的名字）。

---

## 5. 后端命令清单（`lib.rs`）

| command | 作用 |
|---|---|
| `get_state` | 快照:providers / tiers / serve / config_error / settings |
| `add_provider` / `remove_provider` | 增删上游;有 key 进 keychain |
| `set_tier` | 设/清某一档 (供应商, 模型)，触发 `rebuild_routing` |
| `save_config` | 校验 + 原子写盘（校验不过不写） |
| `serve_start` / `serve_stop` | 起停后台 serve runtime;起时按 `server.auth` 生成/复用虚拟 Key |
| `connect_agent` | 接入 cc / codex / opencode（见 §7） |
| `set_settings` | 切 auth / metrics 开关 |
| `get_stats` | 用量聚合（since / by 参数） |
| `get_router_table` | 四层路由表视图 |
| `get_plugins` | 插件清单 |
| `check_upgrade` | 版本检查 |

---

## 6. 网关多入站适配器（match_inbound）

**目标:** 让一个代理同时服务多种入站协议——OpenAI 系（Codex/opencode，
`/v1/chat/completions`）与 Anthropic（Claude Code，`/v1/messages`）**同时跑**。

**关键认知:** `match_inbound` **不是**自造的配置或宿主路由表,它是早已在 WIT
`agent-adapter-v1` 里、`agent-openai` 已实现的**适配器 ABI 函数**:
`{ method, path, headers } → { matched, protocol }`。此前只是宿主没调它。

**宿主侧实现（零改 IR / router-core / protocol）:**

1. **`crates/plugin-runtime/src/agent.rs`** —— `AgentPlugin::match_inbound` 薄包装,
   调已生成的 `call_match_inbound` 绑定,返回 `MatchOutcome{ matched, protocol }`。
   注意:它**不在** `AgentAdapter` trait 里（conformance 只管翻译,不管宿主多路复用），
   是 `AgentPlugin` 的固有方法。
2. **`apps/cli/src/config.rs`** —— `PluginsConfig` 从单串 `agent: String` 改为
   `agent: Option<String>`（向后兼容别名）+ `agents: Vec<String>`。访问器
   `effective_agents()` 优先列表、回退单串;`validate()` 兜「至少一个」。
3. **`apps/cli/src/gateway.rs`** —— `Gateway` 持 `Vec<LoadedAgent>`（每个 = 插件 +
   其 manifest 协议）。每请求 `select_agent(method, path, headers)` 逐个问各适配器的
   `match_inbound`（headers 已脱敏成 `HeaderDigest`），**首个 `matched` 者服务**;
   无人认领 → 404「no inbound adapter claims …」。其余管线一字不动。
4. **`apps/cli/src/server.rs`** —— 单路由改为 `/v1/models` GET + **兜底
   （fallback）**。宿主不枚举协议路径,新协议路径**零改 server**——认领与否由适配器
   自己的 `match_inbound` 决定。

**匹配优先级 = `agents` 列表顺序。** agent-openai 的匹配逻辑是
`path.ends_with("/chat/completions")`。

当前桌面模板的 `plugins.agents` 已按 `agent-openai`、`agent-anthropic`、
`agent-openai-responses` 顺序启用 Chat Completions、Anthropic Messages 和 Responses。
旧式仅含 `plugins.agent = "agent-openai"` 的桌面配置会先在内存中迁移为这三项，保存后才
写盘。三套 Adapter 的能力边界见
[桌面App-Agent接入机制.md](桌面App-Agent接入机制.md)。

---

## 7. 接入 agent 与 CC 安全闸（重要 footgun）

`connect_agent` 各 agent 各写各的配置文件,互不冲突、可同时接:

- **Codex** → `~/.codex/config.toml` 加指向本代理的 `model_provider`（key 走环境变量）。
- **opencode** → `~/.config/opencode/opencode.json` 加 openai-compatible provider。
- **Claude Code** → `~/.claude/settings.json` 的 `env` 写 `ANTHROPIC_BASE_URL` +
  `ANTHROPIC_AUTH_TOKEN`。

**⚠️ CC 接入的安全闸——务必理解:** 写 `~/.claude/settings.json` 是**全局**的,会连带
改变**本机上正在运行的每一个 Claude Code**的后续请求目标,包括**用来开发
token-station 的那个会话**。当前桌面模板已包含 `agent-anthropic`，但自定义配置仍可能
移除或改名该 Adapter。

因此 `connect_cc` 仍有前置闸:`anthropic_inbound_ready()` 检查配置里（`agent` 与
`agents` 两处适配器名）是否已挂上名称含 `anthropic` 的入站适配器,**未满足就直接拒绝、
完全不碰 settings.json**。判据落在写文件那一步（不是灰按钮），任何入口都拦得住。
原文件存在时，接入前写
`~/.claude/settings.json.token-station.bak`；重复接入会用最近一次写入前的内容覆盖该备份。

**长期待办:** 改回 scoped 启动（派生带 env 的 CC 子进程,不写全局配置）。

---

## 8. 守住的不变量

GUI 改动同样受内核四条红线约束（见 [架构总览.md](架构总览.md)）:

1. **路由是纯函数** —— GUI 不掺时钟/随机/IO 进路由;三档只是往 `pools` 填值。
2. **决策记录装不下 prompt** —— 用量页读的库结构上无 prompt 列。
3. **打分全整数** —— 切点 / band 都是整数。
4. **插件沙箱** —— provider/agent adapter 是 WASM,无网络、拿不到明文 Key。

---

## 9. 跑起来 / 落点

```bash
cd apps/desktop
npm install
npm run tauri dev        # 起 vite + 编译 Tauri 后端 + 开窗口
```

- 前端改动 HMR 即时生效;**后端（src-tauri）改动 tauri dev 会自动重编重启**。
- 配置文件 `token-station.json` 锚在仓库根（`repo_root()`,`tauri dev` 的 CWD 不稳,
  故用 `CARGO_MANIFEST_DIR` 往上三级)。插件目录 `plugins-dist/`、数据目录
  `token-station-data/` 同样锚绝对路径,serve 在任何 CWD 都找得到。

改动落点速查:

| 要改… | 去… |
|---|---|
| 界面 / 交互 | `apps/desktop/src/App.tsx`、`src/pages/*.tsx` |
| 前后端接口 | `src/api.ts` + `src-tauri/src/lib.rs` 的 `#[tauri::command]` |
| 三档 / 路由重建 | `lib.rs` 的 `rebuild_routing` / `TIER_*` / `CUT_*` |
| 供应商预设 | `src/catalog.ts` |
| 多入站 / 网关 | `apps/cli/src/{gateway,server,config}.rs`、`crates/plugin-runtime/src/agent.rs` |

---

## 10. 尚未做 / 依赖

- **成本仪表** `cost_micros` 现恒空,等定价表（内核 C2#4）。用量页已按「—」显示。
- **产品护城河**（难度分类器 + 评测中心）是**另起的并行仓库**,不在本 app 范围;
  校准好的权重/切点将来回灌到三档面板的默认值。设计见 `docs/design/评测中心设计.md`。
