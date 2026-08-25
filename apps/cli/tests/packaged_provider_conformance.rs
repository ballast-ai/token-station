//! Conformance run against the package as it ships, not against the workspace.
//!
//! `official_package_set` proves a declared fixture directory exists, is
//! non-empty, and pairs its inputs with expectations.
//! `check-provider-fixtures.py` proves the contents still match the South
//! revision they were vendored from. Neither runs the fixtures. A pack can be
//! present, paired, byte-identical to South — and the component beside it can
//! still fail every case, because nothing in this repository had ever executed
//! one against the other.
//!
//! This closes that: the component is built as the package builds it, the pack
//! is loaded from the package's own declared directory, and the suite runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use south_component_conformance::sandbox::SandboxedComponentV1;
use south_component_conformance::{FixturePackV1, run_provider_component_suite_v1};
use south_provider_runtime::{LoadedComponentV1, NoSecretsV1};
use token_station_cli::south_component::{host_expectations, south_component_runtime};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
}

fn package_dir(package: &str) -> PathBuf {
    repo_root().join("plugins/official").join(package)
}

/// The manifest's own declared directory, so the test cannot drift onto a path
/// the package does not actually ship.
fn declared_fixtures(package: &str) -> PathBuf {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(package_dir(package).join("manifest.json"))
            .expect("manifest reads"),
    )
    .expect("manifest is JSON");
    let declared = manifest
        .get("conformance")
        .and_then(|c| c.get("fixtures"))
        .and_then(serde_json::Value::as_str)
        .expect("the package declares a fixture directory");
    package_dir(package).join(declared.trim_end_matches('/'))
}

fn build_component(package: &str, stem: &str) -> PathBuf {
    let dir = package_dir(package);
    let status = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "{package} builds");
    dir.join("target/wasm32-wasip2/release")
        .join(format!("{stem}.wasm"))
}

fn run(package: &str, stem: &str) {
    // The host's own runtime and expectations, so this proves the packaged
    // component loads under the gates the product actually applies — not under
    // a set of limits invented by the test.
    let runtime = south_component_runtime().expect("component runtime builds");
    let wasm = std::fs::read(build_component(package, stem)).expect("component reads");
    let manifest = std::fs::read_to_string(package_dir(package).join("manifest.json"))
        .expect("manifest reads");
    let loaded = LoadedComponentV1::load_embedded(
        &runtime,
        &manifest,
        &wasm,
        &host_expectations(),
        NoSecretsV1,
    )
    .expect("the packaged component loads through every admission gate");

    let pack = FixturePackV1::load(&declared_fixtures(package))
        .expect("the packaged fixture directory loads as a pack");
    let report = run_provider_component_suite_v1(&SandboxedComponentV1::new(loaded), &pack);

    let failures: Vec<String> = report
        .failures()
        .map(|outcome| format!("{outcome:?}"))
        .collect();
    assert!(
        failures.is_empty(),
        "{package}: {} of {} conformance outcomes failed against its own packaged fixtures:\n{}",
        failures.len(),
        report.outcomes().len(),
        failures.join("\n")
    );
    assert!(
        !report.outcomes().is_empty(),
        "{package}: a suite that ran no cases proves nothing"
    );
}

#[test]
fn the_packaged_anthropic_component_passes_its_packaged_fixtures() {
    run("provider-anthropic-v2", "provider_anthropic_v2");
}

#[test]
fn the_packaged_openai_compatible_component_passes_its_packaged_fixtures() {
    run(
        "provider-openai-compatible-v2",
        "provider_openai_compatible_v2",
    );
}
