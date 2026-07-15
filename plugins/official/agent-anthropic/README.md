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
- Current limit: Canonical IR does not represent `thinking` or
  `redacted_thinking`. The adapter rejects these content blocks and does not
  discard them silently.
