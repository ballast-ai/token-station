use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, AgentModelMetadata, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{semantic_json, ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const DEFAULT_MODEL: &[&str] = &["models", "default"];
const TOKEN_STATION_MODEL: &[&str] = &["model", "tokenstation"];
const GROK_46_MODEL: &[&str] = &["model", "grok-4.6"];
const GROK_45_MODEL: &[&str] = &["model", "grok-4.5"];

pub struct GrokBuildConnector;
pub(super) static CONNECTOR: GrokBuildConnector = GrokBuildConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "grok-build-v1",
    agent_id: "grok-build",
    label: "Grok Build config.toml",
    adapter_id: "agent-openai",
    base_url_shape: BaseUrlShape::OriginV1,
    platforms: &[
        Platform::Macos,
        Platform::Linux,
        Platform::Windows,
        Platform::Wsl,
    ],
    config_format: DocumentFormat::Toml,
    config_path_template: "${HOME}/.grok/config.toml",
    owned_fields: &[
        "models.default",
        "model.tokenstation",
        "model.grok-4.6",
        "model.grok-4.5",
    ],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for GrokBuildConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".grok").join("config.toml")
    }

    fn create_dir_error(&self) -> &'static str {
        "Failed to create ~/.grok"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(DEFAULT_MODEL),
            path(TOKEN_STATION_MODEL),
            path(GROK_46_MODEL),
            path(GROK_45_MODEL),
        ]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(&["model", "tokenstation", "api_key"]),
            path(&["model", "grok-4.6", "api_key"]),
            path(&["model", "grok-4.5", "api_key"]),
        ]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err("Cannot connect Grok Build because the gateway did not load agent-openai. config.toml was not changed.".to_string());
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "Grok Build requires a local virtual key.".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Toml(document) = document else {
            return Err("Grok Build received an unsupported config format.".to_string());
        };
        let root = document.as_table();
        for field in ["models", "model"] {
            if root.get(field).is_some_and(|item| !item.is_table_like()) {
                return Err(format!("Grok Build config.toml field {field} must be a table."));
            }
        }
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Grok Build requires a local virtual key.".to_string())?;
        Ok(vec![
            replace(DEFAULT_MODEL, json!("tokenstation")),
            replace(
                TOKEN_STATION_MODEL,
                proxied_model(input.base_url, token, input.model_metadata),
            ),
            // Grok Build 1.0.4 may select a signed-in campaign default after
            // reading models.default. Shadow the released built-in IDs so
            // either campaign choice still reaches Token Station. An
            // allowed_models=["tokenstation"] guard is unsafe: the CLI rejects
            // its campaign-selected grok-4.6 before resolving the custom model.
            replace(
                GROK_46_MODEL,
                proxied_model(input.base_url, token, input.model_metadata),
            ),
            replace(
                GROK_45_MODEL,
                proxied_model(input.base_url, token, input.model_metadata),
            ),
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
            .ok_or_else(|| "Grok Build pre-write validation requires a local virtual key.".to_string())?;
        let expected_limits = input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits);
        let valid = root.pointer("/models/default") == Some(&json!("tokenstation"))
            && ["tokenstation", "grok-4.6", "grok-4.5"]
                .iter()
                .all(|model| {
                    root.pointer(&format!("/model/{model}/model")) == Some(&json!("auto"))
                        && root.pointer(&format!("/model/{model}/base_url"))
                            == Some(&json!(input.base_url))
                        && root.pointer(&format!("/model/{model}/api_key")) == Some(&json!(token))
                        && root.pointer(&format!("/model/{model}/api_backend"))
                            == Some(&json!("chat_completions"))
                        && expected_limits.is_none_or(|(context, output)| {
                            root.pointer(&format!("/model/{model}/context_window"))
                                == Some(&json!(context))
                                && root.pointer(&format!(
                                    "/model/{model}/max_completion_tokens"
                                )) == Some(&json!(output))
                        })
                });
        valid
            .then_some(())
            .ok_or_else(|| "Grok Build pre-write validation failed.".to_string())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Grok Build now uses chat_completions at {}. The original config is protected by the encrypted snapshot and ownership records.",
            input.base_url
        )
    }
}

fn proxied_model(
    base_url: &str,
    token: &str,
    metadata: Option<&AgentModelMetadata>,
) -> serde_json::Value {
    let mut model = json!({
        "model": "auto",
        "base_url": base_url,
        "api_key": token,
        "api_backend": "chat_completions",
        "name": "Token Station Auto"
    });
    if let Some((context, output)) = metadata.and_then(AgentModelMetadata::safe_limits) {
        model["context_window"] = json!(context);
        model["max_completion_tokens"] = json!(output);
    }
    model
}

fn replace(segments: &[&str], value: serde_json::Value) -> PatchOperation {
    PatchOperation {
        operation: PatchKind::Replace,
        path: path(segments),
        value: Some(value),
    }
}
