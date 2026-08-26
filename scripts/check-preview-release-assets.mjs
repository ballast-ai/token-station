#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const args = process.argv.slice(2);
const versionIndex = args.indexOf("--version");
const dirIndex = args.indexOf("--dir");
if (
  versionIndex === -1 ||
  dirIndex === -1 ||
  !args[versionIndex + 1] ||
  !args[dirIndex + 1] ||
  args.length !== 4
) {
  console.error("usage: node scripts/check-preview-release-assets.mjs --version <x.y.z> --dir <directory>");
  process.exit(2);
}

const version = args[versionIndex + 1];
const releaseDir = path.resolve(args[dirIndex + 1]);
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`invalid preview version: ${version}`);
  process.exit(2);
}
if (!fs.existsSync(releaseDir) || !fs.statSync(releaseDir).isDirectory()) {
  console.error(`preview asset directory does not exist: ${releaseDir}`);
  process.exit(1);
}

const updaterAssets = [
  `token-station_${version}_aarch64.app.tar.gz`,
  `token-station_${version}_x86_64.app.tar.gz`,
];
const dmgAssets = [
  `token-station_${version}_aarch64_UNSIGNED-UNNOTARIZED.dmg`,
  `token-station_${version}_x86_64_UNSIGNED-UNNOTARIZED.dmg`,
];
const requiredAssets = [
  ...dmgAssets,
  ...dmgAssets.map((file) => `${file}.sha256`),
  ...updaterAssets,
  ...updaterAssets.map((file) => `${file}.sig`),
  `token-station_${version}_x86_64.msi`,
  `token-station_${version}_x86_64.deb`,
  `token-station_${version}_x86_64.AppImage`,
  `token-station_${version}_x86_64.rpm`,
  "latest.json",
  "SHA256SUMS",
].sort();

const failures = [];
const actualEntries = fs.readdirSync(releaseDir).sort();
for (const file of requiredAssets) {
  const absolute = path.join(releaseDir, file);
  if (!fs.existsSync(absolute) || !fs.lstatSync(absolute).isFile()) {
    failures.push(`missing preview asset: ${file}`);
  } else if (fs.statSync(absolute).size === 0) {
    failures.push(`preview asset is empty: ${file}`);
  }
}
for (const file of actualEntries) {
  if (!requiredAssets.includes(file)) failures.push(`unexpected preview asset: ${file}`);
}

const read = (file) => fs.readFileSync(path.join(releaseDir, file), "utf8");
const sha256 = (file) =>
  crypto.createHash("sha256").update(fs.readFileSync(path.join(releaseDir, file))).digest("hex");

if (requiredAssets.every((file) => fs.existsSync(path.join(releaseDir, file)))) {
  let latest;
  try {
    latest = JSON.parse(read("latest.json"));
  } catch (error) {
    failures.push(`latest.json is invalid: ${error.message}`);
  }
  if (latest) {
    if (latest.version !== version) failures.push("latest.json has the wrong version");
    const expectedPlatforms = {
      "darwin-aarch64": updaterAssets[0],
      "darwin-x86_64": updaterAssets[1],
    };
    const actualPlatforms = Object.keys(latest.platforms ?? {}).sort();
    if (JSON.stringify(actualPlatforms) !== JSON.stringify(Object.keys(expectedPlatforms).sort())) {
      failures.push("latest.json must contain exactly the two macOS preview platforms");
    }
    for (const [platform, file] of Object.entries(expectedPlatforms)) {
      const entry = latest.platforms?.[platform];
      const expectedUrl =
        `https://github.com/ballast-ai/token-station/releases/download/preview-v${version}/${file}`;
      if (entry?.url !== expectedUrl) failures.push(`latest.json has the wrong ${platform} URL`);
      if (entry?.signature !== read(`${file}.sig`).trim()) {
        failures.push(`latest.json has the wrong ${platform} signature`);
      }
    }
  }

  for (const dmg of dmgAssets) {
    const expected = `${sha256(dmg)}  ${dmg}`;
    if (read(`${dmg}.sha256`).trim() !== expected) {
      failures.push(`${dmg}.sha256 does not match the DMG`);
    }
  }

  const checksumEntries = new Map();
  for (const line of read("SHA256SUMS").trim().split(/\r?\n/)) {
    const match = line.match(/^([0-9a-f]{64})  ([^/]+)$/);
    if (!match) {
      failures.push(`SHA256SUMS has an invalid line: ${line}`);
      continue;
    }
    if (checksumEntries.has(match[2])) {
      failures.push(`SHA256SUMS has a duplicate entry: ${match[2]}`);
      continue;
    }
    checksumEntries.set(match[2], match[1]);
  }
  const checksummedAssets = requiredAssets.filter((file) => file !== "SHA256SUMS").sort();
  if (JSON.stringify([...checksumEntries.keys()].sort()) !== JSON.stringify(checksummedAssets)) {
    failures.push("SHA256SUMS must cover every preview asset except itself");
  }
  for (const file of checksummedAssets) {
    if (checksumEntries.get(file) !== sha256(file)) {
      failures.push(`SHA256SUMS has the wrong digest for ${file}`);
    }
  }
}

if (failures.length > 0) {
  console.error("preview release asset check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`preview release assets: PASS (preview-v${version})`);
