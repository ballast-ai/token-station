#!/usr/bin/env bash
set -euo pipefail

readonly project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_scx_experiment="${TOKEN_STATION_TEST_SCX_EXPERIMENT:-0}"
readonly test_root="$(mktemp -d "${TMPDIR:-/tmp}/token-station-install-test.XXXXXX")"
background_pid_one=""
background_pid_two=""
blocked_build_signal=""

cleanup() {
  local status=$?
  trap - EXIT
  local pid
  if [[ -n "$blocked_build_signal" ]]; then
    mkdir -p -- "$(dirname "$blocked_build_signal")"
    touch -- "$blocked_build_signal"
  fi
  for pid in "$background_pid_one" "$background_pid_two"; do
    [[ -n "$pid" ]] || continue
    wait "$pid" >/dev/null 2>&1 || true
  done
  rm -rf -- "$test_root"
  exit "$status"
}
trap cleanup EXIT

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
  local app_name="token-station.app"
  test_bundle_id="com.tokenstation.desktop"
  if [[ "$test_scx_experiment" == "1" ]]; then
    app_name="Token Station.app"
    test_bundle_id="com.tokenstation.desktop.scx"
  fi
  installed_app="$applications/$app_name"
  legacy_app="$applications/Token Station SCX.app"
  stable_app="$applications/token-station.app"
  built_app="$repo/apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/$app_name"

  mkdir -p \
    "$repo/scripts" \
    "$built_app/Contents/MacOS" \
    "$installed_app/Contents/MacOS" \
    "$state" \
    "$fake_bin"
  touch "$built_app/Contents/Info.plist"
  touch "$installed_app/Contents/Info.plist" "$installed_app/Contents/MacOS/token-station"
  echo "old" > "$installed_app/old.version"
  if [[ "$test_scx_experiment" == "1" ]]; then
    mkdir -p "$stable_app"
    echo "protected-stable-version" > "$stable_app/stable.version"
  fi

  cat > "$built_app/Contents/MacOS/token-station" <<'SCRIPT'
#!/usr/bin/env bash
if [[ "${1:-}" == "--self-test-config" ]]; then
  [[ "${CANDIDATE_CONFIG_COMPATIBLE:-1}" == "1" ]]
  exit
fi
exit 0
SCRIPT

  sed \
    -e "s|/Applications/token-station.app|$stable_app|g" \
    -e "s|/Applications/Token Station SCX.app|$legacy_app|g" \
    -e "s|/Applications/Token Station.app|$installed_app|g" \
    -e "s|/usr/libexec/PlistBuddy|$fake_bin/PlistBuddy|g" \
    "$project_root/scripts/install-local-desktop.sh" \
    > "$repo/scripts/install-local-desktop.sh"
  chmod +x "$repo/scripts/install-local-desktop.sh"

  cat > "$repo/scripts/build-desktop.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
echo "$$" >> "$TEST_STATE/build.log"
if [[ "${FAIL_BUILD:-0}" == "1" ]]; then
  exit 33
fi
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
  if [[ -f "$3.bundle-id" ]]; then
    cat "$3.bundle-id"
  else
    echo "$TEST_BUNDLE_ID"
  fi
fi
SCRIPT

  cat > "$fake_bin/codesign" <<'SCRIPT'
#!/usr/bin/env bash
if [[ "${FAIL_INSTALLED_CODESIGN:-0}" == "1" && "$*" == *"$TEST_INSTALLED_APP"* ]]; then
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
if [[ "${OCCUPY_TARGET_DURING_COPY:-0}" == "1" ]]; then
  mkdir -p "$TEST_INSTALLED_APP"
  echo "unrelated" > "$TEST_INSTALLED_APP/unrelated.version"
fi
SCRIPT

  cat > "$fake_bin/osascript" <<'SCRIPT'
#!/usr/bin/env bash
echo "quit" >> "$TEST_STATE/quit.log"
rm -f "$TEST_STATE/legacy-running"
exit 0
SCRIPT

  cat > "$fake_bin/open" <<'SCRIPT'
#!/usr/bin/env bash
# `OPEN_FAILURES_BEFORE_SUCCESS` reproduces LaunchServices answering -600 for
# the first moments after the bundle at this path is replaced.
attempts_file="$TEST_STATE/open.attempts"
attempts=$(cat "$attempts_file" 2>/dev/null || echo 0)
attempts=$((attempts + 1))
echo "$attempts" > "$attempts_file"
if [[ "$attempts" -le "${OPEN_FAILURES_BEFORE_SUCCESS:-0}" ]]; then
  echo "LSOpenURLsWithCompletionHandler() failed with error -600." >&2
  exit 1
fi
touch "$TEST_STATE/opened"
exit 0
SCRIPT

  cat > "$fake_bin/pgrep" <<'SCRIPT'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$TEST_STATE/pgrep.log"
if [[ -e "$TEST_STATE/legacy-running" && "$*" == *"$TEST_LEGACY_APP/Contents/MacOS/token-station"* ]]; then
  exit 0
fi
if [[ -e "$TEST_STATE/opened" && "${RUNNING_AFTER_OPEN:-0}" == "1" ]]; then
  exit 0
fi
exit 1
SCRIPT

  chmod +x \
    "$repo/scripts/build-desktop.sh" \
    "$built_app/Contents/MacOS/token-station" \
    "$fake_bin"/*
}

run_installer() {
  local installer_args=("$repo/scripts/install-local-desktop.sh")
  if [[ "$test_scx_experiment" == "1" ]]; then
    installer_args+=(--scx-experiment)
    local config_dir="$fixture/Library/Application Support/com.tokenstation.desktop.scx"
    mkdir -p "$config_dir"
    if [[ -f "$fixture/token-station.json" ]]; then
      cp "$fixture/token-station.json" "$config_dir/token-station.json"
    fi
  fi
  env \
    HOME="$fixture" \
    PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    TEST_STATE="$state" \
    TEST_BUNDLE_ID="$test_bundle_id" \
    TEST_INSTALLED_APP="$installed_app" \
    TEST_LEGACY_APP="$legacy_app" \
    FAIL_BUILD="${FAIL_BUILD:-0}" \
    OCCUPY_TARGET_DURING_COPY="${OCCUPY_TARGET_DURING_COPY:-0}" \
    DITTO_FAIL="${DITTO_FAIL:-0}" \
    FAIL_INSTALLED_CODESIGN="${FAIL_INSTALLED_CODESIGN:-0}" \
    RUNNING_AFTER_OPEN="${RUNNING_AFTER_OPEN:-0}" \
    OPEN_FAILURES_BEFORE_SUCCESS="${OPEN_FAILURES_BEFORE_SUCCESS:-0}" \
    CANDIDATE_CONFIG_COMPATIBLE="${CANDIDATE_CONFIG_COMPATIBLE:-1}" \
    TOKEN_STATION_DESKTOP_CONFIG="$fixture/token-station.json" \
    WAIT_BUILD="${WAIT_BUILD:-0}" \
    TOKEN_STATION_LAUNCH_CHECK_INTERVAL_SECONDS=0 \
    TOKEN_STATION_LAUNCH_CHECK_SAMPLES=2 \
    "${installer_args[@]}"
}

test_target_name_matches_the_selected_identity() {
  make_fixture "target-name"
  local args=(--print-target)
  if [[ "$test_scx_experiment" == "1" ]]; then
    args+=(--scx-experiment)
  fi
  "$repo/scripts/install-local-desktop.sh" "${args[@]}" > "$fixture/target"
  [[ "$(sed -n '1p' "$fixture/target")" == "$installed_app" ]] \
    || fail "the installer reported the wrong product path"
  [[ "$(sed -n '2p' "$fixture/target")" == "$test_bundle_id" ]] \
    || fail "the product rename changed its bundle identity"
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

test_incompatible_candidate_preserves_old_app() {
  make_fixture "config-incompatible"
  echo '{"version":1}' > "$fixture/token-station.json"

  if CANDIDATE_CONFIG_COMPATIBLE=0 run_installer >"$fixture/output" 2>&1; then
    fail "a candidate that cannot read the current config unexpectedly installed"
  fi
  [[ -f "$installed_app/old.version" ]] \
    || fail "config preflight failure replaced the working App"
  [[ ! -e "$state/opened" ]] \
    || fail "config preflight failure launched the incompatible candidate"
  grep -q "cannot read the current desktop configuration" "$fixture/output" \
    || fail "config preflight failure did not report the compatibility reason"
}

test_concurrent_install_has_single_owner() {
  make_fixture "concurrent"

  (
    set +e
    WAIT_BUILD=1 RUNNING_AFTER_OPEN=1 run_installer >"$fixture/first.output" 2>&1
    echo "$?" > "$fixture/first.status"
  ) &
  local first_pid=$!
  background_pid_one="$first_pid"
  blocked_build_signal="$state/release-build"

  for _ in $(seq 1 1000); do
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
  background_pid_two="$second_pid"

  for _ in $(seq 1 1000); do
    [[ -f "$fixture/second.status" ]] && break
    [[ "$(wc -l < "$state/build.log")" -gt 1 ]] && break
    sleep 0.01
  done
  touch "$state/release-build"
  wait "$first_pid"
  wait "$second_pid"
  background_pid_one=""
  background_pid_two=""
  blocked_build_signal=""

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
  if [[ "$test_scx_experiment" == "1" ]]; then
    [[ "$(cat "$stable_app/stable.version")" == "protected-stable-version" ]] \
      || fail "experimental installation changed the stable App"
  fi
}

test_a_transient_launch_refusal_is_retried_not_rolled_back() {
  make_fixture "launch-retry"

  # Replacing a bundle LaunchServices already knows leaves a window in which
  # `open` answers -600, because the old registration for this bundle id still
  # points at the directory the installer just moved away. A single attempt
  # turned a good build into a failed install and restored the old App — which
  # is what happened reinstalling over a running 1.2.4 while bumping to 1.3.0.
  OPEN_FAILURES_BEFORE_SUCCESS=2 RUNNING_AFTER_OPEN=1 \
    TOKEN_STATION_LAUNCH_CHECK_INTERVAL_SECONDS=0 run_installer \
    >"$fixture/output" 2>&1 \
    || fail "a launch that succeeds on retry was treated as a failed install"
  [[ ! -f "$installed_app/old.version" ]] \
    || fail "a retried launch still rolled back to the old App"
  grep -q "installed and launched" "$fixture/output" \
    || fail "a retried launch omitted the success message"
}

test_a_launch_that_never_succeeds_still_rolls_back() {
  make_fixture "launch-never"

  OPEN_FAILURES_BEFORE_SUCCESS=99 RUNNING_AFTER_OPEN=1 \
    TOKEN_STATION_LAUNCH_CHECK_INTERVAL_SECONDS=0 run_installer \
    >"$fixture/output" 2>&1 \
    && fail "an App that never launches must not be reported as installed"
  [[ -f "$installed_app/old.version" ]] \
    || fail "a permanently failing launch must restore the old App"
}

test_legacy_name_migrates_without_recreating_the_absent_stable_app() {
  make_fixture "legacy-success"
  mv "$installed_app" "$legacy_app"
  rm -rf "$stable_app"
  touch "$state/legacy-running"

  RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1 \
    || fail "the legacy App did not migrate to the new product name"
  [[ -d "$installed_app" && ! -e "$legacy_app" ]] \
    || fail "successful migration did not leave exactly the new App path"
  [[ ! -e "$stable_app" ]] \
    || fail "migration recreated the previously removed stable App"
  [[ -f "$state/quit.log" ]] \
    || fail "migration did not quit the legacy App"
  grep -Fq "$legacy_app/Contents/MacOS/token-station" "$state/pgrep.log" \
    || fail "migration did not wait for the legacy executable to exit"
}

test_legacy_failures_restore_the_original_path() {
  local failure
  for failure in signature launch; do
    make_fixture "legacy-rollback-$failure"
    mv "$installed_app" "$legacy_app"
    local signature_failure=0
    [[ "$failure" != "signature" ]] || signature_failure=1
    if FAIL_INSTALLED_CODESIGN="$signature_failure" run_installer >"$fixture/output" 2>&1; then
      fail "legacy $failure failure unexpectedly reported success"
    fi
    [[ -f "$legacy_app/old.version" && ! -e "$installed_app" ]] \
      || fail "legacy $failure failure did not restore the original path"
    grep -q "旧版本已恢复" "$fixture/output" \
      || fail "legacy $failure failure did not report rollback"
    [[ "$(cat "$stable_app/stable.version")" == "protected-stable-version" ]] \
      || fail "legacy rollback changed the stable App"
  done
}

test_legacy_app_is_untouched_until_build_audit_and_staging_succeed() {
  local failure
  for failure in build audit copy; do
    make_fixture "legacy-preflight-$failure"
    mv "$installed_app" "$legacy_app"
    local build_failure=0 copy_failure=0
    case "$failure" in
      build) build_failure=1 ;;
      audit) echo "org.example.unrelated" > "$built_app/Contents/Info.plist.bundle-id" ;;
      copy) copy_failure=1 ;;
    esac
    if FAIL_BUILD="$build_failure" DITTO_FAIL="$copy_failure" RUNNING_AFTER_OPEN=1 \
      run_installer >"$fixture/output" 2>&1; then
      fail "legacy $failure preflight failure unexpectedly reported success"
    fi
    [[ -f "$legacy_app/old.version" && ! -e "$installed_app" && ! -e "$state/quit.log" ]] \
      || fail "$failure failure touched the previously usable legacy App"
  done
}

test_two_product_paths_are_rejected_without_touching_either_app() {
  make_fixture "two-product-paths"
  cp -R "$installed_app" "$legacy_app"
  if RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1; then
    fail "two current product paths were silently resolved"
  fi
  [[ -f "$installed_app/old.version" && -f "$legacy_app/old.version" && ! -e "$state/quit.log" ]] \
    || fail "the two-path conflict changed an installed App"
  grep -q "Both current and legacy App paths exist" "$fixture/output" \
    || fail "the two-path conflict did not explain the refusal"
}

test_unrelated_new_or_legacy_apps_are_rejected() {
  local location
  for location in current legacy; do
    make_fixture "unrelated-$location"
    local occupied_app="$installed_app"
    if [[ "$location" == "legacy" ]]; then
      mv "$installed_app" "$legacy_app"
      occupied_app="$legacy_app"
    fi
    echo "org.example.unrelated" > "$occupied_app/Contents/Info.plist.bundle-id"
    if RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1; then
      fail "an unrelated $location App was replaced"
    fi
    [[ -f "$occupied_app/old.version" && ! -e "$state/quit.log" ]] \
      || fail "the unrelated $location App was changed or stopped"
    grep -q "unexpected bundle id" "$fixture/output" \
      || fail "the unrelated App refusal omitted the identity mismatch"
  done
}

test_a_dangling_new_path_is_not_treated_as_absent() {
  make_fixture "dangling-target"
  mv "$installed_app" "$legacy_app"
  ln -s "$fixture/missing.app" "$installed_app"
  if RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1; then
    fail "a dangling target link was silently replaced"
  fi
  [[ -L "$installed_app" && -f "$legacy_app/old.version" && ! -e "$state/quit.log" ]] \
    || fail "a dangling target conflict modified either path"
}

test_a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy() {
  make_fixture "late-target-conflict"
  mv "$installed_app" "$legacy_app"
  if OCCUPY_TARGET_DURING_COPY=1 RUNNING_AFTER_OPEN=1 run_installer >"$fixture/output" 2>&1; then
    fail "a target created during staging was silently replaced"
  fi
  [[ -f "$legacy_app/old.version" && -f "$installed_app/unrelated.version" ]] \
    || fail "late conflict rollback removed the foreign target or lost the legacy App"
  grep -q "target App path became occupied" "$fixture/output" \
    || fail "the late target conflict did not explain its refusal"
}

# Run all scenarios by default. Rust tests select individual scenarios.
case "${1:-all}" in
  all)
    test_target_name_matches_the_selected_identity
    test_copy_failure_preserves_old_app
    test_immediate_exit_restores_old_app
    test_incompatible_candidate_preserves_old_app
    test_concurrent_install_has_single_owner
    test_stable_launch_succeeds
    test_a_transient_launch_refusal_is_retried_not_rolled_back
    test_a_launch_that_never_succeeds_still_rolls_back
    if [[ "$test_scx_experiment" == "1" ]]; then
      test_legacy_name_migrates_without_recreating_the_absent_stable_app
      test_legacy_failures_restore_the_original_path
      test_legacy_app_is_untouched_until_build_audit_and_staging_succeed
      test_two_product_paths_are_rejected_without_touching_either_app
      test_unrelated_new_or_legacy_apps_are_rejected
      test_a_dangling_new_path_is_not_treated_as_absent
      test_a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy
    fi
    ;;
  target_name_matches_the_selected_identity) test_target_name_matches_the_selected_identity ;;
  copy_failure_preserves_old_app) test_copy_failure_preserves_old_app ;;
  immediate_exit_restores_old_app) test_immediate_exit_restores_old_app ;;
  incompatible_candidate_preserves_old_app) test_incompatible_candidate_preserves_old_app ;;
  concurrent_install_has_single_owner) test_concurrent_install_has_single_owner ;;
  stable_launch_succeeds) test_stable_launch_succeeds ;;
  a_transient_launch_refusal_is_retried_not_rolled_back) test_a_transient_launch_refusal_is_retried_not_rolled_back ;;
  a_launch_that_never_succeeds_still_rolls_back) test_a_launch_that_never_succeeds_still_rolls_back ;;
  legacy_name_migrates_without_recreating_the_absent_stable_app) test_legacy_name_migrates_without_recreating_the_absent_stable_app ;;
  legacy_failures_restore_the_original_path) test_legacy_failures_restore_the_original_path ;;
  legacy_app_is_untouched_until_build_audit_and_staging_succeed) test_legacy_app_is_untouched_until_build_audit_and_staging_succeed ;;
  two_product_paths_are_rejected_without_touching_either_app) test_two_product_paths_are_rejected_without_touching_either_app ;;
  unrelated_new_or_legacy_apps_are_rejected) test_unrelated_new_or_legacy_apps_are_rejected ;;
  a_dangling_new_path_is_not_treated_as_absent) test_a_dangling_new_path_is_not_treated_as_absent ;;
  a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy) test_a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy ;;
  *) fail "unknown installer test scenario: $1" ;;
esac

echo "install-local-desktop transaction tests passed"
