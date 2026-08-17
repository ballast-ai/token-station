use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, AgentModelMetadata, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{semantic_json, ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const DEFAULT_MODEL: &[&str] = &["default_model"];
const TOKEN_STATION_PROVIDER: &[&str] = &["providers", "tokenstation"];
const TOKEN_STATION_MODEL: &[&str] = &["models", "tokenstation-auto"];

pub struct KimiCodeConnector;
pub(super) static CONNECTOR: KimiCodeConnector = KimiCodeConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "kimi-code-v1",
    agent_id: "kimi-code",
    label: "Kimi Code config.toml",
    adapter_id: "agent-openai",
    base_url_shape: BaseUrlShape::OriginV1,
    platforms: &[
        Platform::Macos,
        Platform::Linux,
        Platform::Windows,
        Platform::Wsl,
    ],
    config_format: DocumentFormat::Toml,
    config_path_template: "${HOME}/.kimi-code/config.toml",
    owned_fields: &[
        "default_model",
        "providers.tokenstation",
        "models.tokenstation-auto",
    ],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for KimiCodeConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".kimi-code").join("config.toml")
    }

    fn create_dir_error(&self) -> &'static str {
        "Failed to create ~/.kimi-code"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(DEFAULT_MODEL),
            path(TOKEN_STATION_PROVIDER),
            path(TOKEN_STATION_MODEL),
        ]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["providers", "tokenstation", "api_key"])]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err("Cannot connect Kimi Code because the gateway did not load agent-openai. config.toml was not changed.".to_string());
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "Kimi Code requires a local virtual key.".to_string())?;
        positive_context_size(input).map(|_| ())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Toml(document) = document else {
            return Err("Kimi Code received an unsupported config format.".to_string());
        };
        let root = document.as_table();
        for field in ["providers", "models"] {
            if root.get(field).is_some_and(|item| !item.is_table_like()) {
                return Err(format!("Kimi Code config.toml field {field} must be a table."));
            }
        }
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Kimi Code requires a local virtual key.".to_string())?;
        let max_context_size = positive_context_size(input)?;
        let mut model = json!({
            "provider": "tokenstation",
            "model": "auto",
            "max_context_size": max_context_size
        });
        if let Some(max_output_size) = safe_output_size(input) {
            model["max_output_size"] = json!(max_output_size);
        }
        Ok(vec![
            replace(DEFAULT_MODEL, json!("tokenstation-auto")),
            replace(
                TOKEN_STATION_PROVIDER,
                json!({
                    "type": "openai",
                    "base_url": input.base_url,
                    "api_key": token
                }),
            ),
            replace(TOKEN_STATION_MODEL, model),
        ])
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
        let token = input
            .token
            .ok_or_else(|| "Kimi Code pre-write validation requires a local virtual key.".to_string())?;
        let max_context_size = positive_context_size(input)?;
        let expected_output_size = safe_output_size(input);
        let valid = root.get("default_model") == Some(&json!("tokenstation-auto"))
            && root.pointer("/providers/tokenstation/type") == Some(&json!("openai"))
            && root.pointer("/providers/tokenstation/base_url") == Some(&json!(input.base_url))
            && root.pointer("/providers/tokenstation/api_key") == Some(&json!(token))
            && root.pointer("/models/tokenstation-auto/provider") == Some(&json!("tokenstation"))
            && root.pointer("/models/tokenstation-auto/model") == Some(&json!("auto"))
            && root.pointer("/models/tokenstation-auto/max_context_size")
                == Some(&json!(max_context_size))
            && expected_output_size.is_none_or(|output| {
                root.pointer("/models/tokenstation-auto/max_output_size") == Some(&json!(output))
            });
        valid
            .then_some(())
            .ok_or_else(|| "Kimi Code pre-write validation failed.".to_string())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Kimi Code now uses OpenAI Chat Completions at {}. The original config is protected by the encrypted snapshot and ownership records.",
            input.base_url
        )
    }
}

fn positive_context_size(input: &ConnectInput<'_>) -> Result<u32, String> {
    let metadata = input
        .model_metadata
        .ok_or_else(|| "Kimi Code requires context limits from the active route. config.toml was not changed.".to_string())?;
    if metadata.context <= 1 {
        return Err("Kimi Code requires a context limit greater than 1. config.toml was not changed.".to_string());
    }
    if metadata.output > 0 && metadata.output >= metadata.context {
        return Err("Kimi Code requires the output limit to be smaller than the context limit. config.toml was not changed.".to_string());
    }
    Ok(metadata.context)
}

fn safe_output_size(input: &ConnectInput<'_>) -> Option<u32> {
    input
        .model_metadata
        .and_then(AgentModelMetadata::safe_limits)
        .map(|(_, output)| output)
}

fn replace(segments: &[&str], value: serde_json::Value) -> PatchOperation {
    PatchOperation {
        operation: PatchKind::Replace,
        path: path(segments),
        value: Some(value),
    }
}
