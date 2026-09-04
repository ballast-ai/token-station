use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::config_codec::{ConfigDocument, DocumentFormat};
use super::types::{BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform};

include!(concat!(env!("OUT_DIR"), "/builtin_connectors.rs"));

pub use claude_code::ClaudeCodeConnector;
pub use claude_desktop::ClaudeDesktopConnector;
pub use codex::CodexConnector;
pub use hermes::HermesConnector;
pub use openclaw::OpenClawConnector;
pub use opencode::OpenCodeConnector;
pub use workbuddy::WorkBuddyConnector;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct AgentModelCost {
    pub input: f64,
    pub output: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl AgentModelCost {
    pub fn is_valid(&self) -> bool {
        [
            Some(self.input),
            Some(self.output),
            self.cache_read,
            self.cache_write,
        ]
        .into_iter()
        .flatten()
        .all(|rate| rate.is_finite() && (0.0..=9_000_000_000.0).contains(&rate))
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AgentModelMetadata {
    pub context: u32,
    pub output: u32,
    pub vision: bool,
    pub tools: bool,
    pub reasoning: bool,
    pub cost: Option<AgentModelCost>,
}

pub const OPENCODE_SAFE_DEFAULT_OUTPUT_TOKENS: u32 = 8_192;

impl AgentModelMetadata {
    pub fn safe_limits(&self) -> Option<(u32, u32)> {
        (self.context > 0 && self.output > 0 && self.output < self.context)
            .then_some((self.context, self.output))
    }

    pub fn opencode_limits(&self) -> Option<(u32, u32)> {
        let output = if self.output == 0 {
            OPENCODE_SAFE_DEFAULT_OUTPUT_TOKENS
        } else {
            self.output
        };
        (self.context > 0 && output < self.context).then_some((self.context, output))
    }
}

pub struct ConnectInput<'a> {
    pub base_url: &'a str,
    pub token: Option<&'a str>,
    pub adapter_ready: bool,
    pub model_metadata: Option<&'a AgentModelMetadata>,
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
    /// Paths owned only by an earlier Connector version. New connections do
    /// not claim them. Refresh and force-forget can clean up matching legacy
    /// values while an old ownership record still names the paths.
    fn legacy_owned_paths(&self) -> Vec<ConfigPath> {
        Vec::new()
    }
    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        Vec::new()
    }
    fn projects_model_metadata(&self) -> bool {
        false
    }
    fn refreshes_managed_configuration(&self) -> bool {
        self.projects_model_metadata()
    }
    fn refresh_requires_baseline(&self, _owned_paths: &[ConfigPath]) -> bool {
        false
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
    fn validate_refresh_source(&self, document: &ConfigDocument) -> Result<(), String> {
        self.validate_source(document)
    }
    fn refresh_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        _owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        self.connect_patch_for_document(document, input)
    }
    fn refresh_patch_with_baseline(
        &self,
        document: &ConfigDocument,
        _baseline: Option<&ConfigDocument>,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        self.refresh_patch_for_document(document, input, owned_paths)
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
    fn disconnect_companion_patch_for_document(
        &self,
        _primary_target: &Path,
        _companion_target: &Path,
        _document: &ConfigDocument,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        Ok(owned_paths
            .iter()
            .cloned()
            .map(|path| PatchOperation {
                operation: PatchKind::Remove,
                path,
                value: None,
            })
            .collect())
    }
    fn project_owned_document(
        &self,
        current: &mut ConfigDocument,
        source: &ConfigDocument,
        owned_paths: &[ConfigPath],
    ) -> Result<(), String> {
        crate::agent_integration::config_codec::project_owned_paths(current, source, owned_paths)
    }
    fn validate_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<(), String>;
    fn validate_refresh_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        _owned_paths: &[ConfigPath],
    ) -> Result<(), String> {
        self.validate_projected(document, input)
    }
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

pub(super) fn owned_paths_with_legacy(connector: &dyn Connector) -> Vec<ConfigPath> {
    let mut paths = connector.owned_paths();
    paths.extend(connector.legacy_owned_paths());
    paths
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
        apply_patch, parse_rendered, parse_source_bytes, prepare_owned_paths_for_write,
        render_document, semantic_json,
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
        assert_eq!(
            find_connector("grok-build-v1").unwrap().agent_id(),
            "grok-build"
        );
        assert_eq!(
            find_connector("kimi-code-v1").unwrap().agent_id(),
            "kimi-code"
        );
        assert_eq!(
            find_connector("deepseek-harness-v1").unwrap().agent_id(),
            "deepseek-harness"
        );
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
                "deepseek-harness-v1",
                "gemini-cli-v1",
                "grok-build-v1",
                "hermes-v1",
                "kimi-code-v1",
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
    fn every_builtin_connector_can_connect_disconnect_and_reconnect() {
        let fixtures: [(&str, &[u8], &str); 11] = [
            (
                "claude-code-v1",
                br#"{"env":null,"keep":"claude-code"}"#,
                "claude-code",
            ),
            (
                "claude-desktop-3p-v1",
                br#"{"keep":"claude-desktop"}"#,
                "claude-desktop",
            ),
            ("codex-v1", b"keep = \"codex\"\n", "codex"),
            (
                "deepseek-harness-v1",
                b"# keep DeepSeek comment\nkeep: deepseek-harness\n",
                "deepseek-harness",
            ),
            ("gemini-cli-v1", b"KEEP=gemini\n", "gemini"),
            ("grok-build-v1", b"keep = \"grok-build\"\n", "grok-build"),
            (
                "hermes-v1",
                b"# keep Hermes comment\nmodel:\nkeep: hermes\n",
                "hermes",
            ),
            (
                "openclaw-v1",
                b"{models:null, agents:null, keep:'openclaw'}",
                "openclaw",
            ),
            ("kimi-code-v1", b"keep = \"kimi-code\"\n", "kimi-code"),
            (
                "opencode-v1",
                br#"{"provider":null,"keep":"opencode"}"#,
                "opencode",
            ),
            (
                "workbuddy-v1",
                br#"{"models":[],"availableModels":[],"keep":"workbuddy"}"#,
                "workbuddy",
            ),
        ];
        let metadata = AgentModelMetadata {
            context: 131_072,
            output: 8_192,
            vision: true,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/lifecycle/v1",
            token: Some("fixture-lifecycle-secret"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };

        assert_eq!(fixtures.len(), builtin_connectors().len());
        for (connector_id, source, marker) in fixtures {
            let connector = find_connector(connector_id).expect("fixture connector is registered");
            let owned_paths = connector.owned_paths();
            let mut document =
                parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();

            prepare_owned_paths_for_write(&mut document, &owned_paths).unwrap();
            connector.validate_source(&document).unwrap();
            let connect = connector
                .connect_patch_for_document(&document, &input)
                .unwrap();
            validate_patch_ownership(&connect, &owned_paths).unwrap();
            apply_patch(&mut document, &connect).unwrap();
            connector.validate_projected(&document, &input).unwrap();

            let disconnect = connector.disconnect_patch_for_document(&document).unwrap();
            validate_patch_ownership(&disconnect, &owned_paths).unwrap();
            apply_patch(&mut document, &disconnect).unwrap();
            let disconnected = render_document(&document, connector.label()).unwrap();
            assert!(
                disconnected.contains(marker),
                "{connector_id} must preserve unowned content: {disconnected}"
            );
            assert!(
                !disconnected.contains("fixture-lifecycle-secret"),
                "{connector_id} must remove the managed credential"
            );

            prepare_owned_paths_for_write(&mut document, &owned_paths).unwrap();
            connector.validate_source(&document).unwrap();
            let reconnect = connector
                .connect_patch_for_document(&document, &input)
                .unwrap();
            validate_patch_ownership(&reconnect, &owned_paths).unwrap();
            apply_patch(&mut document, &reconnect).unwrap();
            connector.validate_projected(&document, &input).unwrap();
            assert!(
                render_document(&document, connector.label())
                    .unwrap()
                    .contains(marker),
                "{connector_id} must preserve unowned content after reconnect"
            );
        }
    }

    #[test]
    fn grok_build_connector_adds_a_namespaced_chat_completions_model() {
        let connector =
            find_connector("grok-build-v1").expect("Grok Build connector is registered");
        let metadata = AgentModelMetadata {
            context: 131_072,
            output: 8_192,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/grok-build/v1",
            token: Some("fixture-grok-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(b"[ui]\ncompact_mode = true\n"),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        prepare_owned_paths_for_write(&mut document, &connector.owned_paths()).unwrap();
        let patch = connector.connect_patch(&input).unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();

        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["models"]["default"], json!("tokenstation"));
        assert!(semantic["models"].get("allowed_models").is_none());
        for model in ["tokenstation", "grok-4.6", "grok-4.5"] {
            assert_eq!(semantic["model"][model]["model"], json!("auto"));
            assert_eq!(
                semantic["model"][model]["api_backend"],
                json!("chat_completions")
            );
            assert_eq!(
                semantic["model"][model]["api_key"],
                json!("fixture-grok-key")
            );
            assert_eq!(semantic["model"][model]["context_window"], json!(131_072));
            assert_eq!(
                semantic["model"][model]["max_completion_tokens"],
                json!(8_192)
            );
        }
        assert_eq!(semantic["ui"]["compact_mode"], json!(true));
    }

    #[test]
    fn kimi_code_connector_adds_a_namespaced_openai_provider_and_model() {
        let connector = find_connector("kimi-code-v1").expect("Kimi Code connector is registered");
        let metadata = AgentModelMetadata {
            context: 131_072,
            output: 8_192,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/kimi-code/v1",
            token: Some("fixture-kimi-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(b"[providers.user]\ntype = \"anthropic\"\napi_key = \"keep\"\n"),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        prepare_owned_paths_for_write(&mut document, &connector.owned_paths()).unwrap();
        let patch = connector.connect_patch(&input).unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();

        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["default_model"], json!("tokenstation-auto"));
        assert_eq!(
            semantic["providers"]["tokenstation"]["type"],
            json!("openai")
        );
        assert_eq!(
            semantic["providers"]["tokenstation"]["api_key"],
            json!("fixture-kimi-key")
        );
        assert_eq!(
            semantic["models"]["tokenstation-auto"]["provider"],
            json!("tokenstation")
        );
        assert_eq!(
            semantic["models"]["tokenstation-auto"]["model"],
            json!("auto")
        );
        assert!(
            semantic["models"]["tokenstation-auto"]["max_context_size"]
                .as_u64()
                .is_some_and(|value| value > 0),
            "Kimi Code refuses to start a session without a positive max_context_size"
        );
        assert_eq!(
            semantic["models"]["tokenstation-auto"]["max_output_size"],
            json!(8_192)
        );
        assert_eq!(semantic["providers"]["user"]["api_key"], json!("keep"));
    }

    #[test]
    fn kimi_code_connector_refuses_unknown_context_limits_before_writing() {
        let connector = find_connector("kimi-code-v1").expect("Kimi Code connector is registered");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/kimi-code/v1",
            token: Some("fixture-kimi-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        let error = connector
            .validate_preconditions(&input)
            .expect_err("Kimi Code requires a known positive context limit");

        assert!(error.contains("context"), "{error}");
    }

    #[test]
    fn kimi_code_connector_rejects_invalid_limits_without_overflowing() {
        let connector = find_connector("kimi-code-v1").expect("Kimi Code connector is registered");
        for (context, output) in [(1, 0), (128_000, 128_000), (128_000, 128_001)] {
            let metadata = AgentModelMetadata {
                context,
                output,
                vision: false,
                tools: true,
                reasoning: true,
                cost: None,
            };
            let input = ConnectInput {
                base_url: "http://127.0.0.1:8787/agents/kimi-code/v1",
                token: Some("fixture-kimi-key"),
                adapter_ready: true,
                model_metadata: Some(&metadata),
            };

            connector
                .validate_preconditions(&input)
                .expect_err("invalid context and output limits must fail before writing");
        }

        let metadata = AgentModelMetadata {
            context: u32::MAX,
            output: u32::MAX - 1,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/kimi-code/v1",
            token: Some("fixture-kimi-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };

        connector
            .validate_preconditions(&input)
            .expect("u32 limits remain valid without arithmetic overflow");
    }

    #[test]
    fn kimi_code_connector_accepts_the_cli_bootstrap_config() {
        let connector = find_connector("kimi-code-v1").expect("Kimi Code connector is registered");
        let metadata = AgentModelMetadata {
            context: 131_072,
            output: 8_192,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/kimi-code/v1",
            token: Some("fixture-kimi-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(
                b"\n[providers]\n\n[models]\n# ~/.kimi-code/config.toml\n# Runtime settings for Kimi Code.\n# This file starts empty so built-in defaults can apply.\n# Login will populate managed Kimi provider and model entries.\n",
            ),
            connector.format(),
            connector.label(),
        )
        .unwrap();

        prepare_owned_paths_for_write(&mut document, &connector.owned_paths())
            .expect("Kimi Code's own bootstrap config is safe to extend");
        let patch = connector.connect_patch(&input).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();
    }

    #[test]
    fn deepseek_harness_connector_projects_settings_and_credentials_together() {
        let connector = find_connector("deepseek-harness-v1")
            .expect("DeepSeek Harness connector is registered");
        let root = std::env::temp_dir().join(format!(
            "token-station-deepseek-harness-companion-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let primary = root.join("settings.yaml");
        let credentials = root.join(".credentials.yaml");
        std::fs::write(&credentials, b"USER_API_KEY: keep-me\n").unwrap();
        let metadata = AgentModelMetadata {
            context: 131_072,
            output: 8_192,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/deepseek-harness/v1",
            token: Some("fixture-dsh-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(
                b"# keep DeepSeek comment\nllm-pi-ai:\n  providers:\n    user:\n      api: anthropic-messages\nkeep: deepseek-harness\n",
            ),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        prepare_owned_paths_for_write(&mut document, &connector.owned_paths()).unwrap();
        let patch = connector.connect_patch(&input).unwrap();
        validate_patch_ownership(&patch, &connector.owned_paths()).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();
        let semantic = semantic_json(&document).unwrap();
        assert_eq!(
            semantic["agent-default-model"]["provider"],
            json!("tokenstation")
        );
        assert_eq!(semantic["agent-default-model"]["model"], json!("auto"));
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["tokenstation"]["api"],
            json!("openai-completions")
        );
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["tokenstation"]["apiKeyEnv"],
            json!("TOKENSTATION_API_KEY")
        );
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["tokenstation"]["models"][0]["contextWindow"],
            json!(131_072)
        );
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["tokenstation"]["models"][0]["maxTokens"],
            json!(8_192)
        );
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["user"]["api"],
            json!("anthropic-messages")
        );
        assert!(render_document(&document, connector.label())
            .unwrap()
            .contains("# keep DeepSeek comment"));

        let companions = connector.companion_projections(&primary, &input).unwrap();
        assert_eq!(companions.len(), 1);
        assert_eq!(companions[0].target_path, credentials);
        assert_eq!(
            companions[0].sensitive_paths,
            [path(&["TOKENSTATION_API_KEY"])]
        );
        let projected = parse_source_bytes(
            Some(companions[0].projected_bytes.as_slice()),
            DocumentFormat::Yaml,
            companions[0].label,
        )
        .unwrap();
        let credentials_semantic = semantic_json(&projected).unwrap();
        assert_eq!(credentials_semantic["USER_API_KEY"], json!("keep-me"));
        assert_eq!(
            credentials_semantic["TOKENSTATION_API_KEY"],
            json!("fixture-dsh-key")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deepseek_harness_accepts_the_stock_onboarding_settings_file() {
        let connector = find_connector("deepseek-harness-v1")
            .expect("DeepSeek Harness connector is registered");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/deepseek-harness/v1",
            token: Some("fixture-dsh-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut document = parse_source_bytes(
            Some(b"ui-onboarding:\n  welcomeNoticeVersion: 2026-08-13.1\n"),
            connector.format(),
            connector.label(),
        )
        .expect("the stock DSH settings file is valid YAML");

        connector
            .validate_source(&document)
            .expect("the stock DSH settings file is safe to extend");
        prepare_owned_paths_for_write(&mut document, &connector.owned_paths())
            .expect("the Token Station paths are safe to create");
        let patch = connector.connect_patch(&input).unwrap();
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();

        let semantic = semantic_json(&document).unwrap();
        assert_eq!(
            semantic["ui-onboarding"]["welcomeNoticeVersion"],
            json!("2026-08-13.1")
        );
        assert_eq!(
            semantic["llm-pi-ai"]["providers"]["tokenstation"]["api"],
            json!("openai-completions")
        );
    }

    #[test]
    fn gemini_connector_preserves_unknown_dotenv_and_never_reports_the_virtual_key() {
        let connector = find_connector("gemini-cli-v1").unwrap();
        let source = b"# user comment\nUNKNOWN=keep-me\n";
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/gemini-cli",
            token: Some("vk-gemini-sensitive"),
            adapter_ready: true,
            model_metadata: None,
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
            model_metadata: None,
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
    fn gemini_connection_recovers_null_optional_auth_containers() {
        let root = std::env::temp_dir().join(format!(
            "token-station-gemini-null-companion-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let primary = root.join(".env");
        let settings = root.join("settings.json");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/gemini-cli",
            token: Some("fixture-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        for source in [
            br#"{"security":null,"keep":true}"#.as_slice(),
            br#"{"security":{"auth":null},"keep":true}"#.as_slice(),
        ] {
            std::fs::write(&settings, source).unwrap();
            let companions = find_connector("gemini-cli-v1")
                .unwrap()
                .companion_projections(&primary, &input)
                .expect("null optional auth containers recover");
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
        }
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
            model_metadata: None,
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
    fn workbuddy_connector_recovers_null_optional_model_arrays() {
        let connector = find_connector("workbuddy-v1").expect("WorkBuddy connector is registered");
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-workbuddy-key"),
            adapter_ready: true,
            model_metadata: Some(&AgentModelMetadata {
                context: 257_550,
                output: 32_768,
                vision: true,
                tools: true,
                reasoning: true,
                cost: None,
            }),
        };
        let mut document = parse_source_bytes(
            Some(br#"{"models":null,"availableModels":null,"keep":true}"#),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        let patch = connector
            .connect_patch_for_document(&document, &input)
            .expect("null optional arrays are empty collections");
        apply_patch(&mut document, &patch).unwrap();
        connector.validate_projected(&document, &input).unwrap();
        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["keep"], json!(true));
        assert_eq!(semantic["models"][0]["maxInputTokens"], json!(257_550));
        assert_eq!(semantic["models"][0]["maxOutputTokens"], json!(32_768));
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
            model_metadata: None,
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

        let metadata = AgentModelMetadata {
            context: 257_550,
            output: 32_768,
            vision: true,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let refresh_input = ConnectInput {
            base_url: input.base_url,
            token: input.token,
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let refresh = connector
            .refresh_patch_for_document(&document, &refresh_input, &connector.owned_paths())
            .expect("a managed native-array model can refresh in place");
        apply_patch(&mut document, &refresh).unwrap();
        connector
            .validate_projected(&document, &refresh_input)
            .unwrap();
        let refreshed = semantic_json(&document).unwrap();
        assert_eq!(refreshed[1]["name"], json!("Token Station Auto"));
        assert_eq!(refreshed[1]["useCustomProtocol"], json!(false));
        assert_eq!(refreshed[1]["maxInputTokens"], json!(257_550));
        assert_eq!(refreshed[1]["maxOutputTokens"], json!(32_768));
        assert_eq!(refreshed[0]["id"], json!("user-model"));

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
            model_metadata: Some(&AgentModelMetadata {
                context: 257_550,
                output: 32_768,
                vision: true,
                tools: true,
                reasoning: true,
                cost: Some(AgentModelCost {
                    input: 0.2,
                    output: 0.6,
                    cache_read: Some(0.04),
                    cache_write: None,
                }),
            }),
        };
        let mut document = parse_source_bytes(
            Some(br#"{"model":"tokenstation/auto","provider":{"tokenstation":null}}"#),
            connector.format(),
            connector.label(),
        )
        .unwrap();
        apply_patch(&mut document, &connector.connect_patch(&input).unwrap()).unwrap();
        connector.validate_projected(&document, &input).unwrap();
        assert!(connector
            .success_message(&input)
            .contains("已同步上下文、输出上限和统一价格"));

        let projected = crate::agent_integration::config_codec::semantic_json(&document).unwrap();
        assert_eq!(projected["model"], json!("tokenstation/auto"));
        assert!(projected["provider"]["tokenstation"].is_object());
        let model = &projected["provider"]["tokenstation"]["models"]["auto"];
        assert_eq!(model["attachment"], json!(true));
        assert_eq!(model["limit"], json!({"context": 257550, "output": 32768}));
        assert_eq!(
            model["cost"],
            json!({"input": 0.2, "output": 0.6, "cache_read": 0.04})
        );
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
    fn metadata_is_projected_into_supported_agent_native_schemas() {
        let metadata = AgentModelMetadata {
            context: 257_550,
            output: 32_768,
            vision: true,
            tools: true,
            reasoning: true,
            cost: Some(AgentModelCost {
                input: 0.2,
                output: 0.6,
                cache_read: Some(0.04),
                cache_write: None,
            }),
        };

        let codex_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("fixture-codex-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut codex = parse_source_bytes(None, DocumentFormat::Toml, "Codex").unwrap();
        apply_patch(
            &mut codex,
            &CodexConnector.connect_patch(&codex_input).unwrap(),
        )
        .unwrap();
        CodexConnector
            .validate_projected(&codex, &codex_input)
            .unwrap();
        assert!(CodexConnector
            .success_message(&codex_input)
            .contains("safe context and automatic compaction limits were synchronized"));
        let codex = semantic_json(&codex).unwrap();
        assert_eq!(codex["model_context_window"], json!(257_550));
        assert_eq!(
            codex["model_auto_compact_token_limit"],
            json!(257_550 - 32_768)
        );

        let openclaw_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/openclaw/v1",
            token: Some("fixture-openclaw-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut openclaw = parse_source_bytes(None, DocumentFormat::Json5, "OpenClaw").unwrap();
        apply_patch(
            &mut openclaw,
            &OpenClawConnector.connect_patch(&openclaw_input).unwrap(),
        )
        .unwrap();
        OpenClawConnector
            .validate_projected(&openclaw, &openclaw_input)
            .unwrap();
        assert!(OpenClawConnector
            .success_message(&openclaw_input)
            .contains("已同步安全模型限制和一致价格"));
        let openclaw = semantic_json(&openclaw).unwrap();
        let model = &openclaw["models"]["providers"]["tokenstation"]["models"][0];
        assert_eq!(model["contextWindow"], json!(257_550));
        assert_eq!(model["maxTokens"], json!(32_768));
        assert_eq!(
            model["cost"],
            json!({"input":0.2,"output":0.6,"cacheRead":0.04})
        );
        assert_eq!(model["input"], json!(["text", "image"]));
        assert_ne!(model["cost"]["input"], json!(0));

        let workbuddy_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-workbuddy-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut workbuddy =
            parse_source_bytes(Some(br#"[]"#), DocumentFormat::Json, "WorkBuddy").unwrap();
        let patch = WorkBuddyConnector
            .connect_patch_for_document(&workbuddy, &workbuddy_input)
            .unwrap();
        apply_patch(&mut workbuddy, &patch).unwrap();
        WorkBuddyConnector
            .validate_projected(&workbuddy, &workbuddy_input)
            .unwrap();
        assert!(WorkBuddyConnector
            .success_message(&workbuddy_input)
            .contains("已同步上下文和最大输出限制"));
        let workbuddy = semantic_json(&workbuddy).unwrap();
        assert_eq!(workbuddy[0]["maxInputTokens"], json!(257_550));
        assert_eq!(workbuddy[0]["maxOutputTokens"], json!(32_768));
        assert_eq!(workbuddy[0]["supportsImages"], json!(true));
        assert_eq!(workbuddy[0]["supportsToolCall"], json!(true));
        assert_eq!(workbuddy[0]["supportsReasoning"], json!(true));

        let hermes_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/nous-hermes-agent/v1",
            token: Some("fixture-hermes-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut hermes = parse_source_bytes(None, DocumentFormat::Yaml, "Hermes").unwrap();
        apply_patch(
            &mut hermes,
            &HermesConnector.connect_patch(&hermes_input).unwrap(),
        )
        .unwrap();
        HermesConnector
            .validate_projected(&hermes, &hermes_input)
            .unwrap();
        assert!(HermesConnector
            .success_message(&hermes_input)
            .contains("已同步安全上下文窗口"));
        let hermes = semantic_json(&hermes).unwrap();
        assert_eq!(hermes["model"]["context_length"], json!(257_550));
    }

    #[test]
    fn partial_metadata_projects_capabilities_without_zero_limits() {
        let metadata = AgentModelMetadata {
            context: 0,
            output: 0,
            vision: true,
            tools: true,
            reasoning: true,
            cost: Some(AgentModelCost {
                input: 0.2,
                output: 0.6,
                cache_read: None,
                cache_write: None,
            }),
        };

        let opencode_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/opencode/v1",
            token: Some("fixture-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut opencode = parse_source_bytes(None, DocumentFormat::Json5, "OpenCode").unwrap();
        apply_patch(
            &mut opencode,
            &OpenCodeConnector.connect_patch(&opencode_input).unwrap(),
        )
        .unwrap();
        OpenCodeConnector
            .validate_projected(&opencode, &opencode_input)
            .unwrap();
        let opencode = semantic_json(&opencode).unwrap();
        let opencode_model = &opencode["provider"]["tokenstation"]["models"]["auto"];
        assert_eq!(opencode_model["attachment"], json!(true));
        assert_eq!(opencode_model["cost"]["output"], json!(0.6));
        assert!(opencode_model.get("limit").is_none());

        let workbuddy_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut workbuddy = parse_source_bytes(None, DocumentFormat::Json, "WorkBuddy").unwrap();
        let workbuddy_patch = WorkBuddyConnector
            .connect_patch_for_document(&workbuddy, &workbuddy_input)
            .unwrap();
        apply_patch(&mut workbuddy, &workbuddy_patch).unwrap();
        let workbuddy = semantic_json(&workbuddy).unwrap();
        let workbuddy_model = &workbuddy["models"][0];
        assert_eq!(workbuddy_model["supportsImages"], json!(true));
        assert_eq!(workbuddy_model["supportsToolCall"], json!(true));
        assert_eq!(workbuddy_model["supportsReasoning"], json!(true));
        assert!(workbuddy_model.get("maxInputTokens").is_none());
        assert!(workbuddy_model.get("maxOutputTokens").is_none());

        for connector in [&CodexConnector as &dyn Connector, &HermesConnector] {
            let input = ConnectInput {
                base_url: "http://127.0.0.1:8787/v1",
                token: Some("fixture-key"),
                adapter_ready: true,
                model_metadata: Some(&metadata),
            };
            let mut document =
                parse_source_bytes(None, connector.format(), connector.label()).unwrap();
            apply_patch(&mut document, &connector.connect_patch(&input).unwrap()).unwrap();
            let projected = semantic_json(&document).unwrap();
            assert!(projected.pointer("/model_context_window").is_none());
            assert!(projected
                .pointer("/model_auto_compact_token_limit")
                .is_none());
            assert!(projected.pointer("/model/context_length").is_none());
        }
    }

    #[test]
    fn metadata_refresh_removes_stale_managed_limits_when_metadata_becomes_unknown() {
        let codex_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("fixture-codex-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut codex = parse_rendered(
            r#"model = "auto"
model_provider = "tokenstation"
model_context_window = 257550
model_auto_compact_token_limit = 224782

[model_providers.tokenstation]
base_url = "http://127.0.0.1:8787/agents/codex/v1"
wire_api = "responses"
requires_openai_auth = false
experimental_bearer_token = "fixture-codex-key"
"#,
            DocumentFormat::Toml,
            "Codex",
        )
        .unwrap();
        let patch = CodexConnector
            .refresh_patch_for_document(&codex, &codex_input, &CodexConnector.owned_paths())
            .unwrap();
        apply_patch(&mut codex, &patch).unwrap();
        let codex = semantic_json(&codex).unwrap();
        assert!(codex.get("model_context_window").is_none());
        assert!(codex.get("model_auto_compact_token_limit").is_none());

        let hermes_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/nous-hermes-agent/v1",
            token: Some("fixture-hermes-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut hermes = parse_rendered(
            "model:\n  default: auto\n  provider: custom\n  base_url: http://127.0.0.1:8787/agents/nous-hermes-agent/v1\n  api_key: fixture-hermes-key\n  api_mode: chat_completions\n  context_length: 257550\n",
            DocumentFormat::Yaml,
            "Hermes",
        )
        .unwrap();
        let patch = HermesConnector
            .refresh_patch_for_document(&hermes, &hermes_input, &HermesConnector.owned_paths())
            .unwrap();
        apply_patch(&mut hermes, &patch).unwrap();
        assert!(semantic_json(&hermes)
            .unwrap()
            .pointer("/model/context_length")
            .is_none());
    }

    #[test]
    fn first_codex_connection_with_unknown_metadata_preserves_user_limits() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("fixture-codex-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut document = parse_rendered(
            "model_context_window = 64000\nmodel_auto_compact_token_limit = 48000\n",
            DocumentFormat::Toml,
            "Codex",
        )
        .unwrap();

        let patch = CodexConnector
            .connect_patch_for_document(&document, &input)
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        CodexConnector
            .validate_projected(&document, &input)
            .unwrap();
        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["model_context_window"], json!(64_000));
        assert_eq!(semantic["model_auto_compact_token_limit"], json!(48_000));
    }

    #[test]
    fn image_support_fails_closed_for_text_only_or_unknown_routes() {
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 16_384,
            vision: false,
            tools: false,
            reasoning: false,
            cost: None,
        };
        let opencode_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/opencode/v1",
            token: Some("fixture-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut opencode = parse_source_bytes(None, DocumentFormat::Json5, "OpenCode").unwrap();
        apply_patch(
            &mut opencode,
            &OpenCodeConnector.connect_patch(&opencode_input).unwrap(),
        )
        .unwrap();
        let opencode = semantic_json(&opencode).unwrap();
        let model = &opencode["provider"]["tokenstation"]["models"]["auto"];
        assert_eq!(model["attachment"], json!(false));
        assert_eq!(model["modalities"]["input"], json!(["text"]));

        let workbuddy_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/workbuddy/v1",
            token: Some("fixture-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut workbuddy = parse_source_bytes(None, DocumentFormat::Json, "WorkBuddy").unwrap();
        apply_patch(
            &mut workbuddy,
            &WorkBuddyConnector.connect_patch(&workbuddy_input).unwrap(),
        )
        .unwrap();
        assert_eq!(
            semantic_json(&workbuddy).unwrap()["models"][0]["supportsImages"],
            json!(false)
        );
        let workbuddy = semantic_json(&workbuddy).unwrap();
        assert_eq!(workbuddy["models"][0]["supportsToolCall"], json!(false));
        assert_eq!(workbuddy["models"][0]["supportsReasoning"], json!(false));
        assert!(workbuddy["models"][0].get("reasoning").is_none());
    }

    #[test]
    fn connectors_recover_only_null_optional_object_containers() {
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 8_192,
            vision: true,
            tools: true,
            reasoning: false,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-secret"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let fixtures: [(&dyn Connector, &[u8]); 4] = [
            (&ClaudeCodeConnector, br#"{"env":null,"keep":true}"#),
            (&OpenCodeConnector, br#"{"provider":null,"keep":true}"#),
            (&HermesConnector, b"model: null\nkeep: true\n"),
            (
                &OpenClawConnector,
                br#"{models:null,agents:null,keep:true}"#,
            ),
        ];

        for (connector, source) in fixtures {
            let mut document =
                parse_source_bytes(Some(source), connector.format(), connector.label()).unwrap();
            connector
                .validate_source(&document)
                .expect("an optional null container is equivalent to absence");
            let patch = connector.connect_patch(&input).unwrap();
            apply_patch(&mut document, &patch).unwrap();
            connector.validate_projected(&document, &input).unwrap();
            assert_eq!(semantic_json(&document).unwrap()["keep"], json!(true));
        }
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
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 8_192,
            vision: true,
            tools: true,
            reasoning: false,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-secret"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
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
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 8_192,
            vision: true,
            tools: true,
            reasoning: false,
            cost: None,
        };
        let good = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("fixture-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let wrong = ConnectInput {
            base_url: "http://127.0.0.1:9999/v1",
            token: Some("wrong-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let not_ready = ConnectInput {
            base_url: good.base_url,
            token: good.token,
            adapter_ready: false,
            model_metadata: Some(&metadata),
        };
        let missing_token = ConnectInput {
            base_url: good.base_url,
            token: None,
            adapter_ready: true,
            model_metadata: Some(&metadata),
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
            assert!(connector.projects_model_metadata());
            assert_eq!(
                connector.legacy_companion_format(
                    Path::new("/fixture/config"),
                    Path::new("/fixture/companion")
                ),
                None
            );
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
            model_metadata: None,
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
            model_metadata: None,
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
            model_metadata: None,
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
