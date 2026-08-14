use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

pub struct OpenCodeConnector;
pub(super) static CONNECTOR: OpenCodeConnector = OpenCodeConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "opencode-v1",
    agent_id: "opencode",
    label: "OpenCode opencode.json",
    adapter_id: "agent-openai",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Json,
    config_path_template: "${HOME}/.config/opencode/opencode.json",
    // Write only provider.tokenstation. The model is selected as tokenstation/auto
    // inside that block, not through the top-level model. owned_fields must match
    // owned_paths() and connect_patch() or ownership and restoration will drift.
    owned_fields: &["provider.tokenstation"],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for OpenCodeConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "opencode-v1"
    }

    fn agent_id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode opencode.json"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Json
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".config").join("opencode").join("opencode.json")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.config/opencode 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["provider", "tokenstation"])]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["provider", "tokenstation", "options", "apiKey"])]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 OpenCode：网关未加载 agent-openai，/v1/chat/completions \
                 无入站适配器。本次未修改 ~/.config/opencode/opencode.json。"
                    .to_string(),
            );
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "OpenCode 接入缺少虚拟 Key".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Json(root) = document else {
            return Err("OpenCode 连接器收到错误的配置格式".to_string());
        };
        if root
            .get("provider")
            .is_none_or(|value| value.is_object() || value.is_null())
        {
            Ok(())
        } else {
            Err("OpenCode opencode.json 的 provider 必须是对象".to_string())
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "OpenCode 接入缺少虚拟 Key".to_string())?;
        let vision = input.model_metadata.is_some_and(|metadata| metadata.vision);
        let mut model = json!({
            "name": "auto (智能路由)",
            "attachment": vision,
            "modalities": {
                "input": if vision { json!(["text", "image"]) } else { json!(["text"]) },
                "output": ["text"]
            }
        });
        if let Some(metadata) = input.model_metadata {
            if let Some((context, output)) = metadata.opencode_limits() {
                model["limit"] = json!({"context": context, "output": output});
            }
            if let Some(cost) = &metadata.cost {
                model["cost"] = serde_json::to_value(cost)
                    .map_err(|error| format!("OpenCode 模型价格序列化失败：{error}"))?;
            }
        }
        Ok(vec![PatchOperation {
            operation: PatchKind::Replace,
            path: path(&["provider", "tokenstation"]),
            value: Some(json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "token-station",
                "options": { "baseURL": input.base_url, "apiKey": token },
                "models": {
                    "auto": model
                }
            })),
        }])
    }

    fn disconnect_patch(&self) -> Vec<PatchOperation> {
        vec![PatchOperation {
            operation: PatchKind::Remove,
            path: path(&["provider", "tokenstation"]),
            value: None,
        }]
    }

    fn validate_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<(), String> {
        self.validate_source(document)?;
        let ConfigDocument::Json(root) = document else {
            unreachable!();
        };
        let provider = &root["provider"]["tokenstation"];
        let token = input
            .token
            .ok_or_else(|| "OpenCode 接入缺少虚拟 Key".to_string())?;
        let metadata_valid = input.model_metadata.is_none_or(|metadata| {
            let model = &provider["models"]["auto"];
            let limits_valid = metadata.opencode_limits().map_or_else(
                || model.get("limit").is_none(),
                |(context, output)| model["limit"] == json!({"context": context, "output": output}),
            );
            limits_valid && metadata.cost.as_ref().map_or_else(
                    || provider["models"]["auto"].get("cost").is_none(),
                    |cost| provider["models"]["auto"]["cost"] == json!(cost),
                )
        });
        let vision = input.model_metadata.is_some_and(|metadata| metadata.vision);
        let expected_input = if vision {
            json!(["text", "image"])
        } else {
            json!(["text"])
        };
        let valid = provider["npm"] == json!("@ai-sdk/openai-compatible")
            && provider["name"] == json!("token-station")
            && provider["models"]["auto"]["name"] == json!("auto (智能路由)")
            && provider["models"]["auto"]["attachment"] == json!(vision)
            && provider["models"]["auto"]["modalities"]["input"] == expected_input
            && provider["models"]["auto"]["modalities"]["output"] == json!(["text"])
            && provider["options"]["baseURL"] == json!(input.base_url)
            && provider["options"]["apiKey"] == json!(token)
            && metadata_valid;
        if valid {
            Ok(())
        } else {
            Err("OpenCode 写入前复验失败".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = match input.model_metadata {
            Some(value) if value.output == 0 && value.opencode_limits().is_some() => {
                "已同步可信上下文；输出上限使用安全默认值 8192，未改写供应商模型能力。"
            }
            Some(value) if value.cost.is_some() => "已同步上下文、输出上限和统一价格。",
            Some(_) => "已同步安全的上下文和输出上限；候选价格不一致或未知，未写入虚假价格。",
            None => "当前路由缺少完整模型上限，未猜测上下文和价格；请先刷新 Provider 模型目录。",
        };
        format!(
            "opencode 已加入 token-station provider(~/.config/opencode/opencode.json,已备份)。\
             在 opencode 里选模型 tokenstation/auto 即可。{metadata}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_integration::connectors::AgentModelMetadata;

    #[test]
    fn missing_provider_output_uses_the_opencode_safe_default() {
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 0,
            vision: false,
            tools: true,
            reasoning: false,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/opencode/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };

        let operations = OpenCodeConnector.connect_patch(&input).unwrap();
        let provider = operations[0].value.as_ref().unwrap();

        assert_eq!(
            provider.pointer("/models/auto/limit"),
            Some(&json!({"context": 128_000, "output": 8_192}))
        );
        assert!(OpenCodeConnector
            .success_message(&input)
            .contains("输出上限使用安全默认值 8192"));
    }
}
