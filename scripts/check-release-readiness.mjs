#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const versionIndex = args.indexOf("--version");
const formal = args.includes("--formal");
const expectedArgumentCount = formal ? 3 : 2;

if (versionIndex === -1 || !args[versionIndex + 1] || args.length !== expectedArgumentCount) {
  console.error("用法：node scripts/check-release-readiness.mjs --version <x.y.z> [--formal]");
  process.exit(2);
}

const expectedVersion = args[versionIndex + 1];
if (!/^\d+\.\d+\.\d+$/.test(expectedVersion)) {
  console.error(`版本号 ${expectedVersion} 不是有效的三段式版本号，例如 1.1.3。`);
  process.exit(2);
}

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function readJson(relativePath) {
  return JSON.parse(read(relativePath));
}

function cargoPackageVersion(relativePath, packageName) {
  const contents = read(relativePath);
  const packages = contents.split("[[package]]").slice(1);
  const entry = packages.find((candidate) => {
    const name = candidate.match(/^\s*name\s*=\s*"([^"]+)"/m)?.[1];
    return name === packageName;
  });
  return entry?.match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
}

function cargoManifestVersion(relativePath) {
  return read(relativePath).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
}

const desktopPackage = readJson("apps/desktop/package.json");
const desktopPackageLock = readJson("apps/desktop/package-lock.json");
const tauriConfig = readJson("apps/desktop/src-tauri/tauri.conf.json");
const observed = [
  ["apps/desktop/package.json", desktopPackage.version],
  ["apps/desktop/package-lock.json", desktopPackageLock.version],
  ["apps/desktop/package-lock.json packages['']", desktopPackageLock.packages?.[""]?.version],
  ["apps/desktop/src-tauri/Cargo.toml", cargoManifestVersion("apps/desktop/src-tauri/Cargo.toml")],
  ["apps/desktop/src-tauri/tauri.conf.json", tauriConfig.version],
  ["apps/cli/Cargo.toml", cargoManifestVersion("apps/cli/Cargo.toml")],
  ["Cargo.lock token-station-cli", cargoPackageVersion("Cargo.lock", "token-station-cli")],
  [
    "apps/desktop/src-tauri/Cargo.lock token-station-cli",
    cargoPackageVersion("apps/desktop/src-tauri/Cargo.lock", "token-station-cli"),
  ],
  [
    "apps/desktop/src-tauri/Cargo.lock token-station-desktop",
    cargoPackageVersion("apps/desktop/src-tauri/Cargo.lock", "token-station-desktop"),
  ],
];

const mismatches = observed.filter(([, actual]) => actual !== expectedVersion);
if (mismatches.length > 0) {
  console.error(`还不能发布 v${expectedVersion}，以下版本没有对齐：`);
  for (const [location, actual] of mismatches) {
    console.error(`- ${location}：当前是 ${actual ?? "未找到"}，需要 ${expectedVersion}`);
  }
  process.exit(1);
}

if (tauriConfig.bundle?.macOS?.minimumSystemVersion !== "11.0") {
  console.error("还不能发布：App bundle 的最低系统版本必须是 macOS 11.0。");
  process.exit(1);
}

if (formal) {
  const releasePublicKey = process.env.TOKEN_STATION_RELEASE_PUBKEY_HEX ?? "";
  if (!releasePublicKey) {
    console.error("还不能正式发布：缺少 CLI 发布公钥 TOKEN_STATION_RELEASE_PUBKEY_HEX。");
    process.exit(1);
  }
  if (!/^[0-9a-f]{64}$/.test(releasePublicKey)) {
    console.error("还不能正式发布：CLI 发布公钥必须是 64 位小写十六进制字符。");
    process.exit(1);
  }
}

console.log(`release version: PASS (${expectedVersion}${formal ? ", formal" : ""})`);
