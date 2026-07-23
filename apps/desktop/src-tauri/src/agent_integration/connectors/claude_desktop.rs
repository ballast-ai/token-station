use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    path, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities,
};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, render_document, semantic_json, ConfigDocument,
    DocumentFormat,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const PROFILE_ID: &str = "7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2";
const PROVIDER: &str = "inferenceProvider";
const BASE_URL: &str = "inferenceGatewayBaseUrl";
const API_KEY: &str = "inferenceGatewayApiKey";
const AUTH_SCHEME: &str = "inferenceGatewayAuthScheme";

pub struct ClaudeDesktopConnector;
pub(super) static CONNECTOR: ClaudeDesktopConnector = ClaudeDesktopConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "claude-desktop-3p-v1",
    agent_id: "claude-desktop",
    label: "Claude Desktop 3P profile",
    adapter_id: "agent-anthropic",
    base_url_shape: BaseUrlShape::Origin,
    platforms: &[Platform::Macos, Platform::Windows],
    config_format: DocumentFormat::Json,
    config_path_template: "${CLAUDE_3P_CONFIG_LIBRARY}/7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2.json",
    owned_fields: &[PROVIDER, BASE_URL, API_KEY, AUTH_SCHEME],
    requires_virtual_key: true,
    restart_required: true,
};

fn profile_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return home
            .join("AppData/Local/Claude-3p/configLibrary")
            .join(format!("{PROFILE_ID}.json"));
    }
    #[cfg(not(target_os = "windows"))]
    home.join("Library/Application Support/Claude-3p/configLibrary")
        .join(format!("{PROFILE_ID}.json"))
}

fn replace(path: ConfigPath, value: Value) -> PatchOperation {
    PatchOperation {
        operation: PatchKind::Replace,
        path,
        value: Some(value),
    }
}

impl Connector for ClaudeDesktopConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        profile_path(home)
    }

    fn create_dir_error(&self) -> &'static str {
        "创建 Claude Desktop configLibrary 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        [PROVIDER, BASE_URL, API_KEY, AUTH_SCHEME]
            .iter()
            .map(|key| path(&[key]))
            .collect()
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&[API_KEY])]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err(
                "暂不能接入 Claude Desktop：网关未加载 agent-anthropic；本次未修改 3P profile。"
                    .to_string(),
            );
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "Claude Desktop 接入缺少本地虚拟 Key".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Json(value) = document else {
            return Err("Claude Desktop Connector 收到错误的配置格式".to_string());
        };
        if value.is_object() {
            Ok(())
        } else {
            Err("Claude Desktop 3P profile 必须是 JSON 对象".to_string())
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Claude Desktop 接入缺少本地虚拟 Key".to_string())?;
        Ok(vec![
            replace(path(&[PROVIDER]), json!("gateway")),
            replace(path(&[BASE_URL]), json!(input.base_url)),
            replace(path(&[API_KEY]), json!(token)),
            replace(path(&[AUTH_SCHEME]), json!("bearer")),
        ])
    }

    fn companion_projections(
        &self,
        primary_target: &Path,
        _input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        let parent = primary_target
            .parent()
            .ok_or_else(|| "Claude Desktop profile 路径缺少父目录".to_string())?;
        let meta_path = parent.join("_meta.json");
        let source = read_config_source(&meta_path)?;
        let mut document = parse_source_bytes(
            source.existed.then_some(source.exact_bytes.as_slice()),
            DocumentFormat::Json,
            "Claude Desktop 3P profile metadata",
        )?;
        let semantic = semantic_json(&document)?;
        let mut entries = semantic
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(PROFILE_ID));
        entries.push(json!({"id": PROFILE_ID, "name": "Token Station"}));
        let operations = vec![
            replace(path(&["appliedId"]), json!(PROFILE_ID)),
            replace(path(&["entries"]), Value::Array(entries)),
        ];
        apply_patch(&mut document, &operations)?;
        let rendered = render_document(&document, "Claude Desktop 3P profile metadata")?;
        Ok(vec![CompanionProjection {
            target_path: meta_path,
            source_existed: source.existed,
            source_bytes: Zeroizing::new(source.exact_bytes.to_vec()),
            original_permissions: source.original_permissions,
            original_owner: source.original_owner,
            projected_bytes: Zeroizing::new(rendered.into_bytes()),
            format: DocumentFormat::Json,
            label: "Claude Desktop 3P profile metadata",
            owned_paths: vec![path(&["appliedId"]), path(&["entries"])],
            sensitive_paths: Vec::new(),
            operations,
        }])
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
        let semantic = semantic_json(document)?;
        let token = input
            .token
            .ok_or_else(|| "Claude Desktop 接入缺少本地虚拟 Key".to_string())?;
        if semantic.get(PROVIDER) == Some(&json!("gateway"))
            && semantic.get(BASE_URL) == Some(&json!(input.base_url))
            && semantic.get(API_KEY) == Some(&json!(token))
            && semantic.get(AUTH_SCHEME) == Some(&json!("bearer"))
        {
            Ok(())
        } else {
            Err("Claude Desktop 3P profile 写入前复验失败".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Claude Desktop 的 Token Station 3P profile 已指向 {}；需要重启 Claude Desktop。",
            input.base_url
        )
    }
}
