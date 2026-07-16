# M4 OpenAI 入站 JSON Schema 能力边界修正设计

## 1. 背景与目标

M4 的 `agent-openai-responses` 和 `agent-openai` 当前都在 manifest 中声明
`json_schema`，但各自的 `normalize_inbound` 分别没有把 Responses `text.format` 和
Chat Completions `response_format` 转换为 Canonical IR 的
`ChatRequest.response_format`，下游 provider 也没有发送该约束。继续保留声明会让调用方
误以为结构化输出已生效，实际却按普通文本请求处理。

本修正不实现 JSON Schema，而是把能力边界改为真实、可观察的行为：普通对话、流式文本、
function tool 和错误链保持不变；结构化输出请求明确失败，不得静默丢失。

## 2. 方案选择

| 方案 | 结果 | 决策 |
|---|---|---|
| 完整实现两类 OpenAI 入站协议到 provider 的端到端转换 | 功能完整，但需要修改 IR/provider 红线并补充更大范围测试 | 本轮不采用 |
| 移除能力声明并明确拒绝结构化输出 | 不影响 M4 已验证主链，且能力边界真实 | 采用 |
| 保留声明并继续把字段放入 extensions | 请求看似成功但约束丢失，违反无损原则 | 禁止 |

## 3. 行为设计

### 3.1 Manifest

从 `agent-openai-responses` 和 `agent-openai` 的 `capabilities` 中移除
`json_schema`。两个 manifest 只声明适配器实际承诺转换的 `chat`、`stream`、
`tool_call` 和 `agent_hint`。

三份 M4 配置中的模型能力 `json_schema` 显式设为 `false`。provider manifest 和
provider 实现属于红线，只记录现状，不作改动。

### 3.2 请求归一

在生成 `ChatRequest` 前分别检查：

- Responses 请求的 `text.format`。
- Chat Completions 请求的 `response_format`。

两类字段遵循相同规则：

- 字段缺失或为 `null`：沿用当前普通文本路径。
- `type = "text"`：视为普通文本格式，允许继续处理。
- `type = "json_schema"` 或 `"json_object"`：返回
  `ErrorCode::Capability`，HTTP 状态保持当前 adapter 的 capability 错误约定。
- 对象形状非法或缺少字符串 `type`：返回 `InvalidRequest`。
- 未识别的 format type：返回 `Capability`，防止未来关键语义被静默忽略。

错误信息必须指出结构化输出尚未获批准进入 Canonical IR/provider 链路，不能
把请求降级成普通文本。

### 3.3 不变范围

以下行为不得因本修正改变：

- Codex 普通对话、流式文本和 function tool 闭环。
- OpenCode、OpenClaw 的普通对话、流式文本和 function tool 闭环。
- 两个 adapter 对现有消息、工具、usage、错误和 Agent hint 的处理。
- Router、Protocol、Provider、release 及路由策略。
- 当前工作区中的 Anthropic 并行修改。

## 4. 测试与文档

在 plugin-runtime 的 official plugin 测试中加载两个真实 WASM，覆盖 `text`、
`json_schema`、`json_object`、非法形状和未知 format type；增加两类 CLI proxy
测试，断言结构化输出请求在归一阶段返回对应入站协议的 capability 错误，且不会进入
router/provider。

现有 `agent.error` fixture 族继续只验证 `map_inbound_error`，不把归一阶段失败错误地塞入
该 fixture 家族。五族 conformance fixtures 全部重跑，证明本修正没有破坏既有契约。

更新两个插件 README、三份 Agent 接入指南、协议盘点和 Responses IR 边界文档，明确
结构化输出暂不支持且会显式失败。

验收至少包括：

1. 两个 OpenAI agent adapter 的 fmt、clippy、WASM build 和 plugin conformance。
2. 现有 normalize、render、stream、error fixtures 不回归。
3. 官方插件一致性与两类协议的 runtime/proxy 针对性测试通过。
4. red-line diff 仍为零，Anthropic 并行修改未被纳入本次变更。

## 5. 完成标准

- 两个 agent manifest 不再声称 `json_schema`。
- 三份 M4 配置不再声称所配模型链路支持 `json_schema`。
- 两类协议的 JSON Schema/JSON Object 请求都不会进入 router 或 provider。
- 调用方收到明确的 capability 错误，而不是普通文本响应。
- M4 已验证的文本、流式和 function tool 主链保持通过。
- 不修改任何红线目录。
