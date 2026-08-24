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

/// Every consumer that repeats the list is checked against it. A new package
/// added to the set but not to a script fails here rather than on the one
/// platform whose packaging noticed.
#[test]
fn every_consumer_names_the_whole_set() {
    let root = repo_root();
    let consumers = [
        ("scripts/build-desktop.sh", "dir"),
        ("scripts/build-release.sh", "dir"),
        ("scripts/prepare-desktop-test-plugins.sh", "dir"),
        ("apps/cli/build.rs", "dir"),
        ("scripts/audit-desktop-artifact.sh", "id"),
        ("apps/desktop/src-tauri/src/lib.rs", "id"),
    ];

    for (path, key) in consumers {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} reads: {error}"));
        for package in packages() {
            let name = field(&package, key);
            assert!(
                source.contains(&name),
                "{path} does not name `{name}`; the official package set lists it"
            );
        }
    }
}
