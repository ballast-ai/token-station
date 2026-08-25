#!/usr/bin/env node
// The §7.3 matrix, exercised against the real resolver.
//
// The property under test is not "the script runs". It is that no combination
// of event and configuration reaches the state this whole phase exists to
// remove: a version tag that succeeds while building nothing.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const resolver = path.join(root, "scripts", "resolve-release-mode.mjs");

const ALL = [
  "TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED",
  "TOKEN_STATION_RELEASE_PUBKEY_HEX",
  "TOKEN_STATION_UPDATER_PUBKEY",
];

function run({ event, ref = "", requested = "", required = ALL, present = ALL }) {
  const outputFile = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "mode-")), "out");
  fs.writeFileSync(outputFile, "");
  const env = {
    ...process.env,
    TS_EVENT: event,
    TS_REF: ref,
    TS_REQUESTED_MODE: requested,
    TS_REQUIRED: required.join(","),
    GITHUB_OUTPUT: outputFile,
  };
  for (const name of required) env[`TS_HAS_${name}`] = present.includes(name) ? "true" : "";
  const result = spawnSync(process.execPath, [resolver], { env, encoding: "utf8" });
  const outputs = Object.fromEntries(
    fs
      .readFileSync(outputFile, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split("=")),
  );
  return { status: result.status, stdout: result.stdout, stderr: result.stderr, outputs };
}

// A version tag with complete configuration builds.
{
  const r = run({ event: "push", ref: "refs/tags/v1.3.0" });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.outputs.mode, "formal");
  assert.equal(r.outputs.formal, "true");
}

// A version tag with the flag missing fails, and names it. This is the case
// that used to be a green run producing nothing.
{
  const r = run({
    event: "push",
    ref: "refs/tags/v1.3.0",
    present: ALL.filter((n) => n !== "TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED"),
  });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED/);
  assert.notEqual(r.outputs.mode, "formal", "a failed mode job must not hand downstream jobs a go-ahead");
}

// Every missing item is listed at once, not one per re-run.
{
  const r = run({ event: "push", ref: "refs/tags/v1.3.0", present: [] });
  assert.equal(r.status, 1);
  for (const name of ALL) assert.match(r.stderr, new RegExp(name));
}

// A manual formal run behaves exactly like a tag, in both directions.
{
  const ok = run({ event: "workflow_dispatch", requested: "formal" });
  assert.equal(ok.status, 0, ok.stderr);
  assert.equal(ok.outputs.mode, "formal");

  const bad = run({ event: "workflow_dispatch", requested: "formal", present: [] });
  assert.equal(bad.status, 1);
}

// A manual source-only run skips, whatever the configuration says. This is the
// one deliberate way to get a run without artifacts.
{
  for (const present of [ALL, []]) {
    const r = run({ event: "workflow_dispatch", requested: "source-only", present });
    assert.equal(r.status, 0, r.stderr);
    assert.equal(r.outputs.mode, "source-only");
    assert.equal(r.outputs.formal, "false");
    assert.match(r.stdout, /SOURCE_ONLY/);
  }
}

// A manual run must say which it wants. Defaulting either way is how the old
// rule went wrong: silence meant "skip", and silence is easy to arrive at.
{
  for (const requested of ["", "release", "yes"]) {
    const r = run({ event: "workflow_dispatch", requested });
    assert.equal(r.status, 1, `mode ${JSON.stringify(requested)} must be refused`);
    assert.match(r.stderr, /explicitly/);
  }
}

// Ordinary branch work never publishes, and never fails for lacking release
// configuration it has no use for.
{
  for (const [event, ref] of [
    ["push", "refs/heads/develop-v2"],
    ["pull_request", "refs/pull/7/merge"],
    ["schedule", ""],
  ]) {
    const r = run({ event, ref, present: [] });
    assert.equal(r.status, 0, r.stderr);
    assert.equal(r.outputs.mode, "source-only");
  }
}

// A tag that is not a version tag is not a release.
{
  const r = run({ event: "push", ref: "refs/tags/nightly-2026-08-25", present: [] });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.outputs.mode, "source-only");
}

// The summary is where a human looks to tell a skip from a build.
{
  const summaryFile = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "sum-")), "summary");
  fs.writeFileSync(summaryFile, "");
  spawnSync(process.execPath, [resolver], {
    env: {
      ...process.env,
      TS_EVENT: "workflow_dispatch",
      TS_REQUESTED_MODE: "source-only",
      TS_REQUIRED: "",
      GITHUB_STEP_SUMMARY: summaryFile,
    },
    encoding: "utf8",
  });
  assert.match(fs.readFileSync(summaryFile, "utf8"), /SOURCE_ONLY/);
}

console.log("release mode policy: all §7.3 cases hold");
