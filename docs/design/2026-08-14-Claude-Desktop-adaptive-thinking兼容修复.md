# Claude Desktop adaptive thinking 兼容修复

## 问题

Claude Desktop 当前会在 Anthropic Messages 请求中发送
`thinking: {"type":"adaptive"}`，并通过 `output_config.effort` 表达推理强度。Token Station
的 Anthropic 入站适配器在路由前拒绝所有非 `disabled` 的 thinking 类型，因此请求以 400
`Anthropic thinking type adaptive is not supported by the configured provider` 结束，调用次数为 0，
实际上尚未访问上游。

当前用户路由到 OpenAI-compatible DeepSeek。该翻译路径已有两项基础能力：入站适配器可以把
原始 thinking 保存到 Canonical IR 扩展，并把 `output_config.effort` 映射为
`reasoning_effort`；Provider 适配器也能读取该扩展。但现有入站校验提前阻断请求，而 Provider
在模型未声明参数能力时会乐观发送 `reasoning_effort`，仍可能令不支持该 OpenAI 参数的上游
返回 400。

## 目标

1. 接受 Claude 当前合法的 `adaptive`、`enabled` 与 `disabled` thinking 类型；未知类型和畸形结构
   仍返回明确的 400。
2. 原生 Anthropic 路由继续原样透传 thinking，不改变其语义。
3. OpenAI-compatible 翻译路由保留 Anthropic thinking 元数据；只有目标模型明确声明支持
   `reasoning_effort` 时，才把 Claude 的 thinking effort 发给上游。
4. 对未声明该能力的 DeepSeek 等模型执行兼容降级：省略不兼容控制字段，但请求继续完成，响应中
   若有 `reasoning_content` 仍按现有逻辑转换为 Anthropic thinking block。
5. 用插件级和网关级公开行为测试覆盖用户截图中的请求形状。

## 范围与非目标

本次只修复协议兼容和能力门控，不把共享 DeepSeek Provider 改为 Anthropic-native endpoint。
共享 Provider 还服务 Codex、OpenCode 等 OpenAI 协议 Agent，直接把其 Base URL 改为
`/anthropic/v1` 会破坏这些客户端。

翻译路径无法承诺与 Anthropic 原生 thinking 完全等价。未明确支持 `reasoning_effort` 的上游会
忽略 Claude 请求的 effort 偏好，这是有意的兼容降级；用户若需要完整语义，应配置独立的
Anthropic-native 上游。本次不自动复制 Provider、迁移密钥或修改用户路由。

## 安全与数据红线

- 不记录或展示 Claude 会话正文、system prompt、虚拟 Key 或 Provider Key。
- 不修改用户 Provider、路由、凭据和 Claude Desktop 接入配置。
- 能力未知时采用保守省略，不能把未经声明的推理参数试探性发往上游。
- 未知 thinking 类型继续失败关闭，避免静默吞掉未来新增且语义不明的协议字段。

## 用户可见行为与失败处理

- 用户在 Claude Desktop 发送带 `thinking.type=adaptive` 的普通消息时，不再看到本地 400。
- 如果当前上游未声明 `reasoning_effort`，Token Station 省略该控制字段并正常转发其余请求。
- 如果模型明确声明支持 `reasoning_effort`，`low`、`medium`、`high` 原值转发，`max` 按现有规则
  限制为 `high`。
- 上游真实的鉴权、额度、限流、网络或协议错误继续按现有错误映射返回，不被本修复掩盖。

## 响应式、键盘与可访问性

本次没有 UI 结构、布局、焦点顺序或键盘交互变化。错误消失后沿用 Claude Desktop 自身的流式
消息呈现。

## 测试与验收

1. Plugin runtime 测试断言 `adaptive` 与 `enabled` 可归一化，并保留原始 thinking 和映射后的
   effort；未知 thinking 类型仍为 capability error。
2. Provider 测试断言带 Anthropic thinking 的请求：模型未声明能力时省略
   `reasoning_effort`，明确声明时才发送。
3. CLI 网关测试发送与 Claude Desktop 相同形状的 `adaptive + output_config.effort` 请求，断言
   请求到达模拟 OpenAI-compatible 上游且不含 `thinking`、`anthropic_thinking` 或
   `reasoning_effort`。
4. 运行相关插件测试、CLI/桌面 Rust 全量测试、前端全量测试与正式构建。
5. 重新执行 `scripts/install-local-desktop.sh`，用真实本地网关发送最小 adaptive 请求，确认不再
   返回截图中的本地 400。

## 实现落点

- `plugins/official/agent-anthropic/src/lib.rs`
- `plugins/official/provider-openai-compatible/src/lib.rs`
- `crates/plugin-runtime/tests/official_plugins.rs`
- `apps/cli/tests/proxy.rs`

## 实现状态

已实现并完成本机验收：

- Anthropic 入站适配器接受 `adaptive`、`enabled` 与 `disabled`，同时保留原始 thinking 和 effort
  兼容元数据；未知类型仍失败关闭。
- OpenAI-compatible Provider 仅在目标模型明确声明 `reasoning_effort` 时转发 Claude
  adaptive/enabled 的推理强度；原生 OpenAI 请求维持既有的未声明能力乐观行为。
- 插件级测试已先在旧实现上分别复现本地 400 和未声明参数被错误发送，再由修复转绿；CLI
  网关测试确认 adaptive 请求到达模拟上游且不泄漏适配器扩展或不兼容参数。
- Rust workspace 全量测试通过；其中 CLI 网关集成 71 passed、1 ignored，官方插件真实 WASM
  28 passed。桌面 Rust 全量测试 351 passed、2 ignored；前端 397 passed，正式构建通过。
- `scripts/install-local-desktop.sh` 已完成隔离构建、插件装载门禁、bundle 签名、产物审计、事务性
  替换与启动健康检查。
- 使用当前 Claude Desktop profile 和当前 DeepSeek 路由执行真实本地请求：非流式 adaptive 请求
  返回 200 message；流式请求返回 200，并完整包含 `message_start` 与 `message_stop`。两条收据均为
  `attempts=1`、`error_code=null`，证明请求已越过此前 `attempts=0` 的本地拒绝点。
