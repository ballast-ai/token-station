# ChatGPT plan provider component

This component translates Canonical IR into the ChatGPT plan Responses dialect.
It implements the pinned South v2 component ABI. It cannot read credentials,
open network connections, or access files. The Host supplies a registration-bound
access token through `chatgpt_access_token`.

The production destination is `https://api.openai.com/v1/responses`.
Loopback `/v1` endpoints are also accepted for isolated tests with synthetic
credentials. The Host confines real account credentials to the official endpoint.
Every upstream request uses `stream: true` and `store: false`. Buffered Agent
requests use the same SSE parser and aggregate its events into a ChatResponse.
Only `response.completed` confirms success. Failure, incomplete responses,
invalid tool arguments, and premature EOF fail the exchange.

Function tools use the `tokenstation_tools` namespace. Function names and call
IDs stay unchanged across replies and history replay. A forced function selects
only that function and uses `tool_choice: required`. Tool calls must have matching
results before the next inference. Unknown content and unsupported required tool
constraints produce explicit capability errors.

## Deliberate limits

- The endpoint rejects temperature, top-p, and maximum output token parameters.
  These common Agent sampling fields are not sent. The UI must explain that the
  Agent's output cap and sampling controls do not constrain plan inference.
- The standalone component rejects stop sequences. The Host implements local matching for buffered, tool-free requests.
  Hosted tools, stored response continuations,
  audio, and unknown content blocks are not translated.
- The pinned South StreamEvent schema cannot carry opaque Responses reasoning.
  This component does not request encrypted reasoning. It does not convert that
  state to an Anthropic signature. Incoming signed or redacted reasoning is
  rejected. Unsigned assistant thinking summaries are omitted from replay because
  they are display summaries, not verifiable model state. Known upstream reasoning
  summaries are not exposed as a different provider's native thinking blocks.
- The parser bounds individual buffered values and SSE fragments to 4 MiB.
  The Host also applies its existing memory, response-size, and cancellation limits.
- Local function tools work through the calling Agent. Hosted web search, MCP,
  image generation, file search, and Code Interpreter are outside this component.

This code does not establish authorization to expose ChatGPT plan usage as a
general-purpose gateway. Product eligibility and current provider terms require
separate evaluation. It never imports Codex credentials, headers, or prompts.

## Verification

Run `cargo test --manifest-path plugins/official/provider-chatgpt-plan-v2/Cargo.toml`.
Build with `cargo build --release --target wasm32-wasip2 --manifest-path plugins/official/provider-chatgpt-plan-v2/Cargo.toml`.
The package includes South conformance fixtures and byte-fragmented parallel tool
tests. Unit tests use synthetic credentials and responses. They do not prove live
account authorization or official endpoint acceptance.
