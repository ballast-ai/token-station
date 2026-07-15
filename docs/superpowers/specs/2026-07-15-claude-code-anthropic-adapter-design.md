# Claude Code Anthropic 入站适配设计（DeepSeek E2E 样例）

## 1. 目标

在不修改 Claude Code 自身逻辑的前提下，使其通过 `ANTHROPIC_BASE_URL`
连接本地 token-station，由新增的 `agent-anthropic` 将 Anthropic Messages
请求归一为 Canonical IR，再由现有路由器选择配置中的 provider /
upstream / model。返回的 `ChatResponse` / `StreamEvent` 重新渲染为
Anthropic Messages 响应。

`agent-anthropic`、Gateway 和路由调用链不感知 DeepSeek 或任何具体模型厂商。
DeepSeek 只是本次 M3 真实端到端验收使用的第一个上游样例，通过现有
`provider-openai-compatible` 和配置接入。

本设计覆盖需求文档的 M0–M3：环境基线、首批 Agent 协议盘点表初版、
Anthropic 非流式适配、流式/工具调用/Hint，以及 Claude Code 端到端
验证。本轮会调研并记录 Claude Code、Codex、opencode 和需向负责人确认
名称的 `openclow`，但只实现 Claude Code 所需的协议链；其他 Agent 的代码扩展
留在 M4。

## 2. 已验证基线

- Rust `1.96.1` 与 `wasm32-wasip2` 已可用。
- `cargo build --release -p token-station-cli` 通过。
- `agent-openai` 可构建为 WASM，现有 `agent-protocol-v1` 16 项检查全过。
- 本机已安装 Claude Code `2.1.170`。
- 当前网关只注册 `/v1/chat/completions` 与 `/v1/models`，尚不能接收
  Claude Code 使用的 `/v1/messages`。

## 3. 方案选择

采用完整 IR 翻译链：

```text
Claude Code
  -> POST /v1/messages
  -> agent-anthropic
  -> ChatRequest / AgentHint
  -> router
  -> configured provider adapter
  -> configured upstream / model
  -> ChatResponse / StreamEvent
  -> agent-anthropic
  -> Anthropic JSON / SSE
```

本次 E2E 配置会将 `configured provider adapter / upstream` 实例化为
`provider-openai-compatible / DeepSeek`。不采用将 Anthropic 请求透传到
`https://api.deepseek.com/anthropic` 的快速方案，
因为它会绕过 Canonical IR，无法完成入站适配器的任务目标。也不在本轮
新增 provider adapter，避免越过需求文档对出站适配器的范围约束。

### 3.1 厂商中立不变量

- agent adapter 只按入站协议分类，不按模型厂商分类。
- Gateway 只组织 adapter -> router -> provider 调用，不包含厂商名分支。
- 模型厂商、Base URL、凭证来源、模型名和路由池只存在运行配置。
- 同一个 `agent-anthropic` 必须能把 Claude Code 请求交给任何满足 IR
  能力要求的已配置 provider adapter。
- 单个出站方言无法覆盖的厂商由未来独立 provider adapter 接入，不回填
  到 agent adapter 或 router 中。

## 4. 组件设计

### 4.1 `agent-anthropic` WASM 适配器

以 `agent-openai` 为模板新增 `plugins/official/agent-anthropic`（不使用当前只生成
provider adapter 的 `plugin new`），并完整实现 `agent-adapter-v1`：

- `manifest.json`：`kind=agent-adapter`、`api_version=agent-adapter-v1`、完整
  `agent_protocols` / `agent_tools` / `conformance`，且 network/filesystem/secrets 权限为空。

- `metadata` / `healthcheck`：声明官方 agent adapter 身份与健康状态。
- `supported_agent_protocols`：声明 `anthropic-messages` 和 `claude-code`。
- `match_inbound`：匹配 `POST /v1/messages` 及其查询参数形式，不把
  `/v1/messages/count_tokens` 当作推理请求。
- `normalize_inbound`：映射顶层 `system`、messages content blocks、tools、
  `max_tokens`、`stop_sequences`、采样参数和 `stream`。
- `extract_agent_hint`：保留已有 transport hints，并仅接受 v1 契约内的
  `planning` / `edit` / `summarize`；不通过解析提示词猜测路由意图。
- `render_response`：将文本、`tool_use`、stop reason 与 usage 渲染为
  Anthropic Messages JSON。
- `render_stream_event`：将 `Delta` / `ToolCallDelta` / `Usage` / `Done` /
  `Error` 渲染为 Anthropic SSE 事件序列。
- `map_inbound_error`：产生 Anthropic `type=error` 错误信封并保留 HTTP 状态。

### 4.2 请求归一规则

- 顶层 `system` 字符串或 text block 数组转成 `Role::System` 消息。
- `user` / `assistant` 文本 block 保留顺序；多段内容用 `Content::Parts`。
- Anthropic `image.source` 中的 URL 或 base64 转为 `ImageUrl`；base64 使用
  data URL 表示，避免改动 IR。具体上游是否支持图片由其 model
  capability 与 provider adapter 决定；本次 DeepSeek E2E 不将图片列为通过项。
- assistant `tool_use` 转为 `ToolCall`，`input` 按 JSON 精确序列化到
  `arguments`。
- user `tool_result` 转为 `Role::Tool` 消息，保留 `tool_use_id`和文本结果。
- 未知字段容忍忽略；可安全保留且属于整个请求的字段放入
  `ChatRequest.extensions`。
- `thinking` / `redacted_thinking` 目前无完整 IR 表示。实施前产出 IR 变更
  请求，写明原协议字段、缺失语义和建议表示，交由对接人评审。首版接入文档
  显式关闭 Claude Code adaptive thinking 与实验 beta，不伪造无损支持；
  未经评审不修改 `crates/protocol`。

### 4.3 流式状态

Anthropic SSE 需要 `message_start -> content_block_* -> message_delta -> message_stop`
顺序，而 Canonical `StreamEvent` 没有显式 start 事件。适配器使用宿主传入的
非 IR `context.stream_id` 维护每条流独立的有界状态：

- 首个内容事件前产生 `message_start`。
- 文本块产生 `text_delta`，工具块产生 `input_json_delta`。
- 每个工具调用使用独立 content block index，支持并行工具增量。
- `Done` 关闭所有已打开 block，产生 stop reason 和 `message_stop`，随后删除
  `stream_id` 状态，防止泄漏。
- `Error` 立即渲染错误并清理状态。

网关为每个请求创建唯一 `stream_id`、响应 ID 和实际路由模型名，不使用
全局单流布尔值，避免并发 Claude Code 子 Agent 串流。

### 4.4 HTTP 入口与鉴权

- `apps/cli/src/server.rs` 新增 `POST /v1/messages`，与 Chat Completions
  共用已有背压和 blocking worker 桥接。
- 请求的 method/path 传入网关，网关在 normalize 前调用
  `match_inbound`，协议不匹配时返回 404/invalid request，不让错误适配器处理。
- `crates/plugin-runtime/src/agent.rs` 只增加已存在 WIT 函数的宿主透传，不调整
  WASM 权限、资源限额、ABI 或凭证边界。
- `/v1/messages?beta=true` 按 path 匹配，不按完整 URI 字符串匹配。
- Claude Code 使用 `ANTHROPIC_AUTH_TOKEN` 携带本地 virtual key。DeepSeek Key
  只由 token-station 从 `DEEPSEEK_API_KEY` 环境变量解析，两者不复用。
- 凭证值不进入 adapter、fixture、配置文件、日志或 Git 提交。
- `x-claude-code-session-id` 等非凭证头可用于识别 `agent_tool=claude-code`，
  但不当作用户身份或路由强制信号。

### 4.5 厂商无关配置与 DeepSeek E2E 样例

产品运行时继续使用现有 `upstreams[*].provider / base_url / auth / models` 和
`router.pools` 组合任意厂商。同一 slot 名的凭证依然按 `(upstream, slot)` 隔离。
`agent-anthropic` 不引入新的厂商配置字段。

本次另新增一份不含凭证的 DeepSeek E2E 示例配置：

- agent：`agent-anthropic`
- provider：现有 `provider-openai-compatible`
- base URL：`https://api.deepseek.com`
- auth slot：`provider_api_key`
- auth source：`DEEPSEEK_API_KEY`
- 主模型：`deepseek-v4-pro`
- 可选廉价模型：`deepseek-v4-flash`

模型名只存在样例配置中，不硬编码到 adapter、Gateway 或 router。
路由器继续根据通用规则、hint、heuristic 和 default pool 选择配置池中的
upstream/model，不新增 DeepSeek 专属策略。

## 5. 协议盘点交付物

M0 新增一份 `agent -> 协议 -> 接入方式 -> 复用/新增 adapter -> 已知限制`
盘点表，首批包含 Claude Code、Codex、opencode 和需确认名称的 `openclow`。

- 盘点使用当前官方文档或实际请求作证据，不把历史协议当成当前事实。
- 盘点表决定 M4 需要的 adapter 数量，但本轮只实现 `anthropic-messages`。
- 若发现 Responses API reasoning item 等 IR 缺口，只记录变更请求，本轮
  不修改 IR。

## 6. 错误处理

| 场景 | 外部行为 |
|---|---|
| 本地 virtual key 缺失/错误 | Anthropic 形状 401，不进入 adapter |
| 非法 JSON / 缺失 model/messages | Anthropic `invalid_request_error` |
| 不支持的 content block | 明确 capability/invalid request，不静默丢弃关键语义 |
| DeepSeek 拒绝 Key | Anthropic `authentication_error` |
| DeepSeek 限流 | Anthropic `rate_limit_error` 并保留 429 |
| 上游不可用 | Anthropic `api_error` / 502–504 |
| 流中失败 | SSE `error` 事件后结束且清理 stream state |

错误映射保持 token-station 闭合 `ErrorCode` 语义，不为匹配 Claude Code 而
修改 router 的可重试判定。

## 7. 测试设计

### 7.1 适配器 fixtures

五族必须齐全并通过：

- `agent.normalize`：system 数组、文本、工具定义、`tool_use`、`tool_result`、
  采样参数、未知字段。
- `agent.hint`：契约内 StepType 与无提示场景。
- `agent.render`：普通文本、工具调用、usage 和 stop reason。
- `agent.stream`：文本增量、工具 JSON 增量、usage、done 和错误。
- `agent.error`：鉴权、限流、非法请求、上游不可用。

一致性套件还必须证明相同输入确定性、未知字段容忍与 WASM 真实加载。

### 7.2 宿主测试

- `/v1/messages` 注册与 method/path 传递。
- Bearer virtual key 正向和 401 负向。
- 协议不匹配拒绝。
- 普通 JSON 响应与 SSE content type/背压不回归。
- 两个不同 `stream_id` 交错渲染时不串状态。

### 7.3 端到端验收

1. 使用本地可控假上游验证 Claude Code 真正发出 `/v1/messages`，并完成
   普通对话、工具调用、流式和错误四条链；此层不依赖付费 API。
2. 通过本机 `DEEPSEEK_API_KEY` 启动真实 DeepSeek 上游，至少完成：
   - Claude Code 普通非交互问答；
   - 流式文本返回；
   - 一次真实工具调用与结果回传；
   - 无效本地 Key 的错误回传。
3. 检查请求日志不含 prompt、DeepSeek Key 或本地 virtual key。

## 8. 接入配方

文档中使用临时 shell 环境变量，不修改用户全局 Claude Code 配置：

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
export ANTHROPIC_AUTH_TOKEN="$(<token-station-data/virtual-key)"
export ANTHROPIC_MODEL=deepseek-v4-pro
export ANTHROPIC_DEFAULT_OPUS_MODEL=deepseek-v4-pro
export ANTHROPIC_DEFAULT_SONNET_MODEL=deepseek-v4-pro
export ANTHROPIC_DEFAULT_HAIKU_MODEL=deepseek-v4-flash
export CLAUDE_CODE_SUBAGENT_MODEL=deepseek-v4-flash
export CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1
export CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1
```

DeepSeek Key 只出现在启动 token-station 的 shell 环境中。启动前使用
下列检查确认当前 shell 已配置，不回显其值：

```bash
test -n "$DEEPSEEK_API_KEY"
```

## 9. 变更边界

必须修改：

- `plugins/official/agent-anthropic/**`
- `apps/cli/src/server.rs`
- `apps/cli/src/gateway.rs`
- `crates/plugin-runtime/src/agent.rs` 中已存在 agent ABI 的最小宿主透传
- 相关 CLI/plugin discovery/load 测试
- `.github/CODEOWNERS` 中 `agent-anthropic` 的独立审计行
- 首批 Agent 协议盘点表
- 不含凭证的 DeepSeek 示例配置
- Claude Code 接入文档

明确不在范围：

- `crates/router-core` 策略语义
- `crates/protocol` IR 结构调整
- `crates/release` 信任链调整
- 官方内置插件发布物料和发布流程调整
- 新增 provider adapter
- Codex、opencode、OpenClaw 的实现
- 将凭证写入仓库或用户全局配置

## 10. 实施与本地提交边界

按可独立审查的阶段本地提交，不 push：

1. 设计规格。
2. 协议盘点表初版与必要的 IR 变更请求。
3. `agent-anthropic` 非流式实现与 normalize/render/error fixtures。
4. 流式、工具调用、Hint 与剩余 fixtures。
5. `/v1/messages` 宿主集成、DeepSeek E2E 样例配置和接入文档。
6. 端到端验收修正。

每个提交前检查暂存区，确保不包含 API Key、virtual key、运行数据、
WASM 构建产物或与任务无关的用户改动。

## 11. 需求追溯矩阵

| 需求文档条目 | 本设计落点 | 验收证据 |
|---|---|---|
| FR-1 适配器完整性 | §4.1 manifest 和全部 ABI | WASM 加载、manifest gate |
| FR-2 Anthropic Messages | §4.1–§4.3 | normalize/render/stream/error fixtures |
| FR-3 目标 Agent 覆盖 | §5 首批盘点，本轮实现 Claude Code | 盘点表 + Claude Code E2E |
| FR-4 协议盘点 | §5 | 初版盘点表 |
| FR-5 Hint 抽取 | §4.1 | `agent.hint` fixtures |
| FR-6 五族 fixtures | §7.1 | conformance 全过 |
| FR-7 接入配方 | §8 | 可执行文档 + E2E |
| NFR-1 无损翻译 | §4.2；IR 缺口走变更请求 | 文本/system/工具/usage/错误 fixtures |
| NFR-2 流式增量 | §4.3 | 逐事件、分块与并发隔离测试 |
| NFR-3 确定性 | §7.1 | conformance determinism gate |
| NFR-4 向前兼容 | §4.2 | unknown-field tolerance gate |
| NFR-5 范围边界 | §9 | diff 红线审计 |
| M0–M3 | §2、§5、§7.3、§10 | 基线命令、盘点表、fixtures、Claude Code E2E |

## 12. 完成定义

只有以下证据同时成立时，“Claude Code 已接通”才成立：

- `agent-anthropic` 完整实现 ABI 且可加载为 WASM。
- 首批 Agent 协议盘点表已产出，且未经评审没有修改 IR / Hint 契约。
- 五族 fixtures 齐全，一致性套件全过。
- `/v1/messages` 真实进入正确 adapter，而非仅存在路由。
- Claude Code 对话、工具、流式、错误四类场景通过。
- 至少一条真实请求由 DeepSeek 回答。
- 无凭证泄漏，Git 工作区只包含预期交付物。
- 所有改动已做本地 commit，未 push。
- 最终改动仅通过 Pull Request 合入，不直接提交到 `main`。
