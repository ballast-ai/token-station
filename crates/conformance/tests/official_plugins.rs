//! The official adapters are held to the same gate as a third-party package.
//!
//! The architecture is explicit that official plugins get no private path in.
//! If these manifests could not pass, the gate would be describing a standard
//! nobody meets, including us.

use token_station_conformance::accepts_manifest;
use token_station_plugin_api::{AdapterKind, AdapterManifest, Capability};

const AGENT_OPENAI: &str = include_str!("../../../plugins/official/agent-openai/manifest.json");
const AGENT_OPENAI_RESPONSES: &str =
    include_str!("../../../plugins/official/agent-openai-responses/manifest.json");
const AGENT_ANTHROPIC: &str =
    include_str!("../../../plugins/official/agent-anthropic/manifest.json");
const PROVIDER_OPENAI_COMPATIBLE: &str =
    include_str!("../../../plugins/official/provider-openai-compatible/manifest.json");

fn parse(source: &str) -> AdapterManifest {
    serde_json::from_str(source).expect("official manifest must match the schema")
}

#[test]
fn official_agent_adapter_passes_the_manifest_gate() {
    for (source, protocol) in [
        (AGENT_OPENAI, "openai-chat-completions"),
        (AGENT_OPENAI_RESPONSES, "openai-responses"),
        (AGENT_ANTHROPIC, "anthropic-messages"),
    ] {
        let manifest = parse(source);

        assert_eq!(accepts_manifest(&manifest), Ok(()));
        assert_eq!(manifest.kind, AdapterKind::Agent);
        assert_eq!(manifest.agent_protocols, [protocol]);
        assert!(manifest.capabilities.contains(&Capability::AgentHint));
        assert!(
            manifest.permissions.secrets.is_empty(),
            "an agent adapter never sees a credential"
        );
    }
}

#[test]
fn official_provider_adapter_passes_the_manifest_gate() {
    let manifest = parse(PROVIDER_OPENAI_COMPATIBLE);

    assert_eq!(accepts_manifest(&manifest), Ok(()));
    assert_eq!(manifest.kind, AdapterKind::Provider);
    assert_eq!(manifest.providers, ["openai-compatible"]);
    assert_eq!(manifest.permissions.secrets, ["provider_api_key"]);
    assert!(
        !manifest.capabilities.contains(&Capability::AgentHint),
        "a provider adapter never sees an inbound request"
    );
}

#[test]
fn openai_agent_manifests_do_not_overclaim_structured_output() {
    for source in [AGENT_OPENAI, AGENT_OPENAI_RESPONSES] {
        let manifest = parse(source);

        assert!(
            !manifest.capabilities.contains(&Capability::JsonSchema),
            "{} does not translate structured output into Canonical IR",
            manifest.name
        );
    }
}

#[test]
fn manifest_name_matches_the_directory_that_holds_it() {
    // The registry keys packages by name; a package whose directory says one
    // thing and whose manifest says another cannot be located again after
    // install.
    assert_eq!(parse(AGENT_OPENAI).name, "agent-openai");
    assert_eq!(parse(AGENT_OPENAI_RESPONSES).name, "agent-openai-responses");
    assert_eq!(parse(AGENT_ANTHROPIC).name, "agent-anthropic");
    assert_eq!(
        parse(PROVIDER_OPENAI_COMPATIBLE).name,
        "provider-openai-compatible"
    );
}

#[test]
fn official_manifests_round_trip_exactly() {
    // Same discipline as the protocol fixtures: pin the wire format, so a field
    // that stops serializing fails here rather than in a third-party package
    // written against a stale schema.
    for source in [
        AGENT_OPENAI,
        AGENT_OPENAI_RESPONSES,
        AGENT_ANTHROPIC,
        PROVIDER_OPENAI_COMPATIBLE,
    ] {
        let expected: serde_json::Value =
            serde_json::from_str(source).expect("manifest is valid JSON");
        let parsed: AdapterManifest =
            serde_json::from_value(expected.clone()).expect("manifest matches the schema");
        let actual = serde_json::to_value(&parsed).expect("manifest is serializable");

        assert_eq!(actual, expected);
    }
}
