#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/sign-formal-release.sh --version <x.y.z> --dir <release-directory> --release-key <file> --updater-key <file> --pub-date <RFC3339> [--notes-file <file>]" >&2
  exit 2
}

version=""
release_dir=""
release_key=""
updater_key=""
pub_date=""
notes_file=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version=${2:-}; shift 2 ;;
    --dir) release_dir=${2:-}; shift 2 ;;
    --release-key) release_key=${2:-}; shift 2 ;;
    --updater-key) updater_key=${2:-}; shift 2 ;;
    --pub-date) pub_date=${2:-}; shift 2 ;;
    --notes-file) notes_file=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
[[ -d "$release_dir" && -f "$release_key" && -f "$updater_key" && -n "$pub_date" ]] || usage
if [[ -n "$notes_file" && ! -f "$notes_file" ]]; then
  echo "release notes file does not exist: $notes_file" >&2
  exit 1
fi
: "${TOKEN_STATION_RELEASE_PUBKEY_HEX:?set the trusted CLI release public key}"
: "${TOKEN_STATION_UPDATER_PUBKEY:?set the trusted Tauri updater public key}"

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tauri="$root/apps/desktop/node_modules/.bin/tauri"
[[ -x "$tauri" ]] || { echo "install the locked desktop dependencies before offline signing" >&2; exit 1; }
node "$root/scripts/check-release-inputs.mjs" --version "$version" --dir "$release_dir"

manifest="$release_dir/manifest.json"
cargo run --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- sign --key "$release_key" "$manifest"
cargo run --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify \
  --pubkey "$TOKEN_STATION_RELEASE_PUBKEY_HEX" "$manifest"

updater_aarch64="$release_dir/token-station_${version}_aarch64.app.tar.gz"
updater_x86_64="$release_dir/token-station_${version}_x86_64.app.tar.gz"
updater_windows_x86_64="$release_dir/token-station_${version}_x86_64.msi"
export TAURI_SIGNING_PRIVATE_KEY_PATH="$updater_key"
"$tauri" signer sign "$updater_aarch64"
"$tauri" signer sign "$updater_x86_64"
if [[ "$version" != "2.0.0" ]]; then
  "$tauri" signer sign "$updater_windows_x86_64"
fi
unset TAURI_SIGNING_PRIVATE_KEY_PATH

updater_artifacts=("$updater_aarch64" "$updater_x86_64")
if [[ "$version" != "2.0.0" ]]; then
  updater_artifacts+=("$updater_windows_x86_64")
fi
for artifact in "${updater_artifacts[@]}"; do
  cargo run --locked --offline --manifest-path "$root/Cargo.toml" \
    -p token-station-release --bin ts-release -- verify-updater \
    --pubkey "$TOKEN_STATION_UPDATER_PUBKEY" "$artifact"
done

manifest_args=(
  --version "$version"
  --pub-date "$pub_date"
  --release-base-url "https://github.com/ballast-ai/token-station/releases/download/v$version"
  --output "$release_dir/latest.json"
  --artifact "darwin-aarch64=$updater_aarch64"
  --artifact "darwin-x86_64=$updater_x86_64"
)
if [[ "$version" == "2.0.0" ]]; then
  manifest_args+=(--platforms darwin-aarch64,darwin-x86_64)
else
  manifest_args+=(--artifact "windows-x86_64=$updater_windows_x86_64")
fi
if [[ -n "$notes_file" ]]; then
  manifest_args+=(--notes-file "$notes_file")
fi
node "$root/scripts/create-desktop-update-manifest.mjs" "${manifest_args[@]}"
node "$root/scripts/create-release-checksums.mjs" "$release_dir"
node "$root/scripts/check-release-assets.mjs" --version "$version" --dir "$release_dir"
echo "formal release offline signing: PASS (v$version)"
