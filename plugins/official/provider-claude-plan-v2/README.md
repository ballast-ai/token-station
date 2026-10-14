# Claude local subscription provider

This South provider uses the existing local Claude Code subscription login.
The component translates Messages requests. The Token Station host runs Claude Code.
The fixed loopback URL is a process transport marker. Do not send network requests to it.

The host exposes caller tools through a private loopback MCP server.
It returns tool calls to the caller before any tool executes.
The next request replays the complete structured conversation, including caller tool results.
Claude Code cannot use local file or command tools.
This provider does not use permission bypass.

Text, caller tools, and buffered SSE are supported.
Images, opaque thinking state, sampling overrides, forced tool selection, and structured output fail with capability errors.
Available model identifiers come from the installed Claude Code initialization metadata.
