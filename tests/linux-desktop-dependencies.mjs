import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

// Check the complete lockfile, including platform and optional dependencies.
// A second, inactive legacy GLib family is still an unresolved dependency.
const lock = readFileSync("apps/desktop/src-tauri/Cargo.lock", "utf8");
const packages = [...lock.matchAll(/\[\[package\]\]\r?\n([\s\S]*?)(?=\r?\n\[\[package\]\]|$)/g)].map(
  ([, block]) => {
    const field = (name) => block.match(new RegExp(`^${name} = "([^"]+)"$`, "m"))?.[1];
    return { name: field("name"), version: field("version"), source: field("source"), checksum: field("checksum") };
  },
);
const core = new Set([
  "glib", "glib-sys", "glib-macros", "gobject-sys", "gio", "gio-sys",
  "gdk-pixbuf", "gdk-pixbuf-sys", "pango", "pango-sys", "cairo-rs", "cairo-sys-rs",
]);
const gtk = new Set(["gtk", "gtk-sys", "gtk3-macros", "gdk", "gdk-sys", "gdkx11", "gdkx11-sys", "gdkwayland", "gdkwayland-sys", "atk", "atk-sys"]);
assert(packages.some(({ name }) => name === "glib"), "The desktop must retain its real Linux GLib implementation.");
assert(packages.some(({ name }) => name === "gtk"), "The desktop must retain its Linux GTK implementation.");
for (const pkg of packages) {
  if (!core.has(pkg.name) && !gtk.has(pkg.name)) continue;
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(pkg.version ?? "");
  assert(match, `Unexpected ${pkg.name} version: ${pkg.version}`);
  const [major, minor] = match.slice(1).map(Number);
  const minimum = core.has(pkg.name) ? 22 : 19;
  assert(major > 0 || minor >= minimum, `Legacy Linux dependency remains: ${pkg.name} ${pkg.version}`);
  assert.equal(pkg.source, "registry+https://github.com/rust-lang/crates.io-index", `${pkg.name} must use the genuine published implementation.`);
  assert.match(pkg.checksum ?? "", /^[a-f0-9]{64}$/, `${pkg.name} needs its upstream archive checksum.`);
}
console.log("Linux desktop dependencies: PASS (published GTK 0.19 / GLib 0.22 family, no legacy copies)");
