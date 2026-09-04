use std::path::{Path, PathBuf};

use serde_json::json;
use token_station_cli::config::CLAUDE_CODE_TOKEN_STATION_AUTO_MODEL_ID;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{
    semantic_json, semantic_value_at, ConfigDocument, DocumentFormat,
};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const OWNED_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "MAX_THINKING_TOKENS",
    "CLAUDE_CODE_DISABLE_THINKING",
    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "ANTHROPIC_CUSTOM_MODEL_OPTION",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
];
const OWNED_FIELDS: &[&str] = &[
    "model",
    "modelPicker",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "MAX_THINKING_TOKENS",
    "CLAUDE_CODE_DISABLE_THINKING",
    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "ANTHROPIC_CUSTOM_MODEL_OPTION",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
];

#[cfg(test)]
const PRE_CUSTOM_MODEL_OWNED_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "MAX_THINKING_TOKENS",
    "CLAUDE_CODE_DISABLE_THINKING",
    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];
const MIN_AUTO_COMPACT_WINDOW: u32 = 100_000;
const MAX_AUTO_COMPACT_WINDOW: u32 = 1_000_000;
const MODEL_PICKER_FIELD: &str = "modelPicker";

struct CustomModelOption {
    id: &'static str,
    name: &'static str,
    description: &'static str,
}

impl CustomModelOption {
    fn env_values(&self) -> [(&'static str, &'static str); 3] {
        [
            ("ANTHROPIC_CUSTOM_MODEL_OPTION", self.id),
            ("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME", self.name),
            (
                "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
                self.description,
            ),
        ]
    }
}

const CUSTOM_MODEL: CustomModelOption = CustomModelOption {
    id: CLAUDE_CODE_TOKEN_STATION_AUTO_MODEL_ID,
    name: "Token Station Auto",
    description: "Routes requests through the active Token Station Agent route.",
};

const LEGACY_DEFAULT_MODEL_ENV_VALUES: &[(&str, &str)] = &[
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "fast"),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", "balanced"),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", "power"),
];

const LEGACY_CUSTOM_MODEL: CustomModelOption = CustomModelOption {
    id: "claude-fable-5-1",
    name: "Fable via Token Station",
    description: "Route Claude Fable through the configured Token Station pool",
};

pub(super) static CONNECTOR: ClaudeCodeConnector = ClaudeCodeConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "claude-code-v1",
    agent_id: "claude-code",
    label: "Claude Code settings.json",
    adapter_id: "agent-anthropic",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::Origin,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Json,
    config_path_template: "${HOME}/.claude/settings.json",
    owned_fields: OWNED_FIELDS,
    requires_virtual_key: true,
    restart_required: true,
};

pub struct ClaudeCodeConnector;

impl Connector for ClaudeCodeConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "claude-code-v1"
    }

    fn agent_id(&self) -> &'static str {
        "claude-code"
    }

    fn label(&self) -> &'static str {
        "Claude Code settings.json"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Json
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".claude").join("settings.json")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.claude 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        let mut paths = OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect::<Vec<_>>();
        paths.push(path(&["model"]));
        paths.push(path(&[MODEL_PICKER_FIELD]));
        paths
    }

    fn legacy_owned_paths(&self) -> Vec<ConfigPath> {
        LEGACY_DEFAULT_MODEL_ENV_VALUES
            .iter()
            .map(|(key, _)| path(&["env", key]))
            .collect()
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["env", "ANTHROPIC_AUTH_TOKEN"])]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn refreshes_managed_configuration(&self) -> bool {
        true
    }

    fn refresh_requires_baseline(&self, owned_paths: &[ConfigPath]) -> bool {
        owned_paths.contains(&path(&[MODEL_PICKER_FIELD]))
            || self
                .owned_paths()
            .iter()
            .any(|owned| !owned_paths.contains(owned))
            || LEGACY_DEFAULT_MODEL_ENV_VALUES
                .iter()
                .any(|(key, _)| owned_paths.contains(&path(&["env", key])))
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 Claude Code:网关入站适配器(plugins.agent)还不支持 Anthropic \
                 协议,agent-anthropic 尚未就位。现在接入会把 ~/.claude/settings.json 指向一个\
                 无法应答 Anthropic 请求的代理,反而掐断你正在运行的 Claude Code。等 agent-anthropic \
                 入站适配器配好后再接。(Codex / opencode 走 OpenAI 协议,现在即可正常接入。)"
                    .to_string(),
            );
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())?;
        route_context_values(input).map(|_| ())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Json(root) = document else {
            return Err("Claude Code 连接器收到错误的配置格式".to_string());
        };
        if !root
            .get("env")
            .is_none_or(|value| value.is_object() || value.is_null())
        {
            return Err("Claude Code settings.json 的 env 必须是对象".to_string());
        }
        if !root.get("availableModels").is_none_or(|value| {
            value.is_null()
                || value
                    .as_array()
                    .is_some_and(|models| models.iter().all(serde_json::Value::is_string))
        }) {
            return Err("Claude Code settings.json 的 availableModels 必须是字符串数组".to_string());
        }
        validate_model_picker(root)?;
        Ok(())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let mut operations = managed_env_values(input)?
            .into_iter()
            .map(|(key, value)| PatchOperation {
                operation: PatchKind::Replace,
                path: path(&["env", key]),
                value: Some(json!(value)),
            })
            .collect::<Vec<_>>();
        operations.push(PatchOperation {
            operation: PatchKind::Replace,
            path: path(&["model"]),
            value: Some(json!(CUSTOM_MODEL.id)),
        });
        operations.push(PatchOperation {
            operation: PatchKind::Replace,
            path: path(&[MODEL_PICKER_FIELD]),
            value: Some(projected_model_picker(None, input)?),
        });
        Ok(operations)
    }

    fn connect_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<PatchOperation>, String> {
        self.validate_source(document)?;
        validate_custom_model_visibility(document)?;
        reject_unowned_picker_model_conflict(document)?;
        let ConfigDocument::Json(root) = document else {
            unreachable!();
        };
        let mut operations = self.connect_patch(input)?;
        let picker = projected_model_picker(root.get(MODEL_PICKER_FIELD), input)?;
        let operation = operations
            .iter_mut()
            .find(|operation| operation.path == path(&[MODEL_PICKER_FIELD]))
            .expect("connect patch always owns modelPicker");
        operation.value = Some(picker);
        Ok(operations)
    }

    fn refresh_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        refreshed_patch(document, None, input, owned_paths)
    }

    fn refresh_patch_with_baseline(
        &self,
        document: &ConfigDocument,
        baseline: Option<&ConfigDocument>,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        refreshed_patch(document, baseline, input, owned_paths)
    }

    fn disconnect_patch(&self) -> Vec<PatchOperation> {
        let mut operations = OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .map(|path| PatchOperation {
                operation: PatchKind::Remove,
                path,
                value: None,
            })
            .collect::<Vec<_>>();
        operations.push(PatchOperation {
            operation: PatchKind::Remove,
            path: path(&["model"]),
            value: None,
        });
        operations.push(PatchOperation {
            operation: PatchKind::Remove,
            path: path(&[MODEL_PICKER_FIELD]),
            value: None,
        });
        operations
    }

    fn disconnect_patch_for_document(
        &self,
        document: &ConfigDocument,
    ) -> Result<Vec<PatchOperation>, String> {
        let ConfigDocument::Json(root) = document else {
            return Err("Claude Code 连接器收到错误的配置格式".to_string());
        };
        validate_model_picker(root)?;
        let mut operations = self
            .disconnect_patch()
            .into_iter()
            .filter(|operation| {
                operation.path != path(&["model"])
                    && operation.path != path(&[MODEL_PICKER_FIELD])
            })
            .collect::<Vec<_>>();
        if root.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id) {
            operations.push(PatchOperation {
                operation: PatchKind::Remove,
                path: path(&["model"]),
                value: None,
            });
        }
        operations.push(match stripped_model_picker(root.get(MODEL_PICKER_FIELD))? {
            Some(picker) => PatchOperation {
                operation: PatchKind::Replace,
                path: path(&[MODEL_PICKER_FIELD]),
                value: Some(picker),
            },
            None => PatchOperation {
                operation: PatchKind::Remove,
                path: path(&[MODEL_PICKER_FIELD]),
                value: None,
            },
        });
        operations.extend(legacy_model_migration_patch(document, None, None)?);
        Ok(operations)
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
        let env = root["env"]
            .as_object()
            .ok_or_else(|| "Claude Code settings.json 的 env 必须是对象".to_string())?;
        if root.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id)
            && valid_projected_model_picker(root, input)?
            && managed_env_values(input)?.iter().all(|(key, value)| {
                env.get(*key).and_then(serde_json::Value::as_str) == Some(value.as_str())
            })
        {
            Ok(())
        } else {
            Err("Claude Code 写入前复验缺少受管 env 字段".to_string())
        }
    }

    fn validate_refresh_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<(), String> {
        if CUSTOM_MODEL
            .env_values()
            .iter()
            .all(|(key, _)| owned_paths.contains(&path(&["env", key])))
        {
            return self.validate_projected(document, input);
        }

        validate_non_custom_fields(document, input)
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
             "Claude Code 已指向 {}(~/.claude/settings.json,已备份)。\
             已保留内置模型，新增并默认选择 Token Station Auto；\
             已关闭当前 Canonical IR 暂不支持的 thinking/beta；\
             使用 /v1/messages，经 agent-anthropic 入站适配器转发。请重启 Claude Code 以加载新模型。",
            input.base_url
        )
    }
}

fn managed_env_values(input: &ConnectInput<'_>) -> Result<Vec<(&'static str, String)>, String> {
    let token = input
        .token
        .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())?;
    let (context, compact_window) = route_context_values(input)?;
    let display_name = custom_model_display_name(
        context
            .parse::<u32>()
            .map_err(|_| "Claude Code 上下文容量超出支持范围".to_string())?,
    );
    let mut values = vec![
        ("ANTHROPIC_BASE_URL", input.base_url.to_string()),
        ("ANTHROPIC_AUTH_TOKEN", token.to_string()),
        ("MAX_THINKING_TOKENS", "0".to_string()),
        ("CLAUDE_CODE_DISABLE_THINKING", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_string()),
    ];
    values.extend([
        ("ANTHROPIC_CUSTOM_MODEL_OPTION", CUSTOM_MODEL.id.to_string()),
        ("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME", display_name),
        (
            "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
            CUSTOM_MODEL.description.to_string(),
        ),
    ]);
    values.extend([
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", context),
        ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", compact_window),
    ]);
    Ok(values)
}

fn custom_model_display_name(context: u32) -> String {
    let context = if context.is_multiple_of(1_000_000) {
        format!("{}M", context / 1_000_000)
    } else if context.is_multiple_of(1_000) {
        format!("{}K", context / 1_000)
    } else {
        context.to_string()
    };
    format!("{} ({context} context)", CUSTOM_MODEL.name)
}

fn projected_model_picker(
    existing: Option<&serde_json::Value>,
    input: &ConnectInput<'_>,
) -> Result<serde_json::Value, String> {
    let (context, _) = route_context_values(input)?;
    let context = context
        .parse::<u32>()
        .map_err(|_| "Claude Code 上下文容量超出支持范围".to_string())?;
    let mut picker = existing
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let object = picker
        .as_object_mut()
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker 必须是对象".to_string())?;
    let mut options = object
        .remove("options")
        .unwrap_or_else(|| json!([]))
        .as_array()
        .cloned()
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker.options 必须是数组".to_string())?;
    options.retain(|row| {
        row.get("model").and_then(serde_json::Value::as_str) != Some(CUSTOM_MODEL.id)
    });
    options.push(json!({
        "model": CUSTOM_MODEL.id,
        "label": custom_model_display_name(context),
        "description": CUSTOM_MODEL.description,
    }));
    object.insert("options".to_string(), json!(options));
    object.insert("replaceBuiltInOptions".to_string(), json!(false));
    Ok(picker)
}

fn validate_model_picker(root: &serde_json::Value) -> Result<(), String> {
    let Some(picker) = root.get(MODEL_PICKER_FIELD).filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let picker = picker
        .as_object()
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker 必须是对象".to_string())?;
    let options = picker
        .get("options")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker.options 必须是数组".to_string())?;
    for (index, row) in options.iter().enumerate() {
        let Some(row) = row.as_object() else {
            return Err(format!(
                "Claude Code settings.json 的 modelPicker.options[{index}] 必须是对象"
            ));
        };
        if row
            .get("model")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|model| model.trim().is_empty())
        {
            return Err(format!(
                "Claude Code settings.json 的 modelPicker.options[{index}].model 必须是非空字符串"
            ));
        }
        for key in ["label", "description", "behavesAs"] {
            if row.get(key).is_some_and(|value| !value.is_string()) {
                return Err(format!(
                    "Claude Code settings.json 的 modelPicker.options[{index}].{key} 必须是字符串"
                ));
            }
        }
    }
    if picker
        .get("replaceBuiltInOptions")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(
            "Claude Code settings.json 的 modelPicker.replaceBuiltInOptions 必须是布尔值"
                .to_string(),
        );
    }
    Ok(())
}

fn valid_projected_model_picker(
    root: &serde_json::Value,
    input: &ConnectInput<'_>,
) -> Result<bool, String> {
    let expected = projected_model_picker(None, input)?["options"][0].clone();
    let picker = root.get(MODEL_PICKER_FIELD);
    Ok(picker
        .and_then(|value| value.get("replaceBuiltInOptions"))
        .and_then(serde_json::Value::as_bool)
        == Some(false)
        && picker
            .and_then(|value| value.get("options"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|options| options.iter().filter(|row| row["model"] == CUSTOM_MODEL.id).count() == 1
                && options.contains(&expected)))
}

fn stripped_model_picker(
    existing: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(existing) = existing.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let mut picker = existing
        .as_object()
        .cloned()
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker 必须是对象".to_string())?;
    let mut options = picker
        .remove("options")
        .unwrap_or_else(|| json!([]))
        .as_array()
        .cloned()
        .ok_or_else(|| "Claude Code settings.json 的 modelPicker.options 必须是数组".to_string())?;
    options.retain(|row| {
        row.get("model").and_then(serde_json::Value::as_str) != Some(CUSTOM_MODEL.id)
    });
    picker.remove("replaceBuiltInOptions");
    if !options.is_empty() || !picker.is_empty() {
        picker.insert("options".to_string(), json!(options));
    }
    if picker.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::Value::Object(picker)))
    }
}

fn route_context_values(input: &ConnectInput<'_>) -> Result<(String, String), String> {
    let metadata = input
        .model_metadata
        .ok_or_else(|| {
            "Claude Code 的 Token Station Auto 缺少可信上下文或最大输出容量；本次未修改配置"
                .to_string()
        })?;
    let (context, _) = metadata.safe_limits().ok_or_else(|| {
        "Claude Code 的 Token Station Auto 缺少可信上下文或最大输出容量；本次未修改配置"
            .to_string()
    })?;
    let max_input = metadata.safe_max_input().ok_or_else(|| {
        "Claude Code 的 Token Station Auto 缺少可信输入预算；本次未修改配置".to_string()
    })?;
    let compact_window = max_input.clamp(
        MIN_AUTO_COMPACT_WINDOW,
        MAX_AUTO_COMPACT_WINDOW,
    );
    Ok((context.to_string(), compact_window.to_string()))
}

fn is_custom_model_key(key: &str) -> bool {
    CUSTOM_MODEL
        .env_values()
        .iter()
        .any(|(custom_key, _)| *custom_key == key)
}

fn validate_non_custom_fields(
    document: &ConfigDocument,
    input: &ConnectInput<'_>,
) -> Result<(), String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    let env = root["env"]
        .as_object()
        .ok_or_else(|| "Claude Code settings.json 的 env 必须是对象".to_string())?;
    if root.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id)
        && valid_projected_model_picker(root, input)?
        && managed_env_values(input)?
        .iter()
        .filter(|(key, _)| !is_custom_model_key(key))
        .all(|(key, value)| {
            env.get(*key).and_then(serde_json::Value::as_str) == Some(value.as_str())
        })
    {
        Ok(())
    } else {
        Err("Claude Code 写入前复验缺少受管 env 字段".to_string())
    }
}

fn refreshed_patch(
    document: &ConfigDocument,
    baseline: Option<&ConfigDocument>,
    input: &ConnectInput<'_>,
    owned_paths: &[ConfigPath],
) -> Result<Vec<PatchOperation>, String> {
    ClaudeCodeConnector.validate_source(document)?;
    validate_custom_model_visibility(document)?;
    reject_unowned_custom_option_conflict(document, input, owned_paths)?;
    if !owned_paths.contains(&path(&[MODEL_PICKER_FIELD])) {
        reject_unowned_picker_model_conflict(document)?;
    }
    reject_owned_model_picker_drift(document, baseline, owned_paths)?;
    reject_unsafe_ownership_widening(document, baseline, owned_paths)?;
    let mut operations = legacy_model_migration_patch(
        document,
        baseline,
        Some(owned_paths),
    )?;
    let ConfigDocument::Json(root) = document else {
        unreachable!();
    };
    let mut managed = ClaudeCodeConnector.connect_patch(input)?;
    let picker = projected_model_picker(root.get(MODEL_PICKER_FIELD), input)?;
    managed
        .iter_mut()
        .find(|operation| operation.path == path(&[MODEL_PICKER_FIELD]))
        .expect("connect patch always owns modelPicker")
        .value = Some(picker);
    operations.extend(managed);
    Ok(operations)
}

fn reject_unowned_picker_model_conflict(document: &ConfigDocument) -> Result<(), String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    let has_conflict = root
        .get(MODEL_PICKER_FIELD)
        .and_then(|picker| picker.get("options"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|options| {
            options.iter().any(|row| {
                row.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id)
            })
        });
    if has_conflict {
        Err(
            "Claude Code modelPicker 已有 ID 为 `token-station-auto` 的未受管自定义行；请先移除或改名该行后再连接"
                .to_string(),
        )
    } else {
        Ok(())
    }
}

fn reject_owned_model_picker_drift(
    document: &ConfigDocument,
    baseline: Option<&ConfigDocument>,
    owned_paths: &[ConfigPath],
) -> Result<(), String> {
    if !owned_paths.contains(&path(&[MODEL_PICKER_FIELD])) {
        return Ok(());
    }
    let baseline = baseline.ok_or_else(|| {
        "Claude Code 旧连接记录缺少原始基线，不能安全刷新 modelPicker；请断开后重新连接"
            .to_string()
    })?;
    let current = semantic_json(document)?;
    let baseline = semantic_json(baseline)?;
    let current_picker = current.get(MODEL_PICKER_FIELD).filter(|value| !value.is_null());
    let baseline_picker = baseline.get(MODEL_PICKER_FIELD).filter(|value| !value.is_null());
    if current_picker.is_none() && baseline_picker.is_none() {
        return Ok(());
    }
    let Some(current_picker) = current_picker.and_then(serde_json::Value::as_object) else {
        return Err("Claude Code 的受管 modelPicker 已被修改；请先恢复或断开连接".to_string());
    };
    let Some(options) = current_picker
        .get("options")
        .and_then(serde_json::Value::as_array)
    else {
        return Err("Claude Code 的受管 modelPicker.options 已被修改；请先恢复或断开连接".to_string());
    };
    let managed_rows = options
        .iter()
        .filter(|row| row.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id))
        .collect::<Vec<_>>();
    if managed_rows.len() != 1 || !valid_managed_picker_row(managed_rows[0]) {
        return Err("Claude Code 的 Token Station Auto modelPicker 行已被修改；请先恢复或断开连接".to_string());
    }

    let mut expected = baseline_picker.cloned().unwrap_or_else(|| json!({}));
    let expected_object = expected
        .as_object_mut()
        .ok_or_else(|| "Claude Code 原始基线的 modelPicker 必须是对象".to_string())?;
    let mut baseline_options = expected_object
        .remove("options")
        .unwrap_or_else(|| json!([]))
        .as_array()
        .cloned()
        .ok_or_else(|| "Claude Code 原始基线的 modelPicker.options 必须是数组".to_string())?;
    baseline_options.retain(|row| {
        row.get("model").and_then(serde_json::Value::as_str) != Some(CUSTOM_MODEL.id)
    });
    baseline_options.push((*managed_rows[0]).clone());
    expected_object.insert("options".to_string(), json!(baseline_options));
    expected_object.insert("replaceBuiltInOptions".to_string(), json!(false));
    if serde_json::Value::Object(current_picker.clone()) == expected {
        Ok(())
    } else {
        Err(
            "Claude Code 的受管 modelPicker 在连接后被用户修改；Token Station 不会把该修改纳入所有权，请先恢复或断开连接"
                .to_string(),
        )
    }
}

fn valid_managed_picker_row(row: &serde_json::Value) -> bool {
    let Some(row) = row.as_object() else {
        return false;
    };
    row.len() == 3
        && row.get("model").and_then(serde_json::Value::as_str) == Some(CUSTOM_MODEL.id)
        && row.get("description").and_then(serde_json::Value::as_str)
            == Some(CUSTOM_MODEL.description)
        && row
            .get("label")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|label| {
                label.starts_with("Token Station Auto (") && label.ends_with(" context)")
            })
}

fn validate_custom_model_visibility(document: &ConfigDocument) -> Result<(), String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    let Some(available) = root
        .get("availableModels")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    if available
        .iter()
        .any(|model| model.as_str() == Some(CUSTOM_MODEL.id))
    {
        Ok(())
    } else {
        Err(
            "Claude Code 的 availableModels 未允许 `token-station-auto`，Token Station Auto 会被隐藏；请先在该允许列表中加入 `token-station-auto`"
                .to_string(),
        )
    }
}

fn reject_unowned_custom_option_conflict(
    document: &ConfigDocument,
    input: &ConnectInput<'_>,
    owned_paths: &[ConfigPath],
) -> Result<(), String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    if CUSTOM_MODEL
        .env_values()
        .iter()
        .all(|(key, _)| owned_paths.contains(&path(&["env", key])))
    {
        return Ok(());
    }

    let env = root.get("env").and_then(serde_json::Value::as_object);
    let custom_keys = CUSTOM_MODEL.env_values().map(|(key, _)| key);
    let slot_is_empty = custom_keys
        .iter()
        .all(|key| env.and_then(|env| env.get(*key)).is_none());
    let expected = managed_env_values(input)?;
    let slot_matches_current = custom_keys.iter().all(|key| {
        let expected = expected
            .iter()
            .find_map(|(expected_key, value)| (*expected_key == *key).then_some(value.as_str()));
        env.and_then(|env| env.get(*key))
            .and_then(serde_json::Value::as_str)
            == expected
    });
    let slot_matches_legacy = LEGACY_CUSTOM_MODEL.env_values().iter().all(|(key, value)| {
        env.and_then(|env| env.get(*key))
            .and_then(serde_json::Value::as_str)
            == Some(*value)
    });
    let slot_is_available = slot_is_empty || slot_matches_current || slot_matches_legacy;
    if slot_is_available {
        Ok(())
    } else {
        Err(
            "Claude Code 的唯一自定义模型槽位已被用户修改；请先断开后重新连接，以确认由 Token Station Auto 接管该槽位"
                .to_string(),
        )
    }
}

fn reject_unsafe_ownership_widening(
    document: &ConfigDocument,
    baseline: Option<&ConfigDocument>,
    owned_paths: &[ConfigPath],
) -> Result<(), String> {
    let newly_owned = ClaudeCodeConnector
        .owned_paths()
        .into_iter()
        .filter(|owned| !owned_paths.contains(owned))
        .collect::<Vec<_>>();
    if newly_owned.is_empty() {
        return Ok(());
    }
    let baseline = baseline.ok_or_else(|| {
        "Claude Code 旧连接记录缺少原始基线，不能安全扩大配置所有权；请断开后重新连接"
            .to_string()
    })?;
    let current = semantic_json(document)?;
    let baseline = semantic_json(baseline)?;
    let legacy_managed = legacy_custom_configuration_is_managed(&current, owned_paths);

    for owned in newly_owned {
        if semantic_value_at(&current, &owned) == semantic_value_at(&baseline, &owned) {
            continue;
        }
        if legacy_managed && is_safe_legacy_ownership_migration(&current, &owned) {
            continue;
        }
        let field = owned.segments.join(".");
        return Err(format!(
            "Claude Code 配置 `{field}` 在旧连接建立后被用户新增或修改，Token Station 不会静默接管；请断开后重新连接以重建基线"
        ));
    }
    Ok(())
}

fn legacy_custom_configuration_is_managed(
    current: &serde_json::Value,
    owned_paths: &[ConfigPath],
) -> bool {
    let env = current.get("env").and_then(serde_json::Value::as_object);
    let has_legacy_ownership = LEGACY_DEFAULT_MODEL_ENV_VALUES
        .iter()
        .any(|(key, _)| owned_paths.contains(&path(&["env", key])));
    has_legacy_ownership
        && LEGACY_CUSTOM_MODEL.env_values().iter().all(|(key, value)| {
            env.and_then(|env| env.get(*key))
                .and_then(serde_json::Value::as_str)
                == Some(*value)
        })
}

fn is_safe_legacy_ownership_migration(
    current: &serde_json::Value,
    owned: &ConfigPath,
) -> bool {
    if owned == &path(&["model"]) {
        return current.get("model").and_then(serde_json::Value::as_str)
            == Some(LEGACY_CUSTOM_MODEL.id);
    }
    LEGACY_CUSTOM_MODEL
        .env_values()
        .iter()
        .any(|(key, _)| owned == &path(&["env", key]))
}

fn legacy_model_migration_patch(
    document: &ConfigDocument,
    baseline: Option<&ConfigDocument>,
    owned_paths: Option<&[ConfigPath]>,
) -> Result<Vec<PatchOperation>, String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    let baseline_env = match baseline {
        Some(ConfigDocument::Json(root)) => root.get("env").and_then(serde_json::Value::as_object),
        Some(_) => return Err("Claude Code 连接器收到错误的基线配置格式".to_string()),
        None => None,
    };
    let env = root.get("env").and_then(serde_json::Value::as_object);
    let had_legacy_model_ownership = owned_paths.is_some_and(|paths| {
        LEGACY_DEFAULT_MODEL_ENV_VALUES
            .iter()
            .any(|(key, _)| paths.contains(&path(&["env", key])))
    });
    let legacy_custom_option_was_selected = owned_paths.is_some_and(|paths| {
        paths.contains(&path(&["env", "ANTHROPIC_CUSTOM_MODEL_OPTION"]))
            && LEGACY_CUSTOM_MODEL.env_values().iter().all(|(key, value)| {
                env.and_then(|env| env.get(*key))
                    .and_then(serde_json::Value::as_str)
                    == Some(*value)
            })
    });
    let mut operations = LEGACY_DEFAULT_MODEL_ENV_VALUES
        .iter()
        .filter(|(key, value)| {
            let model_path = path(&["env", key]);
            let was_owned = owned_paths.is_none_or(|paths| paths.contains(&model_path));
            was_owned
                && env
                    .and_then(|env| env.get(*key))
                    .and_then(serde_json::Value::as_str)
                    == Some(*value)
        })
        .map(|(key, _)| {
            let baseline_value = baseline_env.and_then(|env| env.get(*key)).cloned();
            PatchOperation {
                operation: if baseline_value.is_some() {
                    PatchKind::Replace
                } else {
                    PatchKind::Remove
                },
                path: path(&["env", key]),
                value: baseline_value,
            }
        })
        .collect::<Vec<_>>();
    if had_legacy_model_ownership {
        if let Some(native_model) = root
            .get("model")
            .and_then(serde_json::Value::as_str)
            .and_then(|model| native_model_selection(model, legacy_custom_option_was_selected))
        {
            operations.push(PatchOperation {
                operation: PatchKind::Replace,
                path: path(&["model"]),
                value: Some(json!(native_model)),
            });
        }
    }
    Ok(operations)
}

fn native_model_selection(model: &str, legacy_custom_option_was_selected: bool) -> Option<String> {
    let (family, suffix) = model
        .strip_suffix("[1m]")
        .map(|family| (family, "[1m]"))
        .or_else(|| model.strip_suffix("[2m]").map(|family| (family, "[2m]")))
        .unwrap_or((model, ""));
    let native = match family {
        "fast" => "haiku",
        "balanced" => "sonnet",
        "power" => "opus",
        "claude-fable-5-1" if legacy_custom_option_was_selected => CUSTOM_MODEL.id,
        "claude-fable-5-1" => "fable",
        _ => return None,
    };
    Some(format!("{native}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_integration::config_codec::{apply_patch, parse_source_bytes, semantic_json};
    use crate::agent_integration::connectors::AgentModelMetadata;

    fn route_metadata(context: u32, output: u32) -> AgentModelMetadata {
        AgentModelMetadata {
            context,
            output,
            max_input: context - output,
            vision: true,
            tools: true,
            reasoning: false,
            cost: None,
        }
    }

    #[test]
    fn connection_adds_token_station_auto_without_replacing_native_model_aliases() {
        let metadata = route_metadata(1_000_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(
                br#"{
                    "model": "user-model",
                    "modelPicker": {
                        "options": [
                            {"model": "user-gateway-model", "label": "User gateway"}
                        ],
                        "replaceBuiltInOptions": true,
                        "userExtension": "keep"
                    },
                    "env": {
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "user-haiku",
                        "ANTHROPIC_DEFAULT_SONNET_MODEL": "user-sonnet",
                        "ANTHROPIC_DEFAULT_OPUS_MODEL": "user-opus",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION": "user-custom",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": "User model",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION": "User description"
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let patch = ClaudeCodeConnector
            .connect_patch_for_document(&document, &input)
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        ClaudeCodeConnector
            .validate_projected(&document, &input)
            .unwrap();
        let root = semantic_json(&document).unwrap();
        let env = root["env"].as_object().unwrap();

        assert_eq!(env["ANTHROPIC_BASE_URL"], json!(input.base_url));
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], json!("local-virtual-key"));
        assert_eq!(root["model"], json!("token-station-auto"));
        assert_eq!(root["modelPicker"]["replaceBuiltInOptions"], json!(false));
        assert_eq!(root["modelPicker"]["userExtension"], json!("keep"));
        let picker = root["modelPicker"]["options"].as_array().unwrap();
        assert_eq!(picker.len(), 2, "one user row and one managed row remain");
        assert_eq!(picker[0]["model"], json!("user-gateway-model"));
        assert_eq!(picker[0]["label"], json!("User gateway"));
        assert_eq!(picker[1]["model"], json!("token-station-auto"));
        assert_eq!(picker[1]["label"], json!("Token Station Auto (1M context)"));
        assert!(picker[1].get("behavesAs").is_none());
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], json!("user-haiku"));
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], json!("user-sonnet"));
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("user-opus"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION"],
            json!("token-station-auto")
        );
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"],
            json!("Token Station Auto (1M context)")
        );
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
            json!("Routes requests through the active Token Station Agent route.")
        );
        assert_eq!(env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], json!("1000000"));
        assert_eq!(
            env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"],
            json!("967232")
        );
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_FABLE_MODEL"));
        assert!(!env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"));
        for key in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ] {
            assert!(
                !OWNED_ENV_KEYS.contains(&key),
                "the Connector must not own Claude Code native model aliases: {key}"
            );
        }
        assert!(ClaudeCodeConnector.owned_paths().contains(&path(&["model"])));
        assert!(ClaudeCodeConnector
            .owned_paths()
            .contains(&path(&["modelPicker"])));
        for key in [
            "ANTHROPIC_CUSTOM_MODEL_OPTION",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        ] {
            assert!(
                OWNED_ENV_KEYS.contains(&key),
                "the Connector must own its one custom model option: {key}"
            );
        }
        assert!(ClaudeCodeConnector.refreshes_managed_configuration());
        assert!(ClaudeCodeConnector.projects_model_metadata());
        assert!(ClaudeCodeConnector.capabilities().restart_required);
        assert!(ClaudeCodeConnector.refresh_requires_baseline(
            &ClaudeCodeConnector
                .owned_paths()
                .into_iter()
                .chain(ClaudeCodeConnector.legacy_owned_paths())
                .collect::<Vec<_>>()
        ));
        assert!(ClaudeCodeConnector
            .refresh_requires_baseline(&ClaudeCodeConnector.owned_paths()));
        let without_picker = ClaudeCodeConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["modelPicker"]))
            .collect::<Vec<_>>();
        assert!(ClaudeCodeConnector.refresh_requires_baseline(&without_picker));
    }

    #[test]
    fn managed_refresh_removes_only_the_legacy_token_station_model_overrides() {
        let metadata = route_metadata(200_000, 16_384);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
            Some(
                br#"{
                    "model": "power[1m]",
                    "env": {
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "fast",
                        "ANTHROPIC_DEFAULT_SONNET_MODEL": "balanced",
                        "ANTHROPIC_DEFAULT_OPUS_MODEL": "user-opus",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION": "claude-fable-5-1",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": "Fable via Token Station",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION": "Route Claude Fable through the configured Token Station pool"
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        let previously_owned = ClaudeCodeConnector
            .owned_paths()
            .into_iter()
            .chain(ClaudeCodeConnector.legacy_owned_paths())
            .collect::<Vec<_>>();

        let baseline = parse_source_bytes(None, DocumentFormat::Json, "Claude Code").unwrap();
        let patch = ClaudeCodeConnector
            .refresh_patch_with_baseline(
                &document,
                Some(&baseline),
                &input,
                &previously_owned,
            )
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        ClaudeCodeConnector
            .validate_refresh_projected(&document, &input, &previously_owned)
            .unwrap();
        let root = semantic_json(&document).unwrap();
        let env = root["env"].as_object().unwrap();

        assert_eq!(root["model"], json!("token-station-auto"));
        assert_eq!(
            root["modelPicker"]["options"][0]["model"],
            json!("token-station-auto")
        );
        assert_eq!(root["modelPicker"]["replaceBuiltInOptions"], json!(false));
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"));
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_SONNET_MODEL"));
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("user-opus"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION"],
            json!("token-station-auto")
        );
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"],
            json!("Token Station Auto (200K context)")
        );
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
            json!("Routes requests through the active Token Station Agent route.")
        );
    }

    #[test]
    fn managed_refresh_rejects_a_custom_option_changed_after_connection() {
        let metadata = route_metadata(128_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(
                br#"{
                    "env": {
                        "ANTHROPIC_CUSTOM_MODEL_OPTION": "user-custom",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": "User model",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION": "User description"
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        let previously_owned = PRE_CUSTOM_MODEL_OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect::<Vec<_>>();

        let Err(error) = ClaudeCodeConnector.refresh_patch_for_document(
            &document,
            &input,
            &previously_owned,
        ) else {
            panic!("a changed custom model slot must block managed refresh");
        };
        assert!(error.contains("唯一自定义模型槽位已被用户修改"));
        assert_eq!(
            semantic_json(&document).unwrap()["env"]["ANTHROPIC_CUSTOM_MODEL_OPTION"],
            json!("user-custom")
        );
    }

    #[test]
    fn managed_refresh_rejects_an_unowned_picker_added_after_connection() {
        let metadata = route_metadata(200_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(
                br#"{
                    "modelPicker": {
                        "options": [{"model": "user-model", "label": "User model"}],
                        "replaceBuiltInOptions": true
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        let baseline = parse_source_bytes(None, DocumentFormat::Json, "Claude Code").unwrap();
        let previously_owned = ClaudeCodeConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["modelPicker"]))
            .collect::<Vec<_>>();

        let Err(error) = ClaudeCodeConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &previously_owned,
        ) else {
            panic!("an unowned picker added after connection must block ownership widening");
        };

        assert!(error.contains("modelPicker"), "{error}");
        assert!(error.contains("断开后重新连接"), "{error}");
        assert_eq!(
            semantic_json(&document).unwrap()["modelPicker"]["options"][0]["model"],
            json!("user-model")
        );
    }

    #[test]
    fn managed_refresh_rejects_a_user_picker_edit_after_connection() {
        let metadata = route_metadata(200_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let baseline_bytes = br#"{"modelPicker":{"options":[{"model":"baseline-model"}]}}"#;
        let baseline = parse_source_bytes(
            Some(baseline_bytes),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        let mut document =
            parse_source_bytes(Some(baseline_bytes), DocumentFormat::Json, "Claude Code").unwrap();
        let connect = ClaudeCodeConnector
            .connect_patch_for_document(&document, &input)
            .unwrap();
        apply_patch(&mut document, &connect).unwrap();
        let ConfigDocument::Json(root) = &mut document else {
            unreachable!();
        };
        root["modelPicker"]["options"]
            .as_array_mut()
            .unwrap()
            .insert(1, json!({"model": "added-after-connect"}));

        let Err(error) = ClaudeCodeConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &ClaudeCodeConnector.owned_paths(),
        ) else {
            panic!("refresh must not absorb a user picker edit into managed ownership");
        };
        assert!(error.contains("modelPicker"), "{error}");
    }

    #[test]
    fn force_disconnect_removes_only_the_token_station_picker_row() {
        let mut document = parse_source_bytes(
            Some(
                br#"{
                    "model": "token-station-auto",
                    "modelPicker": {
                        "options": [
                            {"model": "user-model", "label": "User model"},
                            {"model": "token-station-auto", "label": "Token Station Auto (1M context)", "description": "Routes requests through the active Token Station Agent route."}
                        ],
                        "replaceBuiltInOptions": false,
                        "userExtension": "keep"
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let disconnect = ClaudeCodeConnector
            .disconnect_patch_for_document(&document)
            .unwrap();
        apply_patch(&mut document, &disconnect).unwrap();
        let root = semantic_json(&document).unwrap();

        assert!(root.get("model").is_none());
        assert_eq!(root["modelPicker"]["options"].as_array().unwrap().len(), 1);
        assert_eq!(root["modelPicker"]["options"][0]["model"], "user-model");
        assert_eq!(root["modelPicker"]["userExtension"], "keep");
        assert!(root["modelPicker"].get("replaceBuiltInOptions").is_none());
    }

    #[test]
    fn force_disconnect_preserves_a_native_model_selected_after_connection() {
        let mut document = parse_source_bytes(
            Some(br#"{"model":"opus[1m]","modelPicker":{"options":[{"model":"token-station-auto","label":"Token Station Auto (1M context)","description":"Routes requests through the active Token Station Agent route."}],"replaceBuiltInOptions":false}}"#),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let disconnect = ClaudeCodeConnector
            .disconnect_patch_for_document(&document)
            .unwrap();
        apply_patch(&mut document, &disconnect).unwrap();

        assert_eq!(semantic_json(&document).unwrap()["model"], "opus[1m]");
    }

    #[test]
    fn force_disconnect_keeps_empty_options_with_user_picker_extensions() {
        let mut document = parse_source_bytes(
            Some(br#"{"modelPicker":{"options":[{"model":"token-station-auto","label":"Token Station Auto (1M context)","description":"Routes requests through the active Token Station Agent route."}],"replaceBuiltInOptions":false,"userExtension":"keep"}}"#),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let disconnect = ClaudeCodeConnector
            .disconnect_patch_for_document(&document)
            .unwrap();
        apply_patch(&mut document, &disconnect).unwrap();
        ClaudeCodeConnector.validate_source(&document).unwrap();
        let root = semantic_json(&document).unwrap();

        assert_eq!(root["modelPicker"]["options"], json!([]));
        assert_eq!(root["modelPicker"]["userExtension"], "keep");
    }

    #[test]
    fn legacy_refresh_rejects_an_unowned_picker_row_with_the_managed_id() {
        let metadata = route_metadata(200_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let picker = br#"{"modelPicker":{"options":[{"model":"token-station-auto","label":"User label","description":"User description"}]}}"#;
        let document =
            parse_source_bytes(Some(picker), DocumentFormat::Json, "Claude Code").unwrap();
        let baseline =
            parse_source_bytes(Some(picker), DocumentFormat::Json, "Claude Code").unwrap();
        let previously_owned = PRE_CUSTOM_MODEL_OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect::<Vec<_>>();

        let Err(error) = ClaudeCodeConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &previously_owned,
        ) else {
            panic!("an unowned same-ID picker row must block ownership widening");
        };
        assert!(error.contains("token-station-auto"), "{error}");
        assert!(error.contains("自定义行"), "{error}");
    }

    #[test]
    fn managed_refresh_rejects_same_custom_id_with_user_metadata() {
        let metadata = route_metadata(200_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(
                br#"{
                    "env": {
                        "ANTHROPIC_CUSTOM_MODEL_OPTION": "token-station-auto",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": "User-owned label",
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION": "User-owned description"
                    }
                }"#,
            ),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        let baseline = parse_source_bytes(None, DocumentFormat::Json, "Claude Code").unwrap();
        let previously_owned = PRE_CUSTOM_MODEL_OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect::<Vec<_>>();

        let Err(error) = ClaudeCodeConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &previously_owned,
        ) else {
            panic!("matching only the custom ID must not authorize user metadata overwrite");
        };
        assert!(error.contains("唯一自定义模型槽位"), "{error}");
    }

    #[test]
    fn managed_refresh_tracks_the_effective_route_limits() {
        let initial_metadata = route_metadata(200_000, 32_000);
        let initial_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&initial_metadata),
        };
        let mut document = parse_source_bytes(
            None,
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();
        apply_patch(
            &mut document,
            &ClaudeCodeConnector.connect_patch(&initial_input).unwrap(),
        )
        .unwrap();

        let refreshed_metadata = route_metadata(1_000_000, 32_768);
        let refreshed_input = ConnectInput {
            model_metadata: Some(&refreshed_metadata),
            ..initial_input
        };
        let owned_paths = ClaudeCodeConnector.owned_paths();
        let baseline = parse_source_bytes(None, DocumentFormat::Json, "Claude Code").unwrap();
        let patch = ClaudeCodeConnector
            .refresh_patch_with_baseline(
                &document,
                Some(&baseline),
                &refreshed_input,
                &owned_paths,
            )
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        ClaudeCodeConnector
            .validate_refresh_projected(&document, &refreshed_input, &owned_paths)
            .unwrap();

        let root = semantic_json(&document).unwrap();
        assert_eq!(
            root["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"],
            json!("1000000")
        );
        assert_eq!(
            root["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"],
            json!("967232")
        );
    }

    #[test]
    fn context_projection_clamps_auto_compaction_to_claude_supported_bounds() {
        let low = route_metadata(64_000, 8_000);
        let low_input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&low),
        };
        assert_eq!(
            route_context_values(&low_input).unwrap(),
            ("64000".to_string(), "100000".to_string())
        );

        let high = route_metadata(2_000_000, 100_000);
        let high_input = ConnectInput {
            model_metadata: Some(&high),
            ..low_input
        };
        assert_eq!(
            route_context_values(&high_input).unwrap(),
            ("2000000".to_string(), "1000000".to_string())
        );
    }

    #[test]
    fn connection_fails_closed_without_trusted_route_limits() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        let error = ClaudeCodeConnector
            .validate_preconditions(&input)
            .unwrap_err();
        assert!(error.contains("缺少可信上下文或最大输出容量"));
        assert!(ClaudeCodeConnector.connect_patch(&input).is_err());
    }

    #[test]
    fn connection_rejects_an_allowlist_that_would_hide_token_station_auto() {
        let metadata = route_metadata(200_000, 32_000);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(br#"{"availableModels":["opus","sonnet"]}"#),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let Err(error) = ClaudeCodeConnector.connect_patch_for_document(&document, &input) else {
            panic!("a hidden custom option must block connection");
        };
        assert!(error.contains("availableModels 未允许 `token-station-auto`"));
    }

    #[test]
    fn connection_rejects_an_invalid_existing_model_picker_without_writing() {
        let metadata = route_metadata(200_000, 32_000);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(br#"{"modelPicker":{"options":"not-an-array"}}"#),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let Err(error) = ClaudeCodeConnector.connect_patch_for_document(&document, &input) else {
            panic!("an invalid picker must block connection");
        };

        assert!(error.contains("modelPicker.options"), "{error}");
        assert_eq!(
            semantic_json(&document).unwrap()["modelPicker"]["options"],
            json!("not-an-array")
        );
    }

    #[test]
    fn connection_rejects_an_unowned_picker_row_with_the_managed_id() {
        let metadata = route_metadata(200_000, 32_000);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let document = parse_source_bytes(
            Some(br#"{"modelPicker":{"options":[{"model":"token-station-auto","label":"User row"}]}}"#),
            DocumentFormat::Json,
            "Claude Code",
        )
        .unwrap();

        let Err(error) = ClaudeCodeConnector.connect_patch_for_document(&document, &input) else {
            panic!("an unowned same-ID picker row must block connection");
        };

        assert!(error.contains("未受管自定义行"), "{error}");
    }
}
