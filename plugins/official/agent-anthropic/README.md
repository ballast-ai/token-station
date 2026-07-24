# agent-anthropic

Anthropic Messages 入站 agent adapter：`anthropic-messages` 方言 ⇄ Canonical IR。

它只负责入站协议翻译，不选择 provider、upstream 或模型，也不包含
任何模型厂商分支。请求 header 在进入 WASM 前由 host 脱敏，
adapter 无网络、文件系统或凭证权限。

- 源码：`src/lib.rs`
- 目标：`wasm32-wasip2`
- 验收：`fixtures/` 中的 `agent-protocol-v1` 五族一致性用例
- `thinking` 请求配置保存在请求扩展字段 `anthropic_thinking`；assistant
  历史中的 `thinking` / `redacted_thinking` 原始块保存在消息扩展字段
  `anthropic_thinking_blocks`（含原数组位置）。这两个字段不改变 Canonical
  IR 的核心 schema，也不会由不认识它们的 provider 自动序列化到上游。
