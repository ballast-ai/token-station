//! `manifest.json` is a security declaration written by a third party. These
//! tests cover what happens when it is written *wrong* — which, for a package
//! format, is the interesting half.

use token_station_plugin_api::{AdapterKind, AdapterManifest};

const VALID: &str = r#"{
  "name": "provider-openai-compatible",
  "version": "1.0.0",
  "kind": "provider-adapter",
  "api_version": "provider-adapter-v1",
  "providers": ["openai-compatible"],
  "capabilities": ["chat", "stream"],
  "permissions": { "network": false, "filesystem": false, "secrets": ["provider_api_key"] },
  "conformance": { "required_suite": "provider-protocol-v1", "fixtures": "fixtures/" }
}"#;

fn with_permissions_block_named(key: &str) -> String {
    VALID.replace("\"permissions\":", &format!("\"{key}\":"))
}

#[test]
fn a_well_formed_manifest_parses_and_validates() {
    let manifest: AdapterManifest = serde_json::from_str(VALID).expect("valid manifest");

    assert_eq!(manifest.kind, AdapterKind::Provider);
    assert_eq!(manifest.validate(), Ok(()));
}

#[test]
fn a_misspelled_permissions_block_is_refused_rather_than_defaulted() {
    // The hazard: with `serde(flatten)` or a defaulted `permissions`, this would
    // deserialize into an all-false permission set and validate clean. The
    // registry would then hold a record saying the package asked for nothing,
    // and canary, rollback and audit would all reason about that record.
    let misspelled = with_permissions_block_named("permissoins");

    let parsed: Result<AdapterManifest, _> = serde_json::from_str(&misspelled);

    assert!(
        parsed.is_err(),
        "an unknown manifest key must not be silently ignored"
    );
}

#[test]
fn an_absent_permissions_block_is_refused() {
    let without = VALID.replace(
        "\"permissions\": { \"network\": false, \"filesystem\": false, \"secrets\": [\"provider_api_key\"] },\n  ",
        "",
    );

    let parsed: Result<AdapterManifest, _> = serde_json::from_str(&without);

    assert!(
        parsed.is_err(),
        "a package must state its permissions, not inherit them"
    );
}

#[test]
fn an_unknown_permission_is_refused() {
    let smuggled = VALID.replace(
        "\"network\": false",
        "\"network\": false, \"raw_sockets\": true",
    );

    let parsed: Result<AdapterManifest, _> = serde_json::from_str(&smuggled);

    assert!(parsed.is_err(), "the sandbox grants a closed set");
}

#[test]
fn an_unknown_top_level_field_is_refused() {
    let extended = VALID.replace("\"name\":", "\"trusted\": true,\n  \"name\":");

    let parsed: Result<AdapterManifest, _> = serde_json::from_str(&extended);

    assert!(
        parsed.is_err(),
        "a new manifest field goes through a version bump, like the ABI"
    );
}

#[test]
fn kind_is_spelled_as_the_architecture_spells_it() {
    let manifest: AdapterManifest = serde_json::from_str(VALID).expect("valid manifest");
    let json = serde_json::to_value(&manifest).expect("serializable manifest");

    assert_eq!(json["kind"], serde_json::json!("provider-adapter"));
    assert_eq!(
        json["api_version"],
        serde_json::json!("provider-adapter-v1")
    );
}

#[test]
fn an_unknown_capability_is_a_version_mismatch() {
    let future = VALID.replace(
        r#""capabilities": ["chat", "stream"]"#,
        r#""capabilities": ["chat", "audio"]"#,
    );

    let parsed: Result<AdapterManifest, _> = serde_json::from_str(&future);

    assert!(
        parsed.is_err(),
        "the host dispatches on capabilities; an unknown one cannot be ignored"
    );
}
