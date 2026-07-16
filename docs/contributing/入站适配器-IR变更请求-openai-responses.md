# OpenAI Responses 入站适配器 IR 变更请求

- 日期：2026-07-16
- 关联范围：FR-4、FR-7、M4
- 当前结论：本轮不修改 `crates/protocol`

## 1. 已能无损表达的 M4 子集

现有 Canonical IR 足以承载 Codex 主链路：

| Responses 语义 | 当前 IR |
|---|---|
| instructions / message | `Message` + `Role` |
| input_text / output_text | `Content` / `ContentPart::Text` |
| input_image URL/data URL | `ContentPart::ImageUrl` |
| function tool | `ToolDef` |
| function_call | `ToolCall` |
| function_call_output | `Role::Tool` + `tool_call_id` |
| temperature / top_p / max_output_tokens | `Sampling` |
| usage / cached / reasoning token 计数 | `Usage` |
| 流式文本 / 工具参数 | `StreamEvent` |
| 标准错误 | `ErrorEnvelope` |

因此 M4 可以在不修改 IR 的前提下完成普通对话、流式、function 工具闭环、
usage 和错误回传。

## 2. 当前不能无损表达

以下 Responses 语义没有稳定的一一对应表示：

- reasoning output item 的 summary、content、encrypted_content；
- computer call / computer call output；
- hosted web search、file search、image generation 等服务端工具 item；
- custom tool、namespace tool、tool search 等非 function 工具；
- input_file / file_id 图片；
- response item phase、服务端状态及其他 Responses 专属生命周期元数据。

这些类型进入 M4 adapter 时必须返回 `Capability` 或 `InvalidRequest`，不能丢弃后继续
路由，也不能塞进普通文本伪装成等价语义。

## 3. 后续候选契约

若负责人决定支持上述能力，建议先评审以下方向，而不是直接修改 v1：

1. 为输入/输出 item 增加可扩展的类型化联合，而非继续把所有语义放入 Message。
2. 将 client-executed、server-executed 工具及其结果分开建模。
3. 为 reasoning 的可见摘要、原始内容、加密内容建立明确的数据保留和隐私策略。
4. 需要破坏现有闭合枚举时发布新 ABI/IR 版本，不原地改变 v1。

## 4. 本轮门禁

- `crates/protocol` 必须保持零差异。
- adapter 对未知普通字段保持前向兼容。
- adapter 对未知关键 item/tool 类型明确失败。
- 验收报告只能声称支持本文件第 1 节子集，不声称完整兼容 Responses API。
