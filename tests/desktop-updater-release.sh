#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_dir="$(mktemp -d "${TMPDIR:-/tmp}/token-station-updater-release.XXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

readonly mac_platforms=(darwin-aarch64 darwin-x86_64)

mkdir -p "$test_dir/payloads"
for platform in "${mac_platforms[@]}"; do
  payload="$test_dir/payloads/token-station-$platform.tar.gz"
  printf 'payload-%s\n' "$platform" >"$payload"
  printf 'trusted-signature-%s\n' "$platform" >"$payload.sig"
done
printf '修复安全问题。\n' >"$test_dir/notes.md"

node "$root/scripts/create-desktop-update-manifest.mjs" \
  --version 1.2.3 \
  --pub-date 2026-08-06T08:00:00Z \
  --release-base-url https://github.com/ballast-ai/token-station/releases/download/v1.2.3 \
  --notes-file "$test_dir/notes.md" \
  --output "$test_dir/latest.json" \
  --artifact "darwin-aarch64=$test_dir/payloads/token-station-darwin-aarch64.tar.gz" \
  --artifact "darwin-x86_64=$test_dir/payloads/token-station-darwin-x86_64.tar.gz"

node - "$test_dir/latest.json" <<'NODE'
const fs = require("node:fs");
const manifest = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (manifest.version !== "1.2.3") throw new Error("wrong version");
for (const platform of ["darwin-aarch64", "darwin-x86_64"]) {
  const entry = manifest.platforms[platform];
  if (!entry?.url.startsWith("https://github.com/ballast-ai/token-station/releases/download/v1.2.3/")) {
    throw new Error(`wrong URL for ${platform}`);
  }
  if (entry.signature !== `trusted-signature-${platform}`) {
    throw new Error(`wrong signature for ${platform}`);
  }
}
if ("windows-x86_64" in manifest.platforms) {
  throw new Error("first updater release must not publish a Windows platform entry");
}
NODE

node "$root/scripts/create-desktop-update-manifest.mjs" \
  --version 1.2.3 \
  --pub-date 2026-08-06T08:00:00Z \
  --release-base-url https://github.com/ballast-ai/token-station/releases/download/v1.2.3 \
  --output "$test_dir/preview-latest.json" \
  --platforms darwin-aarch64 \
  --artifact "darwin-aarch64=$test_dir/payloads/token-station-darwin-aarch64.tar.gz"

node - "$test_dir/preview-latest.json" <<'NODE'
const fs = require("node:fs");
const manifest = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (JSON.stringify(Object.keys(manifest.platforms)) !== JSON.stringify(["darwin-aarch64"])) {
  throw new Error("preview manifest must contain only the selected Apple Silicon platform");
}
NODE

if node "$root/scripts/create-desktop-update-manifest.mjs" \
  --version 1.2.3 \
  --pub-date 2026-08-06T08:00:00Z \
  --release-base-url https://github.com/ballast-ai/token-station/releases/download/v1.2.3 \
  --output "$test_dir/windows-rejected.json" \
  --artifact "darwin-aarch64=$test_dir/payloads/token-station-darwin-aarch64.tar.gz" \
  --artifact "darwin-x86_64=$test_dir/payloads/token-station-darwin-x86_64.tar.gz" \
  --artifact "windows-x86_64=$test_dir/payloads/token-station-windows-x86_64.msi" \
  >"$test_dir/windows-rejected.log" 2>&1; then
  fail "manifest generation accepted a Windows updater artifact for the first release"
fi

rm "$test_dir/payloads/token-station-darwin-x86_64.tar.gz.sig"
if node "$root/scripts/create-desktop-update-manifest.mjs" \
  --version 1.2.3 \
  --pub-date 2026-08-06T08:00:00Z \
  --release-base-url https://github.com/ballast-ai/token-station/releases/download/v1.2.3 \
  --output "$test_dir/rejected.json" \
  --artifact "darwin-aarch64=$test_dir/payloads/token-station-darwin-aarch64.tar.gz" \
  --artifact "darwin-x86_64=$test_dir/payloads/token-station-darwin-x86_64.tar.gz" \
  >"$test_dir/rejected.log" 2>&1; then
  fail "manifest generation accepted a missing offline signature"
fi

workflow="$root/.github/workflows/desktop-release.yml"
node - "$root/apps/desktop/src-tauri/tauri.conf.json" <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (config.plugins?.updater?.pubkey !== "") {
  throw new Error("local updater plugin must initialize with an explicit empty public key");
}
NODE
node - "$workflow" <<'NODE'
const fs = require("node:fs");
const workflow = fs.readFileSync(process.argv[2], "utf8");
const windowsStart = workflow.indexOf("\n  windows:");
const macos = workflow.slice(0, windowsStart);
const windows = workflow.slice(windowsStart);
for (const required of [
  "TOKEN_STATION_UPDATER_PUBKEY: ${{ vars.TOKEN_STATION_UPDATER_PUBKEY }}",
  "tauri signer generate",
  "*.app.tar.gz",
]) {
if (!macos.includes(required)) {
    throw new Error(`macOS release path lost required updater setting: ${required}`);
  }
}
if (!windows.includes("if: ${{ false }} # First public desktop release is macOS-only.")) {
  throw new Error("Windows desktop release job must be explicitly skipped for the first release");
}
for (const forbidden of [
  "TOKEN_STATION_UPDATER_PUBKEY",
  "tauri signer generate",
  "TAURI_SIGNING_PRIVATE_KEY",
]) {
  if (windows.includes(forbidden)) {
    throw new Error(`Windows first release must not configure updater signing: ${forbidden}`);
  }
}
NODE
if grep -Eq '^[[:space:]]+.*\*\.sig[[:space:]]*$' "$workflow"; then
  fail "desktop workflow uploads temporary CI signatures"
fi

echo "desktop updater release tests: PASS"
