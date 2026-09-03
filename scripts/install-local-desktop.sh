#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly bundle_id="com.tokenstation.desktop"
readonly installed_app="/Applications/token-station.app"
readonly installed_parent="$(dirname "$installed_app")"
readonly install_lock="$installed_parent/.token-station.install.lock"
readonly launch_check_interval="${TOKEN_STATION_LAUNCH_CHECK_INTERVAL_SECONDS:-1}"
readonly launch_check_samples="${TOKEN_STATION_LAUNCH_CHECK_SAMPLES:-3}"
readonly launch_open_attempts="${TOKEN_STATION_LAUNCH_OPEN_ATTEMPTS:-5}"
readonly desktop_config="${TOKEN_STATION_DESKTOP_CONFIG:-$HOME/Library/Application Support/$bundle_id/token-station.json}"

staging_app=""
backup_app=""
replacement_active=0
had_previous_app=0

verify_app() {
  local app="$1"
  local label="$2"
  local actual_bundle_id

  if [[ ! -d "$app" ]]; then
    echo "$label not found: $app" >&2
    return 1
  fi
  actual_bundle_id=$(
    /usr/libexec/PlistBuddy -c "Print:CFBundleIdentifier" \
      "$app/Contents/Info.plist"
  )
  if [[ "$actual_bundle_id" != "$bundle_id" ]]; then
    echo "$label has unexpected bundle id: $actual_bundle_id" >&2
    return 1
  fi
  if ! codesign --verify --deep --strict "$app"; then
    echo "$label code signature verification failed" >&2
    return 1
  fi
}

app_is_running() {
  local app="$1"
  local executable

  [[ -d "$app" ]] || return 1
  executable=$(
    /usr/libexec/PlistBuddy -c "Print:CFBundleExecutable" \
      "$app/Contents/Info.plist"
  ) || return 1
  [[ -n "$executable" && "$executable" != */* ]] || return 1
  pgrep -f -x "$app/Contents/MacOS/$executable" >/dev/null 2>&1
}

wait_for_app_exit() {
  local app="$1"
  for _ in 1 2 3 4 5; do
    app_is_running "$app" || return 0
    sleep 1
  done
  ! app_is_running "$app"
}

rollback_installation() {
  local rollback_failed=0

  if [[ "$had_previous_app" -eq 1 && ! -d "$backup_app" ]]; then
    echo "安装失败，旧版本未被替换" >&2
    return 0
  fi
  osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
  wait_for_app_exit "$installed_app" || true
  if [[ -e "$installed_app" ]]; then
    rm -rf -- "$installed_app" || rollback_failed=1
  fi
  if [[ "$had_previous_app" -eq 1 && -d "$backup_app" ]]; then
    if mv -- "$backup_app" "$installed_app"; then
      echo "安装失败，旧版本已恢复" >&2
    else
      echo "安装失败，且旧版本恢复失败；备份保留在：$backup_app" >&2
      rollback_failed=1
    fi
  fi
  return "$rollback_failed"
}

cleanup() {
  local status=$?
  trap - EXIT

  if [[ "$status" -ne 0 && "$replacement_active" -eq 1 ]]; then
    rollback_installation || status=1
  fi
  if [[ -n "$staging_app" && -d "$staging_app" ]]; then
    rm -rf -- "$staging_app"
  fi
  rmdir -- "$install_lock" >/dev/null 2>&1 || true
  exit "$status"
}

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

if ! [[ "$launch_check_interval" =~ ^[0-9]+([.][0-9]+)?$ ]] \
    || ! [[ "$launch_check_samples" =~ ^[1-9][0-9]*$ ]]; then
  echo "invalid launch health-check configuration" >&2
  exit 1
fi

if ! mkdir -- "$install_lock" 2>/dev/null; then
  echo "已有本地桌面安装正在进行" >&2
  exit 1
fi
trap cleanup EXIT

"$root/scripts/build-desktop.sh" --local --target "$target"

verify_app "$built_app" "built app"

built_executable=$(
  /usr/libexec/PlistBuddy -c "Print:CFBundleExecutable" \
    "$built_app/Contents/Info.plist"
)
if [[ -z "$built_executable" || "$built_executable" == */* ]]; then
  echo "built app executable verification failed" >&2
  exit 1
fi
if [[ -f "$desktop_config" ]] \
  && ! "$built_app/Contents/MacOS/$built_executable" --self-test-config "$desktop_config"; then
  echo "candidate cannot read the current desktop configuration; the installed App was not replaced" >&2
  exit 1
fi

if [[ -e "$installed_app" ]]; then
  if [[ ! -d "$installed_app" ]]; then
    echo "refusing to replace non-directory path: $installed_app" >&2
    exit 1
  fi
  verify_app "$installed_app" "installed app"
  had_previous_app=1
fi

staging_app="$(mktemp -d "$installed_parent/.token-station.staging.XXXXXX")"
if ! ditto "$built_app" "$staging_app"; then
  echo "staged app copy failed" >&2
  exit 1
fi
verify_app "$staging_app" "staged app"

if [[ "$had_previous_app" -eq 1 ]]; then
  osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
  if ! wait_for_app_exit "$installed_app"; then
    echo "installed app is still running; refusing to replace it" >&2
    exit 1
  fi
  backup_app="$(mktemp -d "$installed_parent/.token-station.backup.XXXXXX")"
  rmdir -- "$backup_app"
  replacement_active=1
  mv -- "$installed_app" "$backup_app"
else
  replacement_active=1
fi

mv -- "$staging_app" "$installed_app"
staging_app=""
verify_app "$installed_app" "installed app"

installed_executable=$(
  /usr/libexec/PlistBuddy -c "Print:CFBundleExecutable" \
    "$installed_app/Contents/Info.plist"
)
if [[ -z "$installed_executable" || "$installed_executable" == */* ]]; then
  echo "installed app executable verification failed: $installed_executable" >&2
  exit 1
fi

# Replacing the bundle at a path LaunchServices already knows leaves a window
# in which `open` answers -600: the old registration for this bundle id still
# points at the directory we just moved away. It is transient, so a single
# attempt turns a good build into a failed install and rolls it back — which is
# what happens every time someone reinstalls over a running app, the normal
# case while iterating. Retry on the same bounded-wait idiom as
# `wait_for_app_exit`.
launched=0
for _ in $(seq 1 "$launch_open_attempts"); do
  if open "$installed_app"; then
    launched=1
    break
  fi
  sleep "$launch_check_interval"
done
if [[ "$launched" -ne 1 ]]; then
  echo "desktop app launch command failed" >&2
  exit 1
fi

readonly executable_path="$installed_app/Contents/MacOS/$installed_executable"
for _ in $(seq 1 "$launch_check_samples"); do
  sleep "$launch_check_interval"
  if ! pgrep -f -x "$executable_path" >/dev/null 2>&1; then
    echo "desktop app exited during launch health check" >&2
    exit 1
  fi
done

replacement_active=0
if [[ -n "$backup_app" && -d "$backup_app" ]]; then
  rm -rf -- "$backup_app"
  backup_app=""
fi

echo "desktop app installed and launched: $installed_app"
