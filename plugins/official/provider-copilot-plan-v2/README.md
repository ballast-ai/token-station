# Copilot subscription provider

The host binds an existing local Copilot login. The component has no network or filesystem access.
The `local_subscription_token` slot supplies the inference credential. API usage is subscription usage.

The host discovers the actual account catalog from `/models`. The host projects its
`subscription_identity` map into flat `ProviderConfig` extension fields: `copilot_source_kind`
and `copilot_model_endpoints`. The component chooses Responses, Messages,
or Chat Completions only when the selected model advertises that endpoint. Older catalogs without
`supported_endpoints` use Chat Completions. Auto models and disabled policy models are excluded.

Editor logins exchange a GitHub OAuth token for a short-lived Copilot token. CLI logins use their
own integration and token directly. Identity, initiator, and vision headers follow the chosen source.
The host accepts only the fixed GitHub.com and Copilot API destinations. It rejects redirects.

Caller tools remain caller-owned. Messages preserves signed Anthropic thinking. Other endpoints
reject signed thinking. Buffered Messages preserves redacted thinking. The stream ABI cannot carry
redacted thinking, so streamed redacted state returns a capability error. Opaque encrypted Responses reasoning returns a capability error.
Unsupported hosted tools, continuation state, and content fail without a paid API fallback.

The Messages and Chat Completions codecs use the pinned South reference implementation.
The Responses codec is adapted from Token Station's subscription codec. Its local module avoids
linking another component's WIT exports into this component.

Run `cargo test --manifest-path plugins/official/provider-copilot-plan-v2/Cargo.toml`.
Build with `cargo build --locked --release --target wasm32-wasip2` from this directory.
