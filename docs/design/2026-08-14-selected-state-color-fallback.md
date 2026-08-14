# Selected State Color Fallback

## Problem

Vite adds a solid-color fallback for each `color-mix()` declaration. Some macOS WebViews use this fallback. Selected route controls then use a solid blue background. The applied-target label uses a solid green background, which can hide its text.

## Goal

Keep selected controls readable in every supported WebView. Match the existing light selected state in the installed desktop app.

## Scope

This change covers these selectors:

- `.routing-mode-tabs [data-slot="tabs-trigger"]` in its active state.
- `.direct-provider-row.selected`.
- `.direct-provider-row:hover:not(.unavailable)`.
- `.direct-applied-target`.
- `.error-toast` in its information, success, and error states.

The change adds stable light-theme and dark-theme color tokens for their backgrounds, borders, and shadows.

## Non-goals

- Do not change routing behavior.
- Do not change provider selection behavior.
- Do not change semantic success colors in logs, health badges, or usage charts.
- Do not redesign other buttons or cards.

## Safety and Data Boundaries

The change only modifies CSS and its public behavior test. It does not read or write credentials, provider data, Agent settings, or request logs.

## User-visible Behavior

An active routing mode uses a light signal surface with dark text. A selected provider row uses the same light signal surface and keeps its left signal border. A provider row uses a lighter signal surface on hover. The applied-target label uses a light success surface with visible success text.

Each toast uses a light surface for its semantic tone. Its text and close control stay visible.

The dark theme uses darker tinted surfaces. It keeps the same state meaning and readable text.

## Failure Handling

The critical selected, hover, applied-target, and toast rules must not use `color-mix()` for their background, border, or shadow. A WebView without `color-mix()` support must show the same state structure.

## Responsive, Keyboard, and Accessibility Requirements

The change must not modify layout, wrapping, or responsive breakpoints. Existing radio labels and selection states must stay available to assistive technology. Existing `:focus-visible` and `:focus-within` outlines must remain visible.

## Public Test Boundary

The theme style test must verify that:

1. The critical rules use stable theme tokens.
2. The critical rules do not use `color-mix()`.
3. Light and dark theme values exist for the new success and selection tokens.

## Acceptance Criteria

1. The routing-mode selection has no solid blue fill.
2. The selected provider row has no solid blue fill.
3. The applied-target label has no solid green fill.
4. A provider row has no solid blue fill on hover.
5. Information, success, and error toasts have no solid semantic-color fill.
6. Text stays visible in all affected states.
7. The focused control keeps a visible outline.
8. The focused tests pass.
9. The real Tauri development window matches the installed app reference.

## Implementation Points

- Add stable state tokens to `apps/desktop/src/App.css`.
- Use the tokens in the three critical selectors.
- Extend `apps/desktop/src/theme-styles.test.ts`.

## Release Requirement

Run the focused frontend test and inspect the real Tauri window. Keep the current installed app until the candidate passes visual inspection.

## Implementation Status

Implemented. The selected route controls, selected provider row, provider hover state, applied-target label, and semantic toasts now use stable theme tokens. The focused theme tests passed after the fallback rules were added.
