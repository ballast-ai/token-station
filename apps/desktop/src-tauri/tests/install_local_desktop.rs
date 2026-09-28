#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn run_installer_scenario(scenario: &str) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("desktop manifest must be nested under the project root");
    let test_script = project_root.join("tests/install-local-desktop.sh");

    let output = Command::new("bash")
        .arg(&test_script)
        .arg(scenario)
        .output()
        .expect("installer transaction test script must run");

    assert!(
        output.status.success(),
        "installer transaction tests failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
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
