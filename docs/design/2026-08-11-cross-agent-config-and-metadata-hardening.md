# Cross-Agent Configuration and Metadata Hardening

## Problem

The OpenCode repair exposed the same classes of risk in other Agent connectors and protocol adapters.

1. Codex connects with the synthetic model name `auto`, but the connector does not write `model_context_window`. Codex cannot use a built-in model preset for this synthetic model. Its context display and automatic compaction threshold can therefore use unknown model data.
2. WorkBuddy supports `maxInputTokens` and `maxOutputTokens` for custom models. The connector omits both fields, even though WorkBuddy uses `maxInputTokens` as the context-window denominator.
3. OpenClaw writes fixed values of `contextWindow=200000`, `maxTokens=32000`, and zero prices. These values are not derived from the effective Token Station route and can make context and cost data incorrect.
4. The OpenAI Responses adapter converts unsupported provider-hosted tools into empty or incomplete function tools. For example, a hosted `web_search` can appear callable even though the translated provider cannot execute it or return hosted-tool results and citations.
5. Several connectors reject an explicit `null` at an optional container that has the same practical meaning as an absent value. Examples include Claude Code `env`, OpenCode `provider`, Gemini CLI `security` or `auth`, Hermes `model`, OpenClaw route containers, and WorkBuddy model arrays.
6. Hermes accepts an explicit `model.context_length` for custom providers and uses it for context display and compression. The connector selects the synthetic `auto` model but does not project this value.

## Goals

- Project safe model limits, uniform prices, and verified image-input support into each client schema that supports them.
- Calculate metadata from each Agent's effective route, including Agent-specific routing overrides.
- Refuse provider-hosted tools before routing when the translated protocol cannot preserve execution and result semantics.
- Recover only optional `null` containers that the connector must populate.
- Keep required structures and non-null values strict.
- Restore an original null container exactly when Token Station disconnects or rolls back.
- Refresh managed client metadata after a saved Agent route changes.
- Return a model catalog that matches the effective Agent route.

## Scope

The implementation covers the built-in Claude Code, Claude Desktop, Codex, Gemini CLI, Hermes, OpenClaw, OpenCode, and WorkBuddy connectors. It also covers the OpenAI Responses inbound adapter used by Codex.

Cursor has no ordinary file connector in the current registry. Its existing SQLite integration uses a separate backup and verification path, so this change does not make it consume file-connector metadata or null-container rules. Claude and Gemini hosted-tool handling already fails closed and receives regression coverage only.

## Non-goals

- Do not invent prices for clients that have no matching custom-model price fields.
- Do not infer reasoning support from a model name.
- Do not create a hosted web-search, file-search, code-interpreter, image-generation, computer-use, or remote-MCP executor in Token Station.
- Do not replace a non-null scalar or array where an object is required.
- Do not change unrelated Agent settings or repair files outside connector-owned paths.

## Safety and Data Boundaries

1. The metadata calculation uses only the current materialized Token Station configuration. It does not send provider credentials or extra upstream requests.
2. Context and output limits use the minimum across all models reachable by the Agent's effective route. If any candidate lacks a valid limit, metadata remains unknown.
3. Price data is projected only when every reachable candidate has the same complete known price. Mixed or missing prices remain unknown.
4. Image input is advertised only when every reachable candidate has positive vision evidence. Unknown or mixed capability is projected as text-only.
5. A JSON `null` can become an object only while an Add or Replace operation traverses a connector-owned target path. Other scalar and array ancestors still fail.
6. YAML recovery applies only to an explicit null parent of a two-level scalar path that the connector owns. Existing non-null scalar parents still fail.
7. WorkBuddy treats `models: null` and `availableModels: null` as empty arrays because these fields are optional collections. Other value types still fail.
8. Receipts record only finite hosted-tool categories. They must not include tool input, search queries, results, or credentials.
9. A metadata refresh can change only connector-owned paths. It must verify the active ownership record before each write and keep the original encrypted disconnect baseline.
10. Automatic refresh skips an installation unless its current version is verified and still selects the connector recorded by ownership. An incompatible or ambiguous installation remains unchanged and visible for manual repair.
11. Ownership remains at the connector's declared leaf or subtree paths. A materialized null ancestor is restored only when it becomes empty, so a user-added sibling neither causes false drift nor gets removed.
12. A legacy ownership record that widened to a null or absent parent can narrow only when the current managed leaves still match its authenticated parent value. Changed managed leaves remain a hard drift failure.

## User-visible Behavior

### Codex

When the effective Codex route has complete limits, the connector writes:

- `model_context_window` to the safe context limit.
- `model_auto_compact_token_limit` to `context - max_output`, which reserves enough space for one maximum-size output.

The connector owns and restores these fields with the existing model and provider settings. If metadata is incomplete, it removes no user value and does not write a guessed limit.

### WorkBuddy

The `tokenstation-auto` custom model includes `maxInputTokens` and `maxOutputTokens` when route metadata is complete. WorkBuddy can then calculate its context percentage and output limit. The connector does not add unsupported price fields.

`supportsImages` follows verified image-input support from the effective route. A text-only or unknown route no longer claims image support.

`supportsToolCall` requires positive tool evidence for every reachable model. `supportsReasoning` and the reasoning-effort settings appear only when every reachable model declares the `reasoning_effort` parameter.

### Hermes

The `model.context_length` field uses the effective Hermes route's safe context limit. Hermes can then calculate context usage and compression thresholds for the synthetic `auto` model. Unknown metadata remains absent.

### OpenClaw

The `tokenstation/auto` model uses the effective route's safe `contextWindow` and `maxTokens`. Uniform prices map to `input`, `output`, `cacheRead`, and `cacheWrite`. Unknown metadata is omitted instead of using fixed defaults or zero prices.

### OpenCode

The `auto` model advertises image attachments and image input only when every reachable model has positive vision evidence. Otherwise, the connector writes a text-only model contract.

### Provider-hosted Responses Tools

Codex client tools continue to translate:

- `function`
- `custom`
- `namespace`
- `local_shell`
- `tool_search`

A `web_search` with `external_web_access=false` remains a disabled marker and is not sent to the provider. Other `web_search` requests and unsupported hosted-tool types fail locally with `capability`, `attempts=0`, and a finite `provider_tool_unsupported` reason. The error tells the user that the translated provider cannot execute the hosted tool.

The receipt uses `web_search` for Web Search and `other_tool_type` for other provider-hosted tool categories. Both use `provider_tool_unsupported`.

### Route Changes and Model Catalogs

When a user saves and applies an Agent route, Token Station refreshes metadata in every active file connector for that Agent. The refresh uses the existing transaction, ownership, snapshot, drift, and rollback checks. It does not replace the encrypted baseline used for disconnect.

If route metadata becomes incomplete, Codex and Hermes remove the stale limits that Token Station previously managed. Disconnect still restores any original user value from the encrypted baseline.

After proxy startup, Token Station also refreshes compatible managed connectors. It skips installations that currently fail compatibility admission. One unsupported Agent cannot make the running proxy report a network failure.

Each `/agents/<agent>/v1/models` response contains only models reachable through that Agent's effective route. The unscoped `/v1/models` response continues to use the Home route. A hot route reload updates the next model-catalog response without a full gateway restart.

### Optional Null Containers

Reconnect can recover these values when they are explicitly `null`:

- Claude Code `env`
- OpenCode `provider`
- Gemini CLI `security` and `security.auth`
- Hermes `model`
- OpenClaw `models`, `models.providers`, `agents`, `agents.defaults`, and `agents.defaults.model`
- WorkBuddy `models` and `availableModels`

The connector still rejects strings, numbers, booleans, and arrays where an object is required. WorkBuddy still rejects non-null, non-array collection values.

Disconnect and transaction rollback restore an original null parent as null. They do not leave an empty object behind.

If the user adds an unrelated sibling under a parent that Token Station materialized, disconnect removes only Token Station's fields and keeps the sibling. In that case the parent remains an object instead of returning to null.

Records created by the earlier widened-parent implementation migrate to declared connector paths during a compatible metadata refresh. Disconnect can also verify and remove a legacy record without deleting a user-added sibling. The migration reconstructs the authenticated parent from managed leaves only, so it does not accept a changed managed value.

## Accessibility and Failure Handling

This change does not alter page layout or keyboard order. Connector errors must name the affected Agent and configuration field. Hosted-tool errors must explain that the request stopped locally and did not reach an upstream provider.

## Tests and Acceptance Criteria

1. Shared JSON patch tests prove that Add and Replace materialize a null ancestor but reject other non-object ancestors.
2. YAML tests prove safe recovery of `model: null` and rejection of a non-null scalar parent.
3. Connector tests cover the listed null shapes without changing unrelated fields.
4. Route metadata tests prove that each Agent uses its own effective route, safe minimum limits, and uniform-price rule.
5. Codex tests verify context and compaction settings and their restoration boundary.
6. WorkBuddy tests verify token limits in both top-level-array and object schemas.
7. Hermes tests verify the safe context limit and restoration boundary.
8. OpenClaw tests verify real limits and prices and prove that fixed fallback values are absent.
9. OpenAI Responses tests reject hosted tools and preserve client-executed tools.
10. Proxy integration tests prove hosted-tool rejection before any upstream attempt and verify the receipt reason.
11. Full Rust and desktop frontend tests pass.
12. The official local desktop build and artifact audit pass.
13. The installed app starts, reconnects available Agents safely, and restarts the proxy.
14. Reverse-projection tests restore each supported null parent exactly.
15. Route-change tests prove that active owned metadata changes from a larger route to a smaller route without replacing the disconnect baseline.
16. Scoped model-catalog tests reject Home-only and sibling-Agent models and update after a hot route reload.
17. Hosted-tool receipt tests cover Web Search and at least one other provider-hosted tool category.
18. Unknown-metadata refresh tests prove that Codex and Hermes remove stale managed limits without changing the disconnect baseline.
19. Null-parent tests prove that Gemini companion files restore null exactly when empty and preserve user-added siblings without ownership drift.
20. Legacy-ownership tests prove that a widened record narrows on refresh and can disconnect safely after a user adds an unrelated sibling.

## Implementation Status

Implementation and local validation are complete.

- JSON, JSON5, and YAML patches now recover only explicit null parents on connector-owned Add or Replace paths. Connector validation accepts the listed optional null containers and still rejects other wrong types.
- Token Station now calculates metadata for each Agent's effective route. Codex, Hermes, OpenClaw, OpenCode, and WorkBuddy receive only fields that their native configuration supports.
- OpenClaw no longer receives fixed context limits or zero prices. OpenCode and WorkBuddy advertise image input only when all reachable models have positive vision evidence.
- The OpenAI Responses adapter now rejects provider-hosted tools before routing. Client-executed function, custom, namespace, local-shell, and tool-search tools continue to work.
- Managed metadata refresh uses the existing restore transaction. A regression test changes a managed route from 257,550 context tokens to 128,000, preserves the original baseline snapshot ID, and proves that disconnect restores the original null parents.
- Agent model catalogs now follow the effective Home or Agent router. Duplicate model IDs use safe minimum limits and intersected capability evidence.
- Automatic startup refresh skips incompatible or connector-mismatched installations. The frontend no longer mistakes the word `Connector` for a network connection failure.
- Codex and Hermes now remove stale managed limits when refreshed route metadata becomes unknown. Their original user values remain available through the encrypted disconnect baseline.
- Ownership stays on declared connector paths. Empty materialized ancestors collapse back to null or absence, while user-added siblings remain intact and do not trigger false ownership drift.
- Compatible refreshes migrate earlier widened-parent ownership records to declared paths after authenticated leaf verification. Legacy disconnects use the same verification and preserve unrelated siblings.
- `cargo test --workspace` passed. The Desktop Rust library suite passed 268 tests and ignored one read-only environment probe. Desktop integration tests and both workspace and Desktop Clippy checks also passed.
- The frontend passed 30 test files and 288 tests. The production frontend build passed with the existing large-chunk warning.
- `scripts/install-local-desktop.sh` passed the bundled-plugin gate, release build, artifact audit, signature checks, exact installation, and launch checks for the local arm64 app.
- Real App validation used the installed `/Applications/token-station.app`. After quit, reopen, and proxy start, revision 133 ran on `127.0.0.1:8787`. The global route and Agent pages loaded without a null-map error, false network banner, or other configuration banner. Claude Desktop and OpenCode appeared as connected.

This change does not create a DMG.
