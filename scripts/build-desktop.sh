#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/build-desktop.sh <--local|--production> [--target <target-triple>]" >&2
  exit 2
}

[[ $# -ge 1 ]] || usage
mode=${1#--}
shift
case "$mode" in
  local|production) ;;
  *) usage ;;
esac

target=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || usage
      target=$2
      shift 2
      ;;
    *) usage ;;
  esac
done

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly wasm_target="wasm32-wasip2"
readonly plugins=(
  agent-openai
  agent-anthropic
  agent-openai-responses
  agent-gemini
  provider-openai-compatible
)
stage="$(mktemp -d "${TMPDIR:-/tmp}/token-station-desktop.XXXXXX")"
readonly stage
trap 'rm -rf "$stage"' EXIT

if ! rustup target list --installed | grep -qx "$wasm_target"; then
  rustup target add "$wasm_target"
fi

# Set the path remaps BEFORE building the plugins. Otherwise the plugin wasm is
# compiled with the real paths baked into its panic/location strings, and
# `include_bytes!` (the bundled-plugins layer) then embeds them into the desktop
# binary. Remap BOTH the source checkout ($root) AND the cargo registry
# (CARGO_HOME) — dependency panic locations otherwise leak the builder's home
# path (e.g. /Users/<name>/.cargo/...), which carries the username. Mirrors
# scripts/build-release.sh. (std paths are already remapped by rustc to /rustc/.)
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$root=/build"

mkdir -p "$stage/plugins-dist"
for plugin in "${plugins[@]}"; do
  source="$root/plugins/official/$plugin"
  cargo build --locked --release --manifest-path "$source/Cargo.toml" --target "$wasm_target"
  mkdir -p "$stage/plugins-dist/$plugin"
  cp "$source/manifest.json" "$stage/plugins-dist/$plugin/manifest.json"
  cp "$source/target/$wasm_target/release/${plugin//-/_}.wasm" \
    "$stage/plugins-dist/$plugin/adapter.wasm"
done

export TOKEN_STATION_PLUGINS_DIST="$stage/plugins-dist"

cargo test --locked \
  --manifest-path "$root/apps/desktop/src-tauri/Cargo.toml" \
  --features bundled-plugins \
  desktop_bundled_plugins_load_without_an_external_plugin_directory

tauri_args=(build --ci --features bundled-plugins)
bundle_root="$root/apps/desktop/src-tauri/target/release/bundle"
binary_path="$root/apps/desktop/src-tauri/target/release/token-station-desktop"
if [[ -n "$target" ]]; then
  tauri_args+=(--target "$target")
  bundle_root="$root/apps/desktop/src-tauri/target/$target/release/bundle"
  binary_path="$root/apps/desktop/src-tauri/target/$target/release/token-station-desktop"
fi

case "$(uname -s)" in
  Darwin)
    if [[ "$mode" == "local" ]]; then
      export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
    else
      : "${APPLE_SIGNING_IDENTITY:?production macOS build needs APPLE_SIGNING_IDENTITY}"
      if [[ -n "${APPLE_API_ISSUER:-}" || -n "${APPLE_API_KEY:-}" || -n "${APPLE_API_KEY_PATH:-}" ]]; then
        : "${APPLE_API_ISSUER:?App Store Connect notarization needs APPLE_API_ISSUER}"
        : "${APPLE_API_KEY:?App Store Connect notarization needs APPLE_API_KEY}"
        : "${APPLE_API_KEY_PATH:?App Store Connect notarization needs APPLE_API_KEY_PATH}"
      else
        : "${APPLE_ID:?Apple ID notarization needs APPLE_ID}"
        : "${APPLE_PASSWORD:?Apple ID notarization needs APPLE_PASSWORD}"
        : "${APPLE_TEAM_ID:?Apple ID notarization needs APPLE_TEAM_ID}"
      fi
    fi
    ;;
  MINGW*|MSYS*|CYGWIN*)
    binary_path="${binary_path}.exe"
    if [[ "$mode" == "production" ]]; then
      : "${WINDOWS_CERTIFICATE_THUMBPRINT:?production Windows build needs WINDOWS_CERTIFICATE_THUMBPRINT}"
      : "${WINDOWS_TIMESTAMP_URL:?production Windows build needs WINDOWS_TIMESTAMP_URL}"
      [[ "$WINDOWS_CERTIFICATE_THUMBPRINT" =~ ^[[:xdigit:]]{40,64}$ ]] || {
        echo "WINDOWS_CERTIFICATE_THUMBPRINT must be a hexadecimal certificate thumbprint" >&2
        exit 1
      }
      windows_config="$stage/windows-signing.json"
      printf '%s\n' \
        "{\"bundle\":{\"windows\":{\"certificateThumbprint\":\"$WINDOWS_CERTIFICATE_THUMBPRINT\",\"digestAlgorithm\":\"sha256\",\"timestampUrl\":\"$WINDOWS_TIMESTAMP_URL\",\"tsp\":true}}}" \
        >"$windows_config"
      tauri_args+=(--config "$windows_config")
    fi
    ;;
  Linux)
    # Linux packages (deb / AppImage / rpm) are not code-signed the way macOS and
    # Windows artifacts are, so there is no signing/notarization setup here. The
    # build host needs the Tauri Linux system dependencies (webkit2gtk-4.1,
    # libgtk-3, libayatana-appindicator, librsvg2, and — for AppImage — the
    # bundler tooling); see the Linux adaptation design under docs/design.
    ;;
  *)
    echo "desktop release packaging is supported on macOS, Windows, and Linux" >&2
    exit 1
    ;;
esac

(
  cd "$root/apps/desktop"
  npx tauri "${tauri_args[@]}"
)

"$root/scripts/audit-desktop-artifact.sh" \
  --mode "$mode" \
  --binary "$binary_path" \
  --bundle-root "$bundle_root" \
  --source-root "$root"

echo "desktop artifact: PASS ($mode, ${target:-native})"
