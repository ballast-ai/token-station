# Token Station — Logo Exploration v2

This set contains five intentionally different visual directions and five candidates per direction. Each candidate was first explored with built-in image generation, then rebuilt as deterministic SVG geometry for production use.

## Deliverables

- `gallery.html` — interactive light/dark and 64/32/16 px comparison;
- `board-light.png` / `board-dark.png` — static 5 × 5 review boards;
- `small-size-qa.png` — native 64/32/16 px light/dark QA sheet;
- `research.md` — project and competitor audit;
- `selection-matrix.md` — full 25-candidate scorecard;
- `shortlist.md` — final adjudication and refinement recommendation;
- `adoption-notes.md` — exact source-code surfaces affected by a future replacement;
- `dark-variants/` — geometry-identical high-contrast variants for the app's dark canvas;
- `direction-01-switch-cut/` — evolutionary switch/cut marks;
- `direction-02-local-aperture/` — local-first containment marks;
- `direction-03-dual-track-exchange/` — two-route exchange marks;
- `direction-04-token-rack/` — machine-readable token/unit marks;
- `direction-05-signal-control/` — precise industrial-control marks.

Every candidate is supplied as:

- editable SVG (`viewBox="0 0 512 512"`, transparent);
- 1024 × 1024 transparent PNG;
- a generation and cleanup record in the direction's `prompts.md`.

The dark-adaptive set replaces only the main ink colour with `#EEF2F8`; geometry and accent colours are unchanged. This is necessary because the source ink colours have only about `1.02–1.12:1` contrast against the application's `#111620` dark canvas.

## Selection criteria

The shortlist is not chosen by decoration alone. A viable Token Station mark must pass all five checks:

1. **Distinctive silhouette** — recognisable without colour and not built from a familiar AI-logo cliché.
2. **Correct product meaning** — controlled routing, local ownership, or dependable infrastructure; not crypto, chat, security software, sync, or railway software.
3. **Small-size survival** — readable at 16–32 px with no fragile gaps or tiny components.
4. **System fit** — works on the app's light and dark canvases and can become a one-colour tray icon.
5. **Competitor distance** — avoids OpenRouter's geometric letter territory, CC Switch's centre burst, LiteLLM's train, Portkey's portal chevron, and common node/hex/cube motifs.

## Result

- **Visual Champion:** D05-05 Quiet Beacon
- **Semantic Champion:** D03-05 Fallback Switch

Neither is production-ready without a focused refinement pass. See `shortlist.md` for the decision and `selection-matrix.md` for the complete scoring rationale.
