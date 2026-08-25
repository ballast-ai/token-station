#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/build-desktop.sh <--local|--preview|--production> [--target <target-triple>] [--test-version <version>]" >&2
  exit 2
}

[[ $# -ge 1 ]] || usage
mode=${1#--}
shift
case "$mode" in
  local|preview|production) ;;
  *) usage ;;
esac

target=""
test_version=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || usage
      target=$2
      shift 2
      ;;
    --test-version)
      [[ $# -ge 2 ]] || usage
      test_version=$2
      shift 2
      ;;
    *) usage ;;
  esac
done

if [[ -n "$test_version" ]]; then
  [[ "$mode" == "local" ]] || {
    echo "--test-version is restricted to local installer behavior tests" >&2
    exit 2
  }
  [[ "$test_version" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,5}(\.[0-9]{1,5})?$ ]] || {
    echo "--test-version must be a valid numeric MSI version" >&2
    exit 2
  }
fi

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly wasm_target="wasm32-wasip2"
agent_packages="$("$root/scripts/official-packages.py" --kind agent --field dir)"
readonly agent_packages
plugins=()
while IFS= read -r package; do
  plugins+=("$package")
done <<<"$agent_packages"
readonly -a plugins
south_package_dirs="$("$root/scripts/official-packages.py" --kind south-component --field dir)"
readonly south_package_dirs
south_components=()
while IFS= read -r package; do
  south_components+=("$package")
done <<<"$south_package_dirs"
readonly -a south_components


# Stages the fixture directory a package's own manifest declares, rather than
# assuming it is called `fixtures`. The Anthropic component declares
# `fixtures-anthropic/`, so a hardcoded name ships nothing for it and says
# nothing about having skipped it.
stage_declared_fixtures() {
  local source="$1" dest="$2"
  local declared
  # python3, not node: these scripts are otherwise buildable with nothing
  # beyond the Rust toolchain and `npx`, and `tests/build-desktop-verbosity.sh`
  # runs them under `PATH=$fake_bin:/usr/bin:/bin` to prove that. Reaching for
  # node here made that test fail with `node: command not found` and took CI
  # red for three commits. python3 lives in /usr/bin on both runners.
  declared="$(python3 -c '
import json, sys
manifest = json.load(open(sys.argv[1]))
sys.stdout.write((manifest.get("conformance") or {}).get("fixtures") or "")
  ' "$source/manifest.json")"
  [[ -n "$declared" ]] || return 0
  declared="${declared%/}"
  # A declared directory that is not there is a broken package, not a package
  # without fixtures. This used to return quietly, which was right while South
  # still owed the fixtures and the manifest's promise could not be kept. They
  # are vendored now, so a missing directory means the package would ship
  # claiming conformance material it does not carry — and the build that
  # produced it would have said nothing.
  if [[ ! -d "$source/$declared" ]]; then
    echo "$source/manifest.json declares conformance fixtures at '$declared', which does not exist" >&2
    return 1
  fi
  cp -R "$source/$declared" "$dest/$declared"
}

host_os="$(uname -s)"
readonly host_os

is_windows_target=false
if [[ "$target" == *-pc-windows-* ]]; then
  is_windows_target=true
elif [[ -z "$target" ]]; then
  case "$host_os" in
    MINGW*|MSYS*|CYGWIN*) is_windows_target=true ;;
  esac
fi

if [[ "$mode" == "preview" ]]; then
  [[ "$host_os" == "Darwin" ]] || {
    echo "preview desktop updates are supported only on macOS" >&2
    exit 1
  }
  [[ "$target" == "aarch64-apple-darwin" ]] || {
    echo "the preview update channel requires target aarch64-apple-darwin" >&2
    exit 1
  }
  : "${TOKEN_STATION_UPDATER_ENDPOINT:?preview desktop build needs TOKEN_STATION_UPDATER_ENDPOINT}"
  [[ "$TOKEN_STATION_UPDATER_ENDPOINT" == https://* ]] || {
    echo "TOKEN_STATION_UPDATER_ENDPOINT must use HTTPS" >&2
    exit 1
  }
fi

enable_updater_artifacts=false
signing_private_key="${TAURI_SIGNING_PRIVATE_KEY:-}"
signing_private_key_path="${TAURI_SIGNING_PRIVATE_KEY_PATH:-}"
signing_private_key_password="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH TAURI_SIGNING_PRIVATE_KEY_PASSWORD
if [[ "$mode" != "local" && "$is_windows_target" != "true" ]]; then
  enable_updater_artifacts=true
  : "${TOKEN_STATION_UPDATER_PUBKEY:?signed updater build needs TOKEN_STATION_UPDATER_PUBKEY}"
  updater_pubkey_upper="$(LC_ALL=C tr '[:lower:]' '[:upper:]' <<<"$TOKEN_STATION_UPDATER_PUBKEY")"
  if [[ "$updater_pubkey_upper" == *"SECRET KEY"* || "$updater_pubkey_upper" == *"PRIVATE KEY"* ]]; then
    echo "TOKEN_STATION_UPDATER_PUBKEY appears to contain private key material" >&2
    exit 1
  fi
  if [[ -z "$signing_private_key" && -z "$signing_private_key_path" ]]; then
    echo "signed updater build needs TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH" >&2
    exit 1
  fi
  if [[ -z "$signing_private_key" ]]; then
    [[ -f "$signing_private_key_path" && -r "$signing_private_key_path" ]] || {
      echo "TAURI_SIGNING_PRIVATE_KEY_PATH must name a readable private key file" >&2
      exit 1
    }
  fi
fi

stage="$(mktemp -d "${TMPDIR:-/tmp}/token-station-desktop.XXXXXX")"
readonly stage
trap 'rm -rf "$stage"' EXIT

if ! rustup target list --installed | grep -qx "$wasm_target"; then
  rustup target add "$wasm_target"
fi

rust_sysroot="$(rustc --print sysroot)"
readonly rust_sysroot
[[ -n "$rust_sysroot" && -d "$rust_sysroot" ]] || {
  echo "Rust sysroot could not be resolved; the build stopped before creating an artifact." >&2
  exit 1
}

# Set the path remaps BEFORE building the plugins. Otherwise the plugin wasm is
# compiled with the real paths baked into its panic/location strings, and
# `include_bytes!` (the bundled-plugins layer) then embeds them into the desktop
# binary. Remap the source checkout, Cargo Home, and Rust sysroot so dependency
# and standard-library panic locations cannot expose a builder username.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$root=/build --remap-path-prefix=$rust_sysroot=/rustc"

mkdir -p "$stage/plugins-dist"
for plugin in "${plugins[@]}"; do
  source="$root/plugins/official/$plugin"
  cargo build --locked --release --manifest-path "$source/Cargo.toml" --target "$wasm_target"
  mkdir -p "$stage/plugins-dist/$plugin"
  cp "$source/manifest.json" "$stage/plugins-dist/$plugin/manifest.json"
  cp "$source/target/$wasm_target/release/${plugin//-/_}.wasm" \
    "$stage/plugins-dist/$plugin/adapter.wasm"
  stage_declared_fixtures "$source" "$stage/plugins-dist/$plugin"
done

for component in "${south_components[@]}"; do
  source="$root/plugins/official/$component"
  cargo build --locked --release --manifest-path "$source/Cargo.toml" --target "$wasm_target"
  mkdir -p "$stage/plugins-dist/$component"
  cp "$source/manifest.json" "$stage/plugins-dist/$component/manifest.json"
  cp "$source/target/$wasm_target/release/${component//-/_}.wasm" \
    "$stage/plugins-dist/$component/component.wasm"
  stage_declared_fixtures "$source" "$stage/plugins-dist/$component"
done

export TOKEN_STATION_PLUGINS_DIST="$stage/plugins-dist"

cargo test --locked \
  --manifest-path "$root/apps/desktop/src-tauri/Cargo.toml" \
  --target-dir "$stage/desktop-gate" \
  --features bundled-plugins \
  desktop_bundled_plugins_load_without_an_external_plugin_directory

tauri_args=(build --ci --features bundled-plugins)
macos_bundle_kind=""
if [[ "$enable_updater_artifacts" == "true" ]]; then
  updater_artifact_config="$stage/updater-artifacts.json"
  printf '%s\n' \
    "{\"plugins\":{\"updater\":{\"pubkey\":\"$TOKEN_STATION_UPDATER_PUBKEY\"}},\"bundle\":{\"createUpdaterArtifacts\":true}}" \
    >"$updater_artifact_config"
  if [[ "$host_os" != "Darwin" ]]; then
    tauri_args+=(--config "$updater_artifact_config")
  fi
fi
if [[ -n "$test_version" ]]; then
  test_version_config="$stage/test-version.json"
  printf '%s\n' "{\"version\":\"$test_version\"}" >"$test_version_config"
  tauri_args+=(--config "$test_version_config")
  # Installer lifecycle builds need the underlying candle/light diagnostics.
  tauri_args+=(-v -v)
fi
bundle_root="$root/apps/desktop/src-tauri/target/release/bundle"
binary_path="$root/apps/desktop/src-tauri/target/release/token-station-desktop"
if [[ -n "$target" ]]; then
  tauri_args+=(--target "$target")
  bundle_root="$root/apps/desktop/src-tauri/target/$target/release/bundle"
  binary_path="$root/apps/desktop/src-tauri/target/$target/release/token-station-desktop"
fi

case "$host_os" in
  Darwin)
    case "$mode" in
      local)
        export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
        # Local installation consumes the signed .app directly. Building a DMG
        # adds Finder/volume state to the developer loop and can fail even after
        # the application itself is valid, so reserve installers for releases.
        macos_bundle_kind="app"
        ;;
      preview)
        export APPLE_SIGNING_IDENTITY="-"
        macos_bundle_kind="app"
        ;;
      production)
        : "${APPLE_SIGNING_IDENTITY:?production macOS build needs APPLE_SIGNING_IDENTITY}"
        # The formal DMG is assembled after the signed and notarized App passes
        # the desktop artifact audit. Updater payloads are still emitted because
        # createUpdaterArtifacts is enabled above.
        macos_bundle_kind="app"
        : "${APPLE_API_ISSUER:?App Store Connect notarization needs APPLE_API_ISSUER}"
        : "${APPLE_API_KEY:?App Store Connect notarization needs APPLE_API_KEY}"
        : "${APPLE_API_KEY_PATH:?App Store Connect notarization needs APPLE_API_KEY_PATH}"
        ;;
    esac
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
    # bundler tooling).
    ;;
  *)
    echo "desktop release packaging is supported on macOS, Windows, and Linux" >&2
    exit 1
    ;;
esac

if [[ "$host_os" == "Darwin" && "$enable_updater_artifacts" == "true" ]]; then
  # Compile all project and dependency code before exposing updater signing
  # material. The bundle-only phase packages the already-built binary and is
  # the smallest process boundary that needs the private key.
  (
    cd "$root/apps/desktop"
    npx tauri "${tauri_args[@]}" --no-bundle
  )
  if [[ -z "$signing_private_key" ]]; then
    signing_private_key=$(<"$signing_private_key_path")
  fi
  export TAURI_SIGNING_PRIVATE_KEY="$signing_private_key"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$signing_private_key_password"
  bundle_args=(bundle --ci --features bundled-plugins --config "$updater_artifact_config")
  if [[ -n "$target" ]]; then
    bundle_args+=(--target "$target")
  fi
  if [[ -n "$macos_bundle_kind" ]]; then
    bundle_args+=(--bundles "$macos_bundle_kind")
  fi
  (
    cd "$root/apps/desktop"
    npx tauri "${bundle_args[@]}"
  )
  unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  signing_private_key=""
  signing_private_key_password=""
else
  if [[ -n "$macos_bundle_kind" ]]; then
    tauri_args+=(--bundles "$macos_bundle_kind")
  fi
  (
    cd "$root/apps/desktop"
    npx tauri "${tauri_args[@]}"
  )
fi

"$root/scripts/audit-desktop-artifact.sh" \
  --mode "$mode" \
  --binary "$binary_path" \
  --bundle-root "$bundle_root" \
  --source-root "$root" \
  --rust-sysroot "$rust_sysroot"

if [[ "$host_os" == "Darwin" && "$mode" != "local" ]]; then
  app_path="$bundle_root/macos/token-station.app"
  desktop_version=$(sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' \
    "$root/apps/desktop/src-tauri/tauri.conf.json" | head -n 1)
  [[ -n "$desktop_version" ]] || {
    echo "desktop version is missing from tauri.conf.json" >&2
    exit 1
  }
  case "$target" in
    aarch64-apple-darwin) release_architecture="aarch64" ;;
    x86_64-apple-darwin) release_architecture="x86_64" ;;
    *)
      echo "production macOS build needs an explicit aarch64 or x86_64 target" >&2
      exit 1
      ;;
  esac
  if [[ "$mode" == "preview" ]]; then
    dmg_path="$bundle_root/dmg/token-station_${desktop_version}_${release_architecture}_UNSIGNED-UNNOTARIZED.dmg"
    "$root/scripts/package-macos-dmg.sh" \
      --app "$app_path" \
      --output "$dmg_path" \
      --volume-name "Token Station ${desktop_version}" \
      --unsigned-test \
      --app-source-commit "$(git -C "$root" rev-parse HEAD 2>/dev/null || true)" \
      --version "$desktop_version" \
      --architecture "$release_architecture"
  else
    dmg_path="$bundle_root/dmg/token-station_${desktop_version}_${release_architecture}.dmg"
    "$root/scripts/package-macos-dmg.sh" \
      --app "$app_path" \
      --output "$dmg_path" \
      --volume-name "Token Station ${desktop_version}" \
      --signing-identity "$APPLE_SIGNING_IDENTITY" \
      --version "$desktop_version" \
      --architecture "$release_architecture"
  fi
fi

echo "desktop artifact: PASS ($mode, ${target:-native})"
