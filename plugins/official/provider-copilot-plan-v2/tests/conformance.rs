use provider_copilot_plan_v2::CopilotPlan;
use south_component_conformance::{
    accepts_manifest, reported_identity_matches, run_provider_component_suite_v1, FixturePackV1,
    ProviderComponentV1,
};

#[test]
fn package_identity_and_all_south_fixture_gates_pass() {
    let manifest: south_provider_api::ComponentManifestV1 =
        serde_json::from_str(include_str!("../manifest.json")).unwrap();
    accepts_manifest(&manifest).unwrap();
    assert!(reported_identity_matches(
        &CopilotPlan.metadata(),
        &manifest
    ));
    let pack =
        FixturePackV1::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures"))
            .unwrap();
    let report = run_provider_component_suite_v1(&CopilotPlan, &pack);
    assert!(report.is_passing(), "{report}");
}
