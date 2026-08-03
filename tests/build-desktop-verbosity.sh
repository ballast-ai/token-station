#!/usr/bin/env bash
set -euo pipefail

readonly project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_root="$(mktemp -d "${TMPDIR:-/tmp}/token-station-build-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

make_fixture() {
  fixture="$test_root/$1"
  repo="$fixture/repo"
  fake_bin="$fixture/bin"
  state="$fixture/state"

  mkdir -p "$repo/scripts" "$repo/apps/desktop" "$fake_bin" "$state"
  cp "$project_root/scripts/build-desktop.sh" "$repo/scripts/build-desktop.sh"
  chmod +x "$repo/scripts/build-desktop.sh"

  local plugin
  for plugin in \
    agent-openai \
    agent-anthropic \
    agent-openai-responses \
    agent-gemini \
    provider-openai-compatible; do
    mkdir -p "$repo/plugins/official/$plugin/target/wasm32-wasip2/release"
    printf '{}\n' >"$repo/plugins/official/$plugin/manifest.json"
    : >"$repo/plugins/official/$plugin/target/wasm32-wasip2/release/${plugin//-/_}.wasm"
  done

  cat >"$repo/scripts/audit-desktop-artifact.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
SCRIPT

  cat >"$fake_bin/rustup" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == "target list --installed" ]]; then
  echo wasm32-wasip2
  exit 0
fi
exit 1
SCRIPT

  cat >"$fake_bin/cargo" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${RUSTFLAGS:-<unset>}" >>"$TEST_STATE/cargo-rustflags"
SCRIPT

  cat >"$fake_bin/npx" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$TEST_STATE/npm-args"
SCRIPT

  cat >"$fake_bin/uname" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
echo MINGW64_NT
SCRIPT

  chmod +x "$repo/scripts/audit-desktop-artifact.sh" "$fake_bin"/*
}

run_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_STATE="$state" \
    "$repo/scripts/build-desktop.sh" "$@"
}

test_normal_build_does_not_enable_verbose_tauri_logs() {
  make_fixture normal
  run_build --local --target x86_64-pc-windows-msvc >/dev/null

  [[ "$(grep -Fxc -- '-v' "$state/npm-args" || true)" == "0" ]] \
    || fail "normal build unexpectedly enabled verbose Tauri logs"
}

test_test_version_build_enables_two_verbose_levels() {
  make_fixture test-version
  run_build --local --target x86_64-pc-windows-msvc --test-version 250.1.0 >/dev/null

  [[ "$(grep -Fxc -- '-v' "$state/npm-args" || true)" == "2" ]] \
    || fail "test-version build did not pass two verbosity flags to Tauri"
}

test_all_rust_builds_remap_private_host_paths() {
  make_fixture path-remap
  run_build --local --target x86_64-pc-windows-msvc >/dev/null

  repo_root="$(cd "$repo" && pwd)"
  checkout_remap="--remap-path-prefix=$repo_root=/build"
  cargo_home_remap="--remap-path-prefix=$fixture/cargo-home=/cargo"
  [[ "$(grep -Fc -- "$checkout_remap" "$state/cargo-rustflags" || true)" == "6" ]] \
    || fail "plugin and Desktop Rust builds did not share the checkout path remap"
  [[ "$(grep -Fc -- "$cargo_home_remap" "$state/cargo-rustflags" || true)" == "6" ]] \
    || fail "plugin and Desktop Rust builds did not share the Cargo Home path remap"
}

test_normal_build_does_not_enable_verbose_tauri_logs
test_test_version_build_enables_two_verbose_levels
test_all_rust_builds_remap_private_host_paths

echo "build-desktop verbosity tests: PASS"
