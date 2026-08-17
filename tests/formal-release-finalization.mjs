#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

const releaseWorkflow = read(".github/workflows/release.yml");
assert.match(releaseWorkflow, /desktop-macos:/);
assert.match(releaseWorkflow, /needs: \[release-mode, build, reproducibility, desktop-macos\]/);
assert.match(releaseWorkflow, /token-station-desktop-\$\{\{ matrix\.target \}\}/);
assert.match(releaseWorkflow, /apps\/desktop\/src-tauri\/target\/\$\{\{ matrix\.target \}\}\/release\/bundle\/dmg\/\*\.dmg/);
assert.match(releaseWorkflow, /apps\/desktop\/src-tauri\/target\/\$\{\{ matrix\.target \}\}\/release\/bundle\/macos\/\*\.app\.tar\.gz/);
assert.match(releaseWorkflow, /path: desktop-dist\/\*/);
assert.doesNotMatch(releaseWorkflow, /^[ \t]+.*\*\.sig[ \t]*$/m);
assert.match(releaseWorkflow, /Awaiting offline CLI and updater signatures/);

const desktopWorkflow = read(".github/workflows/desktop-release.yml");
assert.match(desktopWorkflow, /workflow_dispatch:/);
assert.doesNotMatch(desktopWorkflow, /push:\s*\n\s*tags:/);

for (const script of [
  "scripts/prepare-formal-release.sh",
  "scripts/sign-formal-release.sh",
  "scripts/publish-formal-release.sh",
]) {
  const contents = read(script);
  assert.match(contents, /^#!\/usr\/bin\/env bash/);
  assert.match(contents, /set -euo pipefail/);
}

const prepare = read("scripts/prepare-formal-release.sh");
assert.match(prepare, /gh release view/);
assert.match(prepare, /isDraft/);
assert.match(prepare, /gh release download/);

const sign = read("scripts/sign-formal-release.sh");
assert.match(sign, /ts-release -- sign/);
assert.match(sign, /signer sign/);
assert.match(sign, /create-desktop-update-manifest\.mjs/);
assert.match(sign, /create-release-checksums\.mjs/);
assert.doesNotMatch(sign, /gh release|curl |wget /);

const publish = read("scripts/publish-formal-release.sh");
assert.match(publish, /ts-release -- verify/);
assert.match(publish, /ts-release -- verify-updater/);
assert.match(publish, /check-release-assets\.mjs/);
assert.match(publish, /gh release upload/);
assert.match(publish, /gh release edit/);
assert.doesNotMatch(publish, /SIGNING_PRIVATE_KEY|release-signing\.key/);

const ci = read(".github/workflows/ci.yml");
assert.match(ci, /node tests\/formal-release-finalization\.mjs/);
const offlinePrime = ci.indexOf("cargo test -p token-station-release --no-run --locked");
const offlineSigning = ci.indexOf("tests/formal-release-signing.sh");
assert.ok(offlinePrime >= 0 && offlinePrime < offlineSigning);

console.log("formal release finalization policy: PASS");
