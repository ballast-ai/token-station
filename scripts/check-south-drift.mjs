#!/usr/bin/env node
// Reports how far the pinned South release has fallen behind, and opens one
// tracking issue when a re-pin is due.
//
// The host pins South by exact revision, so nothing moves it forward on its
// own. South 0.20.0 sat pinned while South shipped sixty-five releases; the
// catch-up then had to absorb every contract change at once. The agreed cadence
// is a monthly re-pin. This check makes a missed month visible instead of
// silent. It never fails a build: drift is a scheduling fact, not a defect.
//
// Usage:
//   node scripts/check-south-drift.mjs            print the assessment
//   node scripts/check-south-drift.mjs --issue    also open or update the issue
//
// The --issue mode needs the `gh` CLI with GH_TOKEN allowed to write issues.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const southRepository = "ballast-ai/token-station-south";
export const ISSUE_TITLE_PREFIX = "Re-pin South";
// A re-pin is due when either limit is passed while a newer release exists.
export const MAX_PIN_AGE_DAYS = 31;
export const MAX_MINOR_GAP = 10;

/** The South release the dependency policy pins (`southRelease`). */
export function pinnedRelease(policySource) {
  const version = policySource.match(/^const southRelease = "(\d+\.\d+\.\d+)";$/m)?.[1];
  if (!version) {
    throw new Error("tests/south-dependency-policy.mjs declares no southRelease");
  }
  return version;
}

function triple(version) {
  const parts = version.replace(/^v/, "").split(".").map(Number);
  if (parts.length !== 3 || parts.some((part) => !Number.isInteger(part) || part < 0)) {
    throw new Error(`not a release version: ${version}`);
  }
  return parts;
}

function compare(left, right) {
  const [a, b] = [triple(left), triple(right)];
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return 0;
}

/**
 * Judges the pin against the published releases.
 *
 * `releases` is a list of `{ version, publishedAt }` (ISO 8601). Pre-releases
 * and drafts must already be filtered out.
 */
export function assess({ pinned, releases, now }) {
  const published = releases.filter((release) => {
    try {
      triple(release.version);
      return true;
    } catch {
      return false;
    }
  });
  const pin = published.find((release) => compare(release.version, pinned) === 0);
  if (!pin) {
    throw new Error(`South ${pinned} is not a published release`);
  }
  const newer = published.filter((release) => compare(release.version, pinned) > 0);
  if (newer.length === 0) {
    return { due: false, pinned, latest: pinned, newer: 0, ageDays: 0, minorGap: 0, reasons: [] };
  }
  const latest = newer.reduce((best, release) =>
    compare(release.version, best.version) > 0 ? release : best,
  );
  const ageDays = Math.floor((now - Date.parse(pin.publishedAt)) / 86_400_000);
  const [pinnedMajor, pinnedMinor] = triple(pinned);
  const [latestMajor, latestMinor] = triple(latest.version);
  // A major bump counts as past the gap limit outright.
  const minorGap =
    latestMajor === pinnedMajor ? latestMinor - pinnedMinor : Number.POSITIVE_INFINITY;
  const reasons = [];
  if (ageDays > MAX_PIN_AGE_DAYS) {
    reasons.push(`the pinned release is ${ageDays} days old (limit ${MAX_PIN_AGE_DAYS})`);
  }
  if (minorGap > MAX_MINOR_GAP) {
    reasons.push(
      Number.isFinite(minorGap)
        ? `South is ${minorGap} minor releases ahead (limit ${MAX_MINOR_GAP})`
        : `South moved to a new major release (${latest.version})`,
    );
  }
  return {
    due: reasons.length > 0,
    pinned,
    latest: latest.version,
    newer: newer.length,
    ageDays,
    minorGap,
    reasons,
  };
}

/** The issue body for a due re-pin. */
export function issueBody(assessment) {
  return [
    `The host pins South ${assessment.pinned}. The latest release is ${assessment.latest} ` +
      `(${assessment.newer} newer release${assessment.newer === 1 ? "" : "s"}).`,
    "",
    "A re-pin is due because:",
    ...assessment.reasons.map((reason) => `- ${reason}`),
    "",
    "Re-pin in one PR per the South upgrade design record: host libraries first, then the",
    "shipped components. Run the full local gate set from `.github/workflows/ci.yml`.",
    "",
    "Opened by `scripts/check-south-drift.mjs`. It updates this issue instead of opening another.",
  ].join("\n");
}

function gh(args) {
  return execFileSync("gh", args, { encoding: "utf8" });
}

function fetchReleases() {
  const pages = gh([
    "api",
    "--paginate",
    `repos/${southRepository}/releases?per_page=100`,
    "--jq",
    ".[] | select(.draft == false and .prerelease == false) | [.tag_name, .published_at] | @tsv",
  ]);
  return pages
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [tag, publishedAt] = line.split("\t");
      return { version: tag.replace(/^v/, ""), publishedAt };
    });
}

function fileIssue(assessment) {
  const title = `${ISSUE_TITLE_PREFIX}: pinned ${assessment.pinned}, latest ${assessment.latest}`;
  const open = JSON.parse(
    gh([
      "issue",
      "list",
      "--state",
      "open",
      "--search",
      `"${ISSUE_TITLE_PREFIX}" in:title`,
      "--json",
      "number,title",
    ]),
  ).filter((issue) => issue.title.startsWith(ISSUE_TITLE_PREFIX));
  const body = issueBody(assessment);
  if (open.length > 0) {
    gh(["issue", "edit", String(open[0].number), "--title", title, "--body", body]);
    return `updated issue #${open[0].number}`;
  }
  return gh(["issue", "create", "--title", title, "--body", body]).trim();
}

function main() {
  const pinned = pinnedRelease(
    fs.readFileSync(path.join(root, "tests/south-dependency-policy.mjs"), "utf8"),
  );
  const assessment = assess({ pinned, releases: fetchReleases(), now: Date.now() });
  if (!assessment.due) {
    console.log(
      `South drift: pinned ${assessment.pinned}, latest ${assessment.latest}; no re-pin due`,
    );
    return;
  }
  console.log(`::warning::South re-pin due: ${assessment.reasons.join("; ")}`);
  if (process.argv.includes("--issue")) {
    console.log(fileIssue(assessment));
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
