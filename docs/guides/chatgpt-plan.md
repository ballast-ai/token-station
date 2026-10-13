# ChatGPT plan connection (experimental)

Token Station can translate local Agent requests to the public ChatGPT plan Responses interface.
It uses a separate Sign in with ChatGPT registration for Token Station.
It does not read Codex credentials or call the private Codex backend.
The transport follows the public `chatgpt-api` approach studied in [Magpie](https://github.com/yetone/magpie).

This experimental bridge is not an OpenAI endorsement of a general-purpose API gateway.
Availability depends on the account, current Sign in with ChatGPT eligibility, and applicable provider terms.
See the [OpenAI Sign in with ChatGPT documentation](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

## Connect an account

1. Open **Models** in Token Station.
2. Find the **ChatGPT plan** panel and select **Continue with ChatGPT**.
3. Complete the OpenAI browser authorization for Token Station.
4. Check that the account is ready for inference.
5. Load the account model catalog and connect the provider.
6. Select that provider and model in the desired route.
7. Save the configuration and apply it to the local gateway.
8. Connect Claude Code or OpenCode through the existing Agent connection workflow.

An identity-only login cannot use plan inference. It does not create a usable provider.
The provider stores a registration reference. It does not store an OAuth token in the configuration.
Model discovery uses that registration's current catalog. It does not guess models from a paid API catalog.

Claude Code keeps its own client-facing model aliases. Token Station selects the actual upstream model through routing.
OpenCode can use the gateway's OpenAI-compatible endpoint and model catalog.
Both Agents execute local function tools themselves. Token Station translates calls and results across protocols.
A Claude Code conversation routed here uses an OpenAI model. It does not convert plan allowance into Claude model credits.

## Allowance and failures

Plan usage shares the account's applicable ChatGPT allowance. Each Agent does not receive a separate allowance.
Token counts are observations of requests. They are not a reliable remaining-plan balance.
The panel links to ChatGPT usage settings. Token Station does not invent an allowance reset time.

A plan route does not fall back to a paid API provider or another ChatGPT registration.
ChatGPT registrations cannot participate in quota rotation.
Subscription usage is excluded from API price synchronization and estimated API costs.
If authorization, eligibility, allowance, or the provider fails, the request reports an error.

## Current protocol limits

- Requests use the public `/v1/responses` endpoint with `stream: true` and `store: false`.
- Function tools use a namespace. Tool names, call IDs, arguments, and results survive history replay.
- The Agent must send complete history. Stored response continuation is unsupported.
- Text and supported image inputs work. Unsupported content fails explicitly.
- Buffered requests without tools support local stop-sequence matching, including Claude Code auto-mode classifier requests.
- Streaming stop sequences, signed or encrypted reasoning, and hosted tools are unsupported.
- Local stop matching trims the returned text after successful upstream completion. It does not stop upstream generation or reduce its usage.
- Local MCP tools can work when the Agent exposes them as function tools.
- Temperature, top-p, and maximum output token settings are not sent. The endpoint does not support those controls.
- Only `response.completed` confirms success. A truncated, failed, or incomplete stream returns an error.

## Credentials and sign-out

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

These tests start an isolated gateway, use the Host account resolver, and let each installed CLI read one
temporary random-marker file. They require an actual completed tool result and matching final answer.
The auto-mode check also requires an observed safety-classifier request and a completed temporary-file copy.
They do not change existing Agent configurations or pass OAuth tokens to either CLI.
The account resolver may refresh the selected registration, and the gateway opens its existing quota database.

On 2026-10-13, Claude Code 2.1.294 and OpenCode 1.18.35 each passed this live tool loop with `gpt-6-luna`:
two successful upstream requests per Agent, subscription receipts, and no estimated API cost.
This verifies that account and model at that time; it does not establish universal eligibility or a remaining balance.
