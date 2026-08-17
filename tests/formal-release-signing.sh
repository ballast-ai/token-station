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
  "token-station_${version}_x86_64.app.tar.gz"; do
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

if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify \
  --pubkey "$test_dir/wrong-cli-key/release-signing.pub" "$stage/manifest.json" >/dev/null 2>&1; then
  echo "wrong CLI release key verified the manifest" >&2
  exit 1
fi
if cargo run --quiet --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify-updater \
  --pubkey "$test_dir/wrong-updater.key.pub" \
  "$stage/token-station_${version}_aarch64.app.tar.gz" >/dev/null 2>&1; then
  echo "wrong updater key verified the payload" >&2
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
