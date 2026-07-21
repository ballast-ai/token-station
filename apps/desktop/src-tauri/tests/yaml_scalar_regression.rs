use serde_json::json;
use token_station_desktop_lib::agent_integration::config_codec::{
    apply_patch, parse_rendered, project_owned_paths, render_document, semantic_json,
    DocumentFormat,
};
use token_station_desktop_lib::agent_integration::connectors::{Connector, HermesConnector};
use token_station_desktop_lib::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

#[test]
fn yaml_replacement_quotes_reserved_string_indicators() {
    let mut document = parse_rendered(
        "model:\n  api_key: current # keep api key comment\n",
        DocumentFormat::Yaml,
        "Hermes",
    )
    .unwrap();
    let operation = PatchOperation {
        operation: PatchKind::Replace,
        path: ConfigPath {
            segments: vec!["model".to_string(), "api_key".to_string()],
        },
        value: Some(json!("@keyring-reference")),
    };

    apply_patch(&mut document, &[operation]).unwrap();

    let rendered = render_document(&document, "Hermes").unwrap();
    assert!(
        !rendered.contains("api_key: @keyring-reference"),
        "{rendered}"
    );
    assert!(rendered.contains("# keep api key comment"), "{rendered}");
    assert_eq!(
        semantic_json(&document).unwrap()["model"]["api_key"],
        "@keyring-reference"
    );
}

#[test]
fn hermes_disconnect_projection_restores_null_and_preserves_adjacent_root_yaml() {
    let baseline = parse_rendered(
        "model:\n  default: original/model\n  provider: original\n  base_url: https://original.invalid/v1\n  api_key: null\n  api_mode: chat_completions\nagent:\n  max_turns: 50\n",
        DocumentFormat::Yaml,
        "Hermes baseline",
    )
    .unwrap();
    let mut current = parse_rendered(
        "# keep root comment\nmodel:\n  default: auto\n  provider: custom\n  base_url: http://127.0.0.1:8787/v1\n  api_key: vk-managed\n  api_mode: chat_completions\nfallback_providers: [] # keep adjacent root field\nagent:\n  max_turns: 150\n",
        DocumentFormat::Yaml,
        "Hermes",
    )
    .unwrap();

    project_owned_paths(&mut current, &baseline, &HermesConnector.owned_paths()).unwrap();

    let rendered = render_document(&current, "Hermes").unwrap();
    assert!(rendered.starts_with("# keep root comment\n"), "{rendered}");
    assert!(rendered.contains("fallback_providers: [] # keep adjacent root field"));
    assert!(rendered.contains("agent:\n  max_turns: 150"), "{rendered}");
    let semantic = semantic_json(&current).unwrap();
    assert_eq!(semantic["model"]["default"], "original/model");
    assert_eq!(semantic["model"]["provider"], "original");
    assert!(semantic["model"]["api_key"].is_null());
    assert_eq!(semantic["fallback_providers"], json!([]));
}

#[test]
fn hermes_disconnect_projection_removes_missing_owned_fields_only() {
    let baseline = parse_rendered(
        "model:\n  user_field: baseline-must-not-replace-current\n",
        DocumentFormat::Yaml,
        "Hermes baseline",
    )
    .unwrap();
    let mut current = parse_rendered(
        "model:\n  default: auto\n  provider: custom\n  base_url: http://127.0.0.1:8787/v1\n  api_key: vk-managed\n  api_mode: chat_completions\n  user_field: keep-current # keep nested comment\nagent:\n  max_turns: 150\n",
        DocumentFormat::Yaml,
        "Hermes",
    )
    .unwrap();

    project_owned_paths(&mut current, &baseline, &HermesConnector.owned_paths()).unwrap();

    let rendered = render_document(&current, "Hermes").unwrap();
    assert!(rendered.contains("user_field: keep-current # keep nested comment"));
    assert!(rendered.contains("agent:\n  max_turns: 150"));
    let semantic = semantic_json(&current).unwrap();
    for field in ["default", "provider", "base_url", "api_key", "api_mode"] {
        assert!(semantic["model"].get(field).is_none(), "{rendered}");
    }
    assert_eq!(semantic["model"]["user_field"], "keep-current");
}
