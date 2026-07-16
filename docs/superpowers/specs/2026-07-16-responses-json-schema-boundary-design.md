# OpenAI Responses JSON Schema 能力边界修正设计

## 1. 背景与目标

M4 的 `agent-openai-responses` 当前在 manifest 中声明 `json_schema`，但
`normalize_inbound` 没有把 Responses `text.format` 转换为 Canonical IR 的
`ChatRequest.response_format`，下游 provider 也没有发送该约束。继续保留声明会让调用方
误以为结构化输出已生效，实际却按普通文本请求处理。

本修正不实现 JSON Schema，而是把能力边界改为真实、可观察的行为：普通对话、流式文本、
function tool 和错误链保持不变；结构化输出请求明确失败，不得静默丢失。

## 2. 方案选择

| 方案 | 结果 | 决策 |
|---|---|---|
| 完整实现 Responses JSON Schema 到 provider 的端到端转换 | 功能完整，但需要修改 IR/provider 红线并补充更大范围测试 | 本轮不采用 |
| 移除能力声明并明确拒绝结构化输出 | 不影响 M4 已验证主链，且能力边界真实 | 采用 |
| 保留声明并继续把字段放入 extensions | 请求看似成功但约束丢失，违反无损原则 | 禁止 |

## 3. 行为设计

### 3.1 Manifest

从 `agent-openai-responses` 的 `capabilities` 中移除 `json_schema`。该 manifest 只声明
适配器实际承诺转换的 `chat`、`stream`、`tool_call` 和 `agent_hint`。

provider 和模型配置不在本次修正范围内，不作改动。

### 3.2 请求归一

在生成 `ChatRequest` 前检查 Responses 请求的 `text.format`：

- `text` 或 `text.format` 缺失、为 `null`：沿用当前普通文本路径。
- `text.format.type = "text"`：视为普通文本格式，允许继续处理。
- `text.format.type = "json_schema"` 或 `"json_object"`：返回
  `ErrorCode::Capability`，HTTP 状态保持当前 adapter 的 capability 错误约定。
- `text.format` 形状非法或缺少字符串 `type`：返回 `InvalidRequest`。
- 未识别的 format type：返回 `Capability`，防止未来关键语义被静默忽略。

错误信息必须指出 Responses 结构化输出尚未获批准进入 Canonical IR/provider 链路，不能
把请求降级成普通文本。

### 3.3 不变范围

以下行为不得因本修正改变：

- Codex 普通对话、流式文本和 function tool 闭环。
- OpenCode、OpenClaw 的 Chat Completions 接入路径。
- `agent-openai-responses` 对现有图片 URL、usage、错误和 Agent hint 的处理。
- Router、Protocol、Provider、release 及路由策略。
- 当前工作区中的 Anthropic 并行修改。

## 4. 测试与文档

增加 adapter Rust 单元测试，覆盖 `text`、`json_schema`、`json_object`、非法形状和
未知 format type；增加 CLI proxy 测试，断言 JSON Schema 请求在归一阶段返回
Responses 形状的 `unsupported_capability`，且不会进入 router/provider。

现有 `agent.error` fixture 族继续只验证 `map_inbound_error`，不把归一阶段失败错误地塞入
该 fixture 家族。五族 conformance fixtures 全部重跑，证明本修正没有破坏既有契约。

更新 Codex 接入指南和 Responses IR 边界文档，明确结构化输出暂不支持且会显式失败。

验收至少包括：

1. Responses adapter fmt、clippy、WASM build 和 plugin conformance。
2. 现有 normalize、render、stream、error fixtures 不回归。
3. 官方插件一致性与 Responses runtime/proxy 针对性测试通过。
4. red-line diff 仍为零，Anthropic 并行修改未被纳入本次变更。

## 5. 完成标准

- manifest 不再声称 `json_schema`。
- JSON Schema/JSON Object 请求不会进入 router 或 provider。
- 调用方收到明确的 capability 错误，而不是普通文本响应。
- M4 已验证的文本、流式和 function tool 主链保持通过。
- 不修改任何红线目录。
