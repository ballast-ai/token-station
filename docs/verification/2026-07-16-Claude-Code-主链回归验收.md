# Claude Code 主链回归验收

- 验收时间：2026-07-16 15:43（Asia/Shanghai）
- 验收提交：`9903e85`
- Claude Code：`2.1.170`
- 入站协议：Anthropic Messages
- 真实上游：DeepSeek `deepseek-v4-flash`
- 隔离实例：`127.0.0.1:8895`

## 结论

真实 Claude Code 主链回归通过。Claude Code 使用真实模型 ID
`deepseek-v4-flash`，经 `agent-anthropic`、Token Station 路由和
`provider-openai-compatible` 到达真实 DeepSeek；普通对话、逐事件流式、`Read`
工具闭环和本地鉴权错误回传均符合 M3 与逐目标 Agent 验收标准。

这是 Token Station 入站适配工作的主验收。Codex、OpenCode、OpenClaw 属 M4 扩展
验收；Codex 的 Responses `output_index` 测试只证明 Responses 专项修复，不能替代
本页的 Claude Code 主链证据。

本结论仅表示当前分支本地实现与验证完成，不表示已 push、已开 PR、已获负责人
批准、已合并或已发布；同步主分支后的 PR-ready 状态需要另行重新验证。

## 隔离与凭证边界

- 验收使用全新 `/tmp/token-station-claude-primary-20260716` 数据、插件和工作目录。
- 用户原有 `127.0.0.1:8787` 服务在验收前后均由 PID `23173` 监听，没有停止、
  重启或改写。
- DeepSeek Key 只存在于 Token Station 服务进程环境；Claude Code 进程通过
  `env -u DEEPSEEK_API_KEY` 明确移除该变量，只获得隔离实例的本地 virtual key。
- 首次启动输出中的临时 virtual key 被立即作废；实际验收换用全新数据目录生成的
  新 key，启动输出经过遮蔽。
- 实际验收 virtual key 文件权限为 `0600`。

## 真实普通对话与流式

Claude Code 使用以下非交互参数发起真实请求，并要求只返回固定标记
`CLAUDE_PRIMARY_OK`：

```text
--print --output-format stream-json --include-partial-messages --verbose
```

结果：

- Claude Code 退出码为 0，最终结果包含精确固定标记；
- 输出共 18 行事件，其中 14 行是 `stream_event`；
- 事件包含 `message_start`、`content_block_start`、7 个 `text_delta`、
  `content_block_stop`、`message_delta` 和 `message_stop`；
- Token Station 只新增 1 条请求记录：

```text
protocol=anthropic-messages
requested_model=deepseek-v4-flash
stream=1
status=200
attempts=1
upstream=deepseek
model=deepseek-v4-flash
```

这同时证明客户端模型 ID、Token Station `requested_model` 和实际上游模型三层均为
真实的 `deepseek-v4-flash`，没有用 Claude 兼容假名掩盖路由结果。

## 真实 Read 工具闭环

隔离工作区创建 marker 文件，提示 Claude Code 必须用 `Read` 读取绝对路径，成功后
只回答 `CLAUDE_PRIMARY_TOOL_OK`。客户端只开放 `Read` 工具。

实际事件链：

1. 第一轮请求经 Token Station 到达 DeepSeek；
2. DeepSeek 返回 `tool_use`，名称为 `Read`，参数中的 `file_path` 精确指向隔离
   marker 文件；
3. Claude Code 实际执行 `Read` 并生成非错误 `tool_result`；
4. 第二轮请求携带该 `tool_result` 再次经过 Token Station 到达 DeepSeek；
5. Claude Code 最终返回精确标记 `CLAUDE_PRIMARY_TOOL_OK`，退出码为 0。

两轮 metrics 均为 `protocol=anthropic-messages`、`stream=1`、`status=200`、
`attempts=1`、`upstream=deepseek`、`model=deepseek-v4-flash`。请求的 message 数从
2 增至 4，证明第二轮确实带回工具结果，而不是客户端本地伪造最终文本。

## 真实错误回传

另起真实 Claude Code `--print` 请求并注入错误本地 key：

- Claude Code 连续产生 `api_retry` 事件，`error_status=401`、
  `error=authentication_failed`；
- 观察到第 1 至第 5 次重试后人工终止，避免等待完整退避；
- Token Station metrics 在请求前后均为 3 条，没有新增路由记录；
- 因此错误请求在本地鉴权层被拒绝，没有进入 router 或真实 DeepSeek；
- 中止后的 Claude Code result 为 error，使用量为 0。

该测试证明真实客户端能收到 Anthropic 风格 401 并按自身策略重试。metrics 不记录
鉴权前拒绝是当前实现边界，不应把它误报为上游错误。

## 内容、凭证和红线审计

对隔离实例的 `requests.log` 和 `metrics.sqlite` 进行定值扫描，均未检出：

- DeepSeek API Key；
- 实际 virtual key；
- 错误本地 key；
- 普通对话固定标记；
- 工具最终固定标记；
- marker 文件内容。

相对 `origin/main...HEAD` 的红线 diff 计数：

```text
crates/router-core                                      0
crates/protocol                                         0
crates/release                                          0
plugins/official/provider-openai-compatible/src         0
```

本轮回归没有修改出站 provider、路由策略、共享 IR、发布信任链、用户全局 Claude Code
配置或用户原有 8787 服务。

## 覆盖边界

- 本页覆盖 Claude Code 普通文本、Anthropic SSE、`Read` 工具、工具结果回传和本地
  401；不宣称 Anthropic Messages 所有 beta 能力均兼容。
- `thinking` / `redacted_thinking` 仍未进入 Canonical IR；本次使用
  `CLAUDE_CODE_DISABLE_THINKING=1`，没有越过 IR 变更红线。
- `/v1/messages/count_tokens` 尚未实现，Claude Code 继续使用本地估算。
- M4 三个扩展 Agent 的证据见
  [M4 多 Agent 入站验收](2026-07-16-M4-多-Agent-入站验收.md)。
