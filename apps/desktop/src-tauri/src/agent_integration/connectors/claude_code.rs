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
        home.join(".claude/settings.json")
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

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&["env", "ANTHROPIC_AUTH_TOKEN"])]
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
        if root.get("env").is_none_or(serde_json::Value::is_object) {
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
        let ConfigDocument::Json(root) = document else {
            unreachable!();
        };
        let env = root["env"]
            .as_object()
            .ok_or_else(|| "Claude Code settings.json 的 env 必须是对象".to_string())?;
        let token = input
            .token
            .ok_or_else(|| "Claude Code 接入缺少虚拟 Key".to_string())?;
        let expected = [input.base_url, token, "0", "1", "1", "1", "1"];
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
             已关闭当前 Canonical IR 暂不支持的 thinking/beta；\
             使用 /v1/messages，经 agent-anthropic 入站适配器转发。",
            input.base_url
        )
    }
}
