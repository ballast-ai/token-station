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

function jobNames(workflow) {
  const jobs = workflow.split("\njobs:\n", 2)[1];
  assert.ok(jobs, "workflow must declare jobs");
  return [...jobs.matchAll(/^  ([a-z][a-z0-9-]+):\n/gm)].map(
    (match) => match[1],
  );
}

const [ci, fullCi, platform] = await Promise.all([
  read(".github/workflows/ci.yml"),
  read(".github/workflows/full-ci.yml"),
  read(".github/workflows/platform.yml"),
]);

assert.deepEqual(
  pushBranches(ci, "required CI"),
  ["main"],
  "required CI push branches must be limited to main",
);
assert.match(
  ci,
  /\n  pull_request:\n/,
  "required CI must validate pull requests",
);
assert.deepEqual(
  jobNames(ci),
  ["rust", "desktop-rust", "frontend"],
  "push and pull request CI must contain only the three required jobs",
);
assert.deepEqual(
  jobNames(fullCi),
  [
    "rust",
    "rust-coverage",
    "desktop-rust",
    "desktop-coverage",
    "supply-chain",
    "release-gates",
    "msrv",
    "frontend",
  ],
  "full CI must preserve every release validation job",
);
assert.match(
  fullCi,
  /\n  workflow_call:\n/,
  "full CI must be reusable by the release workflow",
);
assert.match(
  fullCi,
  /- run: node scripts\/check-integration-branch-policy\.mjs/,
  "full CI must execute the integration branch policy check",
);
assert.match(
  platform,
  /\n  workflow_call:\n/,
  "platform gates must be reusable by the release workflow",
);
assert.doesNotMatch(
  platform,
  /\n  (?:push|pull_request|release):\n/,
  "platform gates must not run for routine repository events",
);

console.log("integration branch CI policy: PASS");
