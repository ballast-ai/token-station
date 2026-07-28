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
