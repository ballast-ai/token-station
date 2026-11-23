//! Conformance run against the package as it ships, not against the workspace.
//!
//! `official_package_set` proves a declared fixture directory exists, is
//! non-empty, and pairs its inputs with expectations.
//! `check-provider-fixtures.py` proves the contents still match the South
//! revision they were vendored from, or a repository-authored provenance record.
//! Neither runs the fixtures. A pack can be
//! present, paired, byte-identical to South — and the component beside it can
//! still fail every case, because nothing in this repository had ever executed
//! one against the other.
//!
//! This closes that: the component is built as the package builds it, the pack
//! is loaded from the package's own declared directory, and the suite runs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use south_component_conformance::sandbox::SandboxedComponentV1;
use south_component_conformance::{FixturePackV1, run_provider_component_suite_v1};
use south_provider_runtime::{LoadedComponentV1, NoSecretsV1};
use token_station_cli::south_component::{host_admission, south_component_runtime};

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

/// One registered conformance gap: the check that fails and the case it fails on.
type Gap = (&'static str, &'static str);

/// The usage rows South's suite requires since South 0.43.0 (South #131).
/// The remaining plan components carry none of them. Adding them is not a fixture edit:
/// those components still turn a missing upstream usage report into zeros,
/// which the rows exist to refuse. The plan-component rebuild retires these
/// gaps together with the rows. The two South-sourced components already ship
/// the rows from South 0.85.1 and register nothing.
const USAGE_ROWS_NOT_SHIPPED: [Gap; 5] = [
    ("Coverage", "provider.response.usage"),
    ("Coverage", "provider.response.missing-usage"),
    ("Coverage", "provider.response.cached-usage"),
    ("Coverage", "provider.stream.usage-terminal"),
    ("Coverage", "provider.stream.no-usage"),
];

fn run(package: &str, stem: &str, registered: &[Gap]) {
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
        &host_admission(),
        NoSecretsV1,
    )
    .expect("the packaged component loads through every admission gate");

    let pack = FixturePackV1::load(&declared_fixtures(package))
        .expect("the packaged fixture directory loads as a pack");
    let report = run_provider_component_suite_v1(&SandboxedComponentV1::new(loaded), &pack);
    assert!(
        !report.outcomes().is_empty(),
        "{package}: a suite that ran no cases proves nothing"
    );

    // The registered gaps are exact. An unregistered failure turns this red,
    // and so does a registered gap that starts to pass: retire it then.
    let failing: BTreeSet<(String, String)> = report
        .failures()
        .map(|outcome| (format!("{:?}", outcome.check), outcome.case.clone()))
        .collect();
    let expected: BTreeSet<(String, String)> = registered
        .iter()
        .map(|(check, case)| ((*check).to_owned(), (*case).to_owned()))
        .collect();
    let unregistered: Vec<String> = report
        .failures()
        .filter(|outcome| {
            !expected.contains(&(format!("{:?}", outcome.check), outcome.case.clone()))
        })
        .map(|outcome| format!("{outcome:?}"))
        .collect();
    let retired: Vec<String> = expected
        .difference(&failing)
        .map(|(check, case)| format!("{check} {case}"))
        .collect();
    assert!(
        unregistered.is_empty() && retired.is_empty(),
        "{package}: conformance outcomes differ from the registered gaps\n\
         unregistered failures:\n{}\nregistered gaps that now pass:\n{}",
        unregistered.join("\n"),
        retired.join("\n"),
    );
}

#[test]
fn the_packaged_anthropic_component_passes_the_full_suite() {
    run("provider-anthropic-v2", "provider_anthropic_v2", &[]);
}

#[test]
fn the_packaged_openai_compatible_component_passes_the_full_suite() {
    run(
        "provider-openai-compatible-v2",
        "provider_openai_compatible_v2",
        &[],
    );
}

#[test]
fn the_packaged_chatgpt_plan_component_fails_only_its_registered_gaps() {
    run(
        "provider-chatgpt-plan-v2",
        "provider_chatgpt_plan_v2",
        &USAGE_ROWS_NOT_SHIPPED,
    );
}

#[test]
fn the_packaged_copilot_component_fails_only_its_registered_gaps() {
    run(
        "provider-copilot-plan-v2",
        "provider_copilot_plan_v2",
        &USAGE_ROWS_NOT_SHIPPED,
    );
}

#[test]
fn the_packaged_google_component_passes_the_full_suite() {
    run("provider-codeassist-v2", "provider_codeassist_v2", &[]);
}

#[test]
fn the_packaged_claude_component_passes_the_full_suite() {
    run("provider-claude-plan-v2", "provider_claude_plan_v2", &[]);
}
