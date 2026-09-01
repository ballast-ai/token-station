#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const args = process.argv.slice(2);
const versionIndex = args.indexOf("--version");
const fileIndex = args.indexOf("--file");

if (
  versionIndex === -1 ||
  fileIndex === -1 ||
  !args[versionIndex + 1] ||
  !args[fileIndex + 1] ||
  args.length !== 4
) {
  console.error(
    "usage: node scripts/check-formal-release-notes.mjs --version <x.y.z> --file <release-notes.md>",
  );
  process.exit(2);
}

const version = args[versionIndex + 1];
const notesPath = path.resolve(args[fileIndex + 1]);

if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`invalid formal release version: ${version}`);
  process.exit(2);
}
if (!fs.existsSync(notesPath) || !fs.statSync(notesPath).isFile()) {
  console.error(`formal release notes do not exist: ${notesPath}`);
  process.exit(1);
}

const notes = fs.readFileSync(notesPath, "utf8");
const firstContentLine = notes.split(/\r?\n/).find((line) => line.trim() !== "")?.trim();
const expectedHeading = `# Token Station v${version}`;
const failures = [];

if (firstContentLine !== expectedHeading) {
  failures.push(`the first heading must be exactly: ${expectedHeading}`);
}
if (notes.trim().length < expectedHeading.length + 40) {
  failures.push("the notes must contain a useful formal release summary");
}

const previewMarkers = [
  [/(?:^|\W)this preview(?:\W|$)/iu, '"this preview"'],
  [/(?:^|\W)preview release(?:\W|$)/iu, '"preview release"'],
  [/(?:^|\W)pre-?release(?:\W|$)/iu, '"pre-release"'],
  [/(?:^|\W)unsigned(?:\W|$)/iu, '"unsigned"'],
  [/(?:^|\W)unnotarized(?:\W|$)/iu, '"unnotarized"'],
  [
    /\u9884\u89c8\u7248|\u6d4b\u8bd5\u7248|\u672a\u7b7e\u540d|\u672a\u516c\u8bc1/u,
    "a Chinese preview or unsigned warning",
  ],
];
for (const [pattern, label] of previewMarkers) {
  if (pattern.test(notes)) failures.push(`the notes contain ${label}`);
}

if (failures.length > 0) {
  console.error(`formal release notes check failed for v${version}:`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`formal release notes: PASS (v${version})`);
