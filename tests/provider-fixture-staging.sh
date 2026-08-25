#!/usr/bin/env bash
# The staging helper both release and Desktop builds use, exercised directly.
#
# Two properties, and the second is the one that was missing. A package that
# declares fixtures must carry them into the artifact — otherwise the shipped
# package claims conformance material it does not have, and every check we run
# in the workspace still passes because the workspace copy is fine. And a
# declared directory that is not there must fail the build rather than be
# skipped: it used to return quietly, which was right only while South still
# owed the fixtures.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail() { echo "FAIL: $*" >&2; exit 1; }

# Pull the helper out of the real script rather than restating it, so this
# cannot pass against a copy that has drifted from what builds actually run.
for script in scripts/build-release.sh scripts/build-desktop.sh; do
  helper="$(sed -n '/^stage_declared_fixtures()/,/^}/p' "$root/$script")"
  [[ -n "$helper" ]] || fail "$script no longer defines stage_declared_fixtures"

  workdir="$(mktemp -d)"
  trap 'rm -rf "$workdir"' EXIT

  # A package that declares fixtures and has them: the directory must arrive.
  source_dir="$workdir/pkg"
  mkdir -p "$source_dir/fixtures-declared"
  printf '{"conformance":{"fixtures":"fixtures-declared/"}}' > "$source_dir/manifest.json"
  printf '{}' > "$source_dir/fixtures-declared/case.input.json"
  dest_dir="$workdir/dest"
  mkdir -p "$dest_dir"
  (
    eval "$helper"
    stage_declared_fixtures "$source_dir" "$dest_dir"
  ) || fail "$script: staging a present fixture directory must succeed"
  [[ -f "$dest_dir/fixtures-declared/case.input.json" ]] \
    || fail "$script: the declared directory did not reach the artifact"

  # A package that declares fixtures and lacks them: the build must stop.
  missing_dir="$workdir/missing"
  mkdir -p "$missing_dir"
  printf '{"conformance":{"fixtures":"fixtures-declared/"}}' > "$missing_dir/manifest.json"
  if (
    eval "$helper"
    stage_declared_fixtures "$missing_dir" "$dest_dir"
  ) 2>"$workdir/stderr"; then
    fail "$script: a declared directory that does not exist must fail the build"
  fi
  grep -q "fixtures-declared" "$workdir/stderr" \
    || fail "$script: the failure must name the directory it expected"

  # A package that declares nothing is not a failure; most agents declare none.
  quiet_dir="$workdir/quiet"
  mkdir -p "$quiet_dir"
  printf '{}' > "$quiet_dir/manifest.json"
  (
    eval "$helper"
    stage_declared_fixtures "$quiet_dir" "$dest_dir"
  ) || fail "$script: a package with no declared fixtures must stage cleanly"

  rm -rf "$workdir"
  trap - EXIT
done

echo "provider fixture staging: present, missing and undeclared all behave"
