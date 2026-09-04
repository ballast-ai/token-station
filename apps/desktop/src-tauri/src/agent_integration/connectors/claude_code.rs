use std::path::{Path, PathBuf};

use serde_json::json;

use super::{path, AgentModelMetadata, ConnectInput, Connector, ConnectorCapabilities};
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
    "ANTHROPIC_CUSTOM_MODEL_OPTION",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
];

#[cfg(test)]
const TRANSPORT_ENV_KEYS: &[&str] = &[
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

const CUSTOM_MODEL_ENV_VALUES: &[(&str, &str)] = &[
    ("ANTHROPIC_CUSTOM_MODEL_OPTION", "auto"),
    (
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "Token Station Auto",
    ),
    (
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "Routes requests through the active Token Station Agent route.",
    ),
];

const LEGACY_DEFAULT_MODEL_ENV_VALUES: &[(&str, &str)] = &[
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "fast"),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", "balanced"),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", "power"),
];

const LEGACY_CUSTOM_MODEL_ENV_VALUES: &[(&str, &str)] = &[
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
        OWNED_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect()
    }

    fn legacy_owned_paths(&self) -> Vec<ConfigPath> {
        let mut paths = LEGACY_DEFAULT_MODEL_ENV_VALUES
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

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn refreshes_managed_configuration(&self) -> bool {
        true
    }

    fn refresh_requires_baseline(&self, owned_paths: &[ConfigPath]) -> bool {
        LEGACY_DEFAULT_MODEL_ENV_VALUES
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
        Ok(managed_env_values(input)?
            .into_iter()
            .map(|(key, value)| PatchOperation {
                operation: PatchKind::Replace,
                path: path(&["env", key]),
                value: Some(json!(value)),
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
        preserve_unowned_custom_option(document, owned_paths, &mut operations)?;
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
        preserve_unowned_custom_option(document, owned_paths, &mut operations)?;
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
        if managed_env_values(input)?.iter().all(|(key, value)| {
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
        if CUSTOM_MODEL_ENV_VALUES
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
             已保留内置模型并新增 Token Station Auto；\
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
    Ok(vec![
        ("ANTHROPIC_BASE_URL", input.base_url.to_string()),
        ("ANTHROPIC_AUTH_TOKEN", token.to_string()),
        ("MAX_THINKING_TOKENS", "0".to_string()),
        ("CLAUDE_CODE_DISABLE_THINKING", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_string()),
        (
            CUSTOM_MODEL_ENV_VALUES[0].0,
            CUSTOM_MODEL_ENV_VALUES[0].1.to_string(),
        ),
        (
            CUSTOM_MODEL_ENV_VALUES[1].0,
            CUSTOM_MODEL_ENV_VALUES[1].1.to_string(),
        ),
        (
            CUSTOM_MODEL_ENV_VALUES[2].0,
            CUSTOM_MODEL_ENV_VALUES[2].1.to_string(),
        ),
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", context),
        ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", compact_window),
    ])
}

fn route_context_values(input: &ConnectInput<'_>) -> Result<(String, String), String> {
    let (context, output) = input
        .model_metadata
        .and_then(AgentModelMetadata::safe_limits)
        .ok_or_else(|| {
            "Claude Code 的 Token Station Auto 缺少可信上下文或最大输出容量；本次未修改配置"
                .to_string()
        })?;
    let compact_window = (context - output).clamp(
        MIN_AUTO_COMPACT_WINDOW,
        MAX_AUTO_COMPACT_WINDOW,
    );
    Ok((context.to_string(), compact_window.to_string()))
}

fn is_custom_model_key(key: &str) -> bool {
    CUSTOM_MODEL_ENV_VALUES
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
    if managed_env_values(input)?
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

fn preserve_unowned_custom_option(
    document: &ConfigDocument,
    owned_paths: &[ConfigPath],
    operations: &mut Vec<PatchOperation>,
) -> Result<(), String> {
    let ConfigDocument::Json(root) = document else {
        return Err("Claude Code 连接器收到错误的配置格式".to_string());
    };
    if CUSTOM_MODEL_ENV_VALUES
        .iter()
        .any(|(key, _)| owned_paths.contains(&path(&["env", key])))
    {
        return Ok(());
    }

    let env = root.get("env").and_then(serde_json::Value::as_object);
    let slot_is_available = CUSTOM_MODEL_ENV_VALUES.iter().all(|(key, expected)| {
        let current = env
            .and_then(|env| env.get(*key))
            .and_then(serde_json::Value::as_str);
        current.is_none()
            || current == Some(*expected)
            || LEGACY_CUSTOM_MODEL_ENV_VALUES
                .iter()
                .find_map(|(legacy_key, legacy)| (*legacy_key == *key).then_some(*legacy))
                == current
    });
    if !slot_is_available {
        operations.retain(|operation| {
            !CUSTOM_MODEL_ENV_VALUES
                .iter()
                .any(|(key, _)| operation.path == path(&["env", key]))
        });
    }
    Ok(())
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
            && LEGACY_CUSTOM_MODEL_ENV_VALUES.iter().all(|(key, value)| {
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
        "claude-fable-5-1" if legacy_custom_option_was_selected => "auto",
        "claude-fable-5-1" => "fable",
        _ => return None,
    };
    Some(format!("{native}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_integration::config_codec::{apply_patch, parse_source_bytes, semantic_json};

    fn route_metadata(context: u32, output: u32) -> AgentModelMetadata {
        AgentModelMetadata {
            context,
            output,
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
        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION"], json!("auto"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"],
            json!("Token Station Auto")
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
        assert!(!ClaudeCodeConnector
            .refresh_requires_baseline(&ClaudeCodeConnector.owned_paths()));
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
        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION"], json!("auto"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"],
            json!("Token Station Auto")
        );
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
            json!("Routes requests through the active Token Station Agent route.")
        );
    }

    #[test]
    fn managed_refresh_does_not_claim_a_custom_option_added_after_connection() {
        let metadata = route_metadata(128_000, 32_768);
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/claude-code",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };
        let mut document = parse_source_bytes(
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
        let previously_owned = TRANSPORT_ENV_KEYS
            .iter()
            .map(|key| path(&["env", key]))
            .collect::<Vec<_>>();

        let patch = ClaudeCodeConnector
            .refresh_patch_for_document(&document, &input, &previously_owned)
            .unwrap();
        apply_patch(&mut document, &patch).unwrap();
        let newly_owned = previously_owned
            .iter()
            .filter(|path| patch.iter().any(|operation| operation.path == **path))
            .cloned()
            .collect::<Vec<_>>();
        ClaudeCodeConnector
            .validate_refresh_projected(&document, &input, &newly_owned)
            .unwrap();
        let root = semantic_json(&document).unwrap();
        let env = root["env"].as_object().unwrap();

        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION"], json!("user-custom"));
        assert_eq!(env["ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"], json!("User model"));
        assert_eq!(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"],
            json!("User description")
        );
        assert!(newly_owned.iter().all(|owned| {
            !CUSTOM_MODEL_ENV_VALUES
                .iter()
                .any(|(key, _)| *owned == path(&["env", key]))
        }));
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
        let patch = ClaudeCodeConnector
            .refresh_patch_for_document(&document, &refreshed_input, &owned_paths)
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
}
