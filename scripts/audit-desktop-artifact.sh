#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/audit-desktop-artifact.sh --mode <local|preview|production> --binary <path> --bundle-root <path> --source-root <path> --rust-sysroot <path> --private-cargo-home <path>" >&2
  exit 2
}

mode=""
binary=""
bundle_root=""
source_root=""
rust_sysroot=""
private_cargo_home=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode=${2:-}; shift 2 ;;
    --binary) binary=${2:-}; shift 2 ;;
    --bundle-root) bundle_root=${2:-}; shift 2 ;;
    --source-root) source_root=${2:-}; shift 2 ;;
    --rust-sysroot) rust_sysroot=${2:-}; shift 2 ;;
    --private-cargo-home) private_cargo_home=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$mode" == "local" || "$mode" == "preview" || "$mode" == "production" ]] || usage
[[ -n "$binary" && -n "$bundle_root" && -n "$source_root" && -n "$rust_sysroot" && -n "$private_cargo_home" ]] || usage
[[ -f "$binary" ]] || { echo "desktop executable missing: $binary" >&2; exit 1; }

strings_file="$(mktemp "${TMPDIR:-/tmp}/token-station-strings.XXXXXX")"
self_test_report="$(mktemp "${TMPDIR:-/tmp}/token-station-self-test.XXXXXX")"
expected_ids="$(mktemp "${TMPDIR:-/tmp}/token-station-package-ids.XXXXXX")"
readonly strings_file
readonly self_test_report
readonly expected_ids
trap 'rm -f "$strings_file" "$self_test_report" "$expected_ids"' EXIT
"$source_root/scripts/official-packages.py" --field id | LC_ALL=C sort >"$expected_ids"

self_test_output="$self_test_report"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) self_test_output="$(cygpath -w "$self_test_report")" ;;
esac
"$binary" --self-test-bundled-plugins "$self_test_output" || {
  echo "desktop executable builtin-plugin self-test failed" >&2
  [[ -s "$self_test_report" ]] && sed -n '1,120p' "$self_test_report" >&2
  exit 1
}
node - "$self_test_report" "$expected_ids" <<'NODE'
const fs = require("node:fs");
const report = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const expected = fs.readFileSync(process.argv[3], "utf8").trim().split(/\r?\n/);
const actual = Array.isArray(report.plugins)
  ? report.plugins.map((plugin) => plugin.id).sort()
  : [];
if (
  report.passed !== true ||
  report.bundle?.id !== "com.tokenstation.desktop" ||
  report.storage?.data_directory_private !== true ||
  report.storage?.private_file_verified !== true ||
  report.storage?.credential_read !== false ||
  report.gateway?.loadable !== true ||
  JSON.stringify(actual) !== JSON.stringify(expected) ||
  report.plugins.some(
    (plugin) => plugin.source !== "builtin" || plugin.loadable !== true,
  )
) {
  throw new Error(`installed desktop self-test report is incomplete: ${JSON.stringify(report)}`);
}
NODE

strings -a "$binary" >"$strings_file"

if grep -Fq "$source_root" "$strings_file"; then
  echo "desktop executable leaks the source checkout path: $source_root" >&2
  exit 1
fi

# Dependency panic/location strings otherwise embed the builder's cargo home
# (e.g. /Users/<name>/.cargo/...), which carries the username — a personal-info
# leak. Windows release builds use an isolated CARGO_HOME, but this audit still
# checks the original private directory so stale target artifacts cannot hide
# the leak (see scripts/build-desktop.sh).
if grep -Fq "$private_cargo_home" "$strings_file"; then
  echo "desktop executable leaks the cargo home path: $private_cargo_home" >&2
  exit 1
fi

if grep -Fq "$rust_sysroot" "$strings_file"; then
  echo "desktop executable leaks the Rust sysroot path" >&2
  exit 1
fi

builder_home="${HOME:-}"
if [[ -n "$builder_home" ]] && grep -Fq "$builder_home/" "$strings_file"; then
  echo "desktop executable leaks the builder home path" >&2
  exit 1
fi

while IFS= read -r plugin; do
  grep -Fq "$plugin" "$strings_file" || {
    echo "desktop executable is missing builtin plugin marker: $plugin" >&2
    exit 1
  }
done < <("$source_root/scripts/official-packages.py" --field id)

case "$(uname -s)" in
  Darwin)
    app="$(find "$bundle_root/macos" -maxdepth 1 -type d -name '*.app' -print -quit)"
    [[ -n "$app" ]] || { echo "macOS app bundle missing under $bundle_root/macos" >&2; exit 1; }
    codesign --verify --deep --strict --verbose=2 "$app"
    entitlements="$(codesign --display --entitlements :- "$app" 2>&1)"
    grep -Fq "com.apple.security.cs.allow-unsigned-executable-memory" <<<"$entitlements" || {
      echo "macOS app is missing the Wasmtime executable-memory entitlement" >&2
      exit 1
    }
    if [[ "$mode" == "preview" ]]; then
      signature="$(codesign --display --verbose=4 "$app" 2>&1)"
      grep -Fq "Signature=adhoc" <<<"$signature" || {
        echo "macOS preview app is not ad-hoc signed" >&2
        exit 1
      }
    elif [[ "$mode" == "production" ]]; then
      signature="$(codesign --display --verbose=4 "$app" 2>&1)"
      grep -Fq "Authority=Developer ID Application" <<<"$signature" || {
        echo "macOS production app is not signed with Developer ID Application" >&2
        exit 1
      }
      grep -Eq '^TeamIdentifier=.+$' <<<"$signature" || {
        echo "macOS production app has no TeamIdentifier" >&2
        exit 1
      }
      spctl --assess --type execute --verbose=4 "$app"
      xcrun stapler validate "$app"
    fi
    ;;
  MINGW*|MSYS*|CYGWIN*)
    installer="$(find "$bundle_root/msi" -maxdepth 1 -type f -name '*.msi' -print -quit 2>/dev/null || true)"
    [[ -n "$installer" ]] || { echo "Windows MSI missing under $bundle_root/msi" >&2; exit 1; }
    nsis="$(find "$bundle_root/nsis" -maxdepth 1 -type f -name '*.exe' -print -quit 2>/dev/null || true)"
    [[ -z "$nsis" ]] || { echo "Windows formal build unexpectedly produced NSIS: $nsis" >&2; exit 1; }
    [[ "$mode" == "local" ]] && {
      echo "desktop artifact audit: PASS"
      exit 0
    }
    windows_binary="$(cygpath -w "$binary")"
    status="$(powershell.exe -NoProfile -NonInteractive -Command \
      "(Get-AuthenticodeSignature -LiteralPath '$windows_binary').Status")"
    [[ "$status" == *Valid* ]] || {
      echo "Windows production executable signature is not valid: $status" >&2
      exit 1
    }
    windows_installer="$(cygpath -w "$installer")"
    status="$(powershell.exe -NoProfile -NonInteractive -Command \
      "(Get-AuthenticodeSignature -LiteralPath '$windows_installer').Status")"
    [[ "$status" == *Valid* ]] || {
      echo "Windows production installer signature is not valid: $status" >&2
      exit 1
    }
    ;;
  Linux)
    # Linux packages (deb / AppImage / rpm) are not code-signed, so the audit
    # verifies the binary is a real Linux executable and at least one package was
    # produced — not a signature.
    [[ -x "$binary" ]] || { echo "Linux binary missing or not executable: $binary" >&2; exit 1; }
    file "$binary" | grep -Fq "ELF" || {
      echo "Linux binary is not an ELF executable: $binary" >&2
      exit 1
    }
    deb="$(find "$bundle_root/deb" -maxdepth 1 -type f -name '*.deb' -print -quit 2>/dev/null || true)"
    appimage="$(find "$bundle_root/appimage" -maxdepth 1 -type f -name '*.AppImage' -print -quit 2>/dev/null || true)"
    rpm="$(find "$bundle_root/rpm" -maxdepth 1 -type f -name '*.rpm' -print -quit 2>/dev/null || true)"
    [[ -n "$deb" || -n "$appimage" || -n "$rpm" ]] || {
      echo "no Linux package (.deb / .AppImage / .rpm) found under $bundle_root" >&2
      exit 1
    }
    ;;
  *)
    echo "desktop artifact audit is supported on macOS, Windows, and Linux" >&2
    exit 1
    ;;
esac

echo "desktop artifact audit: PASS"
