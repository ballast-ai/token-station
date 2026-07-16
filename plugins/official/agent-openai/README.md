# agent-openai

OpenAI-compatible 入站 agent adapter：`openai-chat-completions` 方言 ⇄ Canonical IR。

北向插件。它看到的请求 header 已由 host 脱敏（凭证值不可见），`agent-adapter-v1`
world 也没有任何可以命名凭证的 import——runtime 会拒绝加载任何 import 了
`token-station:adapter/host` 的 agent 组件。

- 源码：`src/lib.rs`，编译目标 `wasm32-wasip2`（`cargo build --target wasm32-wasip2`）
- 验收：`fixtures/` 是 `agent-protocol-v1` 套件的 fixture 包；
  `crates/plugin-runtime/tests/official_plugins.rs` 把编好的 `.wasm` 装进 runtime
  跑全套门
- 与 conformance 自测里的原生参考实现保持同构，fixture 包钉住两者的输出一致

当前只承载普通文本输出。`response_format.type` 为 `json_schema` 或
`json_object` 时会明确返回 capability 错误；manifest 不声明 `json_schema`，不会把
结构化输出请求静默降级成普通文本。
