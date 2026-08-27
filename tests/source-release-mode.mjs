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

const desktopWorkflow = read(".github/workflows/desktop-release.yml");
assert.match(
  desktopWorkflow,
  /platform:\n(?: {8}.*\n)*? {8}default: macos\n {8}options: \[macos, windows, all\]/,
  "desktop release dispatch must default to the configured macOS lane",
);
assert.match(
  desktopWorkflow,
  /inputs\.platform == 'macos' \|\| inputs\.platform == 'all'/,
);
assert.match(
  desktopWorkflow,
  /inputs\.platform == 'windows' \|\| inputs\.platform == 'all'/,
);

// Platform credentials belong to the selected platform job. Keeping them in
// the shared release-mode gate would make a macOS-only run require Windows
// credentials (and vice versa).
const releaseModeBlock = desktopWorkflow.slice(
  desktopWorkflow.indexOf("  release-mode:"),
  desktopWorkflow.indexOf("  macos-preflight:"),
);
for (const platformCredential of [
  "TOKEN_STATION_UPDATER_PUBKEY",
  "APPLE_CERTIFICATE",
  "APPLE_CERTIFICATE_PASSWORD",
  "APPLE_SIGNING_IDENTITY",
  "APPLE_KEYCHAIN_PASSWORD",
  "APPLE_API_ISSUER",
  "APPLE_API_KEY",
  "APPLE_API_KEY_CONTENT",
  "WINDOWS_CERTIFICATE",
  "WINDOWS_CERTIFICATE_PASSWORD",
  "WINDOWS_TIMESTAMP_URL",
]) {
  assert.doesNotMatch(
    releaseModeBlock,
    new RegExp(`(?:TS_HAS_)?${platformCredential}`),
    `shared release-mode gate must not require ${platformCredential}`,
  );
}
assert.doesNotMatch(releaseModeBlock, /\$\{\{\s*secrets\./);

const macosPreflightBlock = desktopWorkflow.slice(
  desktopWorkflow.indexOf("  macos-preflight:"),
  desktopWorkflow.indexOf("  windows-preflight:"),
);
for (const required of [
  "TOKEN_STATION_UPDATER_PUBKEY",
  "APPLE_CERTIFICATE",
  "APPLE_CERTIFICATE_PASSWORD",
  "APPLE_SIGNING_IDENTITY",
  "APPLE_KEYCHAIN_PASSWORD",
  "APPLE_API_ISSUER",
  "APPLE_API_KEY",
  "APPLE_API_KEY_CONTENT",
]) {
  assert.match(
    macosPreflightBlock,
    new RegExp(`TS_HAS_${required}: \\$\\{\\{ (?:vars|secrets)\\.${required} != '' \\}\\}`),
    `macOS preflight must check ${required} by presence only`,
  );
}

const windowsPreflightBlock = desktopWorkflow.slice(
  desktopWorkflow.indexOf("  windows-preflight:"),
  desktopWorkflow.indexOf("  macos:"),
);
for (const required of [
  "WINDOWS_CERTIFICATE",
  "WINDOWS_CERTIFICATE_PASSWORD",
  "WINDOWS_TIMESTAMP_URL",
]) {
  assert.match(
    windowsPreflightBlock,
    new RegExp(`TS_HAS_${required}: \\$\\{\\{ secrets\\.${required} != '' \\}\\}`),
    `Windows preflight must check ${required} by presence only`,
  );
}

const macosJobBlock = desktopWorkflow.slice(
  desktopWorkflow.indexOf("  macos:"),
  desktopWorkflow.indexOf("  windows:"),
);
const windowsJobBlock = desktopWorkflow.slice(desktopWorkflow.indexOf("  windows:"));
assert.doesNotMatch(macosJobBlock.slice(0, macosJobBlock.indexOf("    steps:")), /\n    env:/);
assert.doesNotMatch(windowsJobBlock.slice(0, windowsJobBlock.indexOf("    steps:")), /\n    env:/);
assert.match(macosJobBlock, /name: Remove temporary signing material\n {8}if: always\(\)/);
assert.match(windowsJobBlock, /name: Remove temporary signing material\n {8}if: always\(\)/);

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
