use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{semantic_json, ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const PROVIDER_PATH: &[&str] = &["models", "providers", "tokenstation"];
const PRIMARY_PATH: &[&str] = &["agents", "defaults", "model", "primary"];

pub struct OpenClawConnector;
pub(super) static CONNECTOR: OpenClawConnector = OpenClawConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "openclaw-v1",
    agent_id: "openclaw",
    label: "OpenClaw openclaw.json",
    adapter_id: "agent-openai",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Json5,
    config_path_template: "${HOME}/.openclaw/openclaw.json",
    owned_fields: &["models.providers.tokenstation", "agents.defaults.model.primary"],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for OpenClawConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "openclaw-v1"
    }

    fn agent_id(&self) -> &'static str {
        "openclaw"
    }

    fn label(&self) -> &'static str {
        "OpenClaw openclaw.json"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Json5
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".openclaw/openclaw.json")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.openclaw 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(PROVIDER_PATH), path(PRIMARY_PATH)]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["models", "providers", "tokenstation", "apiKey"])]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 OpenClaw：网关未加载 agent-openai，/v1/chat/completions 无入站适配器。本次未修改 openclaw.json。"
                    .to_string(),
            );
        }
        if input.token.is_none() {
            return Err("OpenClaw 接入缺少虚拟 Key".to_string());
        }
        Ok(())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let root = semantic_json(document)?;
        for pointer in [
            "",
            "/models",
            "/models/providers",
            "/agents",
            "/agents/defaults",
            "/agents/defaults/model",
        ] {
            if root
                .pointer(pointer)
                .and_then(serde_json::Value::as_object)
                .is_some_and(|object| object.contains_key("$include"))
            {
                return Err(
                    "OpenClaw owned path 的祖先使用 $include，当前 Connector 为避免覆盖 include 合并语义而拒绝写入"
                        .to_string(),
                );
            }
        }
        for (pointer, label) in [
            ("/models", "models"),
            ("/models/providers", "models.providers"),
            ("/agents", "agents"),
            ("/agents/defaults", "agents.defaults"),
            ("/agents/defaults/model", "agents.defaults.model"),
        ] {
            if root
                .pointer(pointer)
                .is_some_and(|value| !value.is_object())
            {
                return Err(format!("OpenClaw openclaw.json 的 {label} 必须是对象"));
            }
        }
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "OpenClaw 接入缺少虚拟 Key".to_string())?;
        Ok(vec![
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(PROVIDER_PATH),
                value: Some(json!({
                    "baseUrl": input.base_url,
                    "apiKey": token,
                    "api": "openai-completions",
                    "models": [{
                        "id": "auto",
                        "name": "Token Station Auto",
                        "reasoning": false,
                        "input": ["text"],
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                        "contextWindow": 200000,
                        "maxTokens": 32000
                    }]
                })),
            },
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(PRIMARY_PATH),
                value: Some(json!("tokenstation/auto")),
            },
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
        let provider = root
            .pointer("/models/providers/tokenstation")
            .ok_or_else(|| "OpenClaw 写入前复验缺少 tokenstation provider".to_string())?;
        let token = input
            .token
            .ok_or_else(|| "OpenClaw 接入缺少虚拟 Key".to_string())?;
        let valid = provider["baseUrl"] == json!(input.base_url)
            && provider["apiKey"] == json!(token)
            && provider["api"] == json!("openai-completions")
            && provider["models"][0]["id"] == json!("auto")
            && root.pointer("/agents/defaults/model/primary") == Some(&json!("tokenstation/auto"));
        if valid {
            Ok(())
        } else {
            Err("OpenClaw 写入前复验失败".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "OpenClaw 已通过 openai-completions 指向 {}；配置已进入加密快照和 ownership 管理。",
            input.base_url
        )
    }
}
