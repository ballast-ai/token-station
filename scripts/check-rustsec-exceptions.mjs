import { readFileSync } from "node:fs";

const registryPath = "docs/security/rustsec-exceptions.json";
const lockPath = "apps/desktop/src-tauri/Cargo.lock";
const requiredFields = [
  "id",
  "package",
  "locked_version",
  "category",
  "dependency_path",
  "affected_api",
  "exposure_assessment",
  "platform_scope",
  "justification",
  "owner",
  "approved_by",
  "approved_at",
  "expires_at",
  "upstream_tracking",
  "remediation_trigger",
  "review_evidence",
];

const registry = JSON.parse(readFileSync(registryPath, "utf8"));
if (registry.schema_version !== 1 || !Array.isArray(registry.exceptions)) {
  throw new Error("invalid RustSec exception registry schema");
}

const ids = new Set();
const today = new Date().toISOString().slice(0, 10);
const lock = readFileSync(lockPath, "utf8");
// Inspect every package record, including inactive target-specific duplicates.
// Require an actual registry implementation; a path crate with a repaired
// version label cannot close the advisory. Cargo's locked build separately
// verifies these declared archive checksums against crates.io source bytes.
const packageHeader = /^[ \t]*\[\[[ \t]*(?:package|"package"|'package')[ \t]*\]\][ \t]*(?:#[^\r\n]*)?\r?$/m;
for (const header of lock.match(/^[ \t]*\[\[.*\]\].*$/gm) ?? []) {
  if (!packageHeader.test(header)) {
    throw new Error("Unsupported package table in desktop Cargo.lock.");
  }
}
const packages = lock.split(packageHeader).slice(1).map((block) => {
  const field = (name) => {
    const values = [...block.matchAll(new RegExp(`^${name}[ \\t]*=[ \\t]*(.*)$`, "gm"))];
    if (values.length === 0) return undefined;
    if (values.length !== 1) throw new Error(`Duplicate ${name} in desktop Cargo.lock package.`);
    const quoted = /^("(?:[^"\\]|\\.)*"|'[^']*')[ \t]*(?:#.*)?\r?$/.exec(values[0][1]);
    if (!quoted) throw new Error(`Invalid ${name} field in desktop Cargo.lock package.`);
    return quoted[1].startsWith('"') ? JSON.parse(quoted[1]) : quoted[1].slice(1, -1);
  };
  const pkg = { name: field("name"), version: field("version"), source: field("source"), checksum: field("checksum") };
  if (pkg.name === undefined || pkg.version === undefined) {
    throw new Error("Incomplete package record in desktop Cargo.lock.");
  }
  return pkg;
});
const glib = packages.filter((pkg) => pkg.name === "glib");
if (glib.length === 0) {
  throw new Error("The desktop lockfile must retain a real Linux GLib implementation.");
}
let hasRepair = false;
for (const pkg of glib) {
  const version = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(pkg.version ?? "");
  if (!version || !version.slice(1, 4).map(Number).every(Number.isSafeInteger)) {
    throw new Error(`Invalid GLib version in desktop lockfile: ${pkg.version}`);
  }
  const [major, minor, patch] = version.slice(1, 4).map(Number);
  const beforePatchedRelease = major === 0 && minor === 20 && patch === 0 && version[4] !== undefined;
  if (major === 0 && ((minor >= 15 && minor < 20) || beforePatchedRelease)) {
    throw new Error(`RUSTSEC-2024-0429: affected GLib remains in the desktop lockfile: ${pkg.version}`);
  }
  if (pkg.source !== "registry+https://github.com/rust-lang/crates.io-index") {
    throw new Error(`GLib ${pkg.version} must use the published crates.io implementation.`);
  }
  if (!/^[a-f0-9]{64}$/.test(pkg.checksum ?? "")) {
    throw new Error(`GLib ${pkg.version} must retain its upstream archive checksum.`);
  }
  hasRepair ||= major > 0 || minor > 20 || (minor === 20 && !beforePatchedRelease);
}
if (!hasRepair) {
  throw new Error("The desktop lockfile must contain a repaired GLib release (>=0.20.0).");
}

for (const exception of registry.exceptions) {
  for (const field of requiredFields) {
    const value = exception[field];
    if (value === undefined || value === null || value === "") {
      throw new Error(`${exception.id ?? "unknown"}: missing ${field}`);
    }
  }
  if (ids.has(exception.id)) {
    throw new Error(`duplicate RustSec exception: ${exception.id}`);
  }
  ids.add(exception.id);

  if (!/^RUSTSEC-\d{4}-\d{4}$/.test(exception.id)) {
    throw new Error(`invalid RustSec advisory ID: ${exception.id}`);
  }
  if (exception.id === "RUSTSEC-2024-0429") {
    throw new Error("RUSTSEC-2024-0429 must not be registered as an exception after the GLib repair.");
  }
  const expiry = new Date(`${exception.expires_at}T00:00:00Z`);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(exception.expires_at)
      || !Number.isFinite(expiry.getTime())
      || expiry.toISOString().slice(0, 10) !== exception.expires_at) {
    throw new Error(`${exception.id}: invalid expires_at`);
  }
  if (exception.expires_at < today) {
    throw new Error(`${exception.id}: exception expired on ${exception.expires_at}`);
  }
  if (!Array.isArray(exception.upstream_tracking) || exception.upstream_tracking.length === 0) {
    throw new Error(`${exception.id}: upstream_tracking must be non-empty`);
  }
  if (!Array.isArray(exception.review_evidence) || exception.review_evidence.length === 0) {
    throw new Error(`${exception.id}: review_evidence must be non-empty`);
  }

  if (!packages.some((pkg) => pkg.name === exception.package && pkg.version === exception.locked_version)) {
    throw new Error(
      `${exception.id}: ${exception.package} ${exception.locked_version} is not in the desktop lockfile`,
    );
  }
}

console.log(`RustSec exceptions: PASS (${registry.exceptions.length} active, checked ${today})`);
