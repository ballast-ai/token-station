import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../scripts/check-rustsec-exceptions.mjs", import.meta.url));
const registrySource = "registry+https://github.com/rust-lang/crates.io-index";
const checksum = "e4cf9e5bd8713f1990a92c422998826e0fb34824277b46bc59757373483c6694";
const pkg = (name, version, source = registrySource, digest = checksum) =>
  `[[package]]\nname = "${name}"\nversion = "${version}"\n${source ? `source = "${source}"\n` : ""}${digest ? `checksum = "${digest}"\n` : ""}\n`;
const repaired = pkg("glib", "0.22.10");
const futurePackage = pkg("future-dependency", "1.2.3");
const futureException = {
  id: "RUSTSEC-2099-0001",
  package: "future-dependency",
  locked_version: "1.2.3",
  category: "unsound",
  dependency_path: "desktop -> future-dependency",
  affected_api: ["future_api"],
  exposure_assessment: "Fixture only; no production exception is created.",
  platform_scope: "Linux",
  justification: "Fixture validating generic exception rules.",
  owner: "fixture-owner",
  approved_by: "fixture-approver",
  approved_at: "2026-01-01",
  expires_at: "2999-01-01",
  upstream_tracking: ["https://example.com/upstream"],
  remediation_trigger: "A published upstream repair.",
  review_evidence: ["Fixture evidence."],
};

let cases = 0;
function run(label, lock, exceptions = [], expectedError = null, schemaVersion = 1) {
  const root = mkdtempSync(path.join(tmpdir(), "token-station-rustsec-ledger-"));
  try {
    mkdirSync(path.join(root, "docs/security"), { recursive: true });
    mkdirSync(path.join(root, "apps/desktop/src-tauri"), { recursive: true });
    writeFileSync(path.join(root, "docs/security/rustsec-exceptions.json"), JSON.stringify({ schema_version: schemaVersion, exceptions }));
    writeFileSync(path.join(root, "apps/desktop/src-tauri/Cargo.lock"), `version = 4\n\n${lock}`);
    // Execute the production script with a complete isolated ledger and lock.
    // No implementation helpers are imported or re-created here.
    const result = spawnSync(process.execPath, [script], { cwd: root, encoding: "utf8", timeout: 10_000 });
    assert.equal(result.error, undefined, `${label}: ${result.error}`);
    const output = result.stdout + result.stderr;
    if (expectedError === null) {
      assert.equal(result.status, 0, `${label}: ${output}`);
      assert.match(output, /RustSec exceptions: PASS/);
    } else {
      assert.notEqual(result.status, 0, `${label}: unexpectedly passed`);
      assert.match(output, expectedError, `${label}: wrong failure: ${output}`);
    }
    cases += 1;
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

run("repaired dependency needs no obsolete exception", repaired);
run("CRLF lockfile retains exact matching", repaired.replaceAll("\n", "\r\n"));
run("affected dependency with empty ledger", pkg("glib", "0.18.5"), [], /affected GLib/);
run("inactive affected duplicate after repaired version", repaired + pkg("glib", "0.18.5"), [], /affected GLib/);
run("single-quoted affected duplicate", repaired + pkg("glib", "0.18.5").replaceAll('"', "'"), [], /affected GLib/);
run("spaced package table with affected duplicate", repaired + pkg("glib", "0.18.5").replace("[[package]]", "[[ package ]]"), [], /affected GLib/);
run("escaped package name cannot hide affected dependency", repaired + pkg("glib", "0.18.5").replace('name = "glib"', 'name = "\\u0067lib"'), [], /affected GLib/);
run("incomplete extra package fails closed", repaired + "[[package]]\nversion = \"0.18.5\"\n", [], /Incomplete package record/);
run("unrecognized package table fails closed", repaired + "[[other]]\nname = \"glib\"\nversion = \"0.18.5\"\n", [], /Unsupported package table/);
run("advisory lower bound", pkg("glib", "0.15.0"), [], /affected GLib/);
run("last affected release family", pkg("glib", "0.19.9"), [], /affected GLib/);
run("prerelease before patched release", pkg("glib", "0.20.0-rc.1"), [], /affected GLib/);
run("stable patched lower bound", pkg("glib", "0.20.0"));
run("unaffected old family cannot substitute the repair", pkg("glib", "0.14.9"), [], /repaired GLib/);
run("no GLib implementation", futurePackage, [], /GLib implementation/);
run("path crate with repaired version label", pkg("glib", "0.22.10", null), [], /published crates.io/);
run("Git crate with repaired version label", pkg("glib", "0.22.10", "git+https://example.com/glib#revision"), [], /published crates.io/);
run("missing archive checksum", pkg("glib", "0.22.10", registrySource, null), [], /archive checksum/);
run("invalid archive checksum", pkg("glib", "0.22.10", registrySource, "not-a-checksum"), [], /archive checksum/);
run("malformed dependency version", pkg("glib", "patched"), [], /GLib version/);
run("obsolete exception cannot be renewed", repaired, [{ ...futureException, id: "RUSTSEC-2024-0429", package: "glib", locked_version: "0.22.10" }], /must not be registered/);
run("future valid generic exception", repaired + futurePackage, [futureException]);
run("future expired exception", repaired + futurePackage, [{ ...futureException, expires_at: "2000-01-01" }], /exception expired/);
run("future duplicate exception", repaired + futurePackage, [futureException, futureException], /duplicate RustSec exception/);
run("future missing required owner", repaired + futurePackage, [{ ...futureException, owner: "" }], /missing owner/);
for (const field of Object.keys(futureException)) {
  const incomplete = { ...futureException };
  delete incomplete[field];
  run(`future missing ${field}`, repaired + futurePackage, [incomplete], new RegExp(`missing ${field}`));
}
run("future missing locked package", repaired, [futureException], /is not in the desktop lockfile/);
run("future mismatched locked version", repaired + futurePackage, [{ ...futureException, locked_version: "1.2.4" }], /is not in the desktop lockfile/);
run("future malformed advisory ID", repaired + futurePackage, [{ ...futureException, id: "not-an-advisory" }], /invalid RustSec advisory ID/);
run("future malformed expiry", repaired + futurePackage, [{ ...futureException, expires_at: "future" }], /invalid expires_at/);
run("future impossible calendar expiry", repaired + futurePackage, [{ ...futureException, expires_at: "2999-02-31" }], /invalid expires_at/);
run("future missing upstream tracking", repaired + futurePackage, [{ ...futureException, upstream_tracking: [] }], /upstream_tracking must be non-empty/);
run("future missing review evidence", repaired + futurePackage, [{ ...futureException, review_evidence: [] }], /review_evidence must be non-empty/);
run("package matching is literal rather than a regex", repaired + futurePackage, [{ ...futureException, package: "future.*" }], /is not in the desktop lockfile/);
run("invalid registry schema", repaired, [], /invalid RustSec exception registry schema/, 2);

console.log(`RustSec exception regression: PASS (${cases} production-script cases)`);
