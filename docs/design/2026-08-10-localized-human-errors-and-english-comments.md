# Localized Human-Readable Errors and English Comments

## Problem

Several desktop pages render backend failures with `String(error)`. This can expose implementation terms such as `generation`, `listener`, `model_providers`, and transaction state names. The result may also use a different language from the language selected in the app. A previous comment cleanup also missed JSX and HTML comment syntax.

The first implementation also classified configuration errors with broad regular expressions that did not match the real `ConfigError` display strings. In addition, model-catalog warnings and update-check result messages travel inside successful command results and bypassed the shared formatter.

## Goal

- Show clear English errors when the app language is English.
- Show clear Simplified Chinese errors when the app language is Simplified Chinese.
- Explain what happened and give the user one practical next action.
- Route all user-visible application failures through one formatter.
- Preserve non-secret configuration identifiers that the user needs to repair a pool or rule.
- Keep source-code comments in English, including JSX and HTML comments.

## Scope

This change covers desktop errors shown during startup, configuration, proxy lifecycle operations, provider management, model management, usage and quota loading, Agent routing, model-catalog warnings, and update-check result messages. Comment cleanup covers maintained source files modified by this PR, excluding the protected `crates/router-core/**` tree.

## Non-goals

- Changing backend error types, command contracts, or the frozen Router Core tree.
- Translating log files, protocol values, provider names, model IDs, or test fixtures.
- Hiding request IDs or stable error codes that users need for support.

## Safety and data boundaries

- The formatter must not add API keys, authorization headers, request bodies, or secret-store values to the UI.
- Unknown failures use a localized safe fallback instead of displaying an arbitrary backend string.
- Stable error codes may be displayed, but local paths and transaction snapshots remain internal.
- Existing configuration, credentials, and provider state must not change as part of formatting an error.

## User-visible behavior

The formatter selects one language from the current app language. Each recognized failure contains a plain description and an action. For example, an internal `apply_in_progress` failure becomes an explanation that another configuration update is still running and asks the user to wait before retrying.

If the formatter cannot classify a failure, it shows a safe localized fallback and asks the user to retry or inspect the local logs. It does not mix a Chinese backend sentence into the English UI or an English backend sentence into the Chinese UI.

## State and failure handling

Formatting is pure and does not change application state. Existing retry, save, restart, and recovery controls remain unchanged. Structured request-receipt diagnosis continues to use its more specific error-code guidance.

## Responsive, keyboard, and accessibility requirements

No layout or control changes are introduced. Error text remains in existing banners and panels, so current focus order, keyboard controls, and screen-reader announcement behavior remain unchanged. Text must wrap without requiring horizontal scrolling.

## Public behavior and acceptance criteria

- Every raw application error display covered by this change calls the shared formatter.
- The exact production strings for no pools, an empty pool, and an unknown pool produce actionable localized guidance.
- Known pool and rule identifiers remain visible after localization.
- English mode never receives a Chinese human-readable message from the formatter.
- Simplified Chinese mode never receives an English human-readable message from the formatter.
- Known technical failures produce an explanation and a next action.
- Unknown failures produce a safe localized fallback.
- Existing structured request error-code guidance remains unchanged.
- Comments introduced or modified by this PR are English. Existing comments in the frozen Router Core tree are an explicit protected-baseline exception.

## Implementation locations

- `apps/desktop/src/errors.ts`: shared localized formatter and mappings.
- `apps/desktop/src/components/ProviderModelManager.tsx`: localize warnings returned in successful model-discovery results.
- `apps/desktop/src/pages/About.tsx`: localize update failure messages returned in successful update views.
- Other desktop pages and components: replace raw error rendering with the formatter and pass the selected language.
- `apps/desktop/src/pages/RouterTable.tsx`: translate remaining JSX comments.
- `docs/ui-v2/2026-07-31-桌面端重设计原型.html`: translate the remaining HTML comments.

## Verification and release requirements

After implementation, run focused frontend tests, the full test and build flow required by the repository, then install and launch the desktop app with `scripts/install-local-desktop.sh`. Check at least one known and one unknown failure in both app languages. Record results and remaining gaps here before creating a local commit.

## Implementation status

Complete. Exact production router-error tests, model-catalog/update-result component tests, and safe unknown-error fallback tests pass in both English and Simplified Chinese. The final frontend run passed 29 files and 268 tests; the production build passed. Workspace format, Clippy, tests, and warning-free documentation passed, as did desktop Clippy and the complete desktop test suites.

`scripts/install-local-desktop.sh` built, audited, installed, and launched `/Applications/token-station.app`. In the installed App, the unsupported source-build update result rendered actionable English and Simplified Chinese guidance without a backend sentence or local path. Unknown fallback behavior was verified in public component tests rather than by corrupting persisted user configuration. No formatter-related release gap remains.
