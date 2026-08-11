use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

pub struct CodexConnector;
pub(super) static CONNECTOR: CodexConnector = CodexConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "codex-v1",
    agent_id: "codex",
    label: "Codex config.toml",
    adapter_id: "agent-openai-responses",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Toml,
    config_path_template: "${HOME}/.codex/config.toml",
    // connect_patch also rewrites model to auto and includes it in owned_paths(),
    // so declare it here or ownership metadata, restoration, and display will omit it.
    owned_fields: &[
        "model",
        "model_provider",
        "model_context_window",
        "model_auto_compact_token_limit",
        "model_providers.tokenstation",
    ],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for CodexConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "codex-v1"
    }

    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex config.toml"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Toml
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".codex").join("config.toml")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.codex 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(&["model"]),
            path(&["model_provider"]),
            path(&["model_context_window"]),
            path(&["model_auto_compact_token_limit"]),
            path(&["model_providers", "tokenstation"]),
        ]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&[
            "model_providers",
            "tokenstation",
            "experimental_bearer_token",
        ])]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if input.adapter_ready {
            input
                .token
                .map(|_| ())
                .ok_or_else(|| "Codex 接入缺少本地虚拟 Key".to_string())
        } else {
            Err(
                "暂不能接入 Codex：网关未加载 agent-openai-responses，/v1/responses \
                 无入站适配器。本次未修改 ~/.codex/config.toml。"
                    .to_string(),
            )
        }
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Toml(document) = document else {
            return Err("Codex 连接器收到错误的配置格式".to_string());
        };
        let root = document.as_table();
        match root.get("model_providers") {
            None => Ok(()),
            Some(item) if item.is_table_like() => Ok(()),
            Some(_) => Err("Codex config.toml 的 model_providers 必须是表".to_string()),
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Codex 接入缺少本地虚拟 Key".to_string())?;
        let mut operations = vec![
            replace(&["model"], json!("auto")),
            replace(&["model_provider"], json!("tokenstation")),
            replace(
                &["model_providers", "tokenstation", "base_url"],
                json!(input.base_url),
            ),
        ];
        operations.extend(
            provider_fields(token)
                .into_iter()
                .map(|(field, value)| replace(&["model_providers", "tokenstation", field], value)),
        );
        if let Some(metadata) = input.model_metadata {
            operations.push(replace(
                &["model_context_window"],
                json!(metadata.context),
            ));
            operations.push(replace(
                &["model_auto_compact_token_limit"],
                json!(metadata.context - metadata.output),
            ));
        }
        // Older connectors wrote env_key. Codex prefers it over the embedded
        // bearer token, so leaving it behind makes a GUI connection depend on
        // an environment variable it never receives.
        operations.push(PatchOperation {
            operation: PatchKind::Remove,
            path: path(&["model_providers", "tokenstation", "env_key"]),
            value: None,
        });
        Ok(operations)
    }

    fn refresh_patch_for_document(
        &self,
        _document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<PatchOperation>, String> {
        let mut operations = self.connect_patch(input)?;
        if input.model_metadata.is_none() {
            operations.push(PatchOperation {
                operation: PatchKind::Remove,
                path: path(&["model_context_window"]),
                value: None,
            });
            operations.push(PatchOperation {
                operation: PatchKind::Remove,
                path: path(&["model_auto_compact_token_limit"]),
                value: None,
            });
        }
        Ok(operations)
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
        let ConfigDocument::Toml(document) = document else {
            unreachable!();
        };
        let root = document.as_table();
        let provider = root
            .get("model_providers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|providers| providers.get("tokenstation"))
            .and_then(toml_edit::Item::as_table_like)
            .ok_or_else(|| "Codex 写入前复验缺少 tokenstation provider".to_string())?;
        let valid = root.get("model").and_then(toml_edit::Item::as_str) == Some("auto")
            && root.get("model_provider").and_then(toml_edit::Item::as_str) == Some("tokenstation")
            && provider.get("base_url").and_then(toml_edit::Item::as_str) == Some(input.base_url);
        if !valid {
            return Err("Codex 写入前复验失败".to_string());
        }
        let token = input
            .token
            .ok_or_else(|| "Codex 写入前复验缺少本地虚拟 Key".to_string())?;
        for (field, expected) in provider_fields(token) {
            if !item_matches_json(provider.get(field), &expected) {
                return Err(format!("Codex 写入前复验字段 {field} 失败"));
            }
        }
        if provider.get("env_key").is_some() {
            return Err("Codex 写入前复验遗留 env_key".to_string());
        }
        if let Some(metadata) = input.model_metadata {
            let context = root
                .get("model_context_window")
                .and_then(toml_edit::Item::as_integer);
            let compact = root
                .get("model_auto_compact_token_limit")
                .and_then(toml_edit::Item::as_integer);
            if context != Some(i64::from(metadata.context))
                || compact != Some(i64::from(metadata.context - metadata.output))
            {
                return Err("Codex 写入前复验模型上下文或自动压缩阈值失败".to_string());
            }
        }
        Ok(())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = input.model_metadata.map_or("模型限制未知，未写入上下文设置", |_| {
            "已同步安全上下文窗口和自动压缩阈值"
        });
        format!(
            "Codex 已通过 Responses API 指向 {}(~/.codex/config.toml,已备份；{})。",
            input.base_url, metadata
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

fn provider_fields(token: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("name", json!("token-station")),
        ("wire_api", json!("responses")),
        ("experimental_bearer_token", json!(token)),
        ("requires_openai_auth", json!(false)),
        ("request_max_retries", json!(0)),
        ("stream_max_retries", json!(0)),
    ]
}

fn item_matches_json(item: Option<&toml_edit::Item>, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::String(value) => item.and_then(toml_edit::Item::as_str) == Some(value),
        serde_json::Value::Bool(value) => item.and_then(toml_edit::Item::as_bool) == Some(*value),
        serde_json::Value::Number(value) => {
            item.and_then(toml_edit::Item::as_integer) == value.as_i64()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_field_matcher_rejects_unsupported_json_shapes() {
        assert!(!item_matches_json(None, &json!(null)));
        assert!(!item_matches_json(None, &json!(["unexpected"])));
    }

    #[test]
    fn codex_connection_embeds_the_sensitive_local_virtual_key_for_gui_clients() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        let operations = CodexConnector.connect_patch(&input).unwrap();
        assert!(operations.iter().any(|operation| {
            operation.path
                == path(&[
                    "model_providers",
                    "tokenstation",
                    "experimental_bearer_token",
                ])
                && operation.value == Some(json!("local-virtual-key"))
        }));
        assert!(operations.iter().any(|operation| {
            operation.operation == PatchKind::Remove
                && operation.path == path(&["model_providers", "tokenstation", "env_key"])
        }));
        assert!(CodexConnector.sensitive_paths().contains(&path(&[
            "model_providers",
            "tokenstation",
            "experimental_bearer_token",
        ])));
        assert!(CodexConnector.capabilities().requires_virtual_key);
    }
}
