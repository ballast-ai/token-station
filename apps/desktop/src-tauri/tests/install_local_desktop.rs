#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn run_installer_scenario(scenario: &str) {
    run_installer_scenario_in_modes(scenario, &["0", "1"]);
}

fn run_installer_scenario_in_modes(scenario: &str, experiments: &[&str]) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("desktop manifest must be nested under the project root");
    let test_script = project_root.join("tests/install-local-desktop.sh");

    for experiment in experiments {
        let output = Command::new("bash")
            .arg(&test_script)
            .arg(scenario)
            .env("TOKEN_STATION_TEST_SCX_EXPERIMENT", experiment)
            .output()
            .expect("installer transaction test script must run");

        assert!(
            output.status.success(),
            "installer scenario {scenario} failed (SCX={experiment})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn scx_settings_copy_preserves_stable_state() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let output = Command::new("python3")
        .arg(root.join("tests/scx-experiment-isolation.py"))
        .output()
        .expect("SCX isolation tests must run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_desktop_installer_copy_failure_preserves_old_app() {
    run_installer_scenario("copy_failure_preserves_old_app");
}

#[test]
fn local_desktop_installer_immediate_exit_restores_old_app() {
    run_installer_scenario("immediate_exit_restores_old_app");
}

#[test]
fn local_desktop_installer_incompatible_candidate_preserves_old_app() {
    run_installer_scenario("incompatible_candidate_preserves_old_app");
}

#[test]
fn local_desktop_installer_concurrent_install_has_single_owner() {
    run_installer_scenario("concurrent_install_has_single_owner");
}

#[test]
fn local_desktop_installer_stable_launch_succeeds() {
    run_installer_scenario("stable_launch_succeeds");
}

#[test]
fn local_desktop_installer_a_transient_launch_refusal_is_retried_not_rolled_back() {
    run_installer_scenario("a_transient_launch_refusal_is_retried_not_rolled_back");
}

#[test]
fn local_desktop_installer_a_launch_that_never_succeeds_still_rolls_back() {
    run_installer_scenario("a_launch_that_never_succeeds_still_rolls_back");
}

#[test]
fn local_desktop_installer_target_name_matches_the_selected_identity() {
    run_installer_scenario("target_name_matches_the_selected_identity");
}

#[test]
fn local_desktop_installer_legacy_name_migrates_without_recreating_the_absent_stable_app() {
    run_installer_scenario_in_modes(
        "legacy_name_migrates_without_recreating_the_absent_stable_app",
        &["1"],
    );
}

#[test]
fn local_desktop_installer_legacy_failures_restore_the_original_path() {
    run_installer_scenario_in_modes("legacy_failures_restore_the_original_path", &["1"]);
}

#[test]
fn local_desktop_installer_legacy_app_is_untouched_until_build_audit_and_staging_succeed() {
    run_installer_scenario_in_modes(
        "legacy_app_is_untouched_until_build_audit_and_staging_succeed",
        &["1"],
    );
}

#[test]
fn local_desktop_installer_two_product_paths_are_rejected_without_touching_either_app() {
    run_installer_scenario_in_modes(
        "two_product_paths_are_rejected_without_touching_either_app",
        &["1"],
    );
}

#[test]
fn local_desktop_installer_unrelated_new_or_legacy_apps_are_rejected() {
    run_installer_scenario_in_modes("unrelated_new_or_legacy_apps_are_rejected", &["1"]);
}

#[test]
fn local_desktop_installer_a_dangling_new_path_is_not_treated_as_absent() {
    run_installer_scenario_in_modes("a_dangling_new_path_is_not_treated_as_absent", &["1"]);
}

#[test]
fn local_desktop_installer_a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy() {
    run_installer_scenario_in_modes(
        "a_late_target_conflict_keeps_the_foreign_app_and_restores_legacy",
        &["1"],
    );
}

#[test]
fn local_desktop_build_only_requests_the_app_bundle() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("desktop manifest must be nested under the project root");
    let build_script = std::fs::read_to_string(project_root.join("scripts/build-desktop.sh"))
        .expect("desktop build script must be readable");

    let local_case = build_script
        .split("local)")
        .nth(1)
        .and_then(|tail| tail.split(";;").next())
        .expect("build script must define the local mode");
    assert!(
        local_case.contains("macos_bundle_kind=\"app\"")
            && build_script.contains("tauri_args+=(--bundles \"$macos_bundle_kind\")"),
        "local desktop installation must not depend on release-only DMG packaging"
    );
}
