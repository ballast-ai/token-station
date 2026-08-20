#!/usr/bin/env node

import assert from "node:assert/strict";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const checker = path.join(root, "scripts/check-release-readiness.mjs");

function run(args, releaseKey) {
  const env = { ...process.env };
  if (releaseKey === undefined) {
    delete env.TOKEN_STATION_RELEASE_PUBKEY_HEX;
  } else {
    env.TOKEN_STATION_RELEASE_PUBKEY_HEX = releaseKey;
  }
  return spawnSync(process.execPath, [checker, ...args], {
    cwd: root,
    env,
    encoding: "utf8",
  });
}

const sourceBuild = run(["--version", "1.2.0"]);
assert.equal(sourceBuild.status, 0, sourceBuild.stderr);

const missingKey = run(["--version", "1.2.0", "--formal"]);
assert.equal(missingKey.status, 1);
assert.match(missingKey.stderr, /CLI 发布公钥/);

const invalidKey = run(["--version", "1.2.0", "--formal"], "not-a-key");
assert.equal(invalidKey.status, 1);
assert.match(invalidKey.stderr, /64 位小写十六进制/);

const validKey = run(["--version", "1.2.0", "--formal"], "ab".repeat(32));
assert.equal(validKey.status, 0, validKey.stderr);

console.log("formal release public key gate: PASS");
