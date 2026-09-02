#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
collector="$root/scripts/collect-formal-release-artifacts.sh"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/token-station-formal-artifacts.XXXXXX")
trap 'rm -rf "$test_root"' EXIT

fail() {
  echo "formal release artifact collection test failed: $*" >&2
  exit 1
}

make_artifacts() {
  local artifacts=$1
  local version=${2:-2.0.0}
  mkdir -p \
    "$artifacts/dist-aarch64-apple-darwin" \
    "$artifacts/dist-aarch64-unknown-linux-gnu" \
    "$artifacts/dist-x86_64-apple-darwin" \
    "$artifacts/dist-x86_64-unknown-linux-gnu" \
    "$artifacts/token-station-desktop-aarch64-apple-darwin" \
    "$artifacts/token-station-desktop-x86_64-apple-darwin" \
    "$artifacts/token-station-desktop-x86_64-pc-windows-msvc" \
    "$artifacts/token-station-desktop-linux-x86_64"

  printf 'fixture\n' >"$artifacts/dist-aarch64-apple-darwin/token-station-cli-$version-aarch64-apple-darwin.tar.gz"
  printf 'fixture\n' >"$artifacts/dist-aarch64-unknown-linux-gnu/token-station-cli-$version-aarch64-unknown-linux-gnu.tar.gz"
  printf 'fixture\n' >"$artifacts/dist-x86_64-apple-darwin/token-station-cli-$version-x86_64-apple-darwin.tar.gz"
  printf 'fixture\n' >"$artifacts/dist-x86_64-unknown-linux-gnu/token-station-cli-$version-x86_64-unknown-linux-gnu.tar.gz"
  printf 'fixture\n' >"$artifacts/token-station-desktop-aarch64-apple-darwin/token-station.app.tar.gz"
  printf 'fixture\n' >"$artifacts/token-station-desktop-aarch64-apple-darwin/token-station_${version}_aarch64.dmg"
  printf 'fixture\n' >"$artifacts/token-station-desktop-x86_64-apple-darwin/token-station.app.tar.gz"
  printf 'fixture\n' >"$artifacts/token-station-desktop-x86_64-apple-darwin/token-station_${version}_x86_64.dmg"
  printf 'fixture\n' >"$artifacts/token-station-desktop-x86_64-pc-windows-msvc/token-station_${version}_x86_64.msi"
  if [[ "$version" != "2.0.0" ]]; then
    printf 'temporary signature\n' >"$artifacts/token-station-desktop-x86_64-pc-windows-msvc/token-station_${version}_x86_64.msi.sig"
  fi
  printf 'fixture\n' >"$artifacts/token-station-desktop-linux-x86_64/token-station_${version}_x86_64.AppImage"
  printf 'fixture\n' >"$artifacts/token-station-desktop-linux-x86_64/token-station_${version}_x86_64.deb"
  printf 'fixture\n' >"$artifacts/token-station-desktop-linux-x86_64/token-station_${version}_x86_64.rpm"
}

artifacts="$test_root/artifacts"
output="$test_root/output"
make_artifacts "$artifacts"
"$collector" --version 2.0.0 --artifacts-dir "$artifacts" --out-dir "$output"

expected=$(printf '%s\n' \
  token-station-cli-2.0.0-aarch64-apple-darwin.tar.gz \
  token-station-cli-2.0.0-aarch64-unknown-linux-gnu.tar.gz \
  token-station-cli-2.0.0-x86_64-apple-darwin.tar.gz \
  token-station-cli-2.0.0-x86_64-unknown-linux-gnu.tar.gz \
  token-station_2.0.0_aarch64.app.tar.gz \
  token-station_2.0.0_aarch64.dmg \
  token-station_2.0.0_x86_64.AppImage \
  token-station_2.0.0_x86_64.app.tar.gz \
  token-station_2.0.0_x86_64.deb \
  token-station_2.0.0_x86_64.dmg \
  token-station_2.0.0_x86_64.msi \
  token-station_2.0.0_x86_64.rpm | sort)
actual=$(find "$output" -mindepth 1 -maxdepth 1 -type f -exec basename {} \; | sort)
[[ "$actual" == "$expected" ]] || fail "the collected file set is not exact"
while IFS= read -r file; do
  [[ -s "$output/$file" ]] || fail "the collected file is empty: $file"
done <<<"$expected"

future_artifacts="$test_root/future-artifacts"
make_artifacts "$future_artifacts" 2.0.1
"$collector" --version 2.0.1 --artifacts-dir "$future_artifacts" --out-dir "$test_root/future-output"
rm "$future_artifacts/token-station-desktop-x86_64-pc-windows-msvc/token-station_2.0.1_x86_64.msi.sig"
if "$collector" --version 2.0.1 --artifacts-dir "$future_artifacts" --out-dir "$test_root/missing-windows-signature-output" >/dev/null 2>&1; then
  fail "the collector accepted a future Windows MSI without its temporary updater signature"
fi

with_logs="$test_root/with-logs"
make_artifacts "$with_logs"
mkdir "$with_logs/token-station-windows-msi-lifecycle-logs"
printf 'test log\n' >"$with_logs/token-station-windows-msi-lifecycle-logs/install.log"
if "$collector" --version 2.0.0 --artifacts-dir "$with_logs" --out-dir "$test_root/log-output" >/dev/null 2>&1; then
  fail "the collector accepted an unexpected lifecycle-log artifact"
fi

with_extra_file="$test_root/with-extra-file"
make_artifacts "$with_extra_file"
printf 'unexpected\n' >"$with_extra_file/token-station-desktop-aarch64-apple-darwin/README.txt"
if "$collector" --version 2.0.0 --artifacts-dir "$with_extra_file" --out-dir "$test_root/extra-output" >/dev/null 2>&1; then
  fail "the collector accepted an unexpected file"
fi

echo "formal release artifact collection: PASS"
