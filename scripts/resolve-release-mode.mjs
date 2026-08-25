#!/usr/bin/env node
// Decides whether a run produces formal release artifacts, and refuses to let
// a version tag quietly degrade into a source-only release.
//
// The old rule was a single test: `TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED ==
// "true"` enabled the real jobs, and anything else skipped them. Skipping is a
// green run. So pushing `v1.3.0` at a repository that had never been given its
// release configuration produced a wholly successful workflow that built
// nothing — the failure mode a release pipeline can least afford, because the
// evidence it emits is indistinguishable from success.
//
// This is a pure function of its environment so the policy can be tested
// without GitHub. Config arrives as presence booleans, never as values:
// `secrets.X != ''` is computable in a workflow expression, so no secret ever
// reaches this process's arguments or its log.
//
// Inputs (environment):
//   TS_EVENT           `github.event_name`
//   TS_REF             `github.ref`
//   TS_REQUESTED_MODE  the workflow_dispatch input; "" when the event has none
//   TS_REQUIRED        comma-separated config names this workflow needs
//   TS_HAS_<NAME>      "true" when that config is present, anything else when not
//
// Output: `mode=formal` or `mode=source-only` on GITHUB_OUTPUT, or exit 1
// naming every missing item.

const FORMAL = "formal";
const SOURCE_ONLY = "source-only";

const env = (name) => process.env[name] ?? "";
const fail = (lines) => {
  for (const line of lines) console.error(line);
  process.exit(1);
};

const event = env("TS_EVENT");
const ref = env("TS_REF");
const requested = env("TS_REQUESTED_MODE");
const required = env("TS_REQUIRED")
  .split(",")
  .map((name) => name.trim())
  .filter(Boolean);

const isVersionTag = event === "push" && /^refs\/tags\/v/.test(ref);

let intent;
if (isVersionTag) {
  // A version tag is a release. It has no source-only reading.
  intent = FORMAL;
} else if (event === "workflow_dispatch") {
  if (requested === FORMAL || requested === SOURCE_ONLY) {
    intent = requested;
  } else {
    fail([
      `unknown release mode: ${JSON.stringify(requested)}`,
      `a manual run must ask for "${FORMAL}" or "${SOURCE_ONLY}" explicitly`,
    ]);
  }
} else {
  // Branch pushes, pull requests and schedules never publish.
  intent = SOURCE_ONLY;
}

if (intent === SOURCE_ONLY) {
  const summary = process.env.GITHUB_STEP_SUMMARY;
  const note = "SOURCE_ONLY: formal release artifacts are skipped for this run.";
  if (summary) {
    const { appendFileSync } = await import("node:fs");
    appendFileSync(summary, `${note}\n`);
  }
  console.log(note);
} else {
  const missing = required.filter(
    (name) => env(`TS_HAS_${name}`).trim().toLowerCase() !== "true",
  );
  if (missing.length > 0) {
    fail([
      isVersionTag
        ? `refusing to build ${ref} as a release: formal configuration is incomplete`
        : "refusing a formal run: configuration is incomplete",
      ...missing.map((name) => `  missing: ${name}`),
      "set these in the repository, or run manually with mode=source-only",
    ]);
  }
  console.log("FORMAL: all required release configuration is present.");
}

const output = process.env.GITHUB_OUTPUT;
if (output) {
  const { appendFileSync } = await import("node:fs");
  appendFileSync(output, `mode=${intent}\n`);
  appendFileSync(output, `formal=${intent === FORMAL}\n`);
}
