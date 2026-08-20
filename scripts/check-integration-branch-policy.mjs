#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");

async function read(relativePath) {
  return readFile(resolve(root, relativePath), "utf8");
}

function pushBranches(workflow, name) {
  const block = workflow.match(
    /\n  push:\n    branches:\n((?:      - [^\n]+\n)+)/,
  )?.[1];
  assert.ok(block, `${name} must declare push branches`);
  return block
    .trim()
    .split("\n")
    .map((line) => line.trim().replace(/^-\s+/, ""));
}

const [ci, platform, setupRust] = await Promise.all([
  read(".github/workflows/ci.yml"),
  read(".github/workflows/platform.yml"),
  read(".github/actions/setup-rust/action.yml"),
]);

const integrationBranches = ["main", "develop", "develop-v2"];
const platformBranches = ["main"];
assert.deepEqual(
  pushBranches(ci, "basic CI"),
  integrationBranches,
  "basic CI push branches must include every integration branch",
);
assert.deepEqual(
  pushBranches(platform, "platform gates"),
  platformBranches,
  "platform push branches must be limited to main",
);

for (const [name, source] of [
  ["shared Rust setup", setupRust],
  ["MSRV job", ci],
]) {
  assert.match(
    source,
    /github\.ref == 'refs\/heads\/develop-v2'/,
    `${name} must save caches for develop-v2`,
  );
}

assert.match(
  ci,
  /- run: node scripts\/check-integration-branch-policy\.mjs/,
  "basic CI must execute the integration branch policy check",
);

console.log("integration branch CI policy: PASS");
