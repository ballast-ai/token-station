# ChatGPT plan connection (experimental)

Token Station supports two separate subscription connections, based on the approaches studied in [Magpie](https://github.com/yetone/magpie).

- **Local Codex login** reuses an existing file-backed Codex login through the Codex backend. No browser authorization is required.
- **ChatGPT plan** keeps the separate Sign in with ChatGPT registration and its public Responses interface.

A model name alone does not select an allowance source. Select the connected subscription provider in the route.

This experimental bridge is not an OpenAI endorsement of a general-purpose API gateway.
Availability depends on the account, current Sign in with ChatGPT eligibility, and applicable provider terms.
See the [OpenAI Sign in with ChatGPT documentation](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

## Reuse an existing Codex login

At startup, Token Station detects a supported local Codex login and automatically adds its provider and current model catalog.
No connect click or browser authorization is required. Installing Codex without signing in does not provide subscription access.

1. Open **Models** and wait for automatic discovery.
2. Select the local Codex provider and a discovered model in the desired route.
3. Save and apply the configuration.
4. Connect the Agent through its existing connection workflow.

On a fresh installation, the first discovered model becomes the initial default target.
Existing routes, model selections, and Codex account bindings remain unchanged. Discovery does not start the gateway or connect Agents.
If unsaved edits exist, automatic registration waits until a later launch with a clean configuration.
Deleting a provider or disconnecting its reference prevents automatic reconnection.
To retry or reconnect manually, select **Add model**, then **Subscriptions**, then **ChatGPT**.
Select the intended account or **Use local Codex login**, then wait for validation and model discovery.

Token Station reads `CODEX_HOME/auth.json`, or `~/.codex/auth.json` when `CODEX_HOME` is unset.
This version supports Codex file storage. Keyring, auto, and ephemeral storage modes are not supported.
An API-key-only Codex login cannot supply subscription allowance.
Token Station does not import a token into its configuration or the Agent's configuration.
Its protected local record contains only source and account metadata.

The connection binds the source path, user identity, and account identity.
If Codex signs out or changes accounts, existing routes stop until the intended account is connected again.
**Disconnect reference** removes only the Token Station binding. Codex remains signed in.
Expired credentials can refresh through the fixed OpenAI OAuth endpoint and update the original Codex credential file.
Token Station serializes its own refreshes and rejects observed source changes during refresh.
Codex does not honor this lock. The integration cannot guarantee synchronization with an unrelated Codex process.

This path uses `https://chatgpt.com/backend-api/codex/responses` and the account's current Codex model catalog.
Backend compatibility and account availability can change. This is an experimental integration, not an official general-purpose API guarantee.
Existing ChatGPT plan registrations remain separate and continue to work.

## Connect a separate ChatGPT registration

1. Open **Models** in Token Station.
2. Select **Add model**, then **Subscriptions**, then **ChatGPT API**.
3. Select an existing account or **Continue with ChatGPT**.
4. Complete the OpenAI browser authorization for Token Station.
5. Wait for the account's model catalog and provider connection.
6. Select that provider and model in the desired route.
7. Save the configuration and apply it to the local gateway.
8. Connect a supported Agent through its existing Agent connection workflow.

An identity-only login cannot use plan inference. It does not create a usable provider.
The provider stores a registration reference. It does not store an OAuth token in the configuration.
Model discovery uses that registration's current catalog. It does not guess models from a paid API catalog.

Claude Code keeps its own client-facing model aliases. Token Station selects the actual upstream model through routing.
OpenCode can use the gateway's OpenAI-compatible endpoint and model catalog.
Both Agents execute local function tools themselves. Token Station translates calls and results across protocols.
A Claude Code conversation routed here uses an OpenAI model. It does not convert plan allowance into Claude model credits.

## Manage a plan connection

On **Models**, select **Manage** beside **ChatGPT plan** or **Local Codex login**.
The dialog shows only the account bound to that provider, its connection status, and its model catalog.
Select **Refresh models** to refresh the current account catalog. Select **View usage** to open ChatGPT usage settings.
Expand **Connection details and usage limits** to inspect the non-secret reference or reauthorize a ChatGPT account.
Confirm **Disconnect connection** or **Disconnect reference** to make the associated models unavailable.
The provider and its route bindings remain configured. Deleting a provider remains a separate operation.
If a local reference was disconnected, reconnect the intended login from **Add model**.

## Allowance and failures

Plan usage shares the account's applicable ChatGPT allowance. Each Agent does not receive a separate allowance.
Token counts are observations of requests. They are not a reliable remaining-plan balance.
The panel links to ChatGPT usage settings. Token Station does not invent an allowance reset time.

A plan route does not fall back to a paid API provider or another ChatGPT registration.
Both subscription connection types are excluded from quota rotation.
Subscription usage is excluded from API price synchronization and estimated API costs.
If authorization, eligibility, allowance, or the provider fails, the request reports an error.

## Current protocol limits

- ChatGPT registrations use public `/v1/responses`. Local Codex references use `/backend-api/codex/responses`. Both require `stream: true` and `store: false`.
- Function tools use a namespace. Tool names, call IDs, arguments, and results survive history replay.
- Long or incompatible function names use reversible provider aliases. JSON and SSE replies restore the client identity.
- Codex leading developer `additional_tools` declarations support function tools and custom tools in one namespace level.
- Gemini function selection and call IDs are preserved. Supported legacy parameter schemas are converted to JSON Schema.
- Chat Completions text, JSON object, and JSON schema formats map to Responses `text.format` without removing constraints.
- Anthropic structured response formats and Gemini response schemas or JSON/enum MIME types are not supported by their ingress adapters. They fail before model dispatch. Function-tool parameter schemas remain supported.
- New plan model entries declare JSON schema transport support. This declaration does not verify every model or schema.
- An unsupported reasoning effort remains an explicit parameter error. The Agent can apply its own compatibility retry.
- The Agent must send complete history. Stored response continuation is unsupported.
- Text and supported image inputs work. Unsupported content fails explicitly.
- Buffered requests without tools support local stop-sequence matching, including Claude Code auto-mode classifier requests.
- Streaming stop sequences and signed or encrypted reasoning are unsupported.
- Native hosted tools are unsupported. Host-managed web search uses the configured search backend instead.
- Local stop matching trims the returned text after successful upstream completion. It does not stop upstream generation or reduce its usage.
- Local MCP tools can work when the Agent exposes them as function tools.
- Temperature, top-p, and maximum output token settings are not sent. The endpoint does not support those controls.
- Only `response.completed` confirms success. A truncated, failed, or incomplete stream returns an error.

## WebFetch and WebSearch

Claude Code executes WebFetch in its own client. Its page-summary request also passes through the selected model route.
Auto mode can send a separate permission-classifier request before the tool runs.
A successful classifier response alone does not prove successful retrieval or summarization.

Claude Code WebSearch sends a separate hosted-search request to Token Station.
Token Station's ChatGPT plan provider does not forward that hosted tool directly.
Enable search and select **Auto** or **Local** mode in Token Station's search settings.
The Host translates the request into a bounded function-tool loop and uses the configured search backend.
It returns retrieved URLs and explicitly marked untrusted evidence to the client.
Client tool calls and retrieved text remain available in later conversation turns.

OpenCode's `webfetch` runs in the client and returns page text to the routed model.
OpenCode 1.18.35 does not expose `websearch` by default for a custom provider such as Token Station.
Its optional Exa or Parallel search runs directly in OpenCode, separately from Token Station's hosted-search translation and Tavily settings.
The opt-in Exa acceptance sets `OPENCODE_ENABLE_EXA=1` only for its isolated test process. Normal user settings remain unchanged.

Tavily uses its own configured key and search credits. These credits are separate from ChatGPT plan allowance.
Search receipts record observed credits, including reported usage on an empty-result response.
Unknown credits stay unknown. A search failure does not trigger a paid-model fallback.

The Host checks declared tools, tool selection, and explicit parallel-call restrictions before retrieval or client dispatch.
Each request permits at most three combined searches and page reads, or a smaller declared limit.
Page reads accept only returned source URLs and public addresses. Static HTML and UTF-8 text are supported.
Empty pages, invalid arguments, blocked addresses, failed model streams, and backend failures remain explicit errors.
Unsupported search constraints fail instead of being silently removed.

## ChatGPT registration credentials and sign-out

The Host validates signed ID tokens, state, nonce, PKCE, issuer, and registration identity.
Account records are owner-only local files under the configured data directory's `chatgpt-accounts` directory.
These files contain credentials. Do not share them or include them in diagnostic bundles.
Refresh rotation and sign-out use a per-registration operating-system lock across processes.
Plugins and frontend views receive no OAuth token values.

Sign-out clears local credentials and attempts remote revocation.
If revocation cannot be confirmed, remove the Token Station connection in ChatGPT settings.
Existing provider references remain visible so routes do not silently switch billing sources.
Sign in to the same registration again to restore its connection.

## Verification boundaries

The test suite covers OAuth with a signed local issuer, rotation races, protocol translation, and tool round trips.
The installed-client regression checks use isolated configurations and a simulated Responses server.
Run these checks without consuming a plan allowance:

```bash
cargo test -p token-station-cli --test chatgpt_tools -- --include-ignored --skip live_ --nocapture
```

Simulated tests do not prove live account eligibility, endpoint acceptance, or allowance charging.
Live validation requires the user's browser authorization and a request through that registered account.
Separately ignored `live_` checks consume the explicitly selected account's plan allowance.
After connecting that account in the desktop app, set these three values in the test process environment:

- `TOKEN_STATION_CHATGPT_LIVE_DATA_DIR`: the app's existing data directory.
- `TOKEN_STATION_CHATGPT_LIVE_ACCOUNT_ID`: the non-secret local registration ID.
- `TOKEN_STATION_CHATGPT_LIVE_MODEL`: a model from that account's current catalog.

Then run:

```bash
cargo test -p token-station-cli --test chatgpt_tools live_ -- --ignored --test-threads=1 --nocapture
```

These tests start an isolated gateway and use the Host account resolver.
File tests require a completed tool result and a matching random-marker answer.
The auto-mode file check also requires a classifier request and a completed temporary-file copy.
Web tests make public network requests and require tool-result pairing, retrieval evidence, and a grounded final answer.
WebSearch uses the existing search settings and can consume the configured backend's search credits.
Web tests use isolated safe-mode sessions. Claude Code's `--bare` mode removes its web tools.
They do not change existing Agent configurations or pass OAuth tokens to either CLI.
The account resolver may refresh the selected registration, and the gateway opens its existing quota database.

For an explicitly authorized local Codex acceptance, also set `TOKEN_STATION_CODEX_LIVE=1`.
Use the same data-directory and model variables. Omit `TOKEN_STATION_CHATGPT_LIVE_ACCOUNT_ID`.
The harness binds the current local Codex source without browser authorization.
It can refresh that source and consumes the selected account's Codex allowance.
Never print or pass its OAuth tokens to the test command.

On 2026-10-13, Claude Code 2.1.294 and OpenCode 1.18.35 each passed this live tool loop with `gpt-6-luna`:
two successful upstream requests per Agent, subscription receipts, and no estimated API cost.
This verifies that account and model at that time; it does not establish universal eligibility or a remaining balance.

On 2026-10-13, the installed CLIs also passed these live web checks with `gpt-6.1-sol`:

- Claude Code auto-mode WebFetch completed page retrieval, permission classification, summary inference, and the final answer.
- Claude Code WebFetch reported a real HTTP 404 without inventing page contents.
- Claude Code auto-mode WebSearch completed Host-managed Tavily retrieval and returned IANA source citations.
- OpenCode webfetch returned the page body and a final answer citing its source.
- OpenCode websearch with temporary Exa opt-in completed client-side retrieval and cited a URL from the actual tool result.

These checks used isolated CLI settings and preserved permission checks. Search billing remained separate from subscription model accounting.
The deterministic search suite covers both Agent protocols, JSON/SSE, domain filters, limits, tool constraints, missing evidence, backend errors, and truncated model streams.


## Agent acceptance matrix

The registry contains eleven connection-capable Agents. Cursor is discovery-only and has no supported connector.
These results cover the installed versions on 2026-10-13 with the selected `gpt-6.1-sol` plan model.
They do not certify every model, plugin, search service, or future client version.

| Agent | Observed version | Acceptance evidence |
| --- | --- | --- |
| Claude Code | 2.1.294 | Live file tools, auto-mode classification, WebFetch, HTTP 404 handling, and Host-managed WebSearch passed. |
| OpenCode | 1.18.35 | Live file tools, webfetch, and optional client-side Exa websearch passed. |
| Codex CLI | 0.162.0-alpha.17.2 | Live namespaced custom execution, file read, apply_patch, verification read, and final answer passed. |
| Gemini CLI protocol | CLI not installed | Live generateContent HTTP read, write, command verification, and final answer passed. Actual CLI execution remains unverified. |
| Grok Build | 1.0.46 | Live read, write or edit, command verification, paired tool results, and final answer passed. |
| Kimi Code | 2.1.1 | Live read, write or edit, command verification, paired tool results, and final answer passed. |
| OpenClaw | 2026.9.8 | Live read, write or edit, command verification, paired tool results, and final answer passed. |
| WorkBuddy embedded CodeBuddy | 2.156.0 | Live read, write or edit, command verification, paired tool results, and final answer passed. |
| DeepSeek Harness | 0.2.0-rc.2 | Live read, write, command verification, and final answer passed with the current Cordis connector fields. |
| Hermes | 0.21.5+9144.gdde8800 | Live read, write, command verification, and final answer passed. Its official title generator also passed a separate live structured-output and parameter-recovery check. |
| Claude Desktop | 2.26454.2 | Connector and Anthropic protocol tests only. Its GUI tool loop has not been accepted. |

Grok, Kimi, OpenClaw, WorkBuddy, and DSH passed both read-only and read/write/command checks.
Hermes uses its existing prepared Python environment with isolated settings and dependency installation disabled.
The checks require paired call IDs, exact fixture bytes, command output, and matching final answers.
Permission rules target only the temporary fixtures and the verification command.
DSH uses its default workspace sandbox. OpenClaw imports the test command into its isolated approval store.
Every completed request uses subscription accounting without API cost estimation or fallback.
Grok also cancelled one auxiliary request during shutdown. That cancellation is recorded separately from successful tool calls.

The official Anthropic provider component cannot preserve structured response formats in its pinned protocol implementation.
It rejects JSON object and JSON schema requests before HTTP dispatch. Plain text requests remain supported.
This limitation does not apply to the ChatGPT plan provider or arbitrary custom providers.

The deterministic four-protocol suite checks JSON/SSE, parallel calls, history replay, named choices, structured formats, and failed or truncated streams:

```sh
cargo test -p token-station-cli --test chatgpt_protocols
```

Additional installed-client checks are in `chatgpt_agents`. They are ignored by default and consume plan allowance when explicitly run.
Set the three live-account variables above. Set `TOKEN_STATION_<AGENT>_CLI` to the installed executable required by each check.
The supported names are `GROK`, `KIMI`, `OPENCLAW`, `WORKBUDDY`, `DSH`, and `HERMES`.
For an existing Hermes environment, set `TOKEN_STATION_HERMES_PYTHON` and `TOKEN_STATION_HERMES_SOURCE`.
The Python path selects its prepared interpreter. The source path selects its installed package root.
The test uses bounded title-generation cleanup and preserves tool permissions.
Hermes `chat --oneshot` closes cached clients during exit, which can interrupt its background title request.
The separate title-generator check keeps the client alive through completion. Its default request receives an unsupported-effort error, then retries successfully.
The retry comes from Hermes and preserves the JSON schema. Token Station does not change the effort or retry across providers.
For the tested model, an explicit `auxiliary.title_generation.extra_body.reasoning_effort: low` also passed.
This optional setting affects title generation only. It is not written by the generic Agent connector.
The test source lists the exact executable overrides. Use existing installations and isolated homes.
These bounded tool checks do not certify arbitrary shell commands, external search services, or GUI sessions.

## Local Codex backend acceptance

On 2026-10-14, the existing local file-backed Codex login connected without a browser or a new authorization grant.
Its catalog returned seven models. Acceptance used the discovered `gpt-6.1-sol` model.

Claude Code passed Read, auto-mode file write, WebFetch summary, real HTTP 404 handling, and Host-managed Tavily WebSearch.
OpenCode passed Read, webfetch, and its opt-in Exa websearch. Codex CLI passed execution, apply_patch, read-back verification, and final output.
Grok, Kimi, OpenClaw, WorkBuddy's embedded CodeBuddy, DSH, and Hermes passed native read/write/command tool loops.
Gemini's four-turn HTTP protocol check passed. Gemini CLI was not installed, and Claude Desktop's GUI tool loop remains unverified.
Cursor remains discovery-only.

Every successful model request used subscription accounting without estimated API cost or paid fallback.
Search service credits remain separate. Grok cancelled one auxiliary request during exit.
Hermes's main tool workflow passed with four successful requests. Its `/props` compatibility probe returned a non-billable 404.
Its independent title generator passed an explicit reasoning rejection, native client retry, and valid structured JSON output.
The oneshot process can exit after the first title error. This does not establish completion of its background title task.

The Codex backend can report a parameter error inside an HTTP 200 SSE stream.
The bridge recognizes only the exact unsupported `reasoning.effort` error and returns a fixed safe parameter error.
It preserves the requested schema and lets the client decide whether to retry.
All other failed or truncated streams remain explicit failures.

### Installed OpenCode desktop acceptance

On 2026-10-14, OpenCode desktop 1.18.29 passed the persistent connector path through the installed Token Station App.
The App encrypted the original configuration before taking ownership of `provider.tokenstation`.
The desktop session selected `tokenstation/power` and reached the local Codex model on the existing independent route.

- Two native reads, one write, one edit, and one shell command completed. Output files matched an unknown random marker byte-for-byte.
- Native WebFetch returned page content and a source URL.
- A real HTTP 404 reached the model. The same turn then fetched a valid page and returned its title.
- Native WebSearch was initially absent. Enabling the client's search environment exposed the tool.
- Exa returned a free MCP rate-limit error. The model reported the failure without inventing search results.
- The supported Parallel backend returned sources. The desktop final answer cited a URL present in the actual tool result.
- OpenCode CLI 1.18.35 also passed Parallel search using the managed global provider and `tokenstation/auto`.

All 17 recorded model requests used the local Codex subscription with HTTP 200, no API cost estimate, and zero fallbacks.
Fourteen came from the desktop workflow. Search backend errors remain separate from model request success.
See [desktop search activation](../agent-capabilities.md#opencode-desktop-activation-on-macos) for the client setup and service limits.
This acceptance covers the tested versions and bounded workflows. It does not certify every tool or external service.
