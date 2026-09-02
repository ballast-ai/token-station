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
  console.error("用法：node scripts/check-release-assets.mjs --version <x.y.z> --dir <资产目录>");
  process.exit(2);
}

const version = args[versionIndex + 1];
const releaseDir = path.resolve(args[dirIndex + 1]);
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`版本号 ${version} 不是有效的三段式版本号。`);
  process.exit(2);
}
if (!fs.existsSync(releaseDir) || !fs.statSync(releaseDir).isDirectory()) {
  console.error(`正式资产目录不存在：${releaseDir}`);
  process.exit(1);
}

const cliAssets = [
  `token-station-cli-${version}-aarch64-apple-darwin.tar.gz`,
  `token-station-cli-${version}-x86_64-apple-darwin.tar.gz`,
  `token-station-cli-${version}-aarch64-unknown-linux-gnu.tar.gz`,
  `token-station-cli-${version}-x86_64-unknown-linux-gnu.tar.gz`,
];
const windowsUpdaterEnabled = version !== "2.0.0";
const updaterAssets = [
  `token-station_${version}_aarch64.app.tar.gz`,
  `token-station_${version}_x86_64.app.tar.gz`,
  ...(windowsUpdaterEnabled ? [`token-station_${version}_x86_64.msi`] : []),
];
const requiredAssets = [
  ...cliAssets,
  `token-station_${version}_aarch64.dmg`,
  `token-station_${version}_x86_64.dmg`,
  ...updaterAssets,
  ...updaterAssets.map((file) => `${file}.sig`),
  ...(!windowsUpdaterEnabled ? [`token-station_${version}_x86_64.msi`] : []),
  `token-station_${version}_x86_64.deb`,
  `token-station_${version}_x86_64.AppImage`,
  `token-station_${version}_x86_64.rpm`,
  "manifest.json",
  "manifest.json.sig",
  "latest.json",
  "SHA256SUMS",
].sort();

const failures = [];
const actualEntries = fs.readdirSync(releaseDir).sort();
for (const file of requiredAssets) {
  const absolutePath = path.join(releaseDir, file);
  if (!fs.existsSync(absolutePath) || !fs.statSync(absolutePath).isFile()) {
    failures.push(`缺少正式资产：${file}`);
  } else if (fs.statSync(absolutePath).size === 0) {
    failures.push(`正式资产是空文件：${file}`);
  }
}
for (const file of actualEntries) {
  if (!requiredAssets.includes(file)) failures.push(`文件不属于正式 Release：${file}`);
}

function read(file) {
  return fs.readFileSync(path.join(releaseDir, file), "utf8");
}

function readJson(file) {
  try {
    return JSON.parse(read(file));
  } catch (error) {
    failures.push(`${file} 不是有效 JSON：${error.message}`);
    return null;
  }
}

function sha256(file) {
  return crypto
    .createHash("sha256")
    .update(fs.readFileSync(path.join(releaseDir, file)))
    .digest("hex");
}

if (requiredAssets.every((file) => fs.existsSync(path.join(releaseDir, file)))) {
  const manifest = readJson("manifest.json");
  if (manifest) {
    if (manifest.version !== version) {
      failures.push(`manifest.json 版本是 ${manifest.version ?? "未知"}，应为 ${version}`);
    }
    const artifacts = Array.isArray(manifest.artifacts) ? manifest.artifacts : [];
    const names = artifacts.map((artifact) => artifact.name).sort();
    if (JSON.stringify(names) !== JSON.stringify([...cliAssets].sort())) {
      failures.push("manifest.json 没有精确覆盖四个平台的 CLI 归档");
    }
    for (const artifact of artifacts) {
      if (cliAssets.includes(artifact.name) && artifact.sha256 !== sha256(artifact.name)) {
        failures.push(`manifest.json 中 ${artifact.name} 的 SHA-256 不正确`);
      }
    }
  }

  const latest = readJson("latest.json");
  if (latest) {
    if (latest.version !== version) {
      failures.push(`latest.json 版本是 ${latest.version ?? "未知"}，应为 ${version}`);
    }
    const expectedPlatforms = {
      "darwin-aarch64": updaterAssets[0],
      "darwin-x86_64": updaterAssets[1],
      ...(windowsUpdaterEnabled ? { "windows-x86_64": updaterAssets[2] } : {}),
    };
    const platformNames = Object.keys(latest.platforms ?? {}).sort();
    if (JSON.stringify(platformNames) !== JSON.stringify(Object.keys(expectedPlatforms).sort())) {
      failures.push(
        windowsUpdaterEnabled
          ? "latest.json 必须精确包含两个 macOS 平台和 Windows x86-64 平台"
          : "Windows v2.0.0 的 latest.json 必须只包含两个 macOS 平台",
      );
    }
    for (const [platform, file] of Object.entries(expectedPlatforms)) {
      const entry = latest.platforms?.[platform];
      const expectedUrl = `https://github.com/ballast-ai/token-station/releases/download/v${version}/${file}`;
      const expectedSignature = read(`${file}.sig`).trim();
      if (entry?.url !== expectedUrl) failures.push(`latest.json 中 ${platform} 的下载地址不正确`);
      if (entry?.signature !== expectedSignature) failures.push(`latest.json 中 ${platform} 没有使用正式离线签名`);
    }
  }

  const checksumEntries = new Map();
  for (const line of read("SHA256SUMS").trim().split(/\r?\n/)) {
    const match = line.match(/^([0-9a-f]{64})  ([^/]+)$/);
    if (!match) {
      failures.push(`SHA256SUMS 包含无法识别的行：${line}`);
      continue;
    }
    checksumEntries.set(match[2], match[1]);
  }
  const checksummedAssets = requiredAssets.filter((file) => file !== "SHA256SUMS").sort();
  if (JSON.stringify([...checksumEntries.keys()].sort()) !== JSON.stringify(checksummedAssets)) {
    failures.push("SHA256SUMS 没有精确覆盖除自身以外的全部正式资产");
  }
  for (const file of checksummedAssets) {
    if (checksumEntries.get(file) !== sha256(file)) failures.push(`SHA256SUMS 中 ${file} 的校验值不正确`);
  }
}

if (failures.length > 0) {
  console.error("正式 Release 资产检查失败：");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`formal release assets: PASS (v${version})`);
