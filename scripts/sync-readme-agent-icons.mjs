#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = path.join(repoRoot, "apps/desktop/public/agents/workbuddy.png");
const diagramPaths = [
  "docs/assets/token-station-architecture-en.svg",
  "docs/assets/token-station-architecture-en-mobile.svg",
  "docs/assets/token-station-architecture-zh-CN.svg",
  "docs/assets/token-station-architecture-zh-CN-mobile.svg",
];
const embeddedPngPattern = /data:image\/png;base64,[A-Za-z0-9+/=]+/g;
const shouldWrite = process.argv.includes("--write");

const source = await readFile(sourcePath);
const expectedUri = `data:image/png;base64,${source.toString("base64")}`;
let staleCount = 0;

for (const relativePath of diagramPaths) {
  const absolutePath = path.join(repoRoot, relativePath);
  const svg = await readFile(absolutePath, "utf8");
  const embeddedImages = svg.match(embeddedPngPattern) ?? [];

  if (embeddedImages.length !== 1) {
    throw new Error(`${relativePath}: expected exactly one embedded PNG, found ${embeddedImages.length}`);
  }

  if (embeddedImages[0] === expectedUri) {
    console.log(`OK ${relativePath}`);
    continue;
  }

  staleCount += 1;
  if (shouldWrite) {
    await writeFile(absolutePath, svg.replace(embeddedPngPattern, expectedUri));
    console.log(`UPDATED ${relativePath}`);
  } else {
    console.error(`STALE ${relativePath}`);
  }
}

if (staleCount > 0 && !shouldWrite) {
  console.error("Run: node scripts/sync-readme-agent-icons.mjs --write");
  process.exitCode = 1;
}
