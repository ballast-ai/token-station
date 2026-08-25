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
  local os_name="${2:-MINGW64_NT}"
  fixture="$test_root/$1"
  repo="$fixture/repo"
  fake_bin="$fixture/bin"
  state="$fixture/state"

  mkdir -p "$repo/scripts" "$repo/apps/desktop/src-tauri" "$repo/plugins/official" "$fake_bin" "$state" "$fixture/rust-sysroot"
  cp "$project_root/scripts/build-desktop.sh" "$repo/scripts/build-desktop.sh"
  cp "$project_root/scripts/official-packages.py" "$repo/scripts/official-packages.py"
  cp "$project_root/plugins/official/packages.json" "$repo/plugins/official/packages.json"
  chmod +x "$repo/scripts/build-desktop.sh" "$repo/scripts/official-packages.py"
  printf '{\n  "version": "1.1.3"\n}\n' >"$repo/apps/desktop/src-tauri/tauri.conf.json"

  local plugin
  while IFS= read -r plugin; do
    mkdir -p "$repo/plugins/official/$plugin/target/wasm32-wasip2/release"
    printf '{}\n' >"$repo/plugins/official/$plugin/manifest.json"
    : >"$repo/plugins/official/$plugin/target/wasm32-wasip2/release/${plugin//-/_}.wasm"
  done < <("$repo/scripts/official-packages.py" --field dir)

  cat >"$repo/scripts/audit-desktop-artifact.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
SCRIPT

  cat >"$repo/scripts/package-macos-dmg.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$TEST_STATE/dmg-package-args"
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

  cat >"$fake_bin/rustc" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == "--print sysroot" ]]; then
  echo "$TEST_RUST_SYSROOT"
  exit 0
fi
exit 1
SCRIPT

  cat >"$fake_bin/npx" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$TEST_STATE/npm-args"
printf '%s' "${TAURI_SIGNING_PRIVATE_KEY:-<unset>}" >"$TEST_STATE/tauri-private-key"
printf '%s\t%s\n' "$*" "${TAURI_SIGNING_PRIVATE_KEY:-<unset>}" >>"$TEST_STATE/tauri-calls"
previous=""
for argument in "$@"; do
  if [[ "$previous" == "--config" && -f "$argument" ]]; then
    cat "$argument" >>"$TEST_STATE/tauri-configs"
  fi
  previous="$argument"
done
SCRIPT

  cat >"$fake_bin/uname" <<SCRIPT
#!/usr/bin/env bash
set -euo pipefail
echo "$os_name"
SCRIPT

  chmod +x "$repo/scripts/audit-desktop-artifact.sh" "$repo/scripts/package-macos-dmg.sh" "$fake_bin"/*
}

run_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    "$repo/scripts/build-desktop.sh" "$@"
}

run_windows_production_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    WINDOWS_CERTIFICATE_THUMBPRINT="0123456789abcdef0123456789abcdef01234567" \
    WINDOWS_TIMESTAMP_URL="https://timestamp.example.test" \
    "$repo/scripts/build-desktop.sh" --production --target x86_64-pc-windows-msvc
}

run_macos_production_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    APPLE_SIGNING_IDENTITY="Developer ID Application: Example Corp (TEAMID)" \
    APPLE_API_ISSUER="issuer" \
    APPLE_API_KEY="key" \
    APPLE_API_KEY_PATH="$fixture/AuthKey_key.p8" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign public key\nRWTESTPUBLICKEY" \
    TAURI_SIGNING_PRIVATE_KEY="untrusted comment: temporary CI key\nRWTESTPRIVATEKEY" \
    "$repo/scripts/build-desktop.sh" --production --target aarch64-apple-darwin
}

run_macos_preview_build() {
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    TOKEN_STATION_UPDATER_ENDPOINT="https://github.com/ballast-ai/token-station/releases/download/updater-preview/latest.json" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign public key\nRWTESTPUBLICKEY" \
    TAURI_SIGNING_PRIVATE_KEY="untrusted comment: preview signing key\nRWTESTPRIVATEKEY" \
    "$repo/scripts/build-desktop.sh" --preview --target aarch64-apple-darwin
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
  rust_sysroot_remap="--remap-path-prefix=$fixture/rust-sysroot=/rustc"
  [[ "$(grep -Fc -- "$checkout_remap" "$state/cargo-rustflags" || true)" == "7" ]] \
    || fail "plugin and Desktop Rust builds did not share the checkout path remap"
  [[ "$(grep -Fc -- "$cargo_home_remap" "$state/cargo-rustflags" || true)" == "7" ]] \
    || fail "plugin and Desktop Rust builds did not share the Cargo Home path remap"
  [[ "$(grep -Fc -- "$rust_sysroot_remap" "$state/cargo-rustflags" || true)" == "7" ]] \
    || fail "plugin and Desktop Rust builds did not share the Rust sysroot path remap"
}

test_production_build_requires_the_official_updater_public_key() {
  make_fixture missing-updater-public-key Darwin
  if env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    APPLE_SIGNING_IDENTITY="Developer ID Application: Example Corp (TEAMID)" \
    APPLE_API_ISSUER="issuer" \
    APPLE_API_KEY="key" \
    APPLE_API_KEY_PATH="$fixture/AuthKey_key.p8" \
    TAURI_SIGNING_PRIVATE_KEY="temporary" \
    "$repo/scripts/build-desktop.sh" --production --target aarch64-apple-darwin \
    >"$state/output" 2>&1; then
    fail "macOS production build accepted a missing updater public key"
  fi
  grep -Fq "TOKEN_STATION_UPDATER_PUBKEY" "$state/output" \
    || fail "missing updater public key error was not explicit"
}

test_production_build_creates_updater_payloads_without_publishing_the_temporary_signature() {
  make_fixture updater-artifacts Darwin
  run_macos_production_build >/dev/null
  grep -Fq '"createUpdaterArtifacts":true' "$state/tauri-configs" \
    || fail "macOS production build did not enable updater artifacts"
  grep -Fxq -- '--architecture' "$state/dmg-package-args" \
    || fail "macOS production build did not invoke the compliant DMG packager"
  grep -Fxq -- 'aarch64' "$state/dmg-package-args" \
    || fail "macOS production build passed the wrong DMG architecture"
}

test_preview_build_creates_signed_updater_payload_and_unsigned_test_dmg() {
  make_fixture preview-updater-artifacts Darwin
  run_macos_preview_build >/dev/null
  grep -Fq '"createUpdaterArtifacts":true' "$state/tauri-configs" \
    || fail "macOS preview build did not enable updater artifacts"
  grep -Fq '"pubkey":"untrusted comment: minisign public key\nRWTESTPUBLICKEY"' "$state/tauri-configs" \
    || fail "macOS preview build did not provide the updater public key to the Tauri bundler"
  grep -Fxq -- '--unsigned-test' "$state/dmg-package-args" \
    || fail "macOS preview build did not use the unsigned test DMG policy"
  grep -Fxq -- '--architecture' "$state/dmg-package-args" \
    || fail "macOS preview build did not pass a DMG architecture"
  grep -Fxq -- 'aarch64' "$state/dmg-package-args" \
    || fail "macOS preview build passed the wrong DMG architecture"
  [[ "$(sed -n '1p' "$state/tauri-calls")" == *"build"*"<unset>" ]] \
    || fail "preview compilation exposed the updater private key"
  [[ "$(sed -n '2p' "$state/tauri-calls")" == *"bundle"*"RWTESTPRIVATEKEY" ]] \
    || fail "preview bundle phase did not receive the updater private key"
}

test_preview_build_loads_the_private_key_path_for_the_tauri_bundler() {
  make_fixture preview-updater-key-path Darwin
  printf '%s\n' 'encrypted-preview-key' >"$fixture/updater.key"
  env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    TOKEN_STATION_UPDATER_ENDPOINT="https://github.com/ballast-ai/token-station/releases/download/updater-preview/latest.json" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign public key\nRWTESTPUBLICKEY" \
    TAURI_SIGNING_PRIVATE_KEY_PATH="$fixture/updater.key" \
    "$repo/scripts/build-desktop.sh" --preview --target aarch64-apple-darwin >/dev/null
  [[ "$(cat "$state/tauri-private-key")" == "encrypted-preview-key" ]] \
    || fail "preview build did not load the updater private key path for the Tauri bundler"
}

test_windows_production_build_does_not_require_updater_artifacts_for_the_first_release() {
  make_fixture windows-no-updater
  run_windows_production_build >/dev/null
  if [[ -f "$state/tauri-configs" ]] && grep -Fq '"createUpdaterArtifacts":true' "$state/tauri-configs"; then
    fail "Windows production build unexpectedly enabled updater artifacts"
  fi
}

test_production_build_rejects_private_material_in_the_public_key_variable() {
  make_fixture private-in-public-variable Darwin
  if env \
    PATH="$fake_bin:/usr/bin:/bin" \
    CARGO_HOME="$fixture/cargo-home" \
    RUSTFLAGS= \
    TEST_RUST_SYSROOT="$fixture/rust-sysroot" \
    TEST_STATE="$state" \
    APPLE_SIGNING_IDENTITY="Developer ID Application: Example Corp (TEAMID)" \
    APPLE_API_ISSUER="issuer" \
    APPLE_API_KEY="key" \
    APPLE_API_KEY_PATH="$fixture/AuthKey_key.p8" \
    TOKEN_STATION_UPDATER_PUBKEY="untrusted comment: minisign encrypted secret key" \
    TAURI_SIGNING_PRIVATE_KEY="temporary" \
    "$repo/scripts/build-desktop.sh" --production --target aarch64-apple-darwin \
    >"$state/output" 2>&1; then
    fail "macOS production build accepted private material in the public key variable"
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
test_preview_build_creates_signed_updater_payload_and_unsigned_test_dmg
test_preview_build_loads_the_private_key_path_for_the_tauri_bundler
test_windows_production_build_does_not_require_updater_artifacts_for_the_first_release
test_production_build_rejects_private_material_in_the_public_key_variable

echo "build-desktop verbosity tests: PASS"
