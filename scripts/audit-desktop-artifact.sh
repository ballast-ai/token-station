#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/audit-desktop-artifact.sh --mode <local|production> --binary <path> --bundle-root <path> --source-root <path>" >&2
  exit 2
}

mode=""
binary=""
bundle_root=""
source_root=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode=${2:-}; shift 2 ;;
    --binary) binary=${2:-}; shift 2 ;;
    --bundle-root) bundle_root=${2:-}; shift 2 ;;
    --source-root) source_root=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$mode" == "local" || "$mode" == "production" ]] || usage
[[ -n "$binary" && -n "$bundle_root" && -n "$source_root" ]] || usage
[[ -f "$binary" ]] || { echo "desktop executable missing: $binary" >&2; exit 1; }

strings_file="$(mktemp "${TMPDIR:-/tmp}/token-station-strings.XXXXXX")"
readonly strings_file
trap 'rm -f "$strings_file"' EXIT
strings -a "$binary" >"$strings_file"

if grep -Fq "$source_root" "$strings_file"; then
  echo "desktop executable leaks the source checkout path: $source_root" >&2
  exit 1
fi

# Dependency panic/location strings otherwise embed the builder's cargo home
# (e.g. /Users/<name>/.cargo/...), which carries the username — a personal-info
# leak. The build must remap CARGO_HOME (see scripts/build-desktop.sh).
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
if [[ -n "$cargo_home" ]] && grep -Fq "$cargo_home" "$strings_file"; then
  echo "desktop executable leaks the cargo home path: $cargo_home" >&2
  exit 1
fi

for plugin in \
  agent-openai \
  agent-anthropic \
  agent-openai-responses \
  agent-gemini \
  provider-openai-compatible; do
  grep -Fq "$plugin" "$strings_file" || {
    echo "desktop executable is missing builtin plugin marker: $plugin" >&2
    exit 1
  }
done

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
    if [[ "$mode" == "production" ]]; then
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
    [[ "$mode" == "local" ]] && exit 0
    windows_binary="$(cygpath -w "$binary")"
    status="$(powershell.exe -NoProfile -NonInteractive -Command \
      "(Get-AuthenticodeSignature -LiteralPath '$windows_binary').Status")"
    [[ "$status" == *Valid* ]] || {
      echo "Windows production executable signature is not valid: $status" >&2
      exit 1
    }
    installer="$(find "$bundle_root" -type f \( -name '*.exe' -o -name '*.msi' \) -print -quit)"
    [[ -n "$installer" ]] || { echo "Windows installer missing under $bundle_root" >&2; exit 1; }
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
