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

  // One rule, not three copies of it. The three workflows drifting apart is
  // how a tag could fail closed in one place and skip silently in another.
  assert.match(
    contents,
    /run: node scripts\/resolve-release-mode\.mjs/,
    `${workflow} must decide its mode with the shared resolver`,
  );

  // The inline shell test the resolver replaced treated "flag is not true" as
  // a source-only run. Nothing may reintroduce it.
  assert.doesNotMatch(
    contents,
    /if \[\[ "\$FORMAL_ARTIFACTS_ENABLED" == "true" \]\]/,
    `${workflow} must not decide the release mode with an inline shell test`,
  );

  // A secret's value must never reach the resolver's environment; only whether
  // it is set.
  const secretValueLeak = /TS_HAS_[A-Z_]+: \$\{\{ secrets\.[A-Z_]+ \}\}/;
  assert.doesNotMatch(
    contents,
    secretValueLeak,
    `${workflow} must pass secret presence, never a secret value`,
  );
}

// A manual run has to say which mode it wants, in every workflow a human can
// start by hand.
for (const workflow of [
  ".github/workflows/release.yml",
  ".github/workflows/desktop-release.yml",
  ".github/workflows/linux-desktop.yml",
]) {
  const contents = read(workflow);
  assert.match(
    contents,
    /options: \[source-only, formal\]/,
    `${workflow} must offer an explicit mode choice`,
  );
}

const notes = read("docs/release/v1.2.0.md");
assert.match(notes, /源码预发布版/);
assert.match(notes, /UNSIGNED-UNNOTARIZED\.dmg/);
assert.match(notes, /未签名[^\u3002]*未公证[^\u3002]*仅供测试/);
assert.match(notes, /不提供[^\u3002]*CLI 二进制/);
assert.doesNotMatch(notes, /Developer ID 签名与 Apple 公证已通过/);

console.log("source release with explicit test DMG mode: PASS");
