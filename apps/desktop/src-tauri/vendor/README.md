# Adapted Linux desktop dependencies

These packages retain their published upstream names, versions, licenses, and source ownership.
They are locally adapted packages, not new upstream releases.
The desktop manifest selects them with explicit Cargo path patches.

The adaptations migrate GTK3 consumers to published GTK 0.19 and GLib 0.22.
The actual GLib, GTK, JavaScriptCore, and Soup implementations come from crates.io with their authentic archive checksums.
No affected GLib implementation or modified GLib version label is included here.
The WebView, window, menu, tray, and dialog features remain enabled.

`provenance.json` records the exact upstream archives and complete original source inventories.
`patches/` records every local source change.
Upstream consumer migration commits are identified in the provenance records when their code is backported.
Dependency lockfiles from upstream archives are omitted. Cargo uses the desktop lockfile for these packages.
All other upstream files and licenses remain present unless an exact adaptation patch records the change.

Run `python3 scripts/check-desktop-vendor.py` from the repository root.
The check verifies archive checksums, reconstructs each source tree, applies the exact patch, and compares every local file.
It rejects undeclared files, missing files, changed package identities, unexpected patch sources, and symlinks.
Run `node tests/linux-desktop-dependencies.mjs` to reject legacy or substituted GTK and GLib implementations throughout the desktop lockfile.

When compatible upstream releases support the repaired dependency family, replace the affected path patches with those releases.
Repeat Linux build, runtime, IPC, desktop tests, source verification, and strict security checks before removing an adaptation.
