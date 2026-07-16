# M4 多 Agent 入站覆盖设计

## 1. 目标

在不修改 Codex、OpenCode、OpenClaw 和 Aider 自身业务逻辑的前提下，
使它们能通过独立配置连接本地 token-station，并完成真实端到端验收。

本设计对应
[入站适配器需求与实现路线](../../contributing/入站适配器-需求与实现路线.md)
的 FR-3、FR-4、FR-6、FR-7 和 M4。Claude Code 的 Anthropic Messages 链路已在
M1–M3 完成，本轮不重新实现。

M4 的完成标准不是“有配置文档”，而是每个目标 Agent 都完成：

1. 普通对话。
2. 流式文本。
3. 真实工具调用与工具结果回传。
4. 本地鉴权错误。
5. 上游错误回传。

## 2. 已确认决策

### 2.1 按协议建适配器

不为每个 Agent 建立专属插件。适配器按入站线协议划分：

| Agent | 入站协议 | 路径 | 适配器决策 |
|---|---|---|---|
| Codex | OpenAI Responses | `POST /v1/responses` | 新增 `agent-openai-responses` |
| OpenCode | OpenAI Chat Completions | `POST /v1/chat/completions` | 复用 `agent-openai` |
| OpenClaw | OpenAI Chat Completions | `POST /v1/chat/completions` | 复用 `agent-openai` |
| Aider | OpenAI Chat Completions | `POST /v1/chat/completions` | 复用 `agent-openai` |

Aider 作为需求中“自行调研补充的主流 Agent”。它使用可自动化验收的
CLI 交互，且无需增加新入站协议。

需求文档中的 `openclow` 按用户已确认的官方项目名 **OpenClaw** 执行。

### 2.2 当前协议证据与本机基线

协议判断以 2026-07-16 当日官方仓库/文档为准：

- Codex 当前 `WireApi` 只保留 Responses，`wire_api="chat"` 已明确移除。
- OpenCode 官方 provider 文档指定 `@ai-sdk/openai-compatible` 用于
  `/v1/chat/completions`，`options.baseURL` 可指向本地代理。
- OpenClaw 官方配置契约允许用 `models.providers.*.api="openai-completions"`
  和 `baseUrl` 指向自定义入口。
- Aider 官方 OpenAI-compatible 文档支持 `--openai-api-base` /
  `AIDER_OPENAI_API_BASE`，模型元数据声明 `/v1/chat/completions`。

本机当前已有 Codex `0.144.2` 和 OpenClaw `2026.6.11`；OpenCode 和 Aider 尚未安装。
后两者的 E2E 工具在进入实施后按官方方式安装到临时隔离位置并锁定验收版本，
不做全局安装。

### 2.3 不扩展为单进程多协议

当前运行配置一次只绑定一个 `plugins.agent`。M4 保持该边界：

- Codex 实例加载 `agent-openai-responses`。
- OpenCode、OpenClaw 和 Aider 实例加载 `agent-openai`。
- Claude Code 继续由独立实例加载 `agent-anthropic`。

用户可以按需切换配置，或者用不同端口启动多个隔离实例。本轮不修改
plugin registry 和配置契约以支持单进程多 Agent adapter。

## 3. 请求数据流

### 3.1 Codex

```text
Codex
  -> POST /v1/responses
  -> agent-openai-responses
  -> ChatRequest / AgentHint
  -> router
  -> configured provider adapter
  -> configured upstream / model
  -> ChatResponse / StreamEvent
  -> agent-openai-responses
  -> Responses JSON / SSE
```

### 3.2 OpenCode / OpenClaw / Aider

```text
Agent
  -> POST /v1/chat/completions
  -> agent-openai
  -> ChatRequest / AgentHint
  -> router
  -> configured provider adapter
  -> configured upstream / model
  -> ChatResponse / StreamEvent
  -> agent-openai
  -> Chat Completions JSON / SSE
```

模型厂商、模型名、Base URL 和凭证只存在运行配置中。Agent adapter、
Gateway 和 router 不增加厂商名分支。

## 4. `agent-openai-responses` 协议设计

### 4.1 适配器完整性

新适配器完整实现 `agent-adapter-v1`：

- `metadata` / `healthcheck`。
- `supported_agent_protocols`：声明 `openai-responses` 和 Codex。
- `match_inbound`：只匹配 `POST /v1/responses`及其 query 形式。
- `normalize_inbound`。
- `extract_agent_hint`。
- `render_response`。
- `render_stream_event`。
- `map_inbound_error`。

`manifest.json` 使用 `kind=agent-adapter`、`api_version=agent-adapter-v1`，权限继续为
network/filesystem 禁用且 secrets 为空。

### 4.2 请求归一

| Responses 输入 | Canonical IR |
|---|---|
| `model` | `ChatRequest.model` |
| `instructions` | `Role::System` message |
| 字符串 `input` | `Role::User` text message |
| `input` 中的 message item | 对应 role 的 `Message` |
| `input_text` | `ContentPart::Text` |
| `input_image` | `ContentPart::ImageUrl` |
| `function_call` | assistant `ToolCall` |
| `function_call_output` | `Role::Tool` message |
| function tool | `ToolDef` |
| `max_output_tokens` | `Sampling.max_output_tokens` |
| `temperature` / `top_p` | `Sampling` 对应字段 |
| `stream` | `ChatRequest.stream` |

`tool_choice`、`parallel_tool_calls`、`reasoning`、`truncation` 等无直接 IR 字段的请求级
参数保存在 `ChatRequest.extensions` 中，不由入站适配器伪造近似语义。

未知字段按 NFR-4 容忍。无法表达的关键 item 类型不得静默丢弃，应返回
`Capability` 或 `InvalidRequest` 错误。

### 4.3 响应渲染

- assistant 文本渲染为 Responses message output item。
- `ToolCall` 渲染为 function call output item，保留 `call_id`、name 和 arguments。
- `Usage` 映射 input/output/cached/reasoning token 计数。
- `FinishReason::ToolCalls` 表示该轮等待工具结果；其他结束原因映射为
  Responses status/incomplete details。
- 响应 ID 和实际模型名由宿主 render context 传入，不使用全局常量。

### 4.4 流式渲染

流式输出保持 Responses 事件顺序：

1. `response.created`。
2. output item/content part 开始事件。
3. `response.output_text.delta` 或 function arguments delta。
4. content part/output item 完成事件。
5. usage 和 `response.completed`。

工具参数的分片按 `stream_id + call_id` 维护每个调用的独立状态。适配器逐事件
输出 delta，在 output item 完成时产生完整 function call item，不缓冲整条响应。
`Done` 或 `Error` 后必须清理所有属于该 `stream_id` 的状态。

### 4.5 Responses / IR 能力边界

现有 IR 能表达 Codex M4 主链路需要的文本、图片、function tools、usage 和错误。
以下 Responses item 目前不能完整无损表达：

- reasoning summary/content 文本本身（reasoning token 计数已可表达）。
- computer call、hosted web search 等服务端工具 item。
- 其他不是 message/function call 的新 output item。

M4 不修改 `crates/protocol`，不声称完整支持 Responses API 所有 item。对关键语义
的缺口单独提交 IR 变更请求；未经评审不改 IR。

## 5. 错误行为

| 场景 | 外部行为 |
|---|---|
| 本地 virtual key 缺失/错误 | Responses 形状的 401，不进入 adapter |
| JSON 非法或必填字段缺失 | Responses `invalid_request_error` |
| 关键 item 不可表达 | 明确 capability/invalid request，不静默丢失 |
| 上游鉴权失败 | Responses error 形状且保留 401/403 |
| 上游限流 | Responses error 形状且保留 429 |
| 上游不可用/超时 | Responses error 形状且保留 502–504 |
| 流中失败 | 产生 Responses failed/error 事件，终止流并清理状态 |

错误映射保持现有闭合 `ErrorCode` 语义，不修改 router 的重试判定。

## 6. 隔离与配置

实施和验收时必须与用户正在进行的模型厂商测试隔离：

- 不停止、重启或修改用户当前 token-station 进程。
- 不占用当前 `8787` 端口。
- 每个验收实例使用独立端口、临时配置和独立数据目录。
- Codex 使用临时 `CODEX_HOME`。
- OpenCode、OpenClaw 和 Aider 使用各自的临时配置入口。
- 不修改用户全局 Agent 配置、shell profile 或现有环境变量。
- 凭证只从启动验收进程的临时 shell 环境解析，不回显、不记录、不入库。

## 7. 验证设计

### 7.1 适配器一致性

`agent-openai-responses` 必须提供并通过五族 fixtures：

- `agent.normalize`：messages、function call/output、tools、采样参数、未知字段。
- `agent.hint`：契约内 StepType 与无 hint 场景。
- `agent.render`：文本、function call、usage、结束原因。
- `agent.stream`：文本增量、工具参数增量、usage、done、error。
- `agent.error`：非法请求、鉴权、限流、能力缺失、上游不可用。

一致性还必须证明确定性、未知字段容忍、WASM 真实加载和 Done/Error 后状态清理。

### 7.2 宿主与回归测试

- `/v1/responses` 路由注册和 method/path 传递。
- Bearer virtual key 正向和 401 负向。
- 协议不匹配时拒绝。
- 普通 JSON 与 SSE content type、背压、错误中止。
- 多个 `stream_id` 交错时不串状态。
- `/v1/messages`、`/v1/chat/completions`、`/v1/models` 不回归。
- 工作区 fmt、clippy、test 和 release build 通过。

### 7.3 假上游 E2E

先用本地可控假上游覆盖：

1. 多段流式文本。
2. function call 参数分片、工具结果回传和最终回答。
3. 401、429、5xx 和流中错误。
4. 交错流隔离和状态清理。

假上游证明协议链路的确定性，不代替真实 Agent 和真实模型验收。

### 7.4 真实 Agent / 模型 E2E

使用同一可用的真实上游基线（首选已验证的 DeepSeek 链路），分别验证
Codex、OpenCode、OpenClaw 和 Aider。每项必须保存：

- Agent 版本。
- 脱敏后的启动与配置方式。
- 普通对话证据。
- 流式证据。
- 真实工具调用及结果回传证据。
- 本地鉴权失败和上游错误证据。
- 日志不含 prompt、真实 key 或 virtual key 的审计结果。

## 8. 影响面地图

| 维度 | 文件/符号或系统 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| 契约 | `plugins/official/agent-openai-responses/**` | 新增 Responses adapter 和五族 fixtures | plugin build/test | 必须修改 |
| HTTP 入口 | `apps/cli/src/server.rs` | 新增 `POST /v1/responses` | CLI HTTP tests | 必须修改 |
| 编排 | `apps/cli/src/gateway.rs` | 为 Responses 传入协议、Agent 识别和独立 render context | gateway/unit/E2E | 必须核实最小改动 |
| WASM 宿主 | `crates/plugin-runtime/src/agent.rs` | 复用现有 ABI；仅在实际缺透传时改动 | runtime tests | 待核实 |
| 插件发现 | plugin catalog/build/package 入口 | 确认新官方 adapter 可发现与构建 | discovery/package tests | 待核实 |
| 审计边界 | `.github/CODEOWNERS` | 增加新 adapter 独立行 | CODEOWNERS diff | 必须修改 |
| 调用方 | Codex/OpenCode/OpenClaw/Aider 临时配置 | 指向隔离实例 | 真实 E2E | 必须交付 |
| 文档 | 协议盘点、四份接入指南、M4 验收报告 | 更新可执行配方与证据 | doc links/commands | 必须修改 |
| IR | `crates/protocol` | 不改；缺口走变更请求 | red-line diff | 明确不在范围 |
| 路由 | `crates/router-core` | 不改 | red-line diff | 明确不在范围 |
| 出站 | provider adapters | 不改 | red-line diff | 明确不在范围 |
| 发布 | `crates/release` | 不改 | red-line diff | 明确不在范围 |

## 9. 建议的最小实施顺序

1. 更新协议盘点并产出 Responses / IR 变更请求。
2. 用 fixtures 先锁定 Responses 支持子集和错误边界。
3. 实现 `agent-openai-responses` 非流式映射。
4. 实现流式、工具分片和状态清理。
5. 最小化接入 `/v1/responses` 宿主链路并跑全量回归。
6. 用独立端口/配置跑假上游 E2E。
7. 分别完成 Codex、OpenCode、OpenClaw、Aider 真实 E2E。
8. 产出接入指南和 M4 验收报告，审计红线、凭证与运行环境隔离。

## 10. 交付物

- `agent-openai-responses` 适配器及五族 fixtures。
- Codex、OpenCode、OpenClaw、Aider 四份接入指南。
- 更新后的首批 Agent 协议盘点。
- Responses reasoning/hosted-tool 等缺口的 IR 变更请求。
- M4 验收报告，分开记录一致性、假上游、真实 E2E 和已知限制。

## 11. 红线与非目标

本轮明确不做：

- 不修改 `crates/router-core`。
- 不修改 `crates/protocol`；IR 调整只能经变更请求和负责人评审。
- 不修改 `crates/release`。
- 不修改 provider adapter 或路由策略。
- 不实现单进程同时加载多个 Agent adapter。
- 不为 Agent 或模型厂商增加专属路由分支。
- 不声称完整支持 Responses API 所有 item。
- 不修改用户全局配置、当前运行服务或模型厂商测试环境。
- 不将 API key、virtual key、prompt 或未脱敏运行数据写入仓库。
- 不直接向 `main` 提交；所有交付经独立分支和 Pull Request 评审。

## 12. 验收结论口径

最终状态必须分开陈述：

- `agent-openai-responses` 一致性是否通过。
- 宿主回归是否通过。
- 假上游 E2E 是否通过。
- 每个真实 Agent / 真实模型 E2E 是否通过。
- Responses / IR 限制仍有哪些。
- 本地实现与验证完成、PR-ready、负责人批准/合并分别处于什么状态。
