use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::config_codec::{ConfigDocument, DocumentFormat};
use super::types::{BaseUrlShape, ConfigPath, PatchOperation, Platform};

include!(concat!(env!("OUT_DIR"), "/builtin_connectors.rs"));

pub use claude_code::ClaudeCodeConnector;
pub use claude_desktop::ClaudeDesktopConnector;
pub use codex::CodexConnector;
pub use hermes::HermesConnector;
pub use openclaw::OpenClawConnector;
pub use opencode::OpenCodeConnector;
pub use workbuddy::WorkBuddyConnector;

pub struct ConnectInput<'a> {
    pub base_url: &'a str,
    pub token: Option<&'a str>,
    pub adapter_ready: bool,
}

/// A second configuration file that must commit with a Connector's primary
/// target. Bytes stay server-side and are zeroized because they may contain a
/// local virtual key.
pub struct CompanionProjection {
    pub target_path: PathBuf,
    pub source_existed: bool,
    pub source_bytes: Zeroizing<Vec<u8>>,
    pub original_permissions: Option<u32>,
    pub original_owner: Option<String>,
    pub projected_bytes: Zeroizing<Vec<u8>>,
    pub format: DocumentFormat,
    pub label: &'static str,
    pub owned_paths: Vec<ConfigPath>,
    pub sensitive_paths: Vec<ConfigPath>,
    pub operations: Vec<PatchOperation>,
}

pub struct ConnectorCapabilities {
    pub connector_id: &'static str,
    pub agent_id: &'static str,
    pub label: &'static str,
    pub adapter_id: &'static str,
    pub base_url_shape: BaseUrlShape,
    pub platforms: &'static [Platform],
    pub config_format: DocumentFormat,
    pub config_path_template: &'static str,
    pub owned_fields: &'static [&'static str],
    pub requires_virtual_key: bool,
    pub restart_required: bool,
}

pub trait Connector: Sync {
    fn capabilities(&self) -> &'static ConnectorCapabilities;
    fn connector_id(&self) -> &'static str {
        self.capabilities().connector_id
    }
    fn agent_id(&self) -> &'static str {
        self.capabilities().agent_id
    }
    fn label(&self) -> &'static str {
        self.capabilities().label
    }
    fn format(&self) -> DocumentFormat {
        self.capabilities().config_format
    }
    fn supports_platform(&self, platform: Platform) -> bool {
        self.capabilities().platforms.contains(&platform)
    }
    fn config_path(&self, home: &Path) -> PathBuf;
    fn create_dir_error(&self) -> &'static str;
    fn owned_paths(&self) -> Vec<ConfigPath>;
    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        Vec::new()
    }
    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String>;
    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String>;
    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String>;
    fn connect_patch_for_document(
        &self,
        _document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<PatchOperation>, String> {
        self.connect_patch(input)
    }
    fn companion_projections(
        &self,
        _primary_target: &Path,
        _input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        Ok(Vec::new())
    }
    fn legacy_companion_format(
        &self,
        _primary_target: &Path,
        _companion_target: &Path,
    ) -> Option<DocumentFormat> {
        None
    }
    fn disconnect_patch(&self) -> Vec<PatchOperation>;
    fn disconnect_patch_for_document(
        &self,
        _document: &ConfigDocument,
    ) -> Result<Vec<PatchOperation>, String> {
        Ok(self.disconnect_patch())
    }
    fn validate_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<(), String>;
    fn success_message(&self, input: &ConnectInput<'_>) -> String;
}

pub fn builtin_connectors() -> &'static [&'static dyn Connector] {
    BUILTIN_CONNECTORS
}

pub fn find_connector(connector_id: &str) -> Option<&'static dyn Connector> {
    BUILTIN_CONNECTORS
        .iter()
        .copied()
        .find(|connector| connector.connector_id() == connector_id)
}

pub(super) fn validate_patch_ownership(
    operations: &[PatchOperation],
    owned_paths: &[ConfigPath],
) -> Result<(), String> {
    for operation in operations {
        let owned = owned_paths.iter().any(|owned| {
            operation.path.segments.len() >= owned.segments.len()
                && operation.path.segments[..owned.segments.len()] == owned.segments
        });
        if !owned {
            return Err(format!("连接器补丁越过受管路径边界：{}", operation.path));
        }
    }
    Ok(())
}

pub(super) fn path(segments: &[&str]) -> ConfigPath {
    ConfigPath {
        segments: segments
            .iter()
            .map(|segment| (*segment).to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent_integration::config_codec::{
        apply_patch, parse_source_bytes, render_document, semantic_json,
    };
    use crate::agent_integration::types::PatchKind;

    #[test]
    fn connector_patch_cannot_escape_owned_paths() {
        let operation = PatchOperation {
            operation: PatchKind::Replace,
            path: path(&["router", "strategy"]),
            value: Some(json!("changed")),
        };

        let error = validate_patch_ownership(&[operation], &[path(&["env"])]).unwrap_err();

        assert!(error.contains("越过受管路径边界"), "{error}");
    }

    #[test]
    fn connector_identities_match_the_builtin_registry_capabilities() {
        assert_eq!(ClaudeCodeConnector.connector_id(), "claude-code-v1");
        assert_eq!(
            ClaudeDesktopConnector.connector_id(),
            "claude-desktop-3p-v1"
        );
        assert_eq!(CodexConnector.connector_id(), "codex-v1");
        assert_eq!(HermesConnector.connector_id(), "hermes-v1");
        assert_eq!(OpenCodeConnector.connector_id(), "opencode-v1");
        assert_eq!(OpenClawConnector.connector_id(), "openclaw-v1");
        assert_eq!(WorkBuddyConnector.connector_id(), "workbuddy-v1");
    }

    #[test]
    fn connector_modules_are_build_registered_without_a_command_match() {
        let ids: Vec<_> = builtin_connectors()
            .iter()
            .map(|connector| connector.capabilities().connector_id)
            .collect();

        assert_eq!(
            ids,
            [
                "claude-code-v1",
                "claude-desktop-3p-v1",
                "codex-v1",
                "gemini-cli-v1",
                "hermes-v1",
                "openclaw-v1",
                "opencode-v1",
                "workbuddy-v1",
            ]
        );
        for id in ids {
            assert_eq!(find_connector(id).unwrap().connector_id(), id);
        }
        assert!(find_connector("future-v1").is_none());
    }

    #[test]
    fn gemini_connector_preserves_unknown_dotenv_and_never_reports_the_virtual_key() {
        let connector = find_connector("gemini-cli-v1").unwrap();
        let source = b"# user comment\nUNKNOWN=keep-me\n";
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/gemini-cli",
            token: Some("vk-gemini-sensitive"),
            adapter_ready: true,
        };
        let baseline =
            parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();
        let mut document =
            parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();
        connector.validate_source(&document).unwrap();
        let patch = connector.connect_patch(&input).unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();
        let rendered = render_document(&document, connector.label()).unwrap();
        assert!(rendered.contains("# user comment"), "{rendered}");
        assert!(rendered.contains("UNKNOWN=keep-me"), "{rendered}");
        assert!(!connector
            .success_message(&input)
            .contains("vk-gemini-sensitive"));

        crate::agent_integration::config_codec::project_owned_paths(
            &mut document,
            &baseline,
            &connector.owned_paths(),
        )
        .unwrap();
        assert_eq!(
            render_document(&document, connector.label()).unwrap(),
            String::from_utf8_lossy(source)
        );
    }

    #[test]
    fn gemini_connection_switches_and_restores_the_auth_mode_in_one_plan() {
        let root = std::env::temp_dir().join(format!(
            "token-station-gemini-companion-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let primary = root.join(".env");
        let settings = root.join("settings.json");
        std::fs::write(
            &settings,
            br#"{"security":{"auth":{"selectedType":"vertex-ai"}},"keep":true}"#,
        )
        .unwrap();
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/gemini-cli",
            token: Some("fixture-virtual-key"),
            adapter_ready: true,
        };

        let companions = find_connector("gemini-cli-v1")
            .unwrap()
            .companion_projections(&primary, &input)
            .unwrap();
        assert_eq!(companions.len(), 1);
        assert_eq!(companions[0].target_path, settings);
        let projected = parse_source_bytes(
            Some(companions[0].projected_bytes.as_slice()),
            DocumentFormat::Json,
            companions[0].label,
        )
        .unwrap();
        let semantic = semantic_json(&projected).unwrap();
        assert_eq!(
            semantic["security"]["auth"]["selectedType"],
            json!("gemini-api-key")
        );
        assert_eq!(semantic["keep"], json!(true));

        let baseline = parse_source_bytes(
            Some(companions[0].source_bytes.as_slice()),
            DocumentFormat::Json,
            companions[0].label,
        )
        .unwrap();
        let mut restored = projected;
        crate::agent_integration::config_codec::project_owned_paths(
            &mut restored,
            &baseline,
            &companions[0].owned_paths,
        )
        .unwrap();
        assert_eq!(
            semantic_json(&restored).unwrap()["security"]["auth"]["selectedType"],
            json!("vertex-ai")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn workbuddy_connector_preserves_unrelated_models_on_connect_and_force_disconnect() {
        let connector = find_connector("workbuddy-v1").expect("WorkBuddy connector is registered");
        let source = br#"{
          "models": [{"id":"user-model","name":"Keep me","url":"http://example.test/v1/chat/completions"}],
          "availableModels": ["user-model"],
          "unknown": {"keep": true}
        }"#;
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-workbuddy-key"),
            adapter_ready: true,
        };
        let mut document =
            parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();
        connector.validate_source(&document).unwrap();
        let patch = connector
            .connect_patch_for_document(&document, &input)
            .unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();

        let connected = crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        assert_eq!(connected["models"].as_array().unwrap().len(), 2);
        assert_eq!(connected["models"][0]["id"], json!("user-model"));
        assert_eq!(connected["models"][1]["id"], json!("tokenstation-auto"));
        assert_eq!(connected["unknown"]["keep"], json!(true));

        let disconnect = connector.disconnect_patch_for_document(&document).unwrap();
        validate_patch_ownership(&disconnect, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &disconnect).unwrap();
        let disconnected =
            crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        assert_eq!(disconnected["models"].as_array().unwrap().len(), 1);
        assert_eq!(disconnected["models"][0]["id"], json!("user-model"));
        assert_eq!(disconnected["availableModels"], json!(["user-model"]));
        assert_eq!(disconnected["unknown"]["keep"], json!(true));
    }

    #[test]
    fn workbuddy_connector_rejects_conflicting_or_malformed_model_arrays() {
        let connector = find_connector("workbuddy-v1").expect("WorkBuddy connector is registered");
        let conflict = parse_source_bytes(
            Some(br#"{"models":[{"id":"tokenstation-auto"}],"availableModels":[]}"#),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        assert!(connector
            .validate_source(&conflict)
            .unwrap_err()
            .contains("已存在模型"));

        let malformed = parse_source_bytes(
            Some(br#"{"models":{},"availableModels":[]}"#),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        assert!(connector
            .validate_source(&malformed)
            .unwrap_err()
            .contains("必须是数组"));
    }

    #[test]
    fn workbuddy_connector_supports_native_top_level_model_array() {
        let connector = find_connector("workbuddy-v1").expect("WorkBuddy connector is registered");
        let source = br#"[
          {"id":"user-model","name":"Keep me","url":"http://example.test/v1/chat/completions"}
        ]"#;
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-workbuddy-key"),
            adapter_ready: true,
        };
        let mut document =
            parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();
        let patch = connector
            .connect_patch_for_document(&document, &input)
            .unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();
        let connected = crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        assert_eq!(connected.as_array().unwrap().len(), 2);
        assert_eq!(connected[0]["id"], json!("user-model"));
        assert_eq!(connected[1]["id"], json!("tokenstation-auto"));

        let disconnect = connector.disconnect_patch_for_document(&document).unwrap();
        validate_patch_ownership(&disconnect, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &disconnect).unwrap();
        let disconnected =
            crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        assert_eq!(
            disconnected,
            json!([{"id":"user-model","name":"Keep me","url":"http://example.test/v1/chat/completions"}])
        );
    }

    #[test]
    fn opencode_connection_advertises_image_attachments_for_the_auto_route() {
        let connector = find_connector("opencode-v1").unwrap();
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/opencode/v1",
            token: Some("fixture-virtual-key"),
            adapter_ready: true,
        };
        let mut document = parse_source_bytes(None, connector.format(), connector.label()).unwrap();
        apply_patch(&mut document, &connector.connect_patch(&input).unwrap()).unwrap();
        connector.validate_projected(&document, &input).unwrap();

        let projected = crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        let model = &projected["provider"]["tokenstation"]["models"]["auto"];
        assert_eq!(model["attachment"], json!(true));
        assert_eq!(
            model["modalities"],
            json!({
                "input": ["text", "image"],
                "output": ["text"]
            })
        );

        let mut missing_capability = projected;
        missing_capability["provider"]["tokenstation"]["models"]["auto"]
            .as_object_mut()
            .unwrap()
            .remove("attachment");
        let stale = ConfigDocument::Json(missing_capability);
        assert!(
            connector.validate_projected(&stale, &input).is_err(),
            "a legacy text-only projection must require a safe reconnect"
        );
    }

    #[test]
    fn connector_fixture_matrix_preserves_unknown_fields_and_handles_missing_or_invalid_config() {
        type ConnectorFixture<'a> = (&'a dyn Connector, &'a [u8], &'a str, &'a str, &'a str);
        let fixtures: [ConnectorFixture<'_>; 4] = [
            (
                &ClaudeCodeConnector,
                include_bytes!("../../../tests/fixtures/config/claude-code/settings.input.json"),
                "KEEP",
                r#"{"env":[]}"#,
                "env 必须是对象",
            ),
            (
                &CodexConnector,
                include_bytes!("../../../tests/fixtures/config/codex/config.input.toml"),
                "# keep root comment",
                "model_providers = []\n",
                "model_providers 必须是表",
            ),
            (
                &OpenCodeConnector,
                include_bytes!("../../../tests/fixtures/config/opencode/opencode.input.json"),
                "existing",
                r#"{"provider":[]}"#,
                "provider 必须是对象",
            ),
            (
                &HermesConnector,
                include_bytes!("../../../tests/fixtures/config/hermes/config.input.yaml"),
                "unknown_model_setting: keep-me",
                "model: []\n",
                "model 必须是对象",
            ),
        ];
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-secret"),
            adapter_ready: true,
        };

        for (connector, existing, unowned_marker, invalid_shape, expected_error) in fixtures {
            for bytes in [None, Some(existing)] {
                let mut document =
                    parse_source_bytes(bytes, connector.format(), connector.label()).unwrap();
                connector.validate_source(&document).unwrap();
                let patch = connector.connect_patch(&input).unwrap();
                validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
                apply_patch(&mut document, &patch).unwrap();
                connector.validate_projected(&document, &input).unwrap();
                let rendered = render_document(&document, connector.label()).unwrap();
                if bytes.is_some() {
                    assert!(rendered.contains(unowned_marker), "{rendered}");
                }
            }

            let malformed_bytes = match connector.format() {
                DocumentFormat::Json => b"{invalid-json".as_slice(),
                DocumentFormat::Json5 => b"{invalid-json5".as_slice(),
                DocumentFormat::Toml => b"invalid = [".as_slice(),
                DocumentFormat::Yaml => b"model:\n  broken: [\n".as_slice(),
                DocumentFormat::Dotenv => b"BROKEN".as_slice(),
            };
            let malformed =
                parse_source_bytes(Some(malformed_bytes), connector.format(), connector.label())
                    .err()
                    .expect("syntax errors are rejected");
            assert!(!malformed.contains("invalid-json"), "{malformed}");

            let document = parse_source_bytes(
                Some(invalid_shape.as_bytes()),
                connector.format(),
                connector.label(),
            )
            .unwrap();
            let error = connector.validate_source(&document).unwrap_err();
            assert!(error.contains(expected_error), "{error}");
        }
    }

    #[test]
    fn connector_contract_matrix_covers_metadata_preconditions_projection_and_disconnect() {
        let home = Path::new("/fixture/home");
        let good = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-virtual-key"),
            adapter_ready: true,
        };
        let wrong = ConnectInput {
            base_url: "http://127.0.0.1:9999/v1",
            token: Some("wrong-key"),
            adapter_ready: true,
        };
        let not_ready = ConnectInput {
            base_url: good.base_url,
            token: good.token,
            adapter_ready: false,
        };
        let missing_token = ConnectInput {
            base_url: good.base_url,
            token: None,
            adapter_ready: true,
        };
        let connectors: [&dyn Connector; 6] = [
            &ClaudeCodeConnector,
            &CodexConnector,
            &HermesConnector,
            &OpenClawConnector,
            &OpenCodeConnector,
            &WorkBuddyConnector,
        ];

        for connector in connectors {
            assert!(!connector.agent_id().is_empty());
            assert!(!connector.label().is_empty());
            assert!(connector.config_path(home).starts_with(home));
            assert!(!connector.create_dir_error().is_empty());
            assert!(!connector.owned_paths().is_empty());
            // The number of owned_fields entries must match the paths actually
            // owned by owned_paths(). Misalignment causes ownership records,
            // restoration, and UI display to omit or invent fields, as seen in
            // the July 2026 OpenCode phantom model and missing Codex model bugs.
            // Connectors may format field strings differently, so validate only
            // the count rather than the strings themselves.
            assert_eq!(
                connector.capabilities().owned_fields.len(),
                connector.owned_paths().len(),
                "{} 的 owned_fields 与 owned_paths() 覆盖数量不一致",
                connector.connector_id()
            );
            assert!(connector
                .capabilities()
                .owned_fields
                .iter()
                .all(|field| !field.is_empty()));
            assert!(connector.sensitive_paths().iter().all(|sensitive| connector
                .owned_paths()
                .iter()
                .any(|owned| { sensitive.segments.starts_with(&owned.segments) })));
            assert!(connector.validate_preconditions(&not_ready).is_err());
            assert!(connector.validate_preconditions(&good).is_ok());
            if connector.capabilities().requires_virtual_key {
                assert!(connector.validate_preconditions(&missing_token).is_err());
            } else {
                assert!(connector.validate_preconditions(&missing_token).is_ok());
            }

            let mut document =
                parse_source_bytes(None, connector.format(), connector.label()).unwrap();
            connector.validate_source(&document).unwrap();
            let patch = connector.connect_patch(&good).unwrap();
            validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
            apply_patch(&mut document, &patch).unwrap();
            connector.validate_projected(&document, &good).unwrap();
            assert!(connector.validate_projected(&document, &wrong).is_err());

            let disconnect = connector.disconnect_patch();
            assert!(!disconnect.is_empty());
            assert!(disconnect
                .iter()
                .all(|operation| operation.operation == PatchKind::Remove
                    && operation.value.is_none()));
            validate_patch_ownership(&disconnect, &connector.owned_paths()).unwrap();
            let success = connector.success_message(&good);
            assert!(!success.contains("fixture-virtual-key"));

            if connector.capabilities().requires_virtual_key {
                assert!(connector.connect_patch(&missing_token).is_err());
            }
        }

        let json = parse_source_bytes(None, DocumentFormat::Json, "fixture").unwrap();
        let toml = parse_source_bytes(None, DocumentFormat::Toml, "fixture").unwrap();
        assert!(ClaudeCodeConnector.validate_source(&toml).is_err());
        assert!(OpenCodeConnector.validate_source(&toml).is_err());
        assert!(CodexConnector.validate_source(&json).is_err());
    }

    #[test]
    fn codex_connector_accepts_and_preserves_inline_model_provider_tables() {
        let source = br#"model_providers = { existing = { name = "keep-me" } }
unknown = "preserved"
"#;
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("fixture-codex-virtual-key"),
            adapter_ready: true,
        };
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        let mut document = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();

        CodexConnector.validate_source(&document).unwrap();
        apply_patch(
            &mut document,
            &CodexConnector.connect_patch(&input).unwrap(),
        )
        .unwrap();
        CodexConnector
            .validate_projected(&document, &input)
            .unwrap();

        let rendered = render_document(&document, "Codex").unwrap();
        assert!(rendered.contains("keep-me"), "{rendered}");
        assert!(rendered.contains("unknown = \"preserved\""), "{rendered}");
        assert!(rendered.contains("tokenstation"), "{rendered}");

        crate::agent_integration::config_codec::project_owned_paths(
            &mut document,
            &baseline,
            &CodexConnector.owned_paths(),
        )
        .unwrap();
        let restored = render_document(&document, "Codex").unwrap();
        assert!(restored.contains("keep-me"), "{restored}");
        assert!(restored.contains("unknown = \"preserved\""), "{restored}");
        assert!(!restored.contains("tokenstation"), "{restored}");
    }

    #[test]
    fn openclaw_connector_preserves_json5_comments_and_restores_only_owned_paths() {
        let source = include_bytes!("../../../tests/fixtures/config/openclaw/openclaw.input.json5");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-openclaw-secret"),
            adapter_ready: true,
        };
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Json5, "OpenClaw").unwrap();
        let mut connected =
            parse_source_bytes(Some(source), DocumentFormat::Json5, "OpenClaw").unwrap();
        let patch = OpenClawConnector.connect_patch(&input).unwrap();
        apply_patch(&mut connected, &patch).unwrap();
        OpenClawConnector
            .validate_projected(&connected, &input)
            .unwrap();
        let first = render_document(&connected, "OpenClaw").unwrap();
        assert!(first.contains("keep this comment"), "{first}");
        assert!(first.contains("unknown Token Station field"), "{first}");
        assert!(first.contains("existing.invalid"), "{first}");

        apply_patch(&mut connected, &patch).unwrap();
        assert_eq!(render_document(&connected, "OpenClaw").unwrap(), first);
        crate::agent_integration::config_codec::project_owned_paths(
            &mut connected,
            &baseline,
            &OpenClawConnector.owned_paths(),
        )
        .unwrap();
        let restored = render_document(&connected, "OpenClaw").unwrap();
        assert!(restored.contains("keep this comment"), "{restored}");
        assert!(restored.contains("existing.invalid"), "{restored}");
        let semantic = crate::agent_integration::config_codec::semantic_json(&connected).unwrap();
        assert!(semantic.pointer("/models/providers/tokenstation").is_none());
        assert!(semantic.pointer("/agents/defaults/model/primary").is_none());

        let missing = parse_source_bytes(None, DocumentFormat::Json5, "OpenClaw").unwrap();
        OpenClawConnector.validate_source(&missing).unwrap();
        let invalid =
            include_bytes!("../../../tests/fixtures/config/openclaw/openclaw.invalid.json5");
        assert!(parse_source_bytes(Some(invalid), DocumentFormat::Json5, "OpenClaw").is_err());
        let included = parse_source_bytes(
            Some(b"{ models: { $include: './models.json5' } }"),
            DocumentFormat::Json5,
            "OpenClaw",
        )
        .unwrap();
        let error = OpenClawConnector.validate_source(&included).unwrap_err();
        assert!(error.contains("$include"), "{error}");
    }

    #[test]
    fn hermes_connector_preserves_yaml_comments_and_restores_only_owned_paths() {
        let source = include_bytes!("../../../tests/fixtures/config/hermes/config.input.yaml");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-hermes-secret"),
            adapter_ready: true,
        };
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Yaml, "Hermes").unwrap();
        let mut connected =
            parse_source_bytes(Some(source), DocumentFormat::Yaml, "Hermes").unwrap();
        let patch = HermesConnector.connect_patch(&input).unwrap();
        apply_patch(&mut connected, &patch).unwrap();
        HermesConnector
            .validate_projected(&connected, &input)
            .unwrap();
        let first = render_document(&connected, "Hermes").unwrap();
        assert!(first.contains("# keep Hermes root comment"), "{first}");
        assert!(first.contains("# keep model comment"), "{first}");
        assert!(first.contains("unknown_model_setting: keep-me"), "{first}");
        assert!(
            first.contains("skin: default # keep unrelated comment"),
            "{first}"
        );

        apply_patch(&mut connected, &patch).unwrap();
        assert_eq!(render_document(&connected, "Hermes").unwrap(), first);
        crate::agent_integration::config_codec::project_owned_paths(
            &mut connected,
            &baseline,
            &HermesConnector.owned_paths(),
        )
        .unwrap();
        let restored = render_document(&connected, "Hermes").unwrap();
        assert!(
            restored.contains("unknown_model_setting: keep-me"),
            "{restored}"
        );
        let semantic = crate::agent_integration::config_codec::semantic_json(&connected).unwrap();
        assert_eq!(semantic["model"]["default"], "original/model");
        assert_eq!(semantic["model"]["provider"], "original-provider");
        assert_eq!(semantic["model"]["api_key"], "original-fixture-key");

        let missing = parse_source_bytes(None, DocumentFormat::Yaml, "Hermes").unwrap();
        HermesConnector.validate_source(&missing).unwrap();
        let invalid = include_bytes!("../../../tests/fixtures/config/hermes/config.invalid.yaml");
        assert!(parse_source_bytes(Some(invalid), DocumentFormat::Yaml, "Hermes").is_err());
    }
}
