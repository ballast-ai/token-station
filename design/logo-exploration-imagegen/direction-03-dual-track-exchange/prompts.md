# Direction 03 — Dual-track Exchange

## Production method

- Use case: `logo-brand`
- Ideation engine: built-in `image_gen`, one independent call per concept.
- Cleanup: the generated rasters were used only as structural sketches. Each selected idea was rebuilt as deterministic SVG geometry to remove generated gradients, highlights, edge noise, checkerboard pseudo-transparency, arrowheads, and accidental overlap.
- PNG export: `rsvg-convert` from the matching SVG at 1024 × 1024. The SVG is the source of truth.
- Shared construction: `viewBox="0 0 512 512"`; transparent canvas; `group id="mark"`; no text, fonts, external images, `use`, gradients, filters, masks, shadows, or background tile; only solid `#182133`, `#5368E8`, and where noted `#55C7E8`.

## Shared brief

```text
Use case: logo-brand
Asset type: Token Station app logo concept
Primary request: Create exactly one original abstract symbol for Token Station, a local-first desktop router for AI requests.
Style/medium: screen-printed vector logo; exact hard-edged geometry; uniform solid fills only; compact industrial wayfinding; strong silhouette; scalable and convertible to pure black and white.
Composition/framing: exactly one centered symbol on a genuinely transparent background; square 1:1 canvas; 15% clear safety margin on every side; no wordmark, caption, extra variants, presentation sheet, or background tile.
Palette: graphite #182133 with cobalt #5368E8 or ice cyan #55C7E8; transparent negative space only.
Constraints: constant visual weight; large readable apertures; clean alpha; readable at 32 px.
Avoid: gradients, color blending, highlights, shadows, glow, texture, noise, dirty alpha, 3D, mockups, rounded-square backgrounds, text, letters, monograms, numbers, arrows, arrowheads, chevrons, nodes, dots, center rays, AI sparkles, starbursts, circles, rings, infinity symbols, chain links, hexagons, cubes, literal trains, lightning bolts, plugs, keys, and watermarks.
```

## 01 — Square Crossover

```text
Primary request: Square Crossover. Two equal-weight routes exchange upper and lower positions around a crisp square transparent core. Use hard 45-degree transitions and four flat terminals. The two colors must remain separate and must not cross, overlap, or imply foreground/background layering.
Palette: graphite #182133 and cobalt #5368E8 only.
```

Ideation: built-in `image_gen`; the initial and targeted correction both introduced tonal gradients, so only the four-terminal crossover structure was retained.

Acceptance: two color-coded diagonal route bodies, each expressed as one aligned pair across the exchange core; 48 × 48 px square negative core; flat terminals; minimum structural thickness 40 px; extents remain inside the 72–440 safe area. The long diagonals suppress the previous four-hook/network reading.

## 02 — Offset Exchange

```text
Primary request: Offset Exchange. Use two equal-weight folded route paths that visually exchange upper and lower positions across a tall transparent switch slot. Every route contains a 45-degree segment. Keep the two colors separated with no crossing, touching, complete T, cross, arrowhead, or foreground/background layering.
Palette: graphite #182133 and cobalt #5368E8 only.
```

Ideation: built-in `image_gen`; its arrow-like main-axis endpoints, complete T silhouette, and gradient rendering were discarded. The final cleanup uses two optically continuous diagonal pairs with a tall offset switch slot.

Acceptance: equal 40–56 px visual weight; both routes include a 45-degree fold; 56 px-wide central negative slot; no complete T or cross; flat ends; extents inside 72–440.

## 03 — Relay Pair

```text
Primary request: Relay Pair. Construct the mark from exactly two thick separate open square-bracket plates. Their stepped jaws interlock around a square transparent core but never touch, overlap, close into loops, or resemble chain links. Preserve four open terminal zones.
Palette: graphite #182133 and ice cyan #55C7E8 only.
```

Ideation: built-in `image_gen`; the generated brackets were too smooth, oversized, and shaded. The final version uses two hard-edged relay plates with staggered jaws.

Acceptance: exactly two independent pieces; 48 px central square core; 40 px top and bottom separation; all narrow structural parts at least 48 px; extents 88–424.

## 04 — Platform Transfer

```text
Primary request: Platform Transfer. Use exactly two continuous orthogonal staircase tracks. Both enter from the left and exit on the right at different heights; the upper graphite rail steps down at an earlier position, while the lower ice-cyan rail steps up later. They must stay separate and must not repeat by rotation, surround a center, or form a four-way radial symbol.
Palette: graphite #182133 and ice cyan #55C7E8 only.
```

Ideation: built-in `image_gen`; its diagonal X and four-way centered construction were rejected. The final cleanup uses only two continuous, independently positioned orthogonal platform tracks.

Acceptance: exactly two continuous tracks; 48 px terminal thickness; graphite changes level at x=184 and ice cyan at x=312, so they are not rotated copies; a 56 px horizontal negative channel separates them; extents inside 72–440.

## 05 — Fallback Switch

```text
Primary request: Fallback Switch. Make an asymmetric failover mark. A graphite primary route stays visually continuous from lower-left to upper-right. A short cobalt route appears only around the central control core to take over for a bounded interval; it must not reach the outer canvas edges.
Palette: graphite #182133 and cobalt #5368E8 only.
```

Ideation: built-in `image_gen`; its separated diagonal inserts and gradient shading were discarded. The final mark uses one dominant graphite switch body and one compact cobalt takeover bracket.

Acceptance: graphite remains continuous and reaches the outer terminals; cobalt is confined to the center; 64 × 64 px square core; both pieces use 56 px modules; asymmetric extents remain inside 72–440.

## Final verification

- SVG source is transparent, self-contained, and contains no textual element.
- Only `path` and `rect` geometry appears inside `g#mark`.
- No gradient, filter, shadow, raster embedding, or external dependency.
- PNGs are exported directly from the SVG source at 1024 × 1024 with alpha.
- Every mark remains recognizable after 32 × 32 downscaling and can be converted to one-color black without changing its geometry.
