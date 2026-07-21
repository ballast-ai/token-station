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
  if (!/^\d{4}-\d{2}-\d{2}$/.test(exception.expires_at)) {
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

  const packageRecord = new RegExp(
    `\\[\\[package\\]\\]\\nname = "${exception.package}"\\nversion = "${exception.locked_version.replaceAll(".", "\\.")}"(?:\\n|$)`,
  );
  if (!packageRecord.test(lock)) {
    throw new Error(
      `${exception.id}: ${exception.package} ${exception.locked_version} is not in the desktop lockfile`,
    );
  }
}

if (!ids.has("RUSTSEC-2024-0429")) {
  throw new Error("required desktop exception RUSTSEC-2024-0429 is not registered");
}

console.log(`RustSec exceptions: PASS (${registry.exceptions.length} active, checked ${today})`);
