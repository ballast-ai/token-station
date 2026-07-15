# Claude Code → token-station → DeepSeek E2E 验收

- 验收日期：2026-07-15
- 验收分支：`feat/claude-code-anthropic-adapter`
- Claude Code：`2.1.170`
- 真实上游：DeepSeek `deepseek-v4-flash`
- 凭证边界：DeepSeek Key 只注入 token-station 服务进程；Claude Code 进程通过
  `env -u DEEPSEEK_API_KEY` 明确移除该变量，只获取本地 virtual key

## 结论

本地实现与验证完成：Claude Code 的 Anthropic Messages 请求能够经过
`agent-anthropic`、Canonical IR、现有路由和
`provider-openai-compatible` 到达真实 DeepSeek；流式文本与一次完整工具调用闭环
通过。尚未 push，也不表示负责人已批准或合并。

## 验收证据

### 插件与配置

- release CLI 成功构建。
- `agent-anthropic` 通过 `agent-protocol-v1` 全部 22 项检查并以 verified 状态
  安装。
- `provider-openai-compatible` 通过 `provider-protocol-v1` 并以 verified 状态
  安装。
- `apps/cli/claude-code-deepseek-config.json` 能被 `upstream list` 和
  `rule list` 正确解析；路由目标为
  `deepseek/deepseek-v4-flash`。

### 真实普通对话与流式

Claude Code 使用协议兼容模型标识 `claude-3-5-haiku-20241022` 发起请求，
token-station metrics 记录：

```text
protocol=anthropic-messages
requested_model=claude-3-5-haiku-20241022
stream=1
status=200
upstream=deepseek
model=deepseek-v4-flash
```

Claude Code 最终得到 `CLAUDE_VIA_TOKEN_STATION_OK`。兼容标识只控制 Claude Code
是否附加 Anthropic thinking 字段；真实模型由 router pool 决定。

### 真实工具调用闭环

要求 Claude Code 先用 `Read` 读取 `rustfmt.toml`，再回答 `max_width`：

1. DeepSeek 通过流式 `tool_use` 生成 `Read` 及文件路径。
2. Claude Code 执行 `Read`，生成非错误 `tool_result`。
3. 第二轮请求携带工具结果再次经过 token-station 到达 DeepSeek。
4. 最终回答为 `MAX_WIDTH=100`。

metrics 新增两条 `stream=1 / status=200 / tool_count=1` 记录，二者实际
upstream/model 均为 `deepseek/deepseek-v4-flash`；最终文本由 6 个 text delta
增量组成。

### 错误与 thinking 边界

- 原始 Anthropic Messages 请求携带错误本地 Bearer 时返回 HTTP 401：
  `authentication_error / missing or invalid local virtual key`。
- Claude Code 对该 401 会自动重试；连续重试期间 metrics 行数保持不变，证明请求
  未进入 Gateway 路由或 DeepSeek。两分钟后人工终止客户端重试。
- Claude Code 直接使用自定义模型名 `deepseek-v4-flash` 时会发送
  `thinking: adaptive`。适配器按既定 IR 红线返回 HTTP 400 capability，而不是
  静默丢字段；对应记录 `attempts=0`，未触达上游。
- mock 全链路测试另外覆盖了本地 401、协议不匹配和上游 401 的 Anthropic 错误
  渲染，不用真实 Key 制造上游鉴权失败。

### 内容与密钥零日志

停止服务并刷盘后，对 5 条文件日志和 5 条 SQLite metrics 记录逐项扫描，以下内容
均不存在：

- `DEEPSEEK_API_KEY` 的实际值；
- 本地 virtual key 的实际值；
- 错误测试 key；
- E2E prompt；
- `Read` 工具结果；
- 最终回答文本。

日志只保存协议、路由、请求特征、状态、延迟与 token 用量。virtual key 文件权限
实测为 `0600`。

## 已知边界

- `thinking` / `redacted_thinking` 等语义需要经过 IR 变更请求评审，本分支没有修改
  `crates/protocol`。
- `/v1/messages/count_tokens` 尚未实现，Claude Code 使用本地估算。
- Claude Code 展示的 `modelUsage` 以兼容标识估算，实际上游与用量以
  token-station metrics 和 DeepSeek 账单为准。
- 本次真实 E2E 覆盖文本、SSE、工具调用、工具结果回传、本地鉴权与 capability
  错误；未宣称 Anthropic Messages 所有 beta 特性均兼容。
