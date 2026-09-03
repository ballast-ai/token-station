# Direction 05 — Signal Control

## Production method

- Use case: `logo-brand`
- Ideation engine: built-in `image_gen`, one independent call for each of the five concepts.
- Production cleanup: every generated raster was treated only as a structural sketch. The selected ideas were rebuilt as deterministic SVG geometry after a dedicated pause / letter / flag / battery / railway misread audit.
- PNG export: `rsvg-convert` from the matching SVG at 1024 × 1024. The SVG is the source of truth.
- Shared construction: `viewBox="0 0 512 512"`; transparent canvas; `g id="mark"`; only `path`, `rect`, and `polygon`; solid `#182133` graphite and `#D89A2B` signal amber.

The generated ideation files were not copied into the project because they introduced tonal gradients and several forbidden literal readings. The five built-in calls nevertheless completed independently and informed the topology review.

## Actual built-in ideation prompt scaffold

Each built-in call used the following shared labeled fields plus the concept-specific `Primary request` below.

```text
Use case: logo-brand
Asset type: Token Station app logo ideation — Direction 05 Signal Control
Style/medium: Swiss wayfinding discipline crossed with a precision industrial control component; screen-printed vector logo; exact hard-edged geometry; uniform solid fills only; restrained, compact, engineered, memorable.
Composition/framing: one centered icon only, 1:1 canvas, genuinely transparent background, 15% clear safety margin, strong silhouette, no enclosing tile.
Color palette: graphite #182133 and signal amber #D89A2B only; transparency is the only negative space.
Constraints: no text or letters; no gradients, highlights, shadows, glow, texture, 3D, mockup, outlines, dirty alpha, or watermark; thick geometry that survives at 32px and converts cleanly to one color.
Avoid: switch UI, toggle, dashboard, electrical circuit diagram, pause icon, H/E/F letters, arrows, chevrons, circles, dots, rings, central hubs, radial rays, meter arcs, lightning, keys, locks, literal flags, batteries, railway signals, traffic lights, train tracks, and literal tools.
```

## 01 — Signal Break

```text
Primary request: Two equal-thickness horizontal graphite beams separated by one decisive central break. A single small eccentric signal-amber control plate overlaps only part of that break, suggesting phase handoff without reconnecting the whole gap.
```

Ideation finding: built-in `image_gen` produced a shaded diagonal plate across a horizontal beam. Its color hierarchy was useful, but the generated composition read as a `T` and used gradients.

Final production redraw: replace the horizontal construction with two independent 16° graphite parallelogram beams. Both share one axis angle but are offset in x and y. A shorter amber parallelogram follows the same angle, enters only the first part of the break, and remains separated from the second beam.

Acceptance: no perpendicular join, `T`, cross, pause pair, arrow, or circuit symbol; 64 px graphite depth and 48 px amber depth; extents remain inside x=72–440 and y=160–352.

## 02 — Gate Latch

```text
Primary request: A dark graphite L-shaped support and one separate dark graphite stop block frame an open rectangular void without closing it. A thick signal-amber latch enters that void from one side but deliberately stops short, leaving a visible calibrated gap.
```

Ideation finding: built-in `image_gen` returned a shaded `C`-like enclosure. The first SVG cleanup still read as `U`, so the production topology was replaced.

Final production redraw: exactly three separated pieces — one graphite L support, one vertically offset graphite stop that leaves a 48 px gap above the bottom beam, and one free 16° amber latch. The latch touches neither graphite piece.

Acceptance: no `U`, magnet, folder, lock, gate, or toggle reading; minimum solid thickness 56 px; at least 40 px separation around the amber latch and between the stop and bottom support; extents x=88–432 and y=104–400.

## 03 — Detent Lever

```text
Primary request: A single thick diagonal graphite control beam with no circular pivot. Cut two or three large square detent notches into one side edge, spaced irregularly enough to avoid a comb. Make only the middle selected contact face signal amber, flush with the beam rather than attached as a knob.
```

Ideation finding: built-in `image_gen` found the diagonal detent metaphor, but the generated beam was shaded and tool-like. The first cleanup bent into a checkmark / receiver silhouette and was discarded.

Final production redraw: one complete 336 × 96 px beam rotated 18°. Three single-side detents have deliberately unequal 36, 48, and 40 px depths. The middle 48 × 48 px detent is occupied by a flush amber insert, while the other two remain transparent.

Acceptance: one unbent primary mass with flat ends; no pivot, checkmark, wrench, receiver, saw, or fine comb; every detent is at least 36 px deep and 44 px wide; rotated extents remain inside the 72–440 safe area.

## 04 — Relay Flag

```text
Primary request: One off-center thick vertical graphite spine and two solid horizontal graphite signal plates of clearly different widths on one side. Add a separate signal-amber takeover plate on the opposite side at a third height. Keep the full composition intentionally asymmetric and do not draw a literal flag.
```

Ideation finding: built-in `image_gen` returned a rounded literal mast / flag construction. The first orthogonal cleanup read as `h/F`, so its topology was abandoned.

Final production redraw: a 56 px off-center spine rotates 12° from vertical. Three 56 px signal boards cross it at the reciprocal 12° angle, alternate right / left / right, and use different lengths of 176, 184, and 144 px. The middle takeover board alone is amber.

Acceptance: no vertical pole, cloth, `h/F/H`, semaphore, or traffic signal; deliberate top/middle/bottom spacing prevents a center ray cluster; all transformed extents remain inside 76–440.

## 05 — Quiet Beacon

```text
Primary request: A compact vertical graphite mass with an intentionally stepped, non-battery outer silhouette. Cut two asymmetric horizontal negative slots into its interior at different lengths and offsets. Insert one flat signal-amber plane into only part of one slot, leaving open negative space around it; the other slot stays empty.
```

Ideation finding: built-in `image_gen` produced a shaded bottle / battery silhouette. The first cleanup removed the terminal but still read as a block `E`, so both the outline and slot topology were replaced.

Final production redraw: two thick offset slant-cut graphite plates overlap to form a compact near-square mass. The upper slot falls about 7° while the lower slot rises about 11°, so the two large apertures are not parallel. Amber occupies only 72 px of the 160 px upper slot; the lower slot remains fully transparent.

Acceptance: no three-horizontal-bar `E`, battery cap, beacon light, dashboard, or enclosure tile; both slot depths are 48 px; plate extents remain inside x=88–424 and y=72–440.

## Final verification

- All five SVG files parse with `xmllint` and contain only allowed inline vector geometry inside `g#mark`.
- No root width/height, text, font, external image, `use`, gradient, filter, mask, clip path, shadow, stroke, opacity, or background tile is present.
- The only solid colors are `#182133` and `#D89A2B`; canvas negative space is transparent.
- All five PNGs were regenerated from the final SVGs at 1024 × 1024 and report RGBA alpha.
- Dedicated 32 × 32 renders were visually checked after the final topology revision; the break, latch, detents, alternating relay boards, and two nonparallel slots remain distinguishable.
- The final marks remain structurally valid when both fills are converted to a single monochrome color.
