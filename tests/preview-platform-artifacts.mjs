#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const workflowPath = path.join(root, ".github/workflows/preview-platform-artifacts.yml");
assert.ok(fs.existsSync(workflowPath), "preview platform artifact workflow is missing");

const workflow = fs.readFileSync(workflowPath, "utf8");
assert.match(workflow, /workflow_dispatch:/);
assert.doesNotMatch(workflow, /push:\s*\n\s*tags:/);
assert.match(workflow, /permissions:\s*\n\s*contents: read/);
assert.match(workflow, /permissions:\s*\n\s*contents: read\s*\n\s*pull-requests: read/);
assert.doesNotMatch(workflow, /contents: write/);
assert.doesNotMatch(workflow, /pull-requests: write/);
assert.doesNotMatch(workflow, /secrets\./);
assert.doesNotMatch(workflow, /gh release/);

assert.match(workflow, /verify-main-full-ci:\n    runs-on:/);
assert.match(workflow, /gh api \\\n\s+--method GET \\/);
assert.match(workflow, /platform-gates:\n    uses: \.\/\.github\/workflows\/platform\.yml/);

const windowsStart = workflow.indexOf("\n  windows:");
const linuxStart = workflow.indexOf("\n  linux:");
assert.ok(windowsStart > 0 && linuxStart > windowsStart, "preview workflow needs Windows and Linux jobs");
const windows = workflow.slice(windowsStart, linuxStart);
const linux = workflow.slice(linuxStart);

for (const job of [windows, linux]) {
  assert.match(job, /needs: \[verify-main-full-ci, platform-gates\]/);
  assert.match(job, /scripts\/build-desktop\.sh --local/);
  assert.doesNotMatch(job, /--production/);
}

assert.match(windows, /runs-on: windows-2025/);
assert.match(windows, /token-station_\$\{version\}_x86_64\.msi/);
assert.match(windows, /name: token-station-preview-windows-x86_64/);

assert.match(linux, /runs-on: ubuntu-22\.04/);
for (const asset of ["x86_64.deb", "x86_64.AppImage", "x86_64.rpm"]) {
  assert.match(linux, new RegExp(`token-station_\\$\\{version\\}_${asset.replace(".", "\\.")}`));
}
assert.match(linux, /name: token-station-preview-linux-x86_64/);

console.log("credential-free preview platform artifact policy: PASS");
