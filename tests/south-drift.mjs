#!/usr/bin/env node
// The South drift rule, exercised without the network.
//
// The property under test: a re-pin is reported due exactly when a newer
// release exists and the pin has passed the age or minor-gap limit, and the
// pinned version is read from the one place the dependency policy declares it.

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  MAX_MINOR_GAP,
  MAX_PIN_AGE_DAYS,
  assess,
  issueBody,
  pinnedRelease,
} from "../scripts/check-south-drift.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const day = 86_400_000;
const pinnedAt = Date.parse("2026-11-19T14:36:29Z");
const release = (version, offsetDays) => ({
  version,
  publishedAt: new Date(pinnedAt + offsetDays * day).toISOString(),
});

// The pinned version comes from the dependency policy, not a second copy.
const policy = fs.readFileSync(path.join(root, "tests/south-dependency-policy.mjs"), "utf8");
assert.match(pinnedRelease(policy), /^\d+\.\d+\.\d+$/);
assert.throws(() => pinnedRelease("const southRelease = 'x';"), /declares no southRelease/);

// Nothing newer: never due, whatever the age.
let result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.0", -3), release("0.85.1", 0)],
  now: pinnedAt + 400 * day,
});
assert.equal(result.due, false);
assert.equal(result.latest, "0.85.1");

// Newer, but young and close: not due.
result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release("0.86.0", 5)],
  now: pinnedAt + MAX_PIN_AGE_DAYS * day,
});
assert.equal(result.due, false);
assert.equal(result.newer, 1);

// Newer and past the age limit: due, for age only.
result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release("0.86.0", 5)],
  now: pinnedAt + (MAX_PIN_AGE_DAYS + 1) * day,
});
assert.equal(result.due, true);
assert.equal(result.reasons.length, 1);
assert.match(result.reasons[0], /days old/);

// Young but past the minor gap: due, for the gap only.
const ahead = `0.${85 + MAX_MINOR_GAP + 1}.0`;
result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release(ahead, 10)],
  now: pinnedAt + 12 * day,
});
assert.equal(result.due, true);
assert.equal(result.minorGap, MAX_MINOR_GAP + 1);
assert.match(result.reasons[0], /minor releases ahead/);

// Exactly at the gap limit is still within it.
result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release(`0.${85 + MAX_MINOR_GAP}.0`, 10)],
  now: pinnedAt + 12 * day,
});
assert.equal(result.due, false);

// A major release is past the gap outright; the latest is chosen by version, not order.
result = assess({
  pinned: "0.85.1",
  releases: [release("1.0.0", 2), release("0.85.1", 0), release("0.86.0", 1)],
  now: pinnedAt + 3 * day,
});
assert.equal(result.due, true);
assert.equal(result.latest, "1.0.0");
assert.match(result.reasons[0], /new major release/);

// Tags that are not release versions are ignored, and an unpublished pin is an error.
result = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release("nightly", 1)],
  now: pinnedAt + 90 * day,
});
assert.equal(result.due, false);
assert.throws(
  () => assess({ pinned: "0.99.0", releases: [release("0.85.1", 0)], now: pinnedAt }),
  /not a published release/,
);

// The issue body names both versions and every reason.
const due = assess({
  pinned: "0.85.1",
  releases: [release("0.85.1", 0), release(ahead, 10)],
  now: pinnedAt + 40 * day,
});
const body = issueBody(due);
assert.match(body, /pins South 0\.85\.1/);
assert.match(body, new RegExp(`latest release is ${ahead.replaceAll(".", "\\.")}`));
for (const reason of due.reasons) {
  assert.ok(body.includes(reason));
}

console.log("South drift rule check: PASS");
