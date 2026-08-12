import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");

// The MSI lifecycle job lives in the platform workflow: Windows and macOS
// gates are not part of basic CI. The policy this file enforces is about *when*
// the installer lifecycle may run, not about which file declares it.
const platformPath = resolve(root, ".github/workflows/platform.yml");
const platform = await readFile(platformPath, "utf8");
const ciPath = resolve(root, ".github/workflows/ci.yml");
const ci = await readFile(ciPath, "utf8");

assert.match(
  platform,
  /\n  release:\n    types:\n      - published\n/,
  "the platform workflow must listen for published Release events",
);
assert.match(
  platform,
  /\n  workflow_dispatch:\s*\n/,
  "the platform workflow must support manual pre-publication validation",
);

const windowsMsi = platform.match(
  /\n  windows-msi:\n[\s\S]*?(?=\n  [a-z][a-z0-9-]+:\n|$)/,
)?.[0];
assert.ok(windowsMsi, "the platform workflow must define the windows-msi job");

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

// The installer filter the condition above depends on must actually exist.
assert.match(
  platform,
  /\n  changes:\n/,
  "the platform workflow must define the changes job the MSI condition reads",
);

// Basic CI has to keep executing this policy check, or the assertions above
// never run on a pull request.
assert.match(
  ci,
  /- run: node scripts\/check-ci-msi-trigger-policy\.mjs/,
  "basic CI must execute this policy check",
);

console.log("Windows MSI trigger policy: PASS");
