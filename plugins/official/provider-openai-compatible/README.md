# provider-openai-compatible

OpenAI-compatible 出站 provider adapter：Canonical IR ⇄ OpenAI 方言的 HTTP 请求/响应。

南向插件。它构造的 `HttpRequestDescriptor` 用 `Auth::Bearer` **命名**凭证槽位
（`provider_api_key`），从不持有明文；host 在 `ProviderConfig::authorize` 验过
目的地之后才注入。流式解析持有跨 chunk 的缓冲——runtime 按流实例化组件，所以
两条流的 body 不会串。

- 源码：`src/lib.rs`，编译目标 `wasm32-wasip2`（`cargo build --target wasm32-wasip2`）
- 验收：`fixtures/` 是 `provider-protocol-v1` 套件的 fixture 包（含必需的 401 用例）；
  `crates/plugin-runtime/tests/official_plugins.rs` 把编好的 `.wasm` 装进 runtime
  跑全套门
- 与 conformance 自测里的原生参考实现保持同构，fixture 包钉住两者的输出一致
