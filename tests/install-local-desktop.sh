#!/usr/bin/env bash
set -euo pipefail

readonly project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_root="$(mktemp -d "${TMPDIR:-/tmp}/token-station-install-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

make_fixture() {
  local name="$1"
  fixture="$test_root/$name"
  repo="$fixture/repo"
  applications="$fixture/Applications"
  state="$fixture/state"
  fake_bin="$fixture/bin"
  installed_app="$applications/token-station.app"
  built_app="$repo/apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/token-station.app"

  mkdir -p \
    "$repo/scripts" \
    "$built_app/Contents/MacOS" \
    "$installed_app/Contents/MacOS" \
    "$state" \
    "$fake_bin"
  touch "$built_app/Contents/Info.plist" "$built_app/Contents/MacOS/token-station"
  touch "$installed_app/Contents/Info.plist" "$installed_app/Contents/MacOS/token-station"
  echo "old" > "$installed_app/old.version"

  sed \
    -e "s|/Applications/token-station.app|$installed_app|g" \
    -e "s|/usr/libexec/PlistBuddy|$fake_bin/PlistBuddy|g" \
    "$project_root/scripts/install-local-desktop.sh" \
    > "$repo/scripts/install-local-desktop.sh"
  chmod +x "$repo/scripts/install-local-desktop.sh"

  cat > "$repo/scripts/build-desktop.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
echo "$$" >> "$TEST_STATE/build.log"
if [[ "${WAIT_BUILD:-0}" == "1" ]]; then
  while [[ ! -e "$TEST_STATE/release-build" ]]; do
    sleep 0.01
  done
fi
SCRIPT

  cat > "$fake_bin/uname" <<'SCRIPT'
#!/usr/bin/env bash
if [[ "${1:-}" == "-s" ]]; then
  echo Darwin
elif [[ "${1:-}" == "-m" ]]; then
  echo arm64
else
  exit 1
fi
SCRIPT

  cat > "$fake_bin/PlistBuddy" <<'SCRIPT'
#!/usr/bin/env bash
if [[ "$*" == *"CFBundleExecutable"* ]]; then
  echo token-station
else
  echo com.tokenstation.desktop
fi
SCRIPT

  cat > "$fake_bin/codesign" <<'SCRIPT'
#!/usr/bin/env bash
if [[ "${FAIL_INSTALLED_CODESIGN:-0}" == "1" && "$*" == *"/Applications/token-station.app"* ]]; then
  exit 31
fi
exit 0
SCRIPT

  cat > "$fake_bin/ditto" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${DITTO_FAIL:-0}" == "1" ]]; then
  exit 32
fi
mkdir -p "$2"
cp -R "$1"/. "$2"/
SCRIPT

  cat > "$fake_bin/osascript" <<'SCRIPT'
#!/usr/bin/env bash
exit 0
SCRIPT

  cat > "$fake_bin/open" <<'SCRIPT'
#!/usr/bin/env bash
touch "$TEST_STATE/opened"
exit 0
SCRIPT

  cat > "$fake_bin/pgrep" <<'SCRIPT'
#!/usr/bin/env bash
if [[ -e "$TEST_STATE/opened" && "${RUNNING_AFTER_OPEN:-0}" == "1" ]]; then
  exit 0
fi
exit 1
SCRIPT

  chmod +x "$repo/scripts/build-desktop.sh" "$fake_bin"/*
}

run_installer() {
  env \
    PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    TEST_STATE="$state" \
    DITTO_FAIL="${DITTO_FAIL:-0}" \
    FAIL_INSTALLED_CODESIGN="${FAIL_INSTALLED_CODESIGN:-0}" \
    RUNNING_AFTER_OPEN="${RUNNING_AFTER_OPEN:-0}" \
    WAIT_BUILD="${WAIT_BUILD:-0}" \
    TOKEN_STATION_LAUNCH_CHECK_INTERVAL_SECONDS=0 \
    TOKEN_STATION_LAUNCH_CHECK_SAMPLES=2 \
    "$repo/scripts/install-local-desktop.sh"
}

test_copy_failure_preserves_old_app() {
  make_fixture "copy-failure"

  if DITTO_FAIL=1 run_installer >"$fixture/output" 2>&1; then
    fail "copy failure unexpectedly reported success"
  fi
  [[ -f "$installed_app/old.version" ]] \
    || fail "copy failure removed the working App"
  grep -q "staged app copy failed" "$fixture/output" \
    || fail "copy failure did not report its stage"
}

test_immediate_exit_restores_old_app() {
  make_fixture "launch-failure"

  if run_installer >"$fixture/output" 2>&1; then
    fail "an App that exited immediately unexpectedly reported success"
  fi
  [[ -f "$installed_app/old.version" ]] \
    || fail "launch failure did not restore the old App"
  ! grep -q "installed and launched" "$fixture/output" \
    || fail "launch failure printed the success message"
}

test_concurrent_install_has_single_owner() {
  make_fixture "concurrent"

  (
    set +e
    WAIT_BUILD=1 RUNNING_AFTER_OPEN=1 run_installer >"$fixture/first.output" 2>&1
    echo "$?" > "$fixture/first.status"
  ) &
  local first_pid=$!

  for _ in $(seq 1 200); do
    [[ -f "$state/build.log" ]] && [[ "$(wc -l < "$state/build.log")" -ge 1 ]] && break
    sleep 0.01
  done
  [[ -f "$state/build.log" ]] || fail "first installer never entered the build"

  (
    set +e
    WAIT_BUILD=1 RUNNING_AFTER_OPEN=1 run_installer >"$fixture/second.output" 2>&1
    echo "$?" > "$fixture/second.status"
  ) &
  local second_pid=$!

  for _ in $(seq 1 100); do
    [[ -f "$fixture/second.status" ]] && break
    [[ "$(wc -l < "$state/build.log")" -gt 1 ]] && break
    sleep 0.01
  done
  touch "$state/release-build"
  wait "$first_pid"
  wait "$second_pid"

  [[ "$(wc -l < "$state/build.log")" -eq 1 ]] \
    || fail "two concurrent installers entered the protected build section"
  [[ "$(cat "$fixture/first.status")" -eq 0 ]] \
    || fail "the lock owner failed to install"
  [[ "$(cat "$fixture/second.status")" -ne 0 ]] \
    || fail "the second installer did not fail on lock contention"
  grep -q "已有本地桌面安装正在进行" "$fixture/second.output" \
    || fail "lock contention did not report the expected reason"
}

test_stable_launch_succeeds() {
  make_fixture "launch-success"

  RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1 \
    || fail "a stable App launch unexpectedly failed"
  [[ ! -f "$installed_app/old.version" ]] \
    || fail "successful installation kept the old App as the active version"
  grep -q "installed and launched" "$fixture/output" \
    || fail "successful installation omitted the success message"
}

test_copy_failure_preserves_old_app
test_immediate_exit_restores_old_app
test_concurrent_install_has_single_owner
test_stable_launch_succeeds

echo "install-local-desktop transaction tests passed"
