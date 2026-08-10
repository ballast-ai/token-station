# Localized Human-Readable Errors and English Comments

## Problem

Several desktop pages render backend failures with `String(error)`. This can expose implementation terms such as `generation`, `listener`, `model_providers`, and transaction state names. The result may also use a different language from the language selected in the app. A previous comment cleanup also missed JSX and HTML comment syntax.

## Goal

- Show clear English errors when the app language is English.
- Show clear Simplified Chinese errors when the app language is Simplified Chinese.
- Explain what happened and give the user one practical next action.
- Route all user-visible application failures through one formatter.
- Keep source-code comments in English, including JSX and HTML comments.

## Scope

This change covers desktop errors shown during startup, configuration, proxy lifecycle operations, provider management, model management, usage and quota loading, and Agent routing. It also covers remaining Chinese comments in maintained source and the UI prototype.

## Non-goals

- Changing backend error types or command contracts.
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
- English mode never receives a Chinese human-readable message from the formatter.
- Simplified Chinese mode never receives an English human-readable message from the formatter.
- Known technical failures produce an explanation and a next action.
- Unknown failures produce a safe localized fallback.
- Existing structured request error-code guidance remains unchanged.
- All maintained source comments are English.

## Implementation locations

- `apps/desktop/src/errors.ts`: shared localized formatter and mappings.
- Desktop pages and components: replace raw error rendering with the formatter and pass the selected language.
- `apps/desktop/src/pages/RouterTable.tsx`: translate remaining JSX comments.
- `docs/ui-v2/2026-07-31-桌面端重设计原型.html`: translate the remaining HTML comments.

## Verification and release requirements

After implementation, run focused frontend tests, the full test and build flow required by the repository, then install and launch the desktop app with `scripts/install-local-desktop.sh`. Check at least one known and one unknown failure in both app languages. Record results and remaining gaps here before creating a local commit.

## Implementation status

Implementation is complete. The focused localized-error suite passed 3 of 3 tests. The Rust workspace suite passed. The full frontend suite passed 241 of 242 tests; the remaining unrelated test reports that selecting `gemma4:latest` moves it behind `qwen3:4b-fast` instead of preserving its list position. The local desktop release build, code-signing check, and artifact audit passed. Per the user's instruction, the built app was not installed or launched and `/Applications/token-station.app` was not replaced.
