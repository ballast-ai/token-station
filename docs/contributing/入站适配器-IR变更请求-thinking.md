# IR 变更请求：Anthropic thinking 内容块

- 状态：待项目负责人评审
- 提出日期：2026-07-15
- 相关适配器：`agent-anthropic`
- 受影响需求：FR-2、NFR-1、NFR-5、§9

## 1. 请求结论

当前 Canonical IR 不能无损表达 Anthropic Messages 的
`thinking` / `redacted_thinking` content block 及其流式增量。
本轮不修改 `crates/protocol`，请项目负责人决定是否扩展 IR。

## 2. 原协议语义

Anthropic Messages 中与 thinking 相关的关键语义包括：

- 请求顶层 `thinking`，可表示 enabled/adaptive/disabled 等模式及
  `budget_tokens`、`display`等控制项。
- 响应 `thinking` block 包含可见 thinking 文本与 `signature`。
- `redacted_thinking` block 保留不可展示但必须原样回传的思考数据。
- 流式响应可出现 `thinking_delta` 和 `signature_delta`。
- 在工具循环中，最近 assistant turn 的 thinking / redacted thinking
  blocks 必须保持完整、顺序不变且原样回传，否则 Anthropic 可返回 400。

证据：[Anthropic Extended thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)。

## 3. 当前 IR 缺口

| IR 位置 | 当前形状 | 缺失语义 |
|---|---|---|
| `ContentPart` | 只有 `Text` / `ImageUrl` | 无 thinking、redacted thinking 和 signature 的有类型表示 |
| `Message` | content + tool calls | 无法保留 content block 间的 thinking/tool/text 原始顺序 |
| `ChatRequest` | 采样、tools、extensions | 无协议中立的 thinking 模式/预算/展示策略 |
| `ChatResponse` | choices/message/usage | 无法将上游 reasoning 作为可往返的内容块返回 |
| `StreamEvent` | text/tool/usage/done/error | 无 thinking/signature 增量事件 |

`Extensions` 能临时保存未知 JSON，但 provider adapter 和 stream
契约没有通用的读写规则，不足以作为“无损支持”的契约。

## 4. 建议的评审方向

建议由 IR 对接人在协议中立性下评审，不直接引入 Anthropic 命名：

1. 为 message content 增加可往返的 reasoning / opaque reasoning block，
   保留原始顺序、可见摘要与不透明 continuation data。
2. 增加协议中立的 reasoning control，区分 mode、budget 与 visibility。
3. 为 `StreamEvent` 增加 reasoning delta / opaque continuation delta，
   并定义 provider 不支持时的 capability 失败语义。
4. 明确哪些字段可进入日志、哪些必须按不透明数据处理，
   避免将 signature / redacted data 误当普通文本。

此处只是形状建议，不在本分支实现。最终命名、版本策略和能力
降级规则由 IR 对接人决定。

## 5. 本轮临时边界

在 IR 变更未获批前：

- Claude Code 接入配方显式设置
  `CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1` 和
  `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`。
- 入站出现 `thinking` / `redacted_thinking` block 时，适配器返回明确的
  capability/invalid request 错误，不静默丢弃、不伪造文本映射。
- 不声称 thinking 语义已无损支持。
- DeepSeek E2E 仅验证文本、工具、流式与错误闭环，不将
  Anthropic thinking 列为通过项。

## 6. 需要负责人回复的问题

1. 是否接受为 IR 增加协议中立 reasoning block/control/event？
2. opaque continuation data 是否允许跨 provider 路由，还是必须粘滞回原 provider/model？
3. IR 版本如何升级，且旧 provider adapter 不支持时应返回哪个
   `ErrorCode` / capability 错误？
