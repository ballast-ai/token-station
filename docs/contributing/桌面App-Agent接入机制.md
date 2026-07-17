# 桌面 App 接入 Agents 的实现机制

面向两类读者：使用桌面 App 接入 Claude Code、Codex、OpenCode 的用户，以及维护或扩展
这条链路的开发者。本文描述当前源码，不把历史状态、计划或单次验收结论当成产品能力。

本文中的“接入 Agent”特指：**修改外部 Agent 的本机配置，使它把 LLM 请求发给
token-station 的回环代理。** App 不会启动 Agent 进程，也不会把 Agent 嵌入桌面应用。

事实来源按以下顺序取舍：当前可执行源码与测试、源码注释、旧文档。三者冲突时以前者
为准。

## 1. 结论：接入包含两条链路

配置链路只在用户点击接入时运行：

```text
App 顶栏 Agent 按钮
  → React connectAgent(kind)
  → Tauri connect_agent command
  → 检查 App 配置可编辑、代理已启动
  → 读取 listen、virtual_key、plugins.agents
  → 修改对应 Agent 的本机配置
  → 备份旧文件，以临时文件 + rename 替换
```

请求链路在 Agent 每次调用模型时运行：

```text
Agent HTTP 请求
  → token-station server
  → 本地虚拟 Key 鉴权
  → Gateway::select_agent
  → 首个 match_inbound 成功的 Agent Adapter
  → Canonical IR
  → Router
  → Provider Adapter
  → 上游模型
```

两条链路的职责不同：桌面端 connector 只负责写 Agent 配置；Agent Adapter 只负责入站
协议翻译；Router 才负责选池、上游和模型。

## 2. 用户如何接入

### 2.1 前置条件

1. 在 App 中配置至少一个供应商和模型，并把模型放入至少一个路由档位；
2. 点击“启动代理”，确认顶栏显示“运行中”；
3. 保持默认的本地鉴权开启。关闭鉴权时存在额外边界，见[本地鉴权](#72-本地鉴权)；
4. 退出或准备重启目标 Agent。Claude Code 尤其不应在写全局配置时继续跑重要任务。

`connect_agent` 在后端再次检查代理是否运行；未运行时直接返回“请先启动代理(serve)再接入
agent”，不会修改 Agent 配置。

### 2.2 点击对应入口

顶栏当前有三个入口：Claude Code、Codex、OpenCode。点击后，App 写入下表所列配置：

| Agent | 配置文件 | App 写入的 Base URL | 鉴权配置 | 入站协议与 Adapter |
|---|---|---|---|---|
| Claude Code | `~/.claude/settings.json` | `http://<listen>` | `ANTHROPIC_AUTH_TOKEN` | `/v1/messages` → `agent-anthropic` |
| Codex | `~/.codex/config.toml` | `http://<listen>/v1` | `env_key = "TOKENSTATION_KEY"` | `/v1/responses` → `agent-openai-responses` |
| OpenCode | `~/.config/opencode/opencode.json` | `http://<listen>/v1` | Provider `apiKey` | `/v1/chat/completions` → `agent-openai` |

`listen` 来自当前运行中的服务，默认是 `127.0.0.1:8787`。Claude Code 需要不带 `/v1`
的根地址；Codex 和 OpenCode 使用带 `/v1` 的 OpenAI 风格 Base URL。

### 2.3 接入后的动作

- Claude Code：重启 Claude Code，使新的环境变量生效；
- Codex：在启动 Codex 的终端设置面板显示的虚拟 Key，再启动 Codex：

  ```bash
  export TOKENSTATION_KEY='<面板显示的虚拟 Key>'
  ```

- OpenCode：重启或重新加载配置，在模型列表中选择 `tokenstation/auto`。

App 当前没有“取消接入”按钮。恢复方法见[备份和恢复](#6-备份和恢复)。

## 3. 配置链路如何实现

### 3.1 前端到 Tauri

[App.tsx](../../apps/desktop/src/App.tsx) 中的 `AGENTS` 定义三个按钮，其 `kind` 分别为
`cc`、`codex`、`opencode`。点击按钮后调用
[api.ts](../../apps/desktop/src/api.ts) 的 `connectAgent(kind)`，后者执行：

```text
invoke<string>("connect_agent", { kind })
```

前端只展示成功消息或错误，不直接读写用户目录。

### 3.2 Tauri 统一分派

[apps/desktop/src-tauri/src/lib.rs](../../apps/desktop/src-tauri/src/lib.rs) 中的
`connect_agent` 完成统一前置检查和分派：

1. `ensure_editable()` 确认 App 启动时没有进入损坏配置的只读保护；
2. `serve_view()` 确认代理正在运行；
3. 从运行状态读取实际 `listen` 和虚拟 Key；
4. 从草稿的 `plugins.agents` / 旧 `plugins.agent` 判断 Anthropic 入站是否配置；
5. 构造 `http://<listen>` 和 `http://<listen>/v1`；
6. 按 `kind` 调用 `connect_cc`、`connect_codex` 或 `connect_opencode`。

当本地鉴权关闭、运行状态没有虚拟 Key 时，后端为需要内嵌 token 的 connector 使用字面量
`token-station-no-auth`。服务端此时不检查 Authorization，因此该值不是有效凭证，只是配置
占位。

### 3.3 桌面端默认入站配置

桌面端模板的 `DESKTOP_AGENTS` 当前按以下顺序加载：

```text
agent-openai
agent-anthropic
agent-openai-responses
```

顺序也是 `match_inbound` 优先级。三个 Adapter 当前认领的路径互斥，因此这一顺序不会让
Chat Completions 抢占 Messages 或 Responses。

若 App 读到旧式配置：`plugins.agents` 为空且 `plugins.agent == "agent-openai"`，
`prepare_desktop_draft` 会在内存中升级为上述三项，并删除旧单值字段。升级只有在用户保存后
才写回磁盘。其他自定义 `plugins.agents` 列表不会被这段迁移逻辑替换。

## 4. 三种 Agent 分别写了什么

### 4.1 Claude Code

`connect_cc_at` 读取 `~/.claude/settings.json`，要求顶层和 `env` 都是 JSON 对象，然后在
`env` 中写入或覆盖：

```json
{
  "ANTHROPIC_BASE_URL": "http://127.0.0.1:8787",
  "ANTHROPIC_AUTH_TOKEN": "<本地虚拟 Key>",
  "MAX_THINKING_TOKENS": "0",
  "CLAUDE_CODE_DISABLE_THINKING": "1",
  "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING": "1",
  "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS": "1",
  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
}
```

示例中的地址是默认值，实际值取运行中的 `listen`。App 保留 `permissions`、其他 `env`
字段和其他顶层设置，但会覆盖上述同名键。

这是**全局配置**，会影响读取 `~/.claude/settings.json` 的所有 Claude Code 进程。写入前，
`anthropic_inbound_ready` 只检查 `plugins.agents` 或旧 `plugins.agent` 中是否有名称包含
`anthropic` 的 Adapter；没有则拒绝并保持文件不变。该检查是包名字符串检查，不是独立的
运行时健康探测。不过 `connect_agent` 又要求代理已经成功启动，启动阶段已实际加载配置中的
Adapter。

### 4.2 Codex

`connect_codex_at` 解析 `~/.codex/config.toml`，保留无关表和字段，但写入或覆盖：

```toml
model = "auto"
model_provider = "tokenstation"

[model_providers.tokenstation]
name = "token-station"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
env_key = "TOKENSTATION_KEY"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
```

App 不把虚拟 Key 写入 TOML，只告诉 Codex 从 `TOKENSTATION_KEY` 读取。`model`、
`model_provider` 和整个 `model_providers.tokenstation` 表会被当前值覆盖；其他配置保留。

### 4.3 OpenCode

`connect_opencode_at` 解析 `~/.config/opencode/opencode.json`，在 `provider` 下写入或覆盖
`tokenstation`：

```json
{
  "provider": {
    "tokenstation": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "token-station",
      "options": {
        "baseURL": "http://127.0.0.1:8787/v1",
        "apiKey": "<本地虚拟 Key>"
      },
      "models": {
        "auto": { "name": "auto (智能路由)" }
      }
    }
  }
}
```

其他 Provider 保留；同名 `provider.tokenstation` 被整体覆盖。重复点击不会新增重复 Provider，
因此结构上是幂等的。

## 5. 请求链路如何实现

### 5.1 Server 不硬编码三套业务处理器

[server.rs](../../apps/cli/src/server.rs) 只为 `GET /v1/models` 注册显式路由，其余请求进入
fallback handler。handler 在阻塞线程中调用 `Gateway::chat(method, path, headers, body)`。
因此新增入站路径通常不需要修改 Server；路径归属由 Adapter 的 `match_inbound` 决定。

本地鉴权发生在 Gateway 之前。鉴权失败时，Server 根据请求路径直接返回 Anthropic、
Responses 或通用 OpenAI 形状的 401，不进入路由和上游，也不会产生 Gateway 的请求记录。

### 5.2 Gateway 选择入站 Adapter

[gateway.rs](../../apps/cli/src/gateway.rs) 在启动时按
`PluginsConfig::effective_agents()` 的顺序加载所有 Agent Adapter：

- `plugins.agents` 非空时使用该列表；
- 列表为空时回退到已废弃的单值 `plugins.agent`；
- 两者都没有时配置校验失败。

每个请求进入 `select_agent` 后，Gateway 构造 `{method, path, headers}`。`headers` 已通过
`HeaderDigest::redacting` 脱敏，Adapter 看不到 Authorization 等原始值。Gateway 依次调用
[AgentPlugin::match_inbound](../../crates/plugin-runtime/src/agent.rs)：

- 第一个返回 `matched = true` 的 Adapter 获得该请求；
- 返回 `matched = false` 时继续下一个；
- Adapter 调用报错时记录到 stderr 并跳过，不能否决其他 Adapter；
- 无 Adapter 认领时返回通用 `invalid_request` 404，不进入上游。

当前映射如下：

| 请求 | Adapter manifest 协议 | 当前匹配规则 |
|---|---|---|
| Claude Code `POST /v1/messages` | `anthropic-messages` | POST，去掉 query/trailing slash 后以 `/v1/messages` 结尾 |
| Codex `POST /v1/responses` | `openai-responses` | POST，去掉 query 后精确等于 `/v1/responses` |
| OpenCode `/v1/chat/completions` | `openai-chat-completions` | path 以 `/chat/completions` 结尾 |

### 5.3 归一、路由和出站

选中 Adapter 后，公共主链保持一致：

1. Agent Adapter 将入站 JSON 归一成 Canonical `ChatRequest` 并抽取 hints；
2. Router 根据规则、hint、启发式和默认池选择上游模型；
3. Provider Adapter 把 Canonical 请求构造成真实上游 HTTP 请求；
4. `ProviderConfig::authorize` 检查目标 URL 和凭证槽；
5. 宿主在通过外泄闸门后才解析并注入上游凭证；
6. Provider Adapter 解析上游响应；
7. 原 Agent Adapter 把 Canonical 响应或流式事件渲染回调用方协议。

因此 Agent Adapter 不选择 Provider、上游或模型，Router 也不需要知道请求来自 Claude Code、
Codex 还是 OpenCode。

## 6. 备份和恢复

### 6.1 当前写入保证

三个 connector 共用 `write_config`：

1. 目标文件存在时先读取原始字节并写入固定备份路径；
2. 新内容写入同目录 `.<文件名>.token-station.tmp`；
3. 最后用 `rename` 替换目标文件。

对应备份路径为：

| 当前配置 | 备份 |
|---|---|
| `~/.claude/settings.json` | `~/.claude/settings.json.token-station.bak` |
| `~/.codex/config.toml` | `~/.codex/config.toml.token-station.bak` |
| `~/.config/opencode/opencode.json` | `~/.config/opencode/opencode.json.token-station.bak` |

必须理解两个边界：

- 原文件不存在时 App 不创建备份；
- 每次成功接入都会覆盖同一个备份文件，所以备份表示**最近一次成功写入前**的状态，重复点击后
  不一定还是首次接入前状态。

JSON/TOML 无法解析、JSON 顶层不是对象、`env` / `provider` 类型错误时，connector 在调用
`write_config` 前失败，原文件不变，也不会生成新备份。备份成功后若临时写入或 rename 失败，
错误会返回给界面；代码当前不自动删除可能留下的临时文件。

### 6.2 从已有备份恢复

先退出对应 Agent，检查当前文件与备份，再执行精确路径覆盖。以下命令会额外保留一份恢复前
快照：

```bash
# Claude Code
diff -u "$HOME/.claude/settings.json.token-station.bak" "$HOME/.claude/settings.json"
cp -p "$HOME/.claude/settings.json" "$HOME/.claude/settings.json.before-token-station-restore"
cp -p "$HOME/.claude/settings.json.token-station.bak" "$HOME/.claude/settings.json"

# Codex
diff -u "$HOME/.codex/config.toml.token-station.bak" "$HOME/.codex/config.toml"
cp -p "$HOME/.codex/config.toml" "$HOME/.codex/config.toml.before-token-station-restore"
cp -p "$HOME/.codex/config.toml.token-station.bak" "$HOME/.codex/config.toml"

# OpenCode
diff -u "$HOME/.config/opencode/opencode.json.token-station.bak" \
  "$HOME/.config/opencode/opencode.json"
cp -p "$HOME/.config/opencode/opencode.json" \
  "$HOME/.config/opencode/opencode.json.before-token-station-restore"
cp -p "$HOME/.config/opencode/opencode.json.token-station.bak" \
  "$HOME/.config/opencode/opencode.json"
```

`diff` 返回 1 通常只表示文件存在差异，不代表命令故障。确认恢复内容后重启 Agent。

### 6.3 没有备份时

如果配置文件由 App 首次创建，不存在自动备份。不要直接删除整个配置文件，除非已经确认其中
没有接入后新增的用户设置。更安全的做法是手动删除 App 管理的键：

- Claude Code：上文列出的 7 个 `env` 键；
- Codex：确认归属后删除顶层 `model = "auto"`、`model_provider = "tokenstation"` 和
  `model_providers.tokenstation`；
- OpenCode：删除 `provider.tokenstation`。

## 7. 安全边界和已知限制

### 7.1 两类 Key 不要混淆

- 本地虚拟 Key：Agent 到 `127.0.0.1` 回环代理的鉴权；
- 上游供应商 Key：token-station 到模型供应商的鉴权，仍由 OS 钥匙串和 Provider 配置管理。

Claude Code / OpenCode 配置中写入的是本地虚拟 Key，不是供应商 Key。Agent Adapter manifest
声明无网络、无文件系统、无 secrets 权限；上游 Key 不会交给 Agent Adapter。

### 7.2 本地鉴权

桌面模板默认 `server.auth = true`。启动时从数据目录加载或创建虚拟 Key，Server 要求
`Authorization: Bearer <key>`。

若关闭本地鉴权：

- Server 的 `virtual_key` 为 `None`，所有本机进程都可访问回环代理；
- Claude Code 和 OpenCode connector 写入 `token-station-no-auth` 占位值，Server 不校验它；
- Codex connector 仍固定写 `env_key = "TOKENSTATION_KEY"`，但面板没有虚拟 Key 可复制。

因此当前一键接入主路径以保持默认鉴权开启为前提。关闭鉴权会削弱本机进程隔离，并让 Codex
的后续配置提示不完整。

### 7.3 协议能力边界

三个官方 manifest 当前都只声明 `chat`、`stream`、`tool_call`、`agent_hint`，没有声明
`json_schema`：

- `agent-anthropic`：Canonical IR 尚未表达 `thinking` / `redacted_thinking`，相关输入明确
  返回能力错误；桌面 connector 因此关闭 Claude Code 的 thinking/beta；
- `agent-openai-responses`：支持当前已实现的文本、图片 URL、function tool、流式和 usage
  子集；reasoning item、computer/hosted tool、file-id 图片及完整 Responses 事件全集不在
  当前支持范围；结构化输出明确拒绝；
- `agent-openai`：Chat Completions 的 `response_format` 为 `json_schema` 或
  `json_object` 时明确拒绝。

“普通文本、流式和本地 function tool 主链可用”不等于完整兼容 Agent 的所有功能。MCP、
浏览器、远程 channel、hosted tools 和 Agent 自身插件也不因为模型请求接入而自动获得支持。

### 7.4 当前产品边界

- App 没有取消接入、自动恢复或启动 scoped Agent 子进程；
- Claude Code 使用全局配置，不是只影响一次新进程；
- App 当前按钮只包含 Claude Code、Codex、OpenCode；OpenClaw 有独立手工指南，但不是桌面
  App 一键接入能力；
- Anthropic 就绪检查按 Adapter 包名是否包含 `anthropic` 判断，不读取 manifest 能力字段；
- 重复接入会刷新固定备份，不能把该备份长期当作首次接入快照。

## 8. 如何扩展新的 Agent

先判断新 Agent 是“复用现有协议”还是“引入新协议”。这是决定改动范围的关键。

### 8.1 复用现有协议

若新 Agent 能使用当前 Chat Completions、Responses 或 Anthropic Messages：

1. 用真实请求和现有 Adapter 测试确认路径、请求字段、流式事件和工具闭环均在已支持子集内；
2. 在 [api.ts](../../apps/desktop/src/api.ts) 的 `AgentKind` 增加类型；
3. 在 [App.tsx](../../apps/desktop/src/App.tsx) 的 `AGENTS` 增加按钮；
4. 在 Tauri 后端新增 `connect_<agent>_at(home, ...)`，解析并保留现有配置；
5. 在 `connect_agent` 中增加分派；
6. 增加“保留无关配置、非法配置不覆盖、备份语义、重复接入”测试。

复用协议时不应新增 Server 路由、Router 分支或 Provider 厂商特判，也不需要为了 Agent 名称
复制一个协议相同的 Adapter。

### 8.2 引入新的入站协议

若现有 Adapter 无法无损表达新协议：

1. 新增独立 `plugins/official/agent-<name>` WASM package 和 manifest；
2. 实现 `match_inbound`、请求归一化、hint 提取、非流式/流式响应渲染和协议形状错误；
3. 对无法映射的关键字段返回 capability error，不静默删除；
4. 增加 conformance fixtures 和 plugin-runtime 真实 WASM 测试；
5. 若桌面端默认支持，将包名加入 `DESKTOP_AGENTS` 并明确匹配优先级；
6. 确保插件能从 `plugins.dir` 发现，若作为官方内置发布，还要同步 CLI builtin 列表、构建脚本
   和 CI；
7. 再按上一节增加 UI 和配置 connector。

只有 Canonical IR 无法无损表达所需语义时，才提出 IR 变更。新增协议路径通常仍不修改
Server，因为 fallback 会把请求交给 Gateway。Router 不增加 Agent/厂商特判。

### 8.3 扩展验收清单

- App 在代理未运行、配置损坏或 connector 写入失败时不会覆盖用户文件；
- 配置已存在时保留无关字段，重复接入结果可预测；
- 本地错误 Key 在 Server 层返回 401，不进入 Router 或上游；
- 普通文本、流式和本地工具闭环只按 manifest 声明的能力验收；
- 不支持字段明确失败，且失败发生在访问上游之前；
- 日志和 metrics 不包含 prompt、response、本地虚拟 Key 或上游 Key；
- 恢复最近一次备份后，Agent 回到最近一次接入写入前状态。

## 9. 测试证据和源码索引

当前自动化证据包括：

- 桌面端测试：默认模板启用三种入站；旧 Chat-only 配置内存迁移；Codex 保留已有 TOML 并
  创建备份；Claude Code 保留已有设置、安全闸与非法 JSON 不写文件；OpenCode 保留其他
  Provider 且重复接入幂等；
- CLI proxy 测试：Responses 与 Anthropic 请求经过现有 Provider 主链；本地鉴权和协议不
  匹配在上游前失败；流式与工具闭环按各自协议渲染；
- plugin-runtime 测试：三个官方 Agent Adapter 通过真实 WASM suite，Anthropic 和
  Responses 的 `match_inbound` 只认领目标请求；
- conformance 测试：官方 manifest 通过门禁，OpenAI 两个 Adapter 不误报结构化输出能力。

维护时优先从以下文件核对行为：

- [桌面前端 App.tsx](../../apps/desktop/src/App.tsx)
- [桌面 IPC api.ts](../../apps/desktop/src/api.ts)
- [桌面 Tauri 后端 lib.rs](../../apps/desktop/src-tauri/src/lib.rs)
- [CLI 配置 config.rs](../../apps/cli/src/config.rs)
- [Server server.rs](../../apps/cli/src/server.rs)
- [Gateway gateway.rs](../../apps/cli/src/gateway.rs)
- [Agent Plugin Runtime](../../crates/plugin-runtime/src/agent.rs)
- [Agent Adapter 一致性要求](入站适配器-需求与实现路线.md)
- [测试指南](测试指南.md)
