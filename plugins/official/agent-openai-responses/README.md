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

Structured output (`text.format.type` set to `json_schema` or `json_object`)
is also rejected explicitly. The adapter does not advertise `json_schema` and
never downgrades a structured-output request to ordinary text.
