use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{path, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, render_document, semantic_json, ConfigDocument, DocumentFormat,
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
const DEPLOYMENT_MODE: &str = "deploymentMode";
const DISABLE_DEPLOYMENT_MODE_CHOOSER: &str = "disableDeploymentModeChooser";
const COWORK_EGRESS_ALLOWED_HOSTS: &str = "coworkEgressAllowedHosts";
const LOOPBACK_EGRESS_HOSTS: &[&str] = &["127.0.0.1", "localhost"];

fn json_companion_projection(
    target_path: PathBuf,
    label: &'static str,
    operations: Vec<PatchOperation>,
) -> Result<CompanionProjection, String> {
    let source = read_config_source(&target_path)?;
    let mut document = parse_source_bytes(
        source.existed.then_some(source.exact_bytes.as_slice()),
        DocumentFormat::Json,
        label,
    )?;
    if !semantic_json(&document)?.is_object() {
        return Err(format!("{label} 必须是 JSON 对象"));
    }
    apply_patch(&mut document, &operations)?;
    let rendered = render_document(&document, label)?;
    let owned_paths = operations
        .iter()
        .map(|operation| operation.path.clone())
        .collect();
    Ok(CompanionProjection {
        target_path,
        source_existed: source.existed,
        source_bytes: Zeroizing::new(source.exact_bytes.to_vec()),
        original_permissions: source.original_permissions,
        original_owner: source.original_owner,
        projected_bytes: Zeroizing::new(rendered.into_bytes()),
        format: DocumentFormat::Json,
        label,
        owned_paths,
        sensitive_paths: Vec::new(),
        operations,
    })
}

fn deployment_mode_paths(primary_target: &Path) -> Result<[PathBuf; 2], String> {
    let app_support = primary_target
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| "Claude Desktop profile 路径缺少 Application Support 父目录".to_string())?;
    Ok([
        app_support.join("Claude").join("config.json"),
        app_support.join("Claude-3p").join("config.json"),
    ])
}

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
    owned_fields: &[
        PROVIDER,
        BASE_URL,
        API_KEY,
        AUTH_SCHEME,
        DISABLE_DEPLOYMENT_MODE_CHOOSER,
        COWORK_EGRESS_ALLOWED_HOSTS,
    ],
    requires_virtual_key: true,
    restart_required: true,
};

fn profile_path(home: &Path) -> PathBuf {
    // Claude Desktop ships only on Windows and macOS — there is no Linux build,
    // and the Registry has no Linux discovery entry, so this connector is never
    // active on Linux. The non-Windows arm is therefore the macOS layout; on any
    // other platform it is inert (the agent is never discovered to reach here).
    #[cfg(target_os = "windows")]
    {
        home.join("AppData")
            .join("Local")
            .join("Claude-3p")
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        home.join("Library")
            .join("Application Support")
            .join("Claude-3p")
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json"))
    }
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
        [
            PROVIDER,
            BASE_URL,
            API_KEY,
            AUTH_SCHEME,
            DISABLE_DEPLOYMENT_MODE_CHOOSER,
            COWORK_EGRESS_ALLOWED_HOSTS,
        ]
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
            replace(path(&[DISABLE_DEPLOYMENT_MODE_CHOOSER]), json!(true)),
            // Claude's Cowork egress must remain restricted to the local
            // Token Station gateway; a wildcard turns this connector into an
            // arbitrary network-access grant.
            replace(path(&[COWORK_EGRESS_ALLOWED_HOSTS]), json!(LOOPBACK_EGRESS_HOSTS)),
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
        let mut projections = vec![CompanionProjection {
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
        }];
        for deployment_path in deployment_mode_paths(primary_target)? {
            projections.push(json_companion_projection(
                deployment_path,
                "Claude Desktop deployment mode",
                vec![replace(path(&[DEPLOYMENT_MODE]), json!("3p"))],
            )?);
        }
        Ok(projections)
    }

    fn legacy_companion_format(
        &self,
        primary_target: &Path,
        companion_target: &Path,
    ) -> Option<DocumentFormat> {
        let metadata = primary_target.parent()?.join("_meta.json");
        if companion_target == metadata {
            return Some(DocumentFormat::Json);
        }
        let deployment_modes = deployment_mode_paths(primary_target).ok()?;
        deployment_modes
            .iter()
            .any(|candidate| candidate == companion_target)
            .then_some(DocumentFormat::Json)
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
            && semantic.get(DISABLE_DEPLOYMENT_MODE_CHOOSER) == Some(&json!(true))
            && semantic.get(COWORK_EGRESS_ALLOWED_HOSTS) == Some(&json!(LOOPBACK_EGRESS_HOSTS))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn claude_desktop_connection_enables_3p_mode_in_both_runtime_configs() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "token-station-claude-desktop-connector-{}-{unique}",
            std::process::id()
        ));
        let profile = root
            .join("Claude-3p/configLibrary")
            .join(format!("{PROFILE_ID}.json"));
        fs::create_dir_all(profile.parent().expect("profile has parent"))
            .expect("profile dir builds");
        fs::create_dir_all(root.join("Claude")).expect("Claude dir builds");
        fs::write(root.join("Claude/config.json"), r#"{"keep":"normal"}"#)
            .expect("normal config writes");
        fs::write(root.join("Claude-3p/config.json"), r#"{"keep":"threep"}"#)
            .expect("3P config writes");

        let projections = CONNECTOR
            .companion_projections(
                &profile,
                &ConnectInput {
                    base_url: "http://127.0.0.1:8787/agents/claude-desktop",
                    token: Some("vk-test"),
                    adapter_ready: true,
                },
            )
            .expect("projections build");

        assert_eq!(projections.len(), 3);
        for target in [
            root.join("Claude/config.json"),
            root.join("Claude-3p/config.json"),
        ] {
            let projection = projections
                .iter()
                .find(|projection| projection.target_path == target)
                .expect("deployment config projection exists");
            let document = parse_source_bytes(
                Some(projection.projected_bytes.as_slice()),
                DocumentFormat::Json,
                projection.label,
            )
            .expect("projected config parses");
            assert_eq!(
                semantic_json(&document).expect("semantic config")[DEPLOYMENT_MODE],
                "3p"
            );
        }

        fs::remove_dir_all(root).expect("temporary files clean up");
    }
}
