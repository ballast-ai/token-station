#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly bundle_id="com.tokenstation.desktop"
readonly installed_app="/Applications/token-station.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "local desktop installation is supported only on macOS" >&2
  exit 1
fi

case "$(uname -m)" in
  arm64) readonly target="aarch64-apple-darwin" ;;
  x86_64) readonly target="x86_64-apple-darwin" ;;
  *)
    echo "unsupported macOS architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

readonly built_app="$root/apps/desktop/src-tauri/target/$target/release/bundle/macos/token-station.app"

"$root/scripts/build-desktop.sh" --local --target "$target"

if [[ ! -d "$built_app" ]]; then
  echo "built app not found: $built_app" >&2
  exit 1
fi

built_bundle_id=$(
  /usr/libexec/PlistBuddy -c "Print:CFBundleIdentifier" \
    "$built_app/Contents/Info.plist"
)
if [[ "$built_bundle_id" != "$bundle_id" ]]; then
  echo "refusing to install unexpected bundle id: $built_bundle_id" >&2
  exit 1
fi

codesign --verify --deep --strict "$built_app"

if [[ -e "$installed_app" ]]; then
  if [[ ! -d "$installed_app" ]]; then
    echo "refusing to replace non-directory path: $installed_app" >&2
    exit 1
  fi
  installed_bundle_id=$(
    /usr/libexec/PlistBuddy -c "Print:CFBundleIdentifier" \
      "$installed_app/Contents/Info.plist"
  )
  if [[ "$installed_bundle_id" != "$bundle_id" ]]; then
    echo "refusing to remove unexpected bundle id: $installed_bundle_id" >&2
    exit 1
  fi

  osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5; do
    pgrep -f "$installed_app/Contents/MacOS/" >/dev/null 2>&1 || break
    sleep 1
  done
  if pgrep -f "$installed_app/Contents/MacOS/" >/dev/null 2>&1; then
    echo "installed app is still running; refusing to delete it" >&2
    exit 1
  fi

  rm -rf -- "$installed_app"
fi

ditto "$built_app" "$installed_app"

installed_bundle_id=$(
  /usr/libexec/PlistBuddy -c "Print:CFBundleIdentifier" \
    "$installed_app/Contents/Info.plist"
)
if [[ "$installed_bundle_id" != "$bundle_id" ]]; then
  echo "installed app bundle id verification failed: $installed_bundle_id" >&2
  exit 1
fi

codesign --verify --deep --strict "$installed_app"
open "$installed_app"

echo "desktop app installed and launched: $installed_app"
