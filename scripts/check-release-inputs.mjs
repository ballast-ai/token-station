#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const args = process.argv.slice(2);
const versionIndex = args.indexOf("--version");
const dirIndex = args.indexOf("--dir");
if (versionIndex === -1 || dirIndex === -1 || args.length !== 4) {
  console.error("usage: node scripts/check-release-inputs.mjs --version <x.y.z> --dir <directory>");
  process.exit(2);
}

const version = args[versionIndex + 1];
const releaseDir = path.resolve(args[dirIndex + 1]);
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`invalid release version: ${version}`);
  process.exit(2);
}
if (!fs.existsSync(releaseDir) || !fs.statSync(releaseDir).isDirectory()) {
  console.error(`release input directory does not exist: ${releaseDir}`);
  process.exit(1);
}

const cliAssets = [
  `token-station-cli-${version}-aarch64-apple-darwin.tar.gz`,
  `token-station-cli-${version}-x86_64-apple-darwin.tar.gz`,
  `token-station-cli-${version}-aarch64-unknown-linux-gnu.tar.gz`,
  `token-station-cli-${version}-x86_64-unknown-linux-gnu.tar.gz`,
];
const required = [
  ...cliAssets,
  `token-station_${version}_aarch64.dmg`,
  `token-station_${version}_x86_64.dmg`,
  `token-station_${version}_aarch64.app.tar.gz`,
  `token-station_${version}_x86_64.app.tar.gz`,
  "manifest.json",
].sort();

const failures = [];
const actual = fs.readdirSync(releaseDir).sort();
for (const file of required) {
  const absolute = path.join(releaseDir, file);
  if (!fs.existsSync(absolute) || !fs.statSync(absolute).isFile()) {
    failures.push(`missing release input: ${file}`);
  } else if (fs.statSync(absolute).size === 0) {
    failures.push(`release input is empty: ${file}`);
  }
}
for (const file of actual) {
  if (!required.includes(file)) failures.push(`unexpected release input: ${file}`);
}

if (failures.length === 0) {
  try {
    const manifest = JSON.parse(fs.readFileSync(path.join(releaseDir, "manifest.json"), "utf8"));
    if (manifest.version !== version) failures.push("manifest.json has the wrong version");
    const artifacts = Array.isArray(manifest.artifacts) ? manifest.artifacts : [];
    const names = artifacts.map((artifact) => artifact.name).sort();
    if (JSON.stringify(names) !== JSON.stringify([...cliAssets].sort())) {
      failures.push("manifest.json must cover exactly the four CLI archives");
    }
    for (const artifact of artifacts) {
      if (!cliAssets.includes(artifact.name)) continue;
      const digest = crypto
        .createHash("sha256")
        .update(fs.readFileSync(path.join(releaseDir, artifact.name)))
        .digest("hex");
      if (artifact.sha256 !== digest) failures.push(`manifest hash is wrong: ${artifact.name}`);
    }
  } catch (error) {
    failures.push(`manifest.json is invalid: ${error.message}`);
  }
}

if (failures.length > 0) {
  console.error("release input check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`formal release inputs: PASS (v${version})`);
