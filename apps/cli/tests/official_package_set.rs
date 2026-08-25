//! One source for the official package set, and a gate that keeps the copies
//! honest against it.
//!
//! The list had drifted into eight places — two build scripts, the desktop
//! bundle, the artifact audit, the CLI's builtin embedding, the test staging
//! script and more — and the Anthropic component reached three of them before
//! the rest noticed. A list that lives in one file and is checked from another
//! cannot drift silently; it turns a Windows-only packaging surprise into a
//! red test on any machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn packages() -> Vec<Value> {
    let source = std::fs::read_to_string(repo_root().join("plugins/official/packages.json"))
        .expect("the package set reads");
    let document: Value = serde_json::from_str(&source).expect("the package set is JSON");
    document["packages"]
        .as_array()
        .expect("packages is an array")
        .clone()
}

fn field(package: &Value, key: &str) -> String {
    package[key].as_str().expect("string field").to_owned()
}

/// Every declared package is a directory that exists and carries a manifest
/// naming the id this file claims for it.
#[test]
fn every_declared_package_exists_and_reports_the_declared_id() {
    for package in packages() {
        let dir = repo_root()
            .join("plugins/official")
            .join(field(&package, "dir"));
        assert!(dir.is_dir(), "{} is not a package directory", dir.display());

        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("manifest.json")).expect("manifest reads"),
        )
        .expect("manifest is JSON");
        assert_eq!(
            manifest["name"].as_str().expect("manifest names itself"),
            field(&package, "id"),
            "{} declares an id the package set does not expect",
            dir.display()
        );
    }
}

/// A package's shell version and its manifest version are the same number.
///
/// They were not: the Anthropic shell shipped `1.0.0` against a manifest
/// claiming `1.0.1`, so the version an operator reads and the version the
/// identity gate compares came from different files.
#[test]
fn each_package_agrees_with_itself_about_its_version() {
    for package in packages() {
        if field(&package, "kind") != "south-component" {
            continue;
        }
        let dir = repo_root()
            .join("plugins/official")
            .join(field(&package, "dir"));
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("manifest.json")).expect("manifest reads"),
        )
        .expect("manifest is JSON");
        let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).expect("Cargo.toml reads");
        let declared = cargo
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .and_then(|rest| rest.split('"').next())
            .expect("the crate declares a version");
        assert_eq!(
            declared,
            manifest["version"].as_str().expect("manifest version"),
            "{} disagrees with its own manifest about its version",
            dir.display()
        );
    }
}

/// Every consumer reads the package set instead of keeping a checked copy.
/// A test that only checks repeated names still permits every copy to drift in
/// its other fields and keeps package addition as a multi-file change.
#[test]
fn every_consumer_reads_the_package_set() {
    let root = repo_root();
    let consumers = [
        ("scripts/build-desktop.sh", "official-packages.py"),
        ("scripts/build-release.sh", "official-packages.py"),
        (
            "scripts/prepare-desktop-test-plugins.sh",
            "official-packages.py",
        ),
        ("tests/build-desktop-verbosity.sh", "official-packages.py"),
        ("apps/cli/build.rs", "plugins/official/packages.json"),
        ("apps/cli/src/plugins.rs", "builtin_official_packages.rs"),
        ("scripts/audit-desktop-artifact.sh", "official-packages.py"),
        ("apps/desktop/src-tauri/src/self_test.rs", "OFFICIAL_PACKAGE_IDS"),
    ];

    for (path, marker) in consumers {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} reads: {error}"));
        assert!(
            source.contains(marker),
            "{path} does not consume the official package set through `{marker}`"
        );
    }

    for entry in std::fs::read_dir(root.join("apps/desktop/src-tauri/src")).expect("desktop src reads")
    {
        let path = entry.expect("desktop src entry reads").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("desktop source reads");
        assert!(
            !source.contains("BUNDLED_PLUGIN_IDS"),
            "the desktop must not keep a second official package id list ({})",
            path.display()
        );
    }
}

/// The shell-facing reader preserves package order and exposes every field
/// needed by build, staging, and audit scripts.
#[test]
fn package_reader_reports_the_declared_packages() {
    for (kind, key) in [("agent", "dir"), ("south-component", "id")] {
        let output = Command::new("python3")
            .arg(repo_root().join("scripts/official-packages.py"))
            .args(["--kind", kind, "--field", key])
            .output()
            .expect("official package reader starts");
        assert!(
            output.status.success(),
            "official package reader failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: Vec<_> = String::from_utf8(output.stdout)
            .expect("reader output is UTF-8")
            .lines()
            .map(str::to_owned)
            .collect();
        let expected: Vec<_> = packages()
            .into_iter()
            .filter(|package| field(package, "kind") == kind)
            .map(|package| field(&package, key))
            .collect();
        assert_eq!(actual, expected, "reader output for {kind}.{key}");
    }
}

/// Every package ships the fixture directory its manifest declares — present,
/// non-empty, and made of complete `.input.json` / `.expected.json` pairs.
///
/// The build scripts used to hardcode the name `fixtures`, so the Anthropic
/// component's `fixtures-anthropic/` would have shipped nothing even once the
/// directory existed, and nothing would have said so. They stage the declared
/// path now. The South components' packs are vendored from the pinned
/// token-station-south revision, which owns their content; changing them here
/// without a matching South change is drift, not an update.
#[test]
fn every_declared_fixture_directory_is_present_and_paired() {
    for package in packages() {
        let dir = repo_root()
            .join("plugins/official")
            .join(field(&package, "dir"));
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("manifest.json")).expect("manifest reads"),
        )
        .expect("manifest is JSON");
        let declared = manifest["conformance"]["fixtures"]
            .as_str()
            .expect("every manifest declares a fixtures path")
            .trim_end_matches('/')
            .to_owned();

        let fixtures = dir.join(&declared);
        assert!(
            fixtures.is_dir(),
            "{} declares `{declared}` and does not ship it",
            dir.display()
        );

        let mut inputs = Vec::new();
        let mut expected = Vec::new();
        for entry in std::fs::read_dir(&fixtures).expect("fixture directory reads") {
            let name = entry
                .expect("fixture entry reads")
                .file_name()
                .into_string()
                .expect("fixture names are UTF-8");
            if let Some(case) = name.strip_suffix(".input.json") {
                inputs.push(case.to_owned());
            } else if let Some(case) = name.strip_suffix(".expected.json") {
                expected.push(case.to_owned());
            }
        }
        inputs.sort();
        expected.sort();
        assert!(
            !inputs.is_empty(),
            "{} ships an empty fixture pack",
            fixtures.display()
        );
        assert_eq!(
            inputs,
            expected,
            "{} has unpaired fixture files",
            fixtures.display()
        );
    }
}
