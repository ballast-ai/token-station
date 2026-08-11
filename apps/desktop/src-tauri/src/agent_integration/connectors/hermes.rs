use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{semantic_json, ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const DEFAULT_PATH: &[&str] = &["model", "default"];
const PROVIDER_PATH: &[&str] = &["model", "provider"];
const BASE_URL_PATH: &[&str] = &["model", "base_url"];
const API_KEY_PATH: &[&str] = &["model", "api_key"];
const API_MODE_PATH: &[&str] = &["model", "api_mode"];
const CONTEXT_LENGTH_PATH: &[&str] = &["model", "context_length"];

pub struct HermesConnector;
pub(super) static CONNECTOR: HermesConnector = HermesConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "hermes-v1",
    agent_id: "nous-hermes-agent",
    label: "Hermes config.yaml",
    adapter_id: "agent-openai",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Yaml,
    config_path_template: "${HOME}/.hermes/config.yaml",
    owned_fields: &[
        "model.default",
        "model.provider",
        "model.base_url",
        "model.api_key",
        "model.api_mode",
        "model.context_length",
    ],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for HermesConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "hermes-v1"
    }

    fn agent_id(&self) -> &'static str {
        "nous-hermes-agent"
    }

    fn label(&self) -> &'static str {
        "Hermes config.yaml"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Yaml
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".hermes").join("config.yaml")
    }

    fn create_dir_error(&self) -> &'static str {
        "创建 ~/.hermes 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        [
            DEFAULT_PATH,
            PROVIDER_PATH,
            BASE_URL_PATH,
            API_KEY_PATH,
            API_MODE_PATH,
            CONTEXT_LENGTH_PATH,
        ]
        .into_iter()
        .map(path)
            .collect()
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(API_KEY_PATH)]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 Hermes：网关未加载 agent-openai，/v1/chat/completions 无入站适配器。本次未修改 config.yaml。"
                    .to_string(),
            );
        }
        if input.token.is_none() {
            return Err("Hermes 接入缺少虚拟 Key".to_string());
        }
        Ok(())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let root = semantic_json(document)?;
        if root
            .get("model")
            .is_some_and(|value| !value.is_object() && !value.is_null())
        {
            return Err("Hermes config.yaml 的 model 必须是对象".to_string());
        }
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Hermes 接入缺少虚拟 Key".to_string())?;
        let mut operations = [
            (DEFAULT_PATH, json!("auto")),
            (PROVIDER_PATH, json!("custom")),
            (BASE_URL_PATH, json!(input.base_url)),
            (API_KEY_PATH, json!(token)),
            (API_MODE_PATH, json!("chat_completions")),
        ]
        .into_iter()
        .map(|(segments, value)| PatchOperation {
            operation: PatchKind::Replace,
            path: path(segments),
            value: Some(value),
        })
        .collect::<Vec<_>>();
        if let Some(metadata) = input.model_metadata {
            operations.push(PatchOperation {
                operation: PatchKind::Replace,
                path: path(CONTEXT_LENGTH_PATH),
                value: Some(json!(metadata.context)),
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
        let root = semantic_json(document)?;
        let token = input
            .token
            .ok_or_else(|| "Hermes 接入缺少虚拟 Key".to_string())?;
        let valid = root.pointer("/model/default") == Some(&json!("auto"))
            && root.pointer("/model/provider") == Some(&json!("custom"))
            && root.pointer("/model/base_url") == Some(&json!(input.base_url))
            && root.pointer("/model/api_key") == Some(&json!(token))
            && root.pointer("/model/api_mode") == Some(&json!("chat_completions"));
        if !valid {
            return Err("Hermes 写入前复验失败".to_string());
        }
        if let Some(metadata) = input.model_metadata {
            if root.pointer("/model/context_length") != Some(&json!(metadata.context)) {
                return Err("Hermes 写入前复验上下文窗口失败".to_string());
            }
        }
        Ok(())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = input.model_metadata.map_or("模型限制未知，未写入猜测值", |_| {
            "已同步安全上下文窗口"
        });
        format!(
            "Hermes 已通过 chat_completions 指向 {}；{}；配置已进入加密快照和 ownership 管理。",
            input.base_url, metadata
        )
    }
}
