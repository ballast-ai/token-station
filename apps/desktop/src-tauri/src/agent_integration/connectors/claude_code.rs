use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{ConfigDocument, DocumentFormat};
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const OWNED_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "MAX_THINKING_TOKENS",
    "CLAUDE_CODE_DISABLE_THINKING",
    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];

const LEGACY_MODEL_ENV_VALUES: &[(&str, &str)] = &[
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "fast"),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", "balanced"),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", "power"),
    ("ANTHROPIC_CUSTOM_MODEL_OPTION", "claude-fable-5-1"),
    (
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "Fable via Token Station",
    ),
    (
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "Route Claude Fable through the configured Token Station pool",
    ),
];

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
    owned_fields: OWNED_ENV_KEYS,
    requires_virtual_key: true,
    restart_required: false,
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
        OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect()
    }

    fn legacy_owned_paths(&self) -> Vec<ConfigPath> {
        let mut paths = LEGACY_MODEL_ENV_VALUES
            .iter()
            .map(|(key, _)| path(&["env", key]))
            .collect::<Vec<_>>();
        // Claude Code may persist a selection made from the old custom picker.
        // This path authorizes a one-time alias migration but is never retained
        // as active Connector ownership.
        paths.push(path(&["model"]));
        paths
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["env", "ANTHROPIC_AUTH_TOKEN"])]
    }

    fn refreshes_managed_configuration(&self) -> bool {
        true
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
            .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Json(root) = document else {
            return Err("Claude Code 连接器收到错误的配置格式".to_string());
        };
        if root
            .get("env")
            .is_none_or(|value| value.is_object() || value.is_null())
        {
            Ok(())
        } else {
            Err("Claude Code settings.json 的 env 必须是对象".to_string())
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())?;
        let values = [
            json!(input.base_url),
            json!(token),
            json!("0"),
            json!("1"),
            json!("1"),
            json!("1"),
            json!("1"),
        ];
        Ok(OWNED_ENV_KEYS
            .iter()
            .zip(values)
            .map(|(key, value)| PatchOperation {
                operation: PatchKind::Replace,
                path: path(&["env", key]),
                value: Some(value),
            })
            .collect())
    }

    fn refresh_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        let mut operations = self.connect_patch(input)?;
        operations.extend(legacy_model_migration_patch(
            document,
            None,
            Some(owned_paths),
        )?);
        Ok(operations)
    }

    fn refresh_patch_with_baseline(
        &self,
        document: &ConfigDocument,
        baseline: Option<&ConfigDocument>,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        let mut operations = self.connect_patch(input)?;
        operations.extend(legacy_model_migration_patch(
            document,
            baseline,
            Some(owned_paths),
        )?);
        Ok(operations)
    }

    fn disconnect_patch(&self) -> Vec<PatchOperation> {
        OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
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
        let mut operations = self.disconnect_patch();
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
        let token = input
            .token
            .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())?;
        let expected = [
            input.base_url,
            token,
            "0",
            "1",
            "1",
            "1",
            "1",
        ];
        if OWNED_ENV_KEYS
            .iter()
            .zip(expected)
            .all(|(key, value)| env.get(*key).and_then(serde_json::Value::as_str) == Some(value))
        {
            Ok(())
        } else {
            Err("Claude Code 写入前复验缺少受管 env 字段".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Claude Code 已指向 {}(~/.claude/settings.json,已备份)。\
             Claude Code 的模型名称保持不变，模型系列在网关内映射；\
             已关闭当前 Canonical IR 暂不支持的 thinking/beta；\
             使用 /v1/messages，经 agent-anthropic 入站适配器转发。",
            input.base_url
        )
    }
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
        LEGACY_MODEL_ENV_VALUES
            .iter()
            .any(|(key, _)| paths.contains(&path(&["env", key])))
    });
    let mut operations = LEGACY_MODEL_ENV_VALUES
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
            .and_then(native_model_selection)
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

fn native_model_selection(model: &str) -> Option<String> {
    let (family, suffix) = model
        .strip_suffix("[1m]")
        .map(|family| (family, "[1m]"))
        .or_else(|| model.strip_suffix("[2m]").map(|family| (family, "[2m]")))
        .unwrap_or((model, ""));
    let native = match family {
        "fast" => "haiku",
        "balanced" => "sonnet",
        "power" => "opus",
        "claude-fable-5-1" => "fable",
        _ => return None,
    };
    Some(format!("{native}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_integration::config_codec::{apply_patch, parse_source_bytes, semantic_json};

    #[test]
    fn connection_preserves_claude_model_names_and_existing_model_configuration() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut document = parse_source_bytes(
            Some(
                br#"{
                    "model": "user-model",
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

        apply_patch(
            &mut document,
            &ClaudeCodeConnector.connect_patch(&input).unwrap(),
        )
        .unwrap();
        ClaudeCodeConnector
            .validate_projected(&document, &input)
            .unwrap();
        let root = semantic_json(&document).unwrap();
        let env = root["env"].as_object().unwrap();

        assert_eq!(env["ANTHROPIC_BASE_URL"], json!(input.base_url));
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], json!("local-virtual-key"));
        assert_eq!(root["model"], json!("user-model"));
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], json!("user-haiku"));
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], json!("user-sonnet"));
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("user-opus"));
        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION"], json!("user-custom"));
        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"], json!("User model"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
            json!("User description")
        );
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_FABLE_MODEL"));
        assert!(!env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"));
        for key in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_CUSTOM_MODEL_OPTION",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        ] {
            assert!(
                !OWNED_ENV_KEYS.contains(&key),
                "the Connector must not own Claude Code model presentation: {key}"
            );
        }
        assert!(ClaudeCodeConnector.refreshes_managed_configuration());
        assert!(!ClaudeCodeConnector.projects_model_metadata());
    }

    #[test]
    fn managed_refresh_removes_only_the_legacy_token_station_model_overrides() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
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

        let patch = ClaudeCodeConnector
            .refresh_patch_for_document(&document, &input, &previously_owned)
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        ClaudeCodeConnector
            .validate_refresh_projected(&document, &input, &previously_owned)
            .unwrap();
        let root = semantic_json(&document).unwrap();
        let env = root["env"].as_object().unwrap();

        assert_eq!(root["model"], json!("opus[1m]"));
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"));
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_SONNET_MODEL"));
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("user-opus"));
        assert!(!env.contains_key("ANTHROPIC_CUSTOM_MODEL_OPTION"));
        assert!(!env.contains_key("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"));
        assert!(!env.contains_key("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"));
    }
}
