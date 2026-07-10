# agent-openai

OpenAI-compatible inbound agent adapter: `openai-chat-completions` dialect to and from Canonical IR.

This is a northbound plugin. The host redacts request headers before the plugin
receives them, so credential values are not visible. The `agent-adapter-v1`
world has no import that can name a credential. The runtime rejects an agent
component that imports `token-station:adapter/host`.

- Source: `src/lib.rs`. Build target: `wasm32-wasip2` (`cargo build --target wasm32-wasip2`).
- Acceptance: `fixtures/` is the fixture package for the `agent-protocol-v1` suite.
  `crates/plugin-runtime/tests/official_plugins.rs` loads the compiled `.wasm`
  into the runtime and runs all gates.
- The implementation has the same structure as the native reference in the
  conformance tests. The fixture package requires equal output from both.
