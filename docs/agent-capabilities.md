# Agent tools, routing, and web search

A model connection does not configure an Agent's search service. Check each capability separately.
Fetching a known URL is not the same as searching for URLs.

## Confirm the route

The Agent connection page shows the routing source and configured target.
For a Direct route, check both the upstream name and model name.
The displayed configuration does not prove that an upstream request succeeded. Check the request log for the actual selection and result.

An Agent can inherit Home's tier definitions while retaining an independent Direct target.
The connection page labels this state as independent routing.
To remove that override, open the Agent's routing page and select **Restore global routing**.
This operation does not change other Agents' routes.

## Image input

Codex Auto accepts text and image input. Reopen an existing Codex session once to reload its model catalog.
The selected model offering must support images. Token Station returns an explicit error when the configured route cannot accept an image.
It never retries an image request with the image removed. Free Tiered routes do not fall back to paid channels.

Open a model's management panel and select **Verify vision** to test that exact offering.
The check sends one synthetic image challenge through the configured transport. It does not send your images or conversations.
A correct answer marks the offering **Verified**. An explicit image refusal marks it **Unsupported**.
Authentication failures, rate limits, malformed replies, and inconclusive answers preserve the existing capability state.
The result and safe error details appear on the model row. Closing and reopening the panel preserves a running check and its feedback.
Changed credentials or plugins invalidate an in-flight result. Saved capability changes apply to a running Gateway automatically.
If you stop the Gateway, capability application does not start it again.

## Search requirements

| Agent | Requirement or limit |
| --- | --- |
| Claude Code | Native WebSearch requires a compatible search route. A translated model route alone does not supply search. |
| Claude Desktop | The native search route must support WebSearch. Webpage access also depends on the App's egress allowlist. |
| Codex | Token Station keeps hosted search disabled for dynamic translated routes. See [native Responses configuration](reference.md). |
| Gemini CLI | The conversion adapter accepts function tools. Google Search and URL Context hosted tools are unsupported. Use a separate search MCP. |
| Grok Build | Tool availability depends on the backend and session. Use a search-capable backend or a search MCP. |
| Kimi Code | FetchURL reads a known URL. Configure a separate supported search service or MCP when no search tool is available. |
| DeepSeek Harness | Native search requires the separate `DEEPSEEK_API_KEY` credential and access to its search service. The model gateway key cannot replace it. |
| Hermes Agent | Configure a supported search service in Hermes and enable web tools. Missing credentials can hide the tools. |
| OpenClaw | Configure and enable a search provider in OpenClaw. Supply provider-specific credentials when required. |
| WorkBuddy | The client provides search separately from the model endpoint. Check its session and service access when search authentication fails. |
| OpenCode | Compatible CLI versions can enable Exa search with `OPENCODE_ENABLE_EXA=1`. Check tool permissions separately. |

These requirements are setup guidance. Token Station does not mark external search services as available without a runtime check.
Model API fees and separate search service fees can differ.

### OpenCode session activation

For macOS or Linux, start one CLI session with:

```sh
OPENCODE_ENABLE_EXA=1 opencode
```

For Windows, start a child command process with:

```bat
cmd /c "set OPENCODE_ENABLE_EXA=1&&opencode"
```

The environment setting ends with that process. These commands do not configure the OpenCode desktop App or shell startup files.
Search queries go to Exa. Confirm the service terms and credentials required by your installed client version.

## Verify a connection

1. Ask the Agent to return a unique text marker.
2. Ask it to read a temporary file and report its exact content.
3. Ask it to edit a separate test fixture and run its tests.
4. Ask for a search with source URLs. Confirm that a search tool ran.
5. Ask it to fetch a known URL. Check this result separately.

Do not treat an HTTP 200 response or a CLI exit code of zero as proof of success.
Check the native tool result and final task output.

Gemini CLI can send function parameters in `parametersJsonSchema` instead of legacy `parameters`.
Token Station preserves either schema. It rejects declarations that combine them or provide a non-object schema.
Malformed model-generated arguments remain errors. The gateway does not guess executable parameters.

See the [Gemini FunctionDeclaration reference](https://ai.google.dev/api/generate-content#FunctionDeclaration) for the schema contract.
