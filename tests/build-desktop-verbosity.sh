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
previous=""
for argument in "$@"; do
  if [[ "$previous" == "--config" && -f "$argument" ]]; then
    cat "$argument" >>"$TEST_STATE/tauri-configs"
  fi
  previous="$argument"
done
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

run_production_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_STATE="$state" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign public key\nRWTESTPUBLICKEY" \
    TAURI_SIGNING_PRIVATE_KEY="untrusted comment: temporary CI key\nRWTESTPRIVATEKEY" \
    WINDOWS_CERTIFICATE_THUMBPRINT="0123456789abcdef0123456789abcdef01234567" \
    WINDOWS_TIMESTAMP_URL="https://timestamp.example.test" \
    "$repo/scripts/build-desktop.sh" --production --target x86_64-pc-windows-msvc
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

test_production_build_requires_the_official_updater_public_key() {
  make_fixture missing-updater-public-key
  if env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_STATE="$state" \
    TAURI_SIGNING_PRIVATE_KEY="temporary" \
    WINDOWS_CERTIFICATE_THUMBPRINT="0123456789abcdef0123456789abcdef01234567" \
    WINDOWS_TIMESTAMP_URL="https://timestamp.example.test" \
    "$repo/scripts/build-desktop.sh" --production --target x86_64-pc-windows-msvc \
    >"$state/output" 2>&1; then
    fail "production build accepted a missing updater public key"
  fi
  grep -Fq "TOKEN_STATION_UPDATER_PUBKEY" "$state/output" \
    || fail "missing updater public key error was not explicit"
}

test_production_build_creates_updater_payloads_without_publishing_the_temporary_signature() {
  make_fixture updater-artifacts
  run_production_build >/dev/null
  grep -Fq '"createUpdaterArtifacts":true' "$state/tauri-configs" \
    || fail "production build did not enable updater artifacts"
}

test_production_build_rejects_private_material_in_the_public_key_variable() {
  make_fixture private-in-public-variable
  if env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_STATE="$state" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign encrypted secret key" \
    TAURI_SIGNING_PRIVATE_KEY="temporary" \
    WINDOWS_CERTIFICATE_THUMBPRINT="0123456789abcdef0123456789abcdef01234567" \
    WINDOWS_TIMESTAMP_URL="https://timestamp.example.test" \
    "$repo/scripts/build-desktop.sh" --production --target x86_64-pc-windows-msvc \
    >"$state/output" 2>&1; then
    fail "production build accepted private material in the public key variable"
  fi
  grep -Fq "appears to contain private key material" "$state/output" \
    || fail "private material rejection did not use the stable public error"
  if grep -Fq "minisign encrypted secret key" "$state/output"; then
    fail "private material was echoed to the build output"
  fi
}

test_normal_build_does_not_enable_verbose_tauri_logs
test_test_version_build_enables_two_verbose_levels
test_all_rust_builds_remap_private_host_paths
test_production_build_requires_the_official_updater_public_key
test_production_build_creates_updater_payloads_without_publishing_the_temporary_signature
test_production_build_rejects_private_material_in_the_public_key_variable

echo "build-desktop verbosity tests: PASS"
