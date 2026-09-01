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
const fullCiPath = resolve(root, ".github/workflows/full-ci.yml");
const fullCi = await readFile(fullCiPath, "utf8");

assert.match(
  platform,
  /\n  workflow_call:\n/,
  "the platform workflow must be reusable before release publication",
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
  "needs.windows-rust.result == 'success' && needs.changes.outputs.installer == 'true'",
  "Windows MSI must run only after the full Windows gate succeeds",
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

const windowsRust = platform.match(
  /\n  windows-rust:\n[\s\S]*?(?=\n  [a-z][a-z0-9-]+:\n|$)/,
)?.[0];
assert.ok(windowsRust, "the platform workflow must define the windows-rust job");
assert.ok(
  windowsRust.indexOf("scripts/prepare-desktop-test-plugins.sh") <
    windowsRust.indexOf("cargo test --workspace"),
  "official plugins must exist before CLI workspace tests run on Windows",
);

// Full CI must keep executing this policy check before a release build.
assert.match(
  fullCi,
  /- run: node scripts\/check-ci-msi-trigger-policy\.mjs/,
  "full CI must execute this policy check",
);

console.log("Windows MSI trigger policy: PASS");
