#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[test]
fn local_desktop_installer_is_transactional_and_checks_launch_health() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("desktop manifest must be nested under the project root");
    let test_script = project_root.join("tests/install-local-desktop.sh");

    let output = Command::new("bash")
        .arg(&test_script)
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
