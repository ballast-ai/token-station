#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

for (const workflow of [
  ".github/workflows/release.yml",
  ".github/workflows/desktop-release.yml",
  ".github/workflows/linux-desktop.yml",
]) {
  const contents = read(workflow);
  assert.match(contents, /TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED/);
  assert.match(contents, /needs\.release-mode\.outputs\.enabled == 'true'/);
}

const notes = read("docs/release/v1.1.3.md");
assert.match(notes, /源码 Release/);
assert.match(notes, /UNSIGNED-UNNOTARIZED\.dmg/);
assert.match(notes, /未签名[^\u3002]*未公证[^\u3002]*仅供测试/);
assert.match(notes, /不提供[^\u3002]*CLI 二进制/);
assert.doesNotMatch(notes, /Developer ID 签名与 Apple 公证已通过/);

console.log("source release with explicit test DMG mode: PASS");
