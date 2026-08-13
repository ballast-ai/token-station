use serde_json::json;
use token_station_desktop_lib::agent_integration::config_codec::{
    apply_patch, parse_rendered, prepare_owned_paths_for_write, project_owned_paths,
    render_document, semantic_json, DocumentFormat,
};
use token_station_desktop_lib::agent_integration::connectors::{
    ConnectInput, Connector, HermesConnector,
};
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

#[test]
fn hermes_reuses_every_safe_empty_model_spelling_without_losing_comments() {
    let input = ConnectInput {
        base_url: "http://127.0.0.1:8787/agents/nous-hermes-agent/v1",
        token: Some("fixture-empty-parent-secret"),
        adapter_ready: true,
        model_metadata: None,
    };
    for spelling in ["", "null", "~", "{}"] {
        let source = format!(
            "# keep root comment\nmodel: {spelling} # keep model comment\nfallback_providers: [] # keep sibling\n"
        );
        let mut document = parse_rendered(&source, DocumentFormat::Yaml, "Hermes").unwrap();
        prepare_owned_paths_for_write(&mut document, &HermesConnector.owned_paths()).unwrap();
        HermesConnector.validate_source(&document).unwrap();
        apply_patch(
            &mut document,
            &HermesConnector.connect_patch(&input).unwrap(),
        )
        .unwrap();
        HermesConnector
            .validate_projected(&document, &input)
            .unwrap();

        let rendered = render_document(&document, "Hermes").unwrap();
        assert!(rendered.starts_with("# keep root comment\n"), "{rendered}");
        assert!(rendered.contains("# keep model comment"), "{rendered}");
        assert!(
            rendered.contains("fallback_providers: [] # keep sibling"),
            "{rendered}"
        );
        assert_eq!(
            semantic_json(&document).unwrap()["model"]["provider"],
            "custom"
        );
    }
}

#[test]
fn hermes_reuses_safe_empty_model_spellings_at_eof_without_a_newline() {
    for source in ["model:", "model: null", "model: ~", "model: {}"] {
        let mut document = parse_rendered(source, DocumentFormat::Yaml, "Hermes").unwrap();
        prepare_owned_paths_for_write(&mut document, &HermesConnector.owned_paths()).unwrap();
        let operations = HermesConnector
            .connect_patch(&ConnectInput {
                base_url: "http://127.0.0.1:8787/v1",
                token: Some("fixture-secret"),
                adapter_ready: true,
                model_metadata: None,
            })
            .unwrap();

        apply_patch(&mut document, &operations).unwrap();

        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["model"]["provider"], "custom", "{source}");
    }
}

#[test]
fn hermes_keeps_ambiguous_model_shapes_blocked() {
    for source in [
        "model: []\n",
        "model: auto\n",
        "model: {keep: true}\n",
        "model: &empty {}\n",
        "model: !!map {}\n",
    ] {
        let mut document = parse_rendered(source, DocumentFormat::Yaml, "Hermes").unwrap();
        let _ = prepare_owned_paths_for_write(&mut document, &HermesConnector.owned_paths());
        assert!(
            HermesConnector.validate_source(&document).is_err()
                || HermesConnector
                    .connect_patch(&ConnectInput {
                        base_url: "http://127.0.0.1:8787/v1",
                        token: Some("fixture-secret"),
                        adapter_ready: true,
                        model_metadata: None,
                    })
                    .and_then(|operations| apply_patch(&mut document, &operations))
                    .is_err(),
            "unsafe model shape must remain blocked: {source}"
        );
    }
}

#[test]
fn yaml_reverse_patch_can_remove_a_parent_created_by_connection() {
    let mut document = parse_rendered(
        "# keep root comment\nfallback_providers: [] # keep sibling\n",
        DocumentFormat::Yaml,
        "Hermes",
    )
    .unwrap();
    let reverse =
        token_station_desktop_lib::agent_integration::config_codec::apply_patch_with_reverse(
            &mut document,
            &[PatchOperation {
                operation: PatchKind::Replace,
                path: ConfigPath {
                    segments: vec!["model".to_string(), "default".to_string()],
                },
                value: Some(json!("auto")),
            }],
        )
        .unwrap();

    apply_patch(&mut document, &reverse).unwrap();

    let rendered = render_document(&document, "Hermes").unwrap();
    assert_eq!(
        rendered,
        "# keep root comment\nfallback_providers: [] # keep sibling\n"
    );
}

#[test]
fn yaml_parent_reverse_preserves_following_root_comments_and_blank_lines() {
    let source = "keep_before: true\n# keep comment before insertion\n\nkeep_after: true\n";
    let mut document = parse_rendered(source, DocumentFormat::Yaml, "Hermes").unwrap();
    let reverse =
        token_station_desktop_lib::agent_integration::config_codec::apply_patch_with_reverse(
            &mut document,
            &[PatchOperation {
                operation: PatchKind::Replace,
                path: ConfigPath {
                    segments: vec!["model".to_string(), "default".to_string()],
                },
                value: Some(json!("auto")),
            }],
        )
        .unwrap();

    apply_patch(&mut document, &reverse).unwrap();

    assert_eq!(render_document(&document, "Hermes").unwrap(), source);
}

#[test]
fn yaml_parent_reverse_removes_internal_trivia_without_orphaning_children() {
    let source = "model:\n  default: auto\n\n# managed block note\n  provider: custom\n# keep sibling comment\n\nfallback_providers: []\n";
    let mut document = parse_rendered(source, DocumentFormat::Yaml, "Hermes").unwrap();

    apply_patch(
        &mut document,
        &[PatchOperation {
            operation: PatchKind::Remove,
            path: ConfigPath {
                segments: vec!["model".to_string()],
            },
            value: None,
        }],
    )
    .unwrap();

    assert_eq!(
        render_document(&document, "Hermes").unwrap(),
        "# keep sibling comment\n\nfallback_providers: []\n"
    );
}
