use std::path::{Path, PathBuf};

use serde_json::json;
use zeroize::Zeroizing;

use super::{
    path, AgentModelMetadata, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities,
};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, prepare_owned_paths_for_write, render_document, semantic_json,
    semantic_value_at, ConfigDocument, DocumentFormat,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const DEFAULT_MODEL: &[&str] = &["$cordis", "agent-default-model", "config"];
const TOKEN_STATION_PROVIDER: &[&str] = &[
    "$cordis",
    "llm-pi-ai",
    "config",
    "providers",
    "tokenstation",
];
const CREDENTIAL_KEY: &str = "TOKENSTATION_API_KEY";

pub struct DeepSeekHarnessConnector;
pub(super) static CONNECTOR: DeepSeekHarnessConnector = DeepSeekHarnessConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "deepseek-harness-v1",
    agent_id: "deepseek-harness",
    label: "DeepSeek Harness cordis.patch.yml",
    adapter_id: "agent-openai",
    base_url_shape: BaseUrlShape::OriginV1,
    platforms: &[
        Platform::Macos,
        Platform::Linux,
        Platform::Windows,
        Platform::Wsl,
    ],
    config_format: DocumentFormat::CordisYaml,
    config_path_template: "${HOME}/.dsh/cordis.patch.yml",
    owned_fields: &[
        "$cordis.agent-default-model.config",
        "$cordis.llm-pi-ai.config.providers.tokenstation",
    ],
    requires_virtual_key: true,
    restart_required: true,
};

impl Connector for DeepSeekHarnessConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".dsh").join("cordis.patch.yml")
    }

    fn create_dir_error(&self) -> &'static str {
        "Failed to create ~/.dsh"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(DEFAULT_MODEL), path(TOKEN_STATION_PROVIDER)]
    }

    fn legacy_owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(&["agent-default-model"]),
            path(&["llm-pi-ai", "providers", "tokenstation"]),
        ]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err("Cannot connect DeepSeek Harness because the gateway did not load agent-openai. cordis.patch.yml was not changed.".to_string());
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "DeepSeek Harness requires a local virtual key.".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let root = semantic_json(document)?;
        let rows = root.as_array().ok_or("DeepSeek Harness cordis.patch.yml must contain a YAML array. Disconnect legacy settings.yaml ownership first.")?;
        for row in rows {
            if matches!(
                row["id"].as_str(),
                Some("agent-default-model" | "llm-pi-ai")
            ) {
                if row
                    .get("config")
                    .is_some_and(|v| !v.is_object() && !v.is_null())
                {
                    return Err("DeepSeek Harness row configuration must be an object.".into());
                }
                if row["id"] == "llm-pi-ai"
                    && row
                        .pointer("/config/providers")
                        .is_some_and(|v| !v.is_object() && !v.is_null())
                {
                    return Err("DeepSeek Harness providers must be an object.".into());
                }
            }
        }
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        input
            .token
            .ok_or_else(|| "DeepSeek Harness requires a local virtual key.".to_string())?;
        let mut model = json!({"id": "auto", "name": "Token Station Auto"});
        if let Some((context, output)) = input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits)
        {
            model["contextWindow"] = json!(context);
            model["maxTokens"] = json!(output);
        }
        Ok(vec![
            replace(
                DEFAULT_MODEL,
                json!({"provider": "tokenstation", "model": "auto"}),
            ),
            replace(
                TOKEN_STATION_PROVIDER,
                json!({
                    "displayName": "Token Station",
                    "apiKeyEnv": CREDENTIAL_KEY,
                    "api": "openai-completions",
                    "baseURL": input.base_url,
                    "retryPolicy": {"mode": "normal", "maxRetries": 0},
                    "models": [model]
                }),
            ),
        ])
    }

    fn companion_projections(
        &self,
        primary_target: &Path,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        let token = input
            .token
            .ok_or_else(|| "DeepSeek Harness requires a local virtual key.".to_string())?;
        let credentials_path = primary_target
            .parent()
            .ok_or_else(|| {
                "DeepSeek Harness cordis.patch.yml must have a .dsh parent directory.".to_string()
            })?
            .join(".credentials.yaml");
        let source = read_config_source(&credentials_path)?;
        let label = "DeepSeek Harness .credentials.yaml";
        let mut document = parse_source_bytes(
            source.existed.then_some(source.exact_bytes.as_slice()),
            DocumentFormat::Yaml,
            label,
        )?;
        if !semantic_json(&document)?.is_object() {
            return Err(
                "DeepSeek Harness .credentials.yaml must contain a YAML object.".to_string(),
            );
        }
        let owned_path = path(&[CREDENTIAL_KEY]);
        let operations = vec![PatchOperation {
            operation: PatchKind::Replace,
            path: owned_path.clone(),
            value: Some(json!(token)),
        }];
        prepare_owned_paths_for_write(&mut document, std::slice::from_ref(&owned_path))?;
        apply_patch(&mut document, &operations)?;
        let projected_bytes = render_document(&document, label)?.into_bytes();
        Ok(vec![CompanionProjection {
            target_path: credentials_path,
            source_existed: source.existed,
            source_bytes: Zeroizing::new(source.exact_bytes.to_vec()),
            original_permissions: source.original_permissions,
            original_owner: source.original_owner,
            projected_bytes: Zeroizing::new(projected_bytes),
            format: DocumentFormat::Yaml,
            label,
            owned_paths: vec![owned_path.clone()],
            sensitive_paths: vec![owned_path],
            operations,
        }])
    }

    fn legacy_companion_format(
        &self,
        primary_target: &Path,
        companion_target: &Path,
    ) -> Option<DocumentFormat> {
        let expected = primary_target.parent()?.join(".credentials.yaml");
        (companion_target == expected).then_some(DocumentFormat::Yaml)
    }

    fn disconnect_patch(&self) -> Vec<PatchOperation> {
        self.owned_paths()
            .into_iter()
            .map(|path| PatchOperation {
                operation: PatchKind::Remove,
                path,
                value: None,
            })
            .collect()
    }

    fn validate_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<(), String> {
        self.validate_source(document)?;
        let root = semantic_json(document)?;
        let expected_limits = input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits);
        let default = semantic_value_at(&root, &path(DEFAULT_MODEL));
        let provider = semantic_value_at(&root, &path(TOKEN_STATION_PROVIDER));
        let valid = default
            .is_some_and(|v| v["provider"] == "tokenstation" && v["model"] == "auto")
            && provider.is_some_and(|v| {
                v["api"] == "openai-completions"
                    && v["apiKeyEnv"] == CREDENTIAL_KEY
                    && v["baseURL"] == input.base_url
                    && v["retryPolicy"]["maxRetries"] == 0
                    && expected_limits.is_none_or(|(context, output)| {
                        v["models"][0]["contextWindow"] == context
                            && v["models"][0]["maxTokens"] == output
                    })
            });
        valid
            .then_some(())
            .ok_or_else(|| "DeepSeek Harness pre-write validation failed.".to_string())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "DeepSeek Harness now uses OpenAI Completions at {}. Restart DSH to load cordis.patch.yml. Overlay and credentials are restored in one transaction.",
            input.base_url
        )
    }
}

fn replace(segments: &[&str], value: serde_json::Value) -> PatchOperation {
    PatchOperation {
        operation: PatchKind::Replace,
        path: path(segments),
        value: Some(value),
    }
}
