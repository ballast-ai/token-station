# agent-anthropic

Anthropic Messages 入站 agent adapter：`anthropic-messages` 方言 ⇄ Canonical IR。

它只负责入站协议翻译，不选择 provider、upstream 或模型，也不包含
任何模型厂商分支。请求 header 在进入 WASM 前由 host 脱敏，
adapter 无网络、文件系统或凭证权限。

- 源码：`src/lib.rs`
- 目标：`wasm32-wasip2`
- 验收：`fixtures/` 中的 `agent-protocol-v1` 五族一致性用例
- 当前边界：Canonical IR 未表达 `thinking` / `redacted_thinking`，
  适配器会明确拒绝这些内容块，不静默丢弃
