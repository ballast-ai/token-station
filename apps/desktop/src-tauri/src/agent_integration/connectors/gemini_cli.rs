use std::path::{Path, PathBuf};

use serde_json::json;
use zeroize::Zeroizing;

use super::{path, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, prepare_owned_paths_for_write, render_document, semantic_json,
    ConfigDocument, DocumentFormat,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{
    BaseUrlShape, ConfigPath, PatchKind, PatchOperation, Platform,
};

const BASE_URL_KEY: &str = "GOOGLE_GEMINI_BASE_URL";
const API_KEY: &str = "GEMINI_API_KEY";
const AUTH_SELECTED_TYPE: &[&str] = &["security", "auth", "selectedType"];
const API_KEY_AUTH_TYPE: &str = "gemini-api-key";

pub struct GeminiCliConnector;
pub(super) static CONNECTOR: GeminiCliConnector = GeminiCliConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "gemini-cli-v1",
    agent_id: "gemini-cli",
    label: "Gemini CLI .gemini/.env",
    adapter_id: "agent-gemini",
    base_url_shape: BaseUrlShape::Origin,
    platforms: &[Platform::Macos, Platform::Linux, Platform::Windows, Platform::Wsl],
    config_format: DocumentFormat::Dotenv,
    config_path_template: "${HOME}/.gemini/.env",
    owned_fields: &[BASE_URL_KEY, API_KEY],
    requires_virtual_key: true,
    restart_required: false,
};

impl Connector for GeminiCliConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".gemini").join(".env")
    }

    fn create_dir_error(&self) -> &'static str {
        "创建 ~/.gemini 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![path(&[BASE_URL_KEY]), path(&[API_KEY])]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&[API_KEY])]
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if !input.adapter_ready {
            return Err("暂不能接入 Gemini CLI：网关未加载 agent-gemini；本次未修改 .gemini/.env。".to_string());
        }
        input
            .token
            .map(|_| ())
            .ok_or_else(|| "Gemini CLI 接入缺少本地虚拟 Key".to_string())
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Dotenv(_) = document else {
            return Err("Gemini CLI Connector 收到错误的配置格式".to_string());
        };
        semantic_json(document).map(|_| ())
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Gemini CLI 接入缺少本地虚拟 Key".to_string())?;
        Ok(vec![
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(&[BASE_URL_KEY]),
                value: Some(json!(input.base_url)),
            },
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(&[API_KEY]),
                value: Some(json!(token)),
            },
        ])
    }

    fn companion_projections(
        &self,
        primary_target: &Path,
        _input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        let settings_path = primary_target
            .parent()
            .ok_or_else(|| "Gemini CLI .env 路径缺少 .gemini 父目录".to_string())?
            .join("settings.json");
        let source = read_config_source(&settings_path)?;
        let label = "Gemini CLI settings.json";
        let mut document = parse_source_bytes(
            source.existed.then_some(source.exact_bytes.as_slice()),
            DocumentFormat::Json,
            label,
        )?;
        if !semantic_json(&document)?.is_object() {
            return Err("Gemini CLI settings.json 必须是 JSON 对象".to_string());
        }
        let owned_path = path(AUTH_SELECTED_TYPE);
        let operations = vec![PatchOperation {
            operation: PatchKind::Replace,
            path: owned_path.clone(),
            value: Some(json!(API_KEY_AUTH_TYPE)),
        }];
        prepare_owned_paths_for_write(&mut document, std::slice::from_ref(&owned_path))?;
        apply_patch(&mut document, &operations)?;
        let projected_bytes = render_document(&document, label)?.into_bytes();
        Ok(vec![CompanionProjection {
            target_path: settings_path,
            source_existed: source.existed,
            source_bytes: Zeroizing::new(source.exact_bytes.to_vec()),
            original_permissions: source.original_permissions,
            original_owner: source.original_owner,
            projected_bytes: Zeroizing::new(projected_bytes),
            format: DocumentFormat::Json,
            label,
            owned_paths: vec![owned_path],
            sensitive_paths: Vec::new(),
            operations,
        }])
    }

    fn legacy_companion_format(
        &self,
        primary_target: &Path,
        companion_target: &Path,
    ) -> Option<DocumentFormat> {
        let expected = primary_target.parent()?.join("settings.json");
        (companion_target == expected).then_some(DocumentFormat::Json)
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
            .ok_or_else(|| "Gemini CLI 接入缺少本地虚拟 Key".to_string())?;
        if semantic.get(BASE_URL_KEY) == Some(&json!(input.base_url))
            && semantic.get(API_KEY) == Some(&json!(token))
        {
            Ok(())
        } else {
            Err("Gemini CLI 写入前复验失败".to_string())
        }
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        format!(
            "Gemini CLI 已通过 GOOGLE_GEMINI_BASE_URL 指向 {}，认证模式已切换为 gemini-api-key；断开时会恢复原认证模式。",
            input.base_url
        )
    }
}
