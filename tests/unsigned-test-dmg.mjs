#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");
const filename = "token-station_1.2.0_aarch64_UNSIGNED-UNNOTARIZED.dmg";
const terminalCommand =
  'sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"';

const desktopBuilder = read("scripts/build-desktop.sh");
assert.match(desktopBuilder, /rustc --print sysroot/);
assert.match(desktopBuilder, /--remap-path-prefix=\$rust_sysroot=\/rustc/);

const desktopAuditor = read("scripts/audit-desktop-artifact.sh");
assert.match(desktopAuditor, /--rust-sysroot/);
assert.match(desktopAuditor, /leaks the Rust sysroot path/);

const packager = read("scripts/package-macos-dmg.sh");
for (const pattern of [
  /--unsigned-test/,
  /UNSIGNED-UNNOTARIZED/,
  /--app-source-commit/,
  /--source-tag/,
  /preview-v/,
  /packaging_source_commit/,
  /\.release-metadata\/unsigned-test-warning\.txt/,
  /\.release-metadata\/provenance\.txt/,
  /notarytool submit/,
  /stapler staple/,
  /-format UDRW/,
  /hdiutil attach "\$writable_dmg" -readwrite -nobrowse -mountpoint "\$layout_mount"/,
  /osascript "\$finder_layout_script" configure "\$layout_mount"/,
  /layout_mount\/\.DS_Store/,
  /layout_mount\/\.fseventsd/,
  /hdiutil convert[\s\S]*-format UDZO/,
]) {
  assert.match(packager, pattern);
}
assert.doesNotMatch(packager, /README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/);

const auditor = read("scripts/audit-macos-dmg.sh");
for (const pattern of [
  /--unsigned-test/,
  /UNSIGNED-UNNOTARIZED/,
  /Signature=adhoc/,
  /mounted_warning/,
  /mounted_provenance/,
  /mounted_background/,
  /stapler validate/,
  /mounted_ds_store/,
  /configure-dmg-layout\.applescript[\s\S]*inspect/,
  /\.fseventsd/,
]) {
  assert.match(auditor, pattern);
}
assert.doesNotMatch(auditor, /README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/);

const finderLayout = read("packaging/macos/configure-dmg-layout.applescript");
assert.match(finderLayout, /folder mountAlias/);
assert.match(finderLayout, /current view to icon view/);
assert.match(finderLayout, /arrangement to not arranged/);
assert.match(finderLayout, /icon size to 128/);
assert.match(finderLayout, /toolbar visible to false/);
assert.match(finderLayout, /sidebar width to 0/);
assert.match(finderLayout, /set bounds to \{100, 100, 1280, 740\}/);
assert.match(finderLayout, /backgroundFile/);
assert.match(finderLayout, /position of item "token-station\.app" .*\{300, 240\}/);
assert.match(finderLayout, /position of item "Applications" .*\{880, 240\}/);
assert.match(finderLayout, /position of item "README\.md" .*\{590, 455\}/);

const readme = read("packaging/macos/README.md");
for (const phrase of ["Install Token Station", "unsigned and not notarized", "SHA-256"]) {
  assert.ok(readme.includes(phrase), `README.md 缺少“${phrase}”`);
}
assert.deepEqual(readme.split(/\r?\n/).filter((line) => line.startsWith("sudo ")), [terminalCommand]);
assert.doesNotMatch(readme, /spctl\s+--master-disable/);

const notes = read("docs/release/v1.2.0.md");
for (const phrase of [filename, "未签名", "未公证", "仅供测试", ".sha256"]) {
  assert.ok(notes.includes(phrase), `Release 说明缺少“${phrase}”`);
}
assert.match(notes, /不提供[^。]*CLI 二进制/);

console.log("unsigned test DMG policy: PASS");
