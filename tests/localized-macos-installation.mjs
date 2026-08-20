#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");
const exists = (relativePath) => fs.existsSync(path.join(root, relativePath));
const terminalCommand =
  'sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"';

const readme = read("packaging/macos/README.md");
for (const phrase of [
  "Install Token Station",
  "official GitHub Release",
  "unsigned and not notarized",
  "Terminal hides password characters",
]) {
  assert.ok(readme.includes(phrase), `README.md is missing: ${phrase}`);
}
assert.deepEqual(
  readme.split(/\r?\n/).filter((line) => line.startsWith("sudo ")),
  [terminalCommand],
  "README.md must contain one canonical copyable command",
);
assert.doesNotMatch(readme, /spctl\s+--master-disable/);
assert.doesNotMatch(readme, /xattr[^\n]*(?:\/Applications[\s"']|~\/|\$HOME)/);

for (const removedPath of [
  "packaging/macos/README.zh-CN.md",
  "packaging/macos/Install Token Station.command",
  "packaging/macos/安装 Token Station.command",
  "packaging/macos/终端启动命令.txt",
]) {
  assert.equal(exists(removedPath), false, `${removedPath} must not be a separate DMG entry`);
}

const packager = read("scripts/package-macos-dmg.sh");
assert.match(packager, /cp "\$readme" "\$stage\/README\.md"/);
assert.match(packager, /mkdir "\$stage\/\.background"/);
assert.match(packager, /mkdir "\$stage\/\.release-metadata"/);
assert.doesNotMatch(packager, /README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/);

const auditor = read("scripts/audit-macos-dmg.sh");
assert.match(auditor, /mounted_readme="\$mount_point\/README\.md"/);
assert.match(auditor, /expected_entries=/);
assert.doesNotMatch(auditor, /README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/);

const finderLayout = read("packaging/macos/configure-dmg-layout.applescript");
assert.match(finderLayout, /position of item "README\.md"/);
assert.doesNotMatch(finderLayout, /README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/);

console.log("single concise macOS installation guide: PASS");
