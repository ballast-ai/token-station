#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const [directory] = process.argv.slice(2);
if (!directory || process.argv.length !== 3) {
  console.error("usage: node scripts/create-release-checksums.mjs <release-directory>");
  process.exit(2);
}

const releaseDir = path.resolve(directory);
const output = path.join(releaseDir, "SHA256SUMS");
if (fs.existsSync(output)) {
  console.error(`refusing to overwrite ${output}`);
  process.exit(1);
}

const files = fs.readdirSync(releaseDir).sort();
if (files.length === 0) {
  console.error("release directory is empty");
  process.exit(1);
}
const lines = files.map((file) => {
  const absolute = path.join(releaseDir, file);
  if (!fs.statSync(absolute).isFile()) throw new Error(`release entry is not a file: ${file}`);
  const digest = crypto.createHash("sha256").update(fs.readFileSync(absolute)).digest("hex");
  return `${digest}  ${file}`;
});
fs.writeFileSync(output, `${lines.join("\n")}\n`, { flag: "wx" });
console.log(`release checksums: ${files.length} file(s)`);
