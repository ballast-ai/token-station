#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const stage = fs.mkdtempSync(path.join(os.tmpdir(), "token-station-release-assets."));
const version = "1.1.3";

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(path.join(stage, file))).digest("hex");
}

function runCheck() {
  return spawnSync(
    process.execPath,
    ["scripts/check-release-assets.mjs", "--version", version, "--dir", stage],
    { cwd: root, encoding: "utf8" },
  );
}

try {
  const cliAssets = [
    `token-station-cli-${version}-aarch64-apple-darwin.tar.gz`,
    `token-station-cli-${version}-x86_64-apple-darwin.tar.gz`,
    `token-station-cli-${version}-aarch64-unknown-linux-gnu.tar.gz`,
    `token-station-cli-${version}-x86_64-unknown-linux-gnu.tar.gz`,
  ];
  const binaryAssets = [
    ...cliAssets,
    `token-station_${version}_aarch64.dmg`,
    `token-station_${version}_x86_64.dmg`,
    `token-station_${version}_aarch64.app.tar.gz`,
    `token-station_${version}_x86_64.app.tar.gz`,
    `token-station_${version}_x86_64.msi`,
    `token-station_${version}_x86_64.deb`,
    `token-station_${version}_x86_64.AppImage`,
    `token-station_${version}_x86_64.rpm`,
  ];
  for (const file of binaryAssets) fs.writeFileSync(path.join(stage, file), `fixture:${file}\n`);

  const updaterAssets = [
    `token-station_${version}_aarch64.app.tar.gz`,
    `token-station_${version}_x86_64.app.tar.gz`,
    `token-station_${version}_x86_64.msi`,
  ];
  for (const file of updaterAssets) fs.writeFileSync(path.join(stage, `${file}.sig`), `signature:${file}\n`);

  const manifest = {
    format_version: 1,
    version,
    created_unix: 1,
    artifacts: cliAssets.map((name) => ({ name, sha256: sha256(name) })),
  };
  fs.writeFileSync(path.join(stage, "manifest.json"), `${JSON.stringify(manifest)}\n`);
  fs.writeFileSync(path.join(stage, "manifest.json.sig"), "manifest-signature\n");

  const releaseBase = `https://github.com/ballast-ai/token-station/releases/download/v${version}`;
  const latest = {
    version,
    notes: "fixture",
    pub_date: "2026-08-11T00:00:00Z",
    platforms: {
      "darwin-aarch64": {
        signature: fs.readFileSync(path.join(stage, `${updaterAssets[0]}.sig`), "utf8").trim(),
        url: `${releaseBase}/${updaterAssets[0]}`,
      },
      "darwin-x86_64": {
        signature: fs.readFileSync(path.join(stage, `${updaterAssets[1]}.sig`), "utf8").trim(),
        url: `${releaseBase}/${updaterAssets[1]}`,
      },
      "windows-x86_64": {
        signature: fs.readFileSync(path.join(stage, `${updaterAssets[2]}.sig`), "utf8").trim(),
        url: `${releaseBase}/${updaterAssets[2]}`,
      },
    },
  };
  fs.writeFileSync(path.join(stage, "latest.json"), `${JSON.stringify(latest)}\n`);

  const checksummed = fs
    .readdirSync(stage)
    .filter((file) => file !== "SHA256SUMS")
    .sort();
  fs.writeFileSync(
    path.join(stage, "SHA256SUMS"),
    `${checksummed.map((file) => `${sha256(file)}  ${file}`).join("\n")}\n`,
  );

  const valid = runCheck();
  if (valid.status !== 0) {
    throw new Error(`完整正式资产应通过检查：${valid.stderr || valid.stdout}`);
  }

  fs.writeFileSync(path.join(stage, "local-user-data.json"), "{}\n");
  const extra = runCheck();
  if (extra.status === 0 || !`${extra.stderr}${extra.stdout}`.includes("不属于正式 Release")) {
    throw new Error("检查器没有拒绝额外的本地数据文件");
  }
  fs.rmSync(path.join(stage, "local-user-data.json"));

  fs.rmSync(path.join(stage, `token-station_${version}_x86_64.dmg`));
  const missing = runCheck();
  if (missing.status === 0 || !`${missing.stderr}${missing.stdout}`.includes("缺少正式资产")) {
    throw new Error("检查器没有拒绝缺失 Intel DMG 的资产目录");
  }

  console.log("formal release asset boundary: PASS");
} finally {
  fs.rmSync(stage, { recursive: true, force: true });
}
