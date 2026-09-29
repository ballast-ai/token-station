#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/collect-formal-release-artifacts.sh --version <x.y.z> --artifacts-dir <directory> --out-dir <empty-directory>" >&2
  exit 2
}

fail() {
  echo "formal release artifact collection stopped: $*" >&2
  exit 1
}

version=""
artifacts_dir=""
out_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version=${2:-}; shift 2 ;;
    --artifacts-dir) artifacts_dir=${2:-}; shift 2 ;;
    --out-dir) out_dir=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && -d "$artifacts_dir" && -n "$out_dir" ]] || usage

if [[ -e "$out_dir" ]]; then
  [[ -d "$out_dir" ]] || fail "the output path is not a directory: $out_dir"
  [[ -z "$(find "$out_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
    fail "the output directory is not empty: $out_dir"
fi

require_exact_entries() {
  local directory=$1
  shift
  [[ -d "$directory" ]] || fail "missing artifact directory: $(basename "$directory")"
  local expected
  local actual
  expected=$(printf '%s\n' "$@" | LC_ALL=C sort)
  actual=$(find "$directory" -mindepth 1 -maxdepth 1 -exec basename {} \; | LC_ALL=C sort)
  [[ "$actual" == "$expected" ]] || {
    echo "expected entries in $directory:" >&2
    printf '%s\n' "$expected" >&2
    echo "actual entries in $directory:" >&2
    printf '%s\n' "$actual" >&2
    fail "the artifact file set is not exact"
  }
  local file
  for file in "$@"; do
    [[ -f "$directory/$file" && -s "$directory/$file" ]] ||
      fail "the artifact file is missing or empty: $directory/$file"
  done
}

readonly cli_aarch64_macos="token-station-cli-$version-aarch64-apple-darwin.tar.gz"
readonly cli_aarch64_linux="token-station-cli-$version-aarch64-unknown-linux-gnu.tar.gz"
readonly cli_x86_64_macos="token-station-cli-$version-x86_64-apple-darwin.tar.gz"
readonly cli_x86_64_linux="token-station-cli-$version-x86_64-unknown-linux-gnu.tar.gz"
readonly macos_aarch64_dmg="token-station_${version}_aarch64.dmg"
readonly macos_x86_64_dmg="token-station_${version}_x86_64.dmg"
readonly windows_msi="token-station_${version}_x86_64.msi"
readonly linux_appimage="token-station_${version}_x86_64.AppImage"
readonly linux_deb="token-station_${version}_x86_64.deb"
readonly linux_rpm="token-station_${version}_x86_64.rpm"

readonly -a artifact_names=(
  dist-aarch64-apple-darwin
  dist-aarch64-unknown-linux-gnu
  dist-x86_64-apple-darwin
  dist-x86_64-unknown-linux-gnu
  token-station-desktop-aarch64-apple-darwin
  token-station-desktop-x86_64-apple-darwin
  token-station-desktop-x86_64-pc-windows-msvc
  token-station-desktop-linux-x86_64
)

expected_artifacts=$(printf '%s\n' "${artifact_names[@]}" | LC_ALL=C sort)
actual_artifacts=$(find "$artifacts_dir" -mindepth 1 -maxdepth 1 -exec basename {} \; | LC_ALL=C sort)
[[ "$actual_artifacts" == "$expected_artifacts" ]] || {
  echo "expected formal artifacts:" >&2
  printf '%s\n' "$expected_artifacts" >&2
  echo "actual formal artifacts:" >&2
  printf '%s\n' "$actual_artifacts" >&2
  fail "the workflow artifact set is not exact"
}

require_exact_entries "$artifacts_dir/dist-aarch64-apple-darwin" "$cli_aarch64_macos"
require_exact_entries "$artifacts_dir/dist-aarch64-unknown-linux-gnu" "$cli_aarch64_linux"
require_exact_entries "$artifacts_dir/dist-x86_64-apple-darwin" "$cli_x86_64_macos"
require_exact_entries "$artifacts_dir/dist-x86_64-unknown-linux-gnu" "$cli_x86_64_linux"
require_exact_entries "$artifacts_dir/token-station-desktop-aarch64-apple-darwin" \
  token-station.app.tar.gz "$macos_aarch64_dmg"
require_exact_entries "$artifacts_dir/token-station-desktop-x86_64-apple-darwin" \
  token-station.app.tar.gz "$macos_x86_64_dmg"
require_exact_entries "$artifacts_dir/token-station-desktop-x86_64-pc-windows-msvc" "$windows_msi"
require_exact_entries "$artifacts_dir/token-station-desktop-linux-x86_64" \
  "$linux_appimage" "$linux_deb" "$linux_rpm"

mkdir -p "$out_dir"
cp "$artifacts_dir/dist-aarch64-apple-darwin/$cli_aarch64_macos" "$out_dir/"
cp "$artifacts_dir/dist-aarch64-unknown-linux-gnu/$cli_aarch64_linux" "$out_dir/"
cp "$artifacts_dir/dist-x86_64-apple-darwin/$cli_x86_64_macos" "$out_dir/"
cp "$artifacts_dir/dist-x86_64-unknown-linux-gnu/$cli_x86_64_linux" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-aarch64-apple-darwin/$macos_aarch64_dmg" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-aarch64-apple-darwin/token-station.app.tar.gz" \
  "$out_dir/token-station_${version}_aarch64.app.tar.gz"
cp "$artifacts_dir/token-station-desktop-x86_64-apple-darwin/$macos_x86_64_dmg" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-x86_64-apple-darwin/token-station.app.tar.gz" \
  "$out_dir/token-station_${version}_x86_64.app.tar.gz"
cp "$artifacts_dir/token-station-desktop-x86_64-pc-windows-msvc/$windows_msi" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-linux-x86_64/$linux_appimage" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-linux-x86_64/$linux_deb" "$out_dir/"
cp "$artifacts_dir/token-station-desktop-linux-x86_64/$linux_rpm" "$out_dir/"

echo "formal release artifact collection: PASS (v$version)"
