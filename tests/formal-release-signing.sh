#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_dir="$(mktemp -d "${TMPDIR:-/tmp}/token-station-formal-signing.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT

readonly version="1.1.3"
readonly stage="$test_dir/release"
mkdir -p "$stage" "$test_dir/cli-key" "$test_dir/wrong-cli-key"

readonly cli_assets=(
  "token-station-cli-$version-aarch64-apple-darwin.tar.gz"
  "token-station-cli-$version-x86_64-apple-darwin.tar.gz"
  "token-station-cli-$version-aarch64-unknown-linux-gnu.tar.gz"
  "token-station-cli-$version-x86_64-unknown-linux-gnu.tar.gz"
)
for file in "${cli_assets[@]}"; do
  printf 'fixture:%s\n' "$file" >"$stage/$file"
done
for file in \
  "token-station_${version}_aarch64.dmg" \
  "token-station_${version}_x86_64.dmg" \
  "token-station_${version}_aarch64.app.tar.gz" \
  "token-station_${version}_x86_64.app.tar.gz" \
  "token-station_${version}_x86_64.msi" \
  "token-station_${version}_x86_64.deb" \
  "token-station_${version}_x86_64.AppImage" \
  "token-station_${version}_x86_64.rpm"; do
  printf 'fixture:%s\n' "$file" >"$stage/$file"
done

SOURCE_DATE_EPOCH=1 cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- manifest \
  --version "$version" --out "$stage/manifest.json" \
  "${cli_assets[@]/#/$stage/}"
cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- keygen --out "$test_dir/cli-key" >/dev/null
cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- keygen --out "$test_dir/wrong-cli-key" >/dev/null

npx --prefix "$root/apps/desktop" tauri signer generate --ci --force \
  --password test-only --write-keys "$test_dir/updater.key" >/dev/null
npx --prefix "$root/apps/desktop" tauri signer generate --ci --force \
  --password test-only --write-keys "$test_dir/wrong-updater.key" >/dev/null

export TOKEN_STATION_RELEASE_PUBKEY_HEX
TOKEN_STATION_RELEASE_PUBKEY_HEX=$(tr -d '[:space:]' <"$test_dir/cli-key/release-signing.pub")
export TOKEN_STATION_UPDATER_PUBKEY
TOKEN_STATION_UPDATER_PUBKEY=$(tr -d '[:space:]' <"$test_dir/updater.key.pub")
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=test-only

"$root/scripts/sign-formal-release.sh" \
  --version "$version" \
  --dir "$stage" \
  --release-key "$test_dir/cli-key/release-signing.key" \
  --updater-key "$test_dir/updater.key" \
  --pub-date 2026-08-17T00:00:00Z

legacy_version=2.0.0
legacy_stage="$test_dir/legacy-release"
mkdir -p "$legacy_stage"
legacy_cli_assets=(
  "token-station-cli-$legacy_version-aarch64-apple-darwin.tar.gz"
  "token-station-cli-$legacy_version-x86_64-apple-darwin.tar.gz"
  "token-station-cli-$legacy_version-aarch64-unknown-linux-gnu.tar.gz"
  "token-station-cli-$legacy_version-x86_64-unknown-linux-gnu.tar.gz"
)
for file in "${legacy_cli_assets[@]}"; do
  printf 'fixture:%s\n' "$file" >"$legacy_stage/$file"
done
for file in \
  "token-station_${legacy_version}_aarch64.dmg" \
  "token-station_${legacy_version}_x86_64.dmg" \
  "token-station_${legacy_version}_aarch64.app.tar.gz" \
  "token-station_${legacy_version}_x86_64.app.tar.gz" \
  "token-station_${legacy_version}_x86_64.msi" \
  "token-station_${legacy_version}_x86_64.deb" \
  "token-station_${legacy_version}_x86_64.AppImage" \
  "token-station_${legacy_version}_x86_64.rpm"; do
  printf 'fixture:%s\n' "$file" >"$legacy_stage/$file"
done
SOURCE_DATE_EPOCH=1 cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- manifest \
  --version "$legacy_version" --out "$legacy_stage/manifest.json" \
  "${legacy_cli_assets[@]/#/$legacy_stage/}"
"$root/scripts/sign-formal-release.sh" \
  --version "$legacy_version" \
  --dir "$legacy_stage" \
  --release-key "$test_dir/cli-key/release-signing.key" \
  --updater-key "$test_dir/updater.key" \
  --pub-date 2026-08-17T00:00:00Z
[[ ! -e "$legacy_stage/token-station_${legacy_version}_x86_64.msi.sig" ]] || {
  echo "Windows v2.0.0 unexpectedly received an updater signature" >&2
  exit 1
}
node -e '
  const fs = require("node:fs");
  const manifest = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (Object.hasOwn(manifest.platforms, "windows-x86_64")) process.exit(1);
' "$legacy_stage/latest.json" || {
  echo "Windows v2.0.0 unexpectedly entered the updater manifest" >&2
  exit 1
}

if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify \
  --pubkey "$test_dir/wrong-cli-key/release-signing.pub" "$stage/manifest.json" >/dev/null 2>&1; then
  echo "wrong CLI release key verified the manifest" >&2
  exit 1
fi
if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify-updater \
  --pubkey "$test_dir/wrong-updater.key.pub" \
  "$stage/token-station_${version}_x86_64.msi" >/dev/null 2>&1; then
  echo "wrong updater key verified the Windows MSI" >&2
  exit 1
fi
if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify-updater \
  --pubkey "$test_dir/wrong-updater.key.pub" \
  "$stage/token-station_${version}_aarch64.app.tar.gz" >/dev/null 2>&1; then
  echo "wrong updater key verified the payload" >&2
  exit 1
fi

printf 'tampered\n' >>"$stage/token-station_${version}_x86_64.msi"
if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify-updater \
  --pubkey "$TOKEN_STATION_UPDATER_PUBKEY" \
  "$stage/token-station_${version}_x86_64.msi" >/dev/null 2>&1; then
  echo "changed Windows MSI passed updater signature verification" >&2
  exit 1
fi

printf 'tampered\n' >>"$stage/token-station_${version}_aarch64.app.tar.gz"
if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify-updater \
  --pubkey "$TOKEN_STATION_UPDATER_PUBKEY" \
  "$stage/token-station_${version}_aarch64.app.tar.gz" >/dev/null 2>&1; then
  echo "changed updater payload passed signature verification" >&2
  exit 1
fi

echo "formal release signing integration: PASS"
