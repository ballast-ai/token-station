# Claude Code → token-station → 四家 OpenAI-compatible 供应商 E2E 验收

- 验收日期：2026-07-16
- 验收分支：`feat/claude-code-anthropic-adapter`
- Claude Code：`2.1.170`
- token-station CLI：`0.2.0`
- 真实上游：Qwen `qwen3.7-max`、Kimi `kimi-k2.7-code`、MiniMax
  `MiniMax-M3`、GLM `glm-5.2`
- 替代模型复测：Kimi `moonshot-v1-128k`、MiniMax `MiniMax-M2.5`
- 凭证边界：四家上游 Key 仅保存于 macOS 钥匙串；Claude Code 只获取各测试配置
  单独生成的本地 virtual key

## 结论

四家均通过真实上游最小探测，说明当前 Base URL、模型 ID 和 Key 可用；但不能据此
宣称四家 Claude Code 全链路均完整兼容。

| 供应商 | 普通流式对话 | `Read` 工具闭环 | 本地鉴权边界 | 验收结论 |
| --- | --- | --- | --- | --- |
| Qwen / `qwen3.7-max` | 通过，精确返回固定标记 | 通过，最终得到 `MAX_WIDTH=100` | 通过 | 当前范围通过 |
| Kimi / `kimi-k2.7-code` | 通过，精确返回固定标记 | 未通过；工具结果回传请求被上游以 400 拒绝 | 通过 | 仅文本兼容，不能宣称 Agent 工具兼容 |
| Kimi / `moonshot-v1-128k` | 通过，精确返回固定标记 | 通过，最终得到 `MAX_WIDTH=100` | 通过 | 当前范围通过 |
| MiniMax / `MiniMax-M3` | 通过路由并包含固定标记，但额外暴露 `<think>` 文本 | 通过，最终包含 `MAX_WIDTH=100` | 通过 | 功能闭环通过，思考内容分层未兼容 |
| MiniMax / `MiniMax-M2.5` | 通过路由并包含固定标记，但额外暴露 `<think>` 文本 | 通过，最终包含 `MAX_WIDTH=100` | 通过 | 换模型未消除思考内容分层问题 |
| GLM / `glm-5.2` | 通过，精确返回固定标记 | 通过，最终得到 `MAX_WIDTH=100` | 通过 | 当前范围通过 |

本次没有修改 router、Canonical IR 或 provider 实现来掩盖差异。Kimi
`kimi-k2.7-code` 的缺口需要单独提交 IR 变更请求；MiniMax 两个被测模型的
`<think>` 分层也属于同一类协议语义问题。Kimi `moonshot-v1-128k` 的通过证明
Kimi 厂商和现有通用 provider 可以配置接入，K2.7 的失败是模型协议语义差异。

## 真实上游探测

使用 release CLI 对每个配置执行一次真实 `upstream test`，结果如下：

```text
qwen3.7-max: ok (6653 ms)
kimi-k2.7-code: ok (432 ms)
MiniMax-M3: ok (1516 ms)
glm-5.2: ok (2371 ms)
moonshot-v1-128k: ok (3996 ms)
MiniMax-M2.5: ok (2012 ms)
```

探测不写入服务 metrics；它只证明单轮 OpenAI-compatible completion 可达，不证明
Claude Code 工具循环兼容。

## Claude Code 全链路证据

四家均逐个独占 `127.0.0.1:8787`，使用独立数据目录。Claude Code 统一使用协议
兼容标识 `claude-3-5-haiku-20241022`，并通过 `--safe-mode`、
`--setting-sources project`、关闭 adaptive thinking 和 experimental betas 隔离用户
配置。实际模型由各单供应商 pool 决定。

### Qwen

- 普通流式请求精确返回固定标记。
- 工具场景成功生成 `Read`，读取 `rustfmt.toml` 后精确返回 `MAX_WIDTH=100`。
- metrics 共 4 条，全部满足 `protocol=anthropic-messages`、`status=200`、
  `upstream=qwen`、`model=qwen3.7-max`；工具场景记录 `tool_count=1`。

### Kimi

- 普通流式请求精确返回固定标记，对应 metrics HTTP 200。
- 工具第一轮成功生成 `Read`，对应 metrics HTTP 200、`tool_count=1`。
- Claude Code 执行工具后，第二轮携带 `tool_result` 回传；对应 metrics 为
  `upstream=kimi`、`model=kimi-k2.7-code`、HTTP 400、
  `error_code=invalidrequest`（文件日志中的协议枚举为 `invalid_request`）。
- [Kimi K2.7 Code 官方文档](https://platform.moonshot.cn/docs/guide/kimi-k2-7-code-quickstart)
  明确要求多步工具调用保留 assistant message 的 `reasoning_content`；
  [思考模式文档](https://platform.moonshot.cn/docs/guide/use-kimi-k2-thinking-model)
  进一步说明该模型始终输出并要求在多轮上下文中原样保留该字段。
- 当前 Canonical IR 与 `provider-openai-compatible` 不承载该字段，因此这是已证实的
  协议语义缺口，不是 Key、路由或模型名错误。

#### Kimi 替代模型：`moonshot-v1-128k`

- 使用独立配置 `apps/cli/claude-code-kimi-moonshot-v1-config.json` 和独立数据目录，
  没有覆盖 `kimi-k2.7-code` 的配置或 metrics。
- 真实 probe 通过，耗时 3996 ms。
- 普通流式请求精确返回固定标记；输出不含 `<think>`。
- 工具第一轮成功生成 `Read`，工具结果回传第二轮也成功，最终精确返回
  `MAX_WIDTH=100`。
- metrics 共 3 条，全部为 `status=200`、`upstream=kimi`、
  `model=moonshot-v1-128k`；工具循环两条记录均为 `tool_count=1`。
- 结论：在不扩展 IR 的前提下，该模型可以完成当前 Claude Code 文本与工具闭环。

### MiniMax

- 普通流式请求命中 `minimax/MiniMax-M3` 并包含固定标记。
- 模型把 `<think>...</think>` 放入普通 `content`；Claude Code 最终可见文本因此
  不是精确标记。
- 工具第一轮与工具结果回传第二轮均为 HTTP 200，最终文本包含
  `MAX_WIDTH=100`；两条记录均为 `tool_count=1`。
- metrics 共 4 条，全部为 `status=200`、`upstream=minimax`、
  `model=MiniMax-M3`。

#### MiniMax 替代模型：`MiniMax-M2.5`

- 使用独立配置 `apps/cli/claude-code-minimax-m2.5-config.json` 和独立数据目录，
  没有覆盖 `MiniMax-M3` 的配置或 metrics。
- 真实 probe 通过，耗时 2012 ms。
- 普通流式请求命中正确模型并包含固定标记，但仍包含完整
  `<think>...</think>`，不是精确标记。
- 工具第一轮与工具结果回传第二轮均为 HTTP 200；最终文本包含
  `MAX_WIDTH=100`，同时仍包含 `<think>...</think>`。
- metrics 共 3 条，全部为 `status=200`、`upstream=minimax`、
  `model=MiniMax-M2.5`；工具循环两条记录均为 `tool_count=1`。
- 结论：换到 M2.5 没有解决思考内容分层问题；其功能闭环与 M3 相同，但上下文窗口
  更小，因此本次证据不支持用 M2.5 替换 M3 作为默认测试模型。

### GLM

- 普通流式请求精确返回固定标记。
- 工具场景成功生成 `Read`，读取 `rustfmt.toml` 后精确返回 `MAX_WIDTH=100`。
- metrics 共 3 条，全部为 `status=200`、`upstream=glm`、
  `model=glm-5.2`；工具循环两条记录均为 `tool_count=1`。

## 鉴权、日志与敏感信息证据

- 六个配置分别使用错误本地 Bearer 发起原始 Anthropic Messages 请求，均返回
  HTTP 401 和 `authentication_error`；请求前后各自 metrics 行数不变，证明错误
  virtual key 没有进入路由或触达上游。
- 停止全部服务并刷盘后，对 6 个 `requests.log` 和 6 个 `metrics.sqlite` 扫描：
  四家上游 Key、六个本地 virtual key、错误测试 key、固定标记、工具提示、文件路径
  和最终答案均无匹配。
- 六个本地 `virtual-key` 文件权限均为 `0600`。
- 测试完成时 `127.0.0.1:8787` 无监听进程。

## 覆盖边界

本次真实 E2E 覆盖：配置解析、真实单轮 completion、Anthropic Messages 入站、
OpenAI-compatible 出站、SSE、Claude Code `Read` 工具调用、工具结果回传、本地鉴权、
路由 metrics 和内容零日志。

未覆盖或未宣称兼容：`reasoning_content`、`reasoning_details`、Anthropic
`thinking` / `redacted_thinking` 的无损转换、视觉输入、JSON Schema、并行工具调用、
长上下文、限流恢复和厂商全部 beta 参数。
