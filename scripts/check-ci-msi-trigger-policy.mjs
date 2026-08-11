import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");
const workflowPath = resolve(root, ".github/workflows/ci.yml");
const workflow = await readFile(workflowPath, "utf8");

assert.match(
  workflow,
  /\n  release:\n    types:\n      - published\n/,
  "CI must listen for published Release events",
);
assert.match(
  workflow,
  /\n  workflow_dispatch:\s*\n/,
  "CI must support manual pre-publication validation",
);

const windowsMsi = workflow.match(
  /\n  windows-msi:\n[\s\S]*?(?=\n  [a-z][a-z0-9-]+:\n|$)/,
)?.[0];
assert.ok(windowsMsi, "CI must define the windows-msi job");

const compactCondition = windowsMsi
  .match(/    if: >-\n([\s\S]*?)\n    runs-on:/)?.[1]
  .replace(/\s+/g, " ")
  .trim();
assert.equal(
  compactCondition,
  "!cancelled() && needs.changes.outputs.installer == 'true' && (github.event_name == 'release' || github.event_name == 'workflow_dispatch')",
  "Windows MSI must run only for an eligible published Release or manual validation",
);
assert.equal(
  windowsMsi.includes("github.event_name == 'pull_request'"),
  false,
  "ordinary pull requests must not run the MSI lifecycle",
);
assert.match(
  workflow,
  /- run: node scripts\/check-ci-msi-trigger-policy\.mjs/,
  "regular CI must execute this policy check",
);
assert.match(
  workflow,
  /- run: node scripts\/check-ci-review-trigger-policy\.mjs/,
  "regular CI must execute the review trigger policy check",
);

console.log("Windows MSI trigger policy: PASS");
