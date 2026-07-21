#!/usr/bin/env bash
set -euo pipefail

readonly script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly checker_source="$script_dir/check-router-core-redline.sh"
readonly scratch_dir="$(mktemp -d)"
readonly fixture_repo="$scratch_dir/repo"
readonly fixture_checker="$scratch_dir/check-router-core-redline.sh"

cleanup() {
  if [[ -n "${scratch_dir:-}" && "$scratch_dir" == /tmp/* ]]; then
    rm -rf -- "$scratch_dir"
  fi
}
trap cleanup EXIT

git -c init.defaultBranch=main init "$fixture_repo" >/dev/null
git -C "$fixture_repo" config user.name 'Token Station Redline Test'
git -C "$fixture_repo" config user.email 'redline-test@invalid.example'
mkdir -p "$fixture_repo/crates/router-core/src" "$fixture_repo/apps/desktop"
printf 'baseline\n' >"$fixture_repo/crates/router-core/src/lib.rs"
printf 'desktop\n' >"$fixture_repo/apps/desktop/state.txt"
git -C "$fixture_repo" add .
git -C "$fixture_repo" commit -m baseline >/dev/null
readonly baseline_sha="$(git -C "$fixture_repo" rev-parse HEAD)"

# Tests run the production script with only its immutable project baseline
# replaced by the fixture repository's immutable baseline.
sed "s/^readonly FROZEN_BASE=.*/readonly FROZEN_BASE='$baseline_sha'/" \
  "$checker_source" >"$fixture_checker"
chmod +x "$fixture_checker"

expect_status() {
  local expected="$1"
  local label="$2"
  shift 2
  local actual

  set +e
  (cd "$fixture_repo" && "$fixture_checker" "$@") >/dev/null 2>&1
  actual=$?
  set -e
  if [[ $actual -ne $expected ]]; then
    echo "FAIL: $label — expected status $expected, got $actual" >&2
    exit 1
  fi
}

expect_status 0 'symbolic HEAD is accepted' HEAD HEAD

printf 'desktop changed\n' >"$fixture_repo/apps/desktop/state.txt"
git -C "$fixture_repo" add apps/desktop/state.txt
git -C "$fixture_repo" commit -m peripheral-change >/dev/null
readonly peripheral_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 0 'peripheral change' "$baseline_sha" "$peripheral_sha"

printf 'core changed\n' >"$fixture_repo/crates/router-core/src/lib.rs"
git -C "$fixture_repo" add crates/router-core/src/lib.rs
git -C "$fixture_repo" commit -m core-content-change >/dev/null
readonly content_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'core content change' "$peripheral_sha" "$content_sha"

printf 'new core file\n' >"$fixture_repo/crates/router-core/src/new.rs"
git -C "$fixture_repo" add crates/router-core/src/new.rs
git -C "$fixture_repo" commit -m core-add >/dev/null
readonly add_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'core file addition' "$content_sha" "$add_sha"

git -C "$fixture_repo" rm crates/router-core/src/new.rs >/dev/null
git -C "$fixture_repo" commit -m core-delete >/dev/null
readonly delete_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'core file deletion' "$add_sha" "$delete_sha"

chmod +x "$fixture_repo/crates/router-core/src/lib.rs"
git -C "$fixture_repo" add crates/router-core/src/lib.rs
git -C "$fixture_repo" commit -m core-mode-change >/dev/null
readonly mode_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'core mode change' "$delete_sha" "$mode_sha"

git -C "$fixture_repo" mv crates/router-core/src/lib.rs crates/router-core/src/router.rs
git -C "$fixture_repo" commit -m core-rename >/dev/null
readonly rename_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'core rename' "$mode_sha" "$rename_sha"

printf 'desktop changed again\n' >"$fixture_repo/apps/desktop/state.txt"
git -C "$fixture_repo" add apps/desktop/state.txt
git -C "$fixture_repo" commit -m peripheral-after-core >/dev/null
readonly drifted_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'fixed gate catches pre-existing drift' "$rename_sha" "$drifted_sha"

git -C "$fixture_repo" rm crates/router-core/src/router.rs >/dev/null
mkdir -p "$fixture_repo/crates/router-core/src"
printf 'baseline\n' >"$fixture_repo/crates/router-core/src/lib.rs"
git -C "$fixture_repo" add crates/router-core/src/lib.rs
git -C "$fixture_repo" commit -m restore-core >/dev/null
readonly restored_sha="$(git -C "$fixture_repo" rev-parse HEAD)"
expect_status 1 'range gate catches restoration' "$drifted_sha" "$restored_sha"
expect_status 0 'net-tree contract allows changed then restored' "$baseline_sha" "$restored_sha"
expect_status 0 'zero base falls back to frozen baseline' \
  0000000000000000000000000000000000000000 "$restored_sha"

expect_status 2 'missing arguments'
expect_status 2 'option-like revision' --help "$restored_sha"
expect_status 2 'zero head fails closed' "$baseline_sha" \
  0000000000000000000000000000000000000000
expect_status 2 'unavailable commit' ffffffffffffffffffffffffffffffffffffffff "$restored_sha"

readonly blob_sha="$(printf 'blob\n' | git -C "$fixture_repo" hash-object --stdin -w)"
expect_status 2 'non-commit object' "$blob_sha" "$restored_sha"

readonly tree_sha="$(git -C "$fixture_repo" rev-parse 'HEAD^{tree}')"
readonly unrelated_sha="$(printf 'unrelated\n' | git -C "$fixture_repo" commit-tree "$tree_sha")"
expect_status 2 'head outside frozen lineage' "$baseline_sha" "$unrelated_sha"

echo 'router-core redline tests: PASS (17 scenarios)'
