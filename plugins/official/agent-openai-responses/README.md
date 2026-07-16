# agent-openai-responses

Official northbound adapter for OpenAI Responses API clients, initially
validated with Codex.

It owns POST /v1/responses, translates the supported Responses subset into the
Canonical IR, and renders canonical responses as Responses JSON or SSE.

Supported in M4:

- text and image input
- function tools, function calls, and function call outputs
- non-streaming and streaming text
- streamed function arguments
- usage and protocol-shaped errors

Reasoning content, computer calls, hosted web search, and other output item
types are rejected explicitly until the Canonical IR has an approved
representation.
