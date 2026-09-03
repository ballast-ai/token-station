# Adoption notes — where the chosen mark must land

This exploration does **not** replace production assets yet. Choosing a candidate is only the first step: Token Station currently has several independent logo surfaces that must be updated together.

## Product surfaces found in the source

- `apps/desktop/public/icon.png` — shared in-product mark and README image; loaded by `TokenStationMark.tsx`.
- `apps/desktop/src/components/LaunchScreen.tsx` — contains a separate hard-coded inline SVG. Replacing only `icon.png` would leave the old switch artwork on launch.
- `apps/desktop/src-tauri/icons/icon-light.png` and `icon-dark.png` — runtime Dock variants selected by `src-tauri/src/dock_icon.rs`.
- `apps/desktop/src-tauri/icons/tray-template.svg` / `.png` — one-colour menu-bar or system-tray artwork.
- `apps/desktop/src-tauri/icons/32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, and `icon.ico` — package icons referenced by `src-tauri/tauri.conf.json`.
- `apps/desktop/src-tauri/icons/Square*Logo.png` and `StoreLogo.png` — Windows tile/store variants.

## Recommended adoption sequence

1. Select one mark and refine its geometry at 512 units.
2. Produce three masters: full-colour light, full-colour dark, and one-colour tray/template. This exploration already includes geometry-identical dark candidates under `dark-variants/`.
3. Test the mark inside a macOS-style rounded app tile at 16, 32, 64, 128, and 256 px.
4. Replace the inline launch SVG and shared web icon from the same geometry source.
5. Regenerate the complete Tauri/macOS/Windows icon matrix; do not hand-edit individual raster sizes.
6. Run the existing `TokenStationMark` test and visual smoke tests on launch, sidebar, Dock, installer, and tray.

## Important constraint

The current product combines a warm ink/vermilion launch identity with a cool grey/indigo application UI. The source ink colours (`#15181B` / `#182133`) measure only about `1.02–1.12:1` against the dark canvas (`#111620`), so one unchanged colour asset cannot serve both themes. The supplied dark variants use `#EEF2F8` for approximately `16.12:1` contrast while keeping geometry and accent colours unchanged. The eventual winner still needs one coherent brand-colour decision across launch, app UI, Dock, installer, and tray.
