import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");
const workflow = await readFile(resolve(root, ".github/workflows/ci.yml"), "utf8");

function jobBody(jobName) {
  const startMarker = `  ${jobName}:\n`;
  const start = workflow.indexOf(startMarker);
  assert.notEqual(start, -1, `CI job '${jobName}' is missing.`);
  const remaining = workflow.slice(start + startMarker.length);
  const nextJob = remaining.search(/^  [a-z0-9_-]+:\n/m);
  return nextJob === -1 ? remaining : remaining.slice(0, nextJob);
}

for (const jobName of ["rust-coverage", "desktop-coverage", "msrv", "macos-compile-check", "windows-rust"]) {
  assert.match(
    jobBody(jobName),
    /if:\s+github\.event_name != 'pull_request'/,
    `${jobName} must not run on every pull request`,
  );
}

for (const jobName of ["rust", "deny", "audit", "desktop-rust", "frontend", "desktop-security"]) {
  assert.doesNotMatch(
    jobBody(jobName),
    /if:\s+github\.event_name != 'pull_request'/,
    `${jobName} must remain available to pull requests`,
  );
}

console.log("CI review trigger policy: PASS");
