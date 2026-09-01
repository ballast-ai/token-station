#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const checker = path.join(root, "scripts/check-formal-release-notes.mjs");
const testDir = fs.mkdtempSync(path.join(os.tmpdir(), "token-station-formal-notes-"));
const currentVersion = JSON.parse(
  fs.readFileSync(path.join(root, "apps/desktop/package.json"), "utf8"),
).version;

function check(name, contents) {
  const file = path.join(testDir, name);
  fs.writeFileSync(file, contents);
  return spawnSync(process.execPath, [checker, "--version", "2.0.0", "--file", file], {
    cwd: root,
    encoding: "utf8",
  });
}

try {
  const currentNotes = spawnSync(
    process.execPath,
    [
      checker,
      "--version",
      currentVersion,
      "--file",
      path.join(root, `docs/release/v${currentVersion}.md`),
    ],
    { cwd: root, encoding: "utf8" },
  );
  assert.equal(currentNotes.status, 0, currentNotes.stderr);

  const formal = check(
    "formal.md",
    "# Token Station v2.0.0\n\nThis stable release contains signed packages for supported platforms.\n",
  );
  assert.equal(formal.status, 0, formal.stderr);

  for (const [name, marker] of [
    ["preview.md", "This preview adds a new desktop package."],
    ["unsigned.md", "The Windows package is unsigned."],
    ["prerelease.md", "This is a pre-release build."],
    ["unnotarized.md", "The macOS package is unnotarized."],
    ["chinese-preview.md", "\u8fd9是测试版。"],
  ]) {
    const result = check(name, `# Token Station v2.0.0\n\n${marker}\n`);
    assert.equal(result.status, 1, `${name} unexpectedly passed`);
    assert.match(result.stderr, /formal release notes check failed/);
  }

  const wrongVersion = check(
    "wrong-version.md",
    "# Token Station v1.9.0\n\nThis stable release contains signed packages for supported platforms.\n",
  );
  assert.equal(wrongVersion.status, 1);
  assert.match(wrongVersion.stderr, /first heading/);
} finally {
  fs.rmSync(testDir, { recursive: true, force: true });
}

console.log("formal release notes policy: PASS");
