use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector};
use crate::agent_integration::config_codec::{ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

pub struct CodexConnector;

impl Connector for CodexConnector {
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
        home.join(".codex/config.toml")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.codex 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(&["model"]),
            path(&["model_provider"]),
            path(&["model_providers", "tokenstation"]),
        ]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if input.adapter_ready {
            Ok(())
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
            Some(item) if item.is_table() => Ok(()),
            Some(_) => Err("Codex config.toml 的 model_providers 必须是表".to_string()),
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let mut operations = vec![
            replace(&["model"], json!("auto")),
            replace(&["model_provider"], json!("tokenstation")),
            replace(
                &["model_providers", "tokenstation", "base_url"],
                json!(input.base_url),
            ),
        ];
        operations.extend(
            provider_fields()
                .into_iter()
                .map(|(field, value)| replace(&["model_providers", "tokenstation", field], value)),
        );
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
            .and_then(toml_edit::Item::as_table)
            .and_then(|providers| providers.get("tokenstation"))
            .and_then(toml_edit::Item::as_table)
            .ok_or_else(|| "Codex 写入前复验缺少 tokenstation provider".to_string())?;
        let valid = root.get("model").and_then(toml_edit::Item::as_str) == Some("auto")
            && root.get("model_provider").and_then(toml_edit::Item::as_str) == Some("tokenstation")
            && provider.get("base_url").and_then(toml_edit::Item::as_str) == Some(input.base_url);
        if !valid {
            return Err("Codex 写入前复验失败".to_string());
        }
        for (field, expected) in provider_fields() {
            if !item_matches_json(provider.get(field), &expected) {
                return Err(format!("Codex 写入前复验字段 {field} 失败"));
            }
        }
        Ok(())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Codex 已通过 Responses API 指向 {}(~/.codex/config.toml,已备份)。\
             Codex 的 key 走环境变量,请在启动 Codex 的终端执行一次:\
             export TOKENSTATION_KEY=<面板上的虚拟 Key>",
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

fn provider_fields() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("name", json!("token-station")),
        ("wire_api", json!("responses")),
        ("env_key", json!("TOKENSTATION_KEY")),
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
}
