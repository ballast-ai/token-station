#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

const importer = read("scripts/import-macos-signing-identity.sh");
const releaseWorkflow = read(".github/workflows/release.yml");
const desktopWorkflow = read(".github/workflows/desktop-release.yml");

assert.match(importer, /RUNNER_TEMP must be an existing absolute directory/);
assert.match(importer, /GITHUB_ENV must be inside RUNNER_TEMP/);
assert.match(importer, /the bundle must contain exactly one leaf certificate/);
assert.match(importer, /the bundle must contain exactly one private key/);
assert.match(importer, /the certificate and private key do not match/);
assert.match(importer, /CN=\$APPLE_SIGNING_IDENTITY/);
assert.match(importer, /-keypbe PBE-SHA1-3DES/);
assert.match(importer, /-certpbe PBE-SHA1-3DES/);
assert.match(importer, /-macalg sha1/);
assert.match(importer, /security find-identity -v -p codesigning/);
assert.match(importer, /trap cleanup_plaintext_material EXIT/);

for (const [name, workflow] of [
  ["formal release", releaseWorkflow],
  ["desktop release", desktopWorkflow],
]) {
  assert.doesNotMatch(
    workflow,
    /security import "\$certificate"/,
    `${name} must not bypass PKCS#12 normalization`,
  );
}

assert.match(
  desktopWorkflow,
  /name: Import Developer ID certificate[\s\S]*?run: scripts\/import-macos-signing-identity\.sh/,
  "desktop release must use the shared macOS identity importer",
);
assert.match(
  releaseWorkflow,
  /ref: \$\{\{ github\.workflow_sha \}\}[\s\S]*?install -m 0700 scripts\/import-macos-signing-identity\.sh "\$RUNNER_TEMP\/import-macos-signing-identity\.sh"[\s\S]*?ref: \$\{\{ needs\.release-target\.outputs\.sha \}\}/,
  "formal release must stage the importer from the trusted workflow revision before checking out the immutable release tag",
);
assert.match(
  releaseWorkflow,
  /name: Import Developer ID certificate\n {8}run: "\$RUNNER_TEMP\/import-macos-signing-identity\.sh"/,
  "formal release must run the staged importer",
);

assert.match(
  desktopWorkflow,
  /name: Import Developer ID certificate[\s\S]*?APPLE_SIGNING_IDENTITY: \$\{\{ secrets\.APPLE_SIGNING_IDENTITY \}\}/,
);
assert.match(
  releaseWorkflow,
  /name: Remove temporary signing material\n {8}if: always\(\)/,
  "the formal release must always clean the temporary certificate and keychain",
);

console.log("macOS signing import policy: PASS");
