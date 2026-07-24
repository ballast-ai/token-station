# agent-anthropic

This inbound Agent adapter translates the Anthropic Messages
`anthropic-messages` dialect to and from Canonical IR.

It translates only the inbound protocol. It does not select a provider,
upstream, or model. It contains no model-provider branches. The host removes
sensitive request headers before the request enters WASM. The adapter has no
network, file-system, or credential access.

- Source: `src/lib.rs`
- Target: `wasm32-wasip2`
- Acceptance: five `agent-protocol-v1` conformance families in `fixtures/`
- The `anthropic_thinking` request extension stores the `thinking` request
  configuration. The `anthropic_thinking_blocks` message extension stores
  original `thinking` and `redacted_thinking` blocks from assistant
  history, including their original array positions. These fields do not
  change the core Canonical IR schema. Providers that do not recognize them
  do not serialize them to the upstream.
