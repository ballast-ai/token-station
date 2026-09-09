use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::json;
use zeroize::Zeroizing;

use super::{CompanionProjection, ConnectInput, Connector, ConnectorCapabilities, path};
use crate::agent_integration::config_codec::{
    ConfigDocument, DocumentFormat, apply_patch, parse_source_bytes, prepare_owned_paths_for_write,
    render_document, semantic_json,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{
    ConfigPath, ConnectorRuntimePaths, PatchKind, PatchOperation,
};

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
    owned_fields: &[
        "models.providers.tokenstation",
        "agents.defaults.model.primary",
    ],
    requires_virtual_key: true,
    restart_required: true,
};

fn model_value(input: &ConnectInput<'_>) -> serde_json::Value {
    let mut model = json!({
        "id": "auto",
        "name": "Token Station Auto",
        "input": ["text"]
    });
    if let Some(metadata) = input.model_metadata {
        if metadata.vision {
            model["input"] = json!(["text", "image"]);
        }
        if let Some((context, output)) = metadata.safe_limits() {
            model["contextWindow"] = json!(context);
            model["maxTokens"] = json!(output);
        }
        if let Some(cost) = &metadata.cost {
            let mut projected = json!({
                "input": cost.input,
                "output": cost.output
            });
            if let Some(rate) = cost.cache_read {
                projected["cacheRead"] = json!(rate);
            }
            if let Some(rate) = cost.cache_write {
                projected["cacheWrite"] = json!(rate);
            }
            model["cost"] = projected;
        }
    }
    model
}

const MAX_AGENT_CATALOGS: usize = 128;

fn existing_agent_directories(
    primary: &Path,
    runtime_paths: &ConnectorRuntimePaths,
) -> Result<BTreeSet<PathBuf>, String> {
    if primary != runtime_paths.primary_config_path
        || !runtime_paths.state_directory.is_absolute()
        || !runtime_paths.effective_home.is_absolute()
    {
        return Err("OpenClaw runtime path context does not match the selected configuration. Rescan OpenClaw.".into());
    }
    let root = runtime_paths.state_directory.join("agents");
    let mut directories = BTreeSet::new();
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("OpenClaw agents directory must be a real directory.".into());
            }
            for (index, entry) in std::fs::read_dir(&root)
                .map_err(|_| "Cannot read OpenClaw agents directory.")?
                .enumerate()
            {
                if index >= MAX_AGENT_CATALOGS {
                    return Err("OpenClaw has too many agent directories to preview safely.".into());
                }
                let entry = entry.map_err(|_| "Cannot read an OpenClaw agent directory.")?;
                let kind = entry
                    .file_type()
                    .map_err(|_| "Cannot inspect an OpenClaw agent directory.")?;
                if kind.is_symlink() {
                    return Err("OpenClaw agent directories must not be symbolic links.".into());
                }
                if kind.is_dir() {
                    directories.insert(entry.path().join("agent"));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("Cannot inspect OpenClaw agents directory.".into()),
    }
    let source = read_config_source(primary)?;
    let document = parse_source_bytes(
        source.existed.then_some(source.exact_bytes.as_slice()),
        DocumentFormat::Json5,
        "OpenClaw configuration",
    )?;
    let config = semantic_json(&document)?;
    if let Some(agents) = config
        .pointer("/agents/list")
        .and_then(serde_json::Value::as_array)
    {
        if agents.len() > MAX_AGENT_CATALOGS {
            return Err("OpenClaw has too many configured agents to preview safely.".into());
        }
        for agent in agents {
            if let Some(value) = agent.get("agentDir") {
                let value = value
                    .as_str()
                    .ok_or("OpenClaw agentDir must be a path string.")?;
                let value = value.trim();
                let normalized = value.replace('\\', "/");
                let directory = if normalized == "~" {
                    runtime_paths.effective_home.clone()
                } else if let Some(relative) = normalized.strip_prefix("~/") {
                    runtime_paths.effective_home.join(relative)
                } else {
                    PathBuf::from(normalized)
                };
                if !directory.is_absolute()
                    || directory
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(
                        "OpenClaw agentDir must be an absolute path without parent traversal."
                            .into(),
                    );
                }
                directories.insert(directory);
            }
        }
    }
    if directories.len() > MAX_AGENT_CATALOGS {
        return Err("OpenClaw has too many agent catalogs to preview safely.".into());
    }
    Ok(directories)
}

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

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Json5
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".openclaw").join("openclaw.json")
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
                .is_some_and(|value| !value.is_object() && !value.is_null())
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
        let model = model_value(input);
        Ok(vec![
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(PROVIDER_PATH),
                value: Some(json!({
                    "baseUrl": input.base_url,
                    "apiKey": token,
                    "auth": "api-key",
                    "api": "openai-completions",
                    "models": [model]
                })),
            },
            PatchOperation {
                operation: PatchKind::Replace,
                path: path(PRIMARY_PATH),
                value: Some(json!("tokenstation/auto")),
            },
        ])
    }

    fn companion_projections(
        &self,
        _primary_target: &Path,
        _input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        Err(
            "openclaw_runtime_context_missing: Rescan OpenClaw before reading runtime catalogs."
                .into(),
        )
    }

    fn companion_projections_with_context(
        &self,
        primary_target: &Path,
        input: &ConnectInput<'_>,
        runtime_paths: Option<&ConnectorRuntimePaths>,
    ) -> Result<Vec<CompanionProjection>, String> {
        let runtime_paths = runtime_paths.ok_or(
            "openclaw_runtime_context_missing: Rescan OpenClaw before reading runtime catalogs.",
        )?;
        let token = input
            .token
            .ok_or("OpenClaw requires a local virtual key.")?;
        let mut projections = Vec::new();
        for directory in existing_agent_directories(primary_target, runtime_paths)? {
            match std::fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                _ => return Err("OpenClaw agentDir must be an accessible real directory.".into()),
            }
            let target_path = directory.join("models.json");
            let source = read_config_source(&target_path)?;
            if !source.existed {
                continue;
            }
            let label = "OpenClaw agent models.json";
            let mut document = parse_source_bytes(
                Some(source.exact_bytes.as_slice()),
                DocumentFormat::Json,
                label,
            )?;
            let root = semantic_json(&document)?;
            if !root.is_object()
                || root
                    .get("providers")
                    .is_some_and(|value| !value.is_object())
            {
                return Err(
                    "OpenClaw models.json must contain an object with a providers object.".into(),
                );
            }
            let Some(provider) = root.pointer("/providers/tokenstation") else {
                continue;
            };
            if !provider.is_object() {
                return Err("OpenClaw runtime tokenstation provider must be an object.".into());
            }
            let owned_paths = vec![
                path(&["providers", "tokenstation", "apiKey"]),
                path(&["providers", "tokenstation", "baseUrl"]),
            ];
            let operations = vec![
                PatchOperation {
                    operation: PatchKind::Replace,
                    path: owned_paths[0].clone(),
                    value: Some(json!(token)),
                },
                PatchOperation {
                    operation: PatchKind::Replace,
                    path: owned_paths[1].clone(),
                    value: Some(json!(input.base_url)),
                },
            ];
            prepare_owned_paths_for_write(&mut document, &owned_paths)?;
            apply_patch(&mut document, &operations)?;
            let projected_bytes = render_document(&document, label)?.into_bytes();
            projections.push(CompanionProjection {
                target_path,
                source_existed: true,
                source_bytes: Zeroizing::new(source.exact_bytes.to_vec()),
                original_permissions: source.original_permissions,
                original_owner: source.original_owner,
                projected_bytes: Zeroizing::new(projected_bytes),
                format: DocumentFormat::Json,
                label,
                sensitive_paths: vec![owned_paths[0].clone()],
                owned_paths,
                operations,
            });
        }
        Ok(projections)
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
            && provider["auth"] == json!("api-key")
            && provider["api"] == json!("openai-completions")
            && provider["models"][0]["id"] == json!("auto")
            && root.pointer("/agents/defaults/model/primary") == Some(&json!("tokenstation/auto"));
        if !valid {
            return Err("OpenClaw 写入前复验失败".to_string());
        }
        if provider["models"][0] != model_value(input) {
            return Err("OpenClaw 写入前复验模型元数据失败".to_string());
        }
        Ok(())
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = match input.model_metadata {
            Some(value) if value.cost.is_some() => "已同步安全模型限制和一致价格",
            Some(_) => "已同步安全模型限制；价格未知，未写入猜测值",
            None => "模型限制和价格未知，未写入猜测值",
        };
        format!(
            "OpenClaw 已通过 openai-completions 指向 {}；{}；配置已进入加密快照和 ownership 管理。",
            input.base_url, metadata
        )
    }

    fn validate_connected_configuration(
        &self,
        primary_target: &Path,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        runtime_paths: Option<&ConnectorRuntimePaths>,
    ) -> Result<(), String> {
        self.validate_projected(document, input)?;
        for companion in
            self.companion_projections_with_context(primary_target, input, runtime_paths)?
        {
            let cached = parse_source_bytes(
                Some(companion.source_bytes.as_slice()),
                companion.format,
                companion.label,
            )?;
            let root = semantic_json(&cached)?;
            if root.pointer("/providers/tokenstation/apiKey") != Some(&json!(input.token))
                || root.pointer("/providers/tokenstation/baseUrl") != Some(&json!(input.base_url))
            {
                return Err("OpenClaw runtime credentials differ from the connection. Reconnect and restart the OpenClaw gateway.".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn runtime_catalogs_expand_custom_agent_dirs_with_the_captured_home() {
        let root = std::env::temp_dir().join(format!(
            "openclaw-custom-home-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let main = root.join("config/openclaw.json");
        let runtime_paths = ConnectorRuntimePaths {
            primary_config_path: main.clone(),
            state_directory: root.join("state"),
            effective_home: root.join("effective-home"),
        };
        let custom = runtime_paths.effective_home.join("custom/models.json");
        std::fs::create_dir_all(main.parent().unwrap()).unwrap();
        std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
        std::fs::write(
            &main,
            br#"{"agents":{"list":[{"id":"custom","agentDir":"~/custom"}]}}"#,
        )
        .unwrap();
        std::fs::write(
            &custom,
            br#"{"providers":{"tokenstation":{"apiKey":"old-key"}}}"#,
        )
        .unwrap();
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("new-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let projections = CONNECTOR
            .companion_projections_with_context(&main, &input, Some(&runtime_paths))
            .unwrap();
        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].target_path, custom);
        assert!(
            CONNECTOR
                .companion_projections_with_context(&main, &input, None)
                .err()
                .unwrap()
                .contains("openclaw_runtime_context_missing")
        );
        assert!(CONNECTOR.companion_projections(&main, &input).is_err());
        let mut different_target = runtime_paths;
        different_target.primary_config_path = root.join("other.json");
        assert!(
            CONNECTOR
                .companion_projections_with_context(&main, &input, Some(&different_target))
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_catalogs_use_captured_state_directory_instead_of_config_parent() {
        let root = std::env::temp_dir().join(format!(
            "token-station-openclaw-paths-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let main = root.join("config/openclaw.json");
        let runtime_paths = ConnectorRuntimePaths {
            primary_config_path: main.clone(),
            state_directory: root.join("state"),
            effective_home: root.join("home"),
        };
        let catalog = runtime_paths
            .state_directory
            .join("agents/main/agent/models.json");
        std::fs::create_dir_all(main.parent().unwrap()).unwrap();
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(&main, b"{}").unwrap();
        std::fs::write(
            &catalog,
            br#"{"providers":{"tokenstation":{"apiKey":"old","baseUrl":"http://old.invalid/v1"}}}"#,
        )
        .unwrap();
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("current-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let projections = CONNECTOR
            .companion_projections_with_context(&main, &input, Some(&runtime_paths))
            .unwrap();
        let actual_targets: Vec<_> = projections
            .iter()
            .map(|projection| projection.target_path.clone())
            .collect();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(actual_targets, vec![catalog]);
    }

    #[test]
    fn cached_runtime_credentials_follow_the_connection_and_preserve_other_fields() {
        let root = std::env::temp_dir().join(format!(
            "token-station-openclaw-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let main = root.join("openclaw.json");
        let runtime_paths = ConnectorRuntimePaths {
            primary_config_path: main.clone(),
            state_directory: root.clone(),
            effective_home: root.join("home"),
        };
        let models = root.join("agents/main/agent/models.json");
        let custom = root.join("custom-agent");
        std::fs::create_dir_all(models.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(
            &main,
            serde_json::to_vec(&json!({"agents":{"list":[{"id":"custom","agentDir":custom}]}}))
                .unwrap(),
        )
        .unwrap();
        let before = json!({"providers":{
            "tokenstation":{"apiKey":"old-key","baseUrl":"http://127.0.0.1:9999/v1","models":[{"id":"auto","name":"Keep"}]},
            "other":{"apiKey":"other-key","baseUrl":"https://example.invalid/v1"}
        }});
        for target in [&models, &custom.join("models.json")] {
            std::fs::write(target, serde_json::to_vec(&before).unwrap()).unwrap();
        }
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/openclaw/v1",
            token: Some("current-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let mut connected = parse_source_bytes(
            Some(&std::fs::read(&main).unwrap()),
            DocumentFormat::Json5,
            "test",
        )
        .unwrap();
        apply_patch(&mut connected, &CONNECTOR.connect_patch(&input).unwrap()).unwrap();
        assert_eq!(
            semantic_json(&connected).unwrap()["models"]["providers"]["tokenstation"]["auth"],
            "api-key"
        );
        assert!(
            CONNECTOR
                .validate_connected_configuration(&main, &connected, &input, Some(&runtime_paths))
                .is_err()
        );
        let projections = CONNECTOR
            .companion_projections_with_context(&main, &input, Some(&runtime_paths))
            .unwrap();
        assert_eq!(projections.len(), 2);
        for projection in projections {
            let after: serde_json::Value =
                serde_json::from_slice(&projection.projected_bytes).unwrap();
            assert_eq!(after["providers"]["tokenstation"]["apiKey"], "current-key");
            assert_eq!(
                after["providers"]["tokenstation"]["baseUrl"],
                input.base_url
            );
            assert_eq!(
                after["providers"]["tokenstation"]["models"],
                before["providers"]["tokenstation"]["models"]
            );
            assert_eq!(after["providers"]["other"], before["providers"]["other"]);
            assert_eq!(projection.owned_paths.len(), 2);
            assert_eq!(projection.sensitive_paths.len(), 1);
            assert_eq!(
                std::fs::read(&projection.target_path).unwrap(),
                projection.source_bytes.as_slice()
            );
        }
        for projection in CONNECTOR
            .companion_projections_with_context(&main, &input, Some(&runtime_paths))
            .unwrap()
        {
            std::fs::write(
                &projection.target_path,
                projection.projected_bytes.as_slice(),
            )
            .unwrap();
        }
        assert!(
            CONNECTOR
                .validate_connected_configuration(&main, &connected, &input, Some(&runtime_paths))
                .is_ok()
        );
        std::fs::remove_file(&models).unwrap();
        std::fs::remove_file(custom.join("models.json")).unwrap();
        assert!(
            CONNECTOR
                .companion_projections_with_context(&main, &input, Some(&runtime_paths))
                .unwrap()
                .is_empty()
        );
        std::fs::write(&models, b"{broken").unwrap();
        assert!(
            CONNECTOR
                .companion_projections_with_context(&main, &input, Some(&runtime_paths))
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
