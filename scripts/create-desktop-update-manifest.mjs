#!/usr/bin/env node

import { basename } from "node:path";
import { readFileSync, writeFileSync } from "node:fs";

const SUPPORTED_PLATFORMS = [
  "darwin-aarch64",
  "darwin-x86_64",
];

function usage(message) {
  if (message) console.error(message);
  console.error(
    "usage: create-desktop-update-manifest.mjs --version <semver> --pub-date <RFC3339> " +
      "--release-base-url <https-url> --output <latest.json> [--notes-file <path>] " +
      "[--platforms <comma-separated-platforms>] " +
      "--artifact <platform=payload> (repeat for all supported platforms)",
  );
  process.exit(2);
}

const values = new Map();
const artifacts = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const flag = process.argv[index];
  const value = process.argv[index + 1];
  if (!flag?.startsWith("--") || value == null) usage(`invalid argument: ${flag ?? ""}`);
  if (flag === "--artifact") {
    const separator = value.indexOf("=");
    if (separator < 1) usage(`invalid artifact mapping: ${value}`);
    const platform = value.slice(0, separator);
    const path = value.slice(separator + 1);
    if (artifacts.has(platform)) usage(`duplicate artifact platform: ${platform}`);
    artifacts.set(platform, path);
  } else {
    if (values.has(flag)) usage(`duplicate argument: ${flag}`);
    values.set(flag, value);
  }
}

const version = values.get("--version");
const pubDate = values.get("--pub-date");
const releaseBaseUrl = values.get("--release-base-url");
const output = values.get("--output");
if (!version || !pubDate || !releaseBaseUrl || !output) usage("missing required argument");
if (!/^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  usage(`invalid semantic version: ${version}`);
}
if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(pubDate)) {
  usage(`invalid RFC3339 publication date: ${pubDate}`);
}

let baseUrl;
try {
  baseUrl = new URL(releaseBaseUrl.endsWith("/") ? releaseBaseUrl : `${releaseBaseUrl}/`);
} catch {
  usage(`invalid release base URL: ${releaseBaseUrl}`);
}
if (baseUrl.protocol !== "https:") usage("release base URL must use HTTPS");

const configuredPlatforms = values.get("--platforms");
const requiredPlatforms = configuredPlatforms
  ? configuredPlatforms.split(",").map((platform) => platform.trim()).filter(Boolean)
  : [...SUPPORTED_PLATFORMS];
if (requiredPlatforms.length === 0 || new Set(requiredPlatforms).size !== requiredPlatforms.length) {
  usage("platform selection must contain unique supported platforms");
}
for (const platform of requiredPlatforms) {
  if (!SUPPORTED_PLATFORMS.includes(platform)) usage(`unsupported platform: ${platform}`);
}

for (const platform of artifacts.keys()) {
  if (!requiredPlatforms.includes(platform)) usage(`artifact is outside the selected platforms: ${platform}`);
}
for (const platform of requiredPlatforms) {
  if (!artifacts.has(platform)) usage(`missing artifact: ${platform}`);
}

const platforms = {};
for (const platform of requiredPlatforms) {
  const payloadPath = artifacts.get(platform);
  const signaturePath = `${payloadPath}.sig`;
  let signature;
  try {
    readFileSync(payloadPath);
    signature = readFileSync(signaturePath, "utf8").trim();
  } catch (error) {
    console.error(`cannot read ${platform} payload or offline signature: ${error.message}`);
    process.exit(1);
  }
  if (!signature) {
    console.error(`offline signature is empty: ${signaturePath}`);
    process.exit(1);
  }
  platforms[platform] = {
    signature,
    url: new URL(encodeURIComponent(basename(payloadPath)), baseUrl).toString(),
  };
}

let notes = "";
const notesFile = values.get("--notes-file");
if (notesFile) notes = readFileSync(notesFile, "utf8").trim();

writeFileSync(
  output,
  `${JSON.stringify({ version, notes, pub_date: pubDate, platforms }, null, 2)}\n`,
  { flag: "wx" },
);
