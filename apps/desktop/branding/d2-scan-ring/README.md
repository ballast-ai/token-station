# D2 Scan Ring artwork

These self-contained SVG files are the production sources for the Token Station product identity.
The standard mark uses a dark T, an indigo route, and two segmented teal scan rings.
The dark variant uses light foreground colors. The tray source is a monochrome alpha template.
Small variants increase ring weight and spacing for menu and favicon sizes.

Run this command from the repository root on macOS:

```sh
python3 scripts/export-brand-icons.py
```

The exporter requires Python 3, librsvg `rsvg-convert`, and macOS `iconutil`.
It updates the public product artwork, native PNG families, Windows ICO, macOS ICNS, and 36px tray template.
Do not edit generated raster assets independently of these sources.
