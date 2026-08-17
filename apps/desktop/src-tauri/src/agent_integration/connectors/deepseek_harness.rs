use std::path::{Path, PathBuf};

use serde_json::json;
use zeroize::Zeroizing;

use super::{
    path, AgentModelMetadata, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities,
};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, prepare_owned_paths_for_write, render_document, semantic_json,
    ConfigDocument, DocumentFormat,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const DEFAULT_MODEL: &[&str] = &["agent-default-model"];
const TOKEN_STATION_PROVIDER: &[&str] = &["llm-pi-ai", "providers", "tokenstation"];
const CREDENTIAL_KEY: &str = "TOKENSTATION_API_KEY";

pub struct DeepSeekHarnessConnector;
pub(super) static CONNECTOR: DeepSeekHarnessConnector = DeepSeekHarnessConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "deepseek-harness-v1",
    agent_id: "deepseek-harness",
    label: "DeepSeek Harness settings.yaml",
    adapter_id: "agent-openai",
    base_url_shape: BaseUrlShape::OriginV1,
    platforms: &[
        Platform::Macos,
        Platform::Linux,
        Platform::Windows,
        Platform::Wsl,
    ],
    config_format: DocumentFormat::Yaml,
    config_path_template: "${HOME}/.dsh/settings.yaml",
    owned_fields: &["agent-default-model", "llm-pi-ai.providers.tokenstation"],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for DeepSeekHarnessConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".dsh").join("settings.yaml")
    }

    fn create_dir_error(&self) -> &'static str {
        "Failed to create ~/.dsh"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(DEFAULT_MODEL), path(TOKEN_STATION_PROVIDER)]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err("Cannot connect DeepSeek Harness because the gateway did not load agent-openai. settings.yaml was not changed.".to_string());
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "DeepSeek Harness requires a local virtual key.".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let root = semantic_json(document)?;
        if !root.is_object() {
            return Err("DeepSeek Harness settings.yaml must contain a YAML object.".to_string());
        }
        for pointer in ["/agent-default-model", "/llm-pi-ai", "/llm-pi-ai/providers"] {
            if root
                .pointer(pointer)
                .is_some_and(|value| !value.is_object() && !value.is_null())
            {
                return Err(format!(
                    "DeepSeek Harness settings.yaml field {pointer} must be an object."
                ));
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
            .ok_or_else(|| "DeepSeek Harness settings.yaml must have a .dsh parent directory.".to_string())?
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
        let valid = root.pointer("/agent-default-model/provider") == Some(&json!("tokenstation"))
            && root.pointer("/agent-default-model/model") == Some(&json!("auto"))
            && root.pointer("/llm-pi-ai/providers/tokenstation/api")
                == Some(&json!("openai-completions"))
            && root.pointer("/llm-pi-ai/providers/tokenstation/apiKeyEnv")
                == Some(&json!(CREDENTIAL_KEY))
            && root.pointer("/llm-pi-ai/providers/tokenstation/baseURL")
                == Some(&json!(input.base_url))
            && expected_limits.is_none_or(|(context, output)| {
                root.pointer("/llm-pi-ai/providers/tokenstation/models/0/contextWindow")
                    == Some(&json!(context))
                    && root.pointer("/llm-pi-ai/providers/tokenstation/models/0/maxTokens")
                        == Some(&json!(output))
            });
        valid
            .then_some(())
            .ok_or_else(|| "DeepSeek Harness pre-write validation failed.".to_string())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "DeepSeek Harness now uses OpenAI Completions at {}. Settings and credentials are restored in one transaction.",
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
