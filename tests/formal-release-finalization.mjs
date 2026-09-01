#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

const releaseWorkflow = read(".github/workflows/release.yml");
const linuxWorkflow = read(".github/workflows/linux-desktop.yml");
assert.match(releaseWorkflow, /desktop-macos:/);
assert.match(releaseWorkflow, /desktop-windows:/);
assert.match(releaseWorkflow, /pull-requests: read/);
assert.match(releaseWorkflow, /release-target:\n    runs-on:/);
assert.match(releaseWorkflow, /release_tag:/);
assert.match(releaseWorkflow, /git merge-base --is-ancestor "\$sha" origin\/main/);
assert.match(releaseWorkflow, /checkout_ref: \$\{\{ needs\.release-target\.outputs\.sha \}\}/);
assert.match(
  releaseWorkflow,
  /gh api \\\n\s+--method GET \\\n\s+"repos\/\$\{\{ github\.repository \}\}\/actions\/workflows\/full-ci\.yml\/runs"/,
);
assert.match(releaseWorkflow, /verify-main-full-ci:\n    needs: release-target\n    runs-on:/);
assert.match(releaseWorkflow, /platform-gates:\n    needs: release-target\n    uses: \.\/\.github\/workflows\/platform\.yml/);
assert.match(releaseWorkflow, /needs: \[release-target, release-mode, verify-main-full-ci, platform-gates, linux-desktop, build, reproducibility, desktop-macos, desktop-windows\]/);
assert.match(releaseWorkflow, /token-station-desktop-\$\{\{ matrix\.target \}\}/);
assert.match(releaseWorkflow, /apps\/desktop\/src-tauri\/target\/\$\{\{ matrix\.target \}\}\/release\/bundle\/dmg\/\*\.dmg/);
assert.match(releaseWorkflow, /apps\/desktop\/src-tauri\/target\/\$\{\{ matrix\.target \}\}\/release\/bundle\/macos\/\*\.app\.tar\.gz/);
assert.match(releaseWorkflow, /path: desktop-dist\/\*/);
for (const asset of ["x86_64.msi", "x86_64.deb", "x86_64.AppImage", "x86_64.rpm"]) {
  assert.match(releaseWorkflow + linuxWorkflow, new RegExp(`token-station_\\$\\{version\\}_${asset.replace(".", "\\.")}`));
}
assert.doesNotMatch(releaseWorkflow, /^[ \t]+.*\*\.sig[ \t]*$/m);
assert.match(releaseWorkflow, /Awaiting offline CLI and updater signatures/);

const desktopWorkflow = read(".github/workflows/desktop-release.yml");
assert.match(desktopWorkflow, /workflow_dispatch:/);
assert.match(
  desktopWorkflow,
  /gh api \\\n\s+--method GET \\\n\s+"repos\/\$\{\{ github\.repository \}\}\/actions\/workflows\/full-ci\.yml\/runs"/,
);
assert.doesNotMatch(desktopWorkflow, /push:\s*\n\s*tags:/);
assert.match(desktopWorkflow, /verify-main-full-ci:\n    runs-on:/);
assert.match(desktopWorkflow, /platform-gates:\n    uses: \.\/\.github\/workflows\/platform\.yml/);
assert.match(
  desktopWorkflow,
  /needs: \[release-mode, macos-preflight, verify-main-full-ci, platform-gates\]/,
);
assert.match(
  desktopWorkflow,
  /needs: \[release-mode, windows-preflight, verify-main-full-ci, platform-gates\]/,
);
assert.doesNotMatch(desktopWorkflow, /if: \$\{\{ false \}\}/);

assert.match(linuxWorkflow, /workflow_call:/);
assert.match(linuxWorkflow, /checkout_ref:/);
assert.doesNotMatch(linuxWorkflow, /push:\s*\n\s*tags:/);
assert.match(releaseWorkflow, /linux-desktop:\n    needs: \[release-target, verify-main-full-ci, platform-gates\]\n    uses: \.\/\.github\/workflows\/linux-desktop\.yml/);

for (const script of [
  "scripts/release-latest-formal.sh",
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
assert.match(publish, /--notes-file/);
assert.doesNotMatch(publish, /SIGNING_PRIVATE_KEY|release-signing\.key/);

const entry = read("scripts/release-latest-formal.sh");
assert.match(entry, /start\) start_release/);
assert.match(entry, /prepare\) prepare_release/);
assert.match(entry, /sign\) sign_release/);
assert.match(entry, /publish\) publish_release/);
assert.match(entry, /branch --show-current/);
assert.match(entry, /status --porcelain/);
assert.match(entry, /rev-parse "\$remote\/main"/);
assert.match(entry, /event=push/);
assert.match(entry, /conclusion == "success"/);
assert.match(entry, /--confirm \$tag/);
assert.match(entry, /check-formal-release-notes\.mjs/);
assert.match(entry, /release transfer directory must be outside the source checkout/);
assert.match(entry, /TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED/);
assert.match(entry, /TOKEN_STATION_RELEASE_PUBKEY_HEX/);
assert.match(entry, /TOKEN_STATION_UPDATER_PUBKEY/);
assert.match(entry, /APPLE_CERTIFICATE_PASSWORD/);
assert.match(entry, /if \[\[ "\$version" != "2\.0\.0" \]\]/);
assert.match(entry, /WINDOWS_CERTIFICATE_PASSWORD/);
assert.match(entry, /gh secret list/);
assert.match(entry, /gh run watch/);
assert.match(entry, /isDraft,isPrerelease/);

assert.match(releaseWorkflow, /--production --unsigned-windows --target x86_64-pc-windows-msvc/);
assert.match(releaseWorkflow, /needs\.release-target\.outputs\.tag != 'v2\.0\.0'/);
assert.match(releaseWorkflow, /scripts\/build-desktop\.sh --production --target x86_64-pc-windows-msvc/);

const desktopBuild = read("scripts/build-desktop.sh");
assert.match(desktopBuild, /--unsigned-windows/);
assert.match(desktopBuild, /restricted to Token Station 2\.0\.0/);
assert.match(desktopBuild, /production Windows build needs WINDOWS_CERTIFICATE_THUMBPRINT/);
assert.match(read(".github/workflows/full-ci.yml"), /tests\/windows-authenticode-audit\.sh/);

const inTreeTransfer = spawnSync(
  path.join(root, "scripts/release-latest-formal.sh"),
  ["prepare", "--dir", path.join(root, "release-transfer")],
  { cwd: root, encoding: "utf8" },
);
assert.equal(inTreeTransfer.status, 1);
assert.match(inTreeTransfer.stderr, /release transfer directory must be outside/);

const ci = read(".github/workflows/full-ci.yml");
assert.match(ci, /node tests\/formal-release-finalization\.mjs/);
const offlinePrime = ci.indexOf("cargo test -p token-station-release --no-run --locked");
const offlineSigning = ci.indexOf("tests/formal-release-signing.sh");
assert.ok(offlinePrime >= 0 && offlinePrime < offlineSigning);

console.log("formal release finalization policy: PASS");
