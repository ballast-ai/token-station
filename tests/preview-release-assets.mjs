#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const checker = path.join(root, "scripts/check-preview-release-assets.mjs");
const version = "9.8.7";
const stage = fs.mkdtempSync(path.join(os.tmpdir(), "token-station-preview-assets-"));

const updaterAssets = [
  `token-station_${version}_aarch64.app.tar.gz`,
  `token-station_${version}_x86_64.app.tar.gz`,
];
const dmgAssets = [
  `token-station_${version}_aarch64_UNSIGNED-UNNOTARIZED.dmg`,
  `token-station_${version}_x86_64_UNSIGNED-UNNOTARIZED.dmg`,
];
const packageAssets = [
  `token-station_${version}_x86_64.msi`,
  `token-station_${version}_x86_64.deb`,
  `token-station_${version}_x86_64.AppImage`,
  `token-station_${version}_x86_64.rpm`,
];

const sha256 = (file) =>
  crypto.createHash("sha256").update(fs.readFileSync(path.join(stage, file))).digest("hex");

for (const file of [...updaterAssets, ...dmgAssets, ...packageAssets]) {
  fs.writeFileSync(path.join(stage, file), `fixture:${file}\n`);
}
for (const file of updaterAssets) {
  fs.writeFileSync(path.join(stage, `${file}.sig`), `signature:${file}\n`);
}
for (const file of dmgAssets) {
  fs.writeFileSync(path.join(stage, `${file}.sha256`), `${sha256(file)}  ${file}\n`);
}

const releaseBase = `https://github.com/ballast-ai/token-station/releases/download/preview-v${version}`;
fs.writeFileSync(
  path.join(stage, "latest.json"),
  JSON.stringify({
    version,
    notes: "Preview fixture",
    pub_date: "2026-08-26T00:00:00Z",
    platforms: {
      "darwin-aarch64": {
        signature: fs.readFileSync(path.join(stage, `${updaterAssets[0]}.sig`), "utf8").trim(),
        url: `${releaseBase}/${updaterAssets[0]}`,
      },
      "darwin-x86_64": {
        signature: fs.readFileSync(path.join(stage, `${updaterAssets[1]}.sig`), "utf8").trim(),
        url: `${releaseBase}/${updaterAssets[1]}`,
      },
    },
  }),
);

const checksummed = fs.readdirSync(stage).sort();
fs.writeFileSync(
  path.join(stage, "SHA256SUMS"),
  `${checksummed.map((file) => `${sha256(file)}  ${file}`).join("\n")}\n`,
);

const run = () =>
  spawnSync(process.execPath, [checker, "--version", version, "--dir", stage], {
    encoding: "utf8",
  });

let result = run();
assert.equal(result.status, 0, result.stderr);
assert.match(result.stdout, /preview release assets: PASS/);

fs.appendFileSync(path.join(stage, packageAssets[0]), "tampered\n");
result = run();
assert.notEqual(result.status, 0, "preview checker accepted a changed Windows MSI");
assert.match(result.stderr, /SHA256SUMS/);

fs.rmSync(stage, { recursive: true, force: true });
console.log("preview release asset contract: PASS");
