# provider-openai-compatible

OpenAI-compatible outbound provider adapter: Canonical IR to and from
OpenAI-dialect HTTP requests and responses.

This is a southbound plugin. Its `HttpRequestDescriptor` uses `Auth::Bearer`
to name the `provider_api_key` credential slot. It never holds the plaintext
credential. The host injects the credential only after
`ProviderConfig::authorize` approves the destination. The streaming parser
keeps a buffer across chunks. The runtime creates one component for each
stream, so bodies from two streams cannot mix.

- Source: `src/lib.rs`. Build target: `wasm32-wasip2` (`cargo build --target wasm32-wasip2`).
- Acceptance: `fixtures/` is the fixture package for the `provider-protocol-v1`
  suite, including the required 401 case.
  `crates/plugin-runtime/tests/official_plugins.rs` loads the compiled `.wasm`
  into the runtime and runs all gates.
- The implementation has the same structure as the native reference in the
  conformance tests. The fixture package requires equal output from both.
