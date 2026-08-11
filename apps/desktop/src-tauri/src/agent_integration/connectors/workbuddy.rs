use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{semantic_json, ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const MODEL_ID: &str = "tokenstation-auto";
const MODELS_PATH: &[&str] = &["models"];
const AVAILABLE_MODELS_PATH: &[&str] = &["availableModels"];

pub struct WorkBuddyConnector;
pub(super) static CONNECTOR: WorkBuddyConnector = WorkBuddyConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "workbuddy-v1",
    agent_id: "workbuddy",
    label: "WorkBuddy models.json",
    adapter_id: "agent-openai",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[crate::agent_integration::types::Platform::Macos],
    config_format: DocumentFormat::Json,
    config_path_template: "${HOME}/.workbuddy/models.json",
    owned_fields: &["models", "availableModels"],
    requires_virtual_key: true,
    restart_required: false,
};

fn model_endpoint(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

fn model_value(input: &ConnectInput<'_>) -> Result<Value, String> {
    let token = input
        .token
        .ok_or_else(|| "WorkBuddy 接入缺少虚拟 Key".to_string())?;
    let tools = input.model_metadata.is_some_and(|metadata| metadata.tools);
    let reasoning = input
        .model_metadata
        .is_some_and(|metadata| metadata.reasoning);
    let mut model = json!({
        "id": MODEL_ID,
        "name": "Token Station",
        "vendor": "Token Station",
        "url": model_endpoint(input.base_url),
        "apiKey": token,
        "supportsToolCall": tools,
        "supportsImages": input.model_metadata.is_some_and(|metadata| metadata.vision),
        "supportsReasoning": reasoning,
        "onlyReasoning": false,
        "useCustomProtocol": true
    });
    if reasoning {
        model["reasoning"] = json!({
            "defaultEffort": "medium",
            "supportedEfforts": ["low", "medium", "high"],
            "canDisableThinking": true
        });
    }
    if let Some(metadata) = input.model_metadata {
        model["maxInputTokens"] = json!(metadata.context);
        model["maxOutputTokens"] = json!(metadata.output);
    }
    Ok(model)
}

fn arrays(document: &ConfigDocument) -> Result<(Vec<Value>, Vec<Value>, bool), String> {
    let root = semantic_json(document)?;
    if let Value::Array(items) = root {
        let available = items
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(|id| json!(id)))
            .collect();
        return Ok((items, available, true));
    }
    let models = match root.get("models") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return Err("WorkBuddy models.json 的 models 必须是数组".to_string()),
    };
    let available = match root.get("availableModels") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return Err("WorkBuddy models.json 的 availableModels 必须是数组".to_string()),
    };
    Ok((models, available, false))
}

fn replace_arrays(models: Vec<Value>, available: Vec<Value>, native_array: bool) -> Vec<PatchOperation> {
    if native_array {
        return vec![PatchOperation {
            operation: PatchKind::Replace,
            path: path(MODELS_PATH),
            value: Some(Value::Array(models)),
        }];
    }
    vec![
        PatchOperation {
            operation: PatchKind::Replace,
            path: path(MODELS_PATH),
            value: Some(Value::Array(models)),
        },
        PatchOperation {
            operation: PatchKind::Replace,
            path: path(AVAILABLE_MODELS_PATH),
            value: Some(Value::Array(available)),
        },
    ]
}

impl Connector for WorkBuddyConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".workbuddy/models.json")
    }

    fn create_dir_error(&self) -> &'static str {
        "创建 ~/.workbuddy 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(MODELS_PATH), path(AVAILABLE_MODELS_PATH)]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(MODELS_PATH)]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 WorkBuddy：网关未加载 agent-openai，/v1/chat/completions 无入站适配器。本次未修改 models.json。"
                    .to_string(),
            );
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "WorkBuddy 接入缺少虚拟 Key".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let (models, _, _) = arrays(document)?;
        if models
            .iter()
            .any(|model| model.get("id").and_then(Value::as_str) == Some(MODEL_ID))
        {
            return Err(format!(
                "WorkBuddy 已存在模型 {MODEL_ID}；为避免覆盖用户配置，本次拒绝接入"
            ));
        }
        Ok(())
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        Ok(replace_arrays(
            vec![model_value(input)?],
            vec![json!(MODEL_ID)],
            false,
        ))
    }

    fn connect_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<PatchOperation>, String> {
        self.validate_source(document)?;
        let (mut models, mut available, native_array) = arrays(document)?;
        models.push(model_value(input)?);
        if !available.iter().any(|item| item.as_str() == Some(MODEL_ID)) {
            available.push(json!(MODEL_ID));
        }
        Ok(replace_arrays(models, available, native_array))
    }

    fn validate_refresh_source(&self, document: &ConfigDocument) -> Result<(), String> {
        arrays(document).map(|_| ())
    }

    fn refresh_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<PatchOperation>, String> {
        let (mut models, mut available, native_array) = arrays(document)?;
        let replacement = model_value(input)?;
        let mut replaced = false;
        for model in &mut models {
            if model.get("id").and_then(Value::as_str) == Some(MODEL_ID) {
                *model = replacement.clone();
                replaced = true;
            }
        }
        if !replaced {
            return Err("WorkBuddy refresh requires the managed tokenstation-auto model".to_string());
        }
        if !available.iter().any(|item| item.as_str() == Some(MODEL_ID)) {
            available.push(json!(MODEL_ID));
        }
        Ok(replace_arrays(models, available, native_array))
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

    fn disconnect_patch_for_document(
        &self,
        document: &ConfigDocument,
    ) -> Result<Vec<PatchOperation>, String> {
        let (models, available, native_array) = arrays(document)?;
        let models = models
            .into_iter()
            .filter(|model| model.get("id").and_then(Value::as_str) != Some(MODEL_ID))
            .collect();
        let available = available
            .into_iter()
            .filter(|item| item.as_str() != Some(MODEL_ID))
            .collect();
        Ok(replace_arrays(models, available, native_array))
    }

    fn validate_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<(), String> {
        let (models, available, _) = arrays(document)?;
        let expected = model_value(input)?;
        let valid = models.iter().any(|model| model == &expected)
            && available.iter().any(|item| item.as_str() == Some(MODEL_ID));
        if valid {
            Ok(())
        } else {
            Err("WorkBuddy 写入前复验失败".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = input.model_metadata.map_or("模型限制未知，未写入猜测值", |_| {
            "已同步上下文和最大输出限制"
        });
        format!(
            "WorkBuddy 已加入 Token Station 自定义模型并指向 {}；{}；原 models.json 已备份。",
            model_endpoint(input.base_url), metadata
        )
    }
}
