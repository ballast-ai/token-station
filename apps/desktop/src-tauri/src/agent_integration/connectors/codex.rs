use std::path::{Path, PathBuf};

use serde_json::json;
use zeroize::Zeroizing;

use super::{
    path, AgentModelMetadata, CompanionProjection, ConnectInput, Connector, ConnectorCapabilities,
};
use crate::agent_integration::config_codec::{
    apply_patch, parse_source_bytes, prepare_owned_paths_for_write, render_document, semantic_json,
    ConfigDocument, DocumentFormat,
};
use crate::agent_integration::plan::read_config_source;
use crate::agent_integration::types::{ConfigPath, PatchKind, PatchOperation};

const MODEL_CATALOG_JSON: &[&str] = &["model_catalog_json"];
const MODEL_CATALOG_RELATIVE_PATH: &str = "model-catalogs/tokenstation.json";
const MODEL_CATALOG_MODELS: &[&str] = &["models"];
const FALLBACK_CONTEXT_WINDOW: u32 = 32_000;

pub struct CodexConnector;
pub(super) static CONNECTOR: CodexConnector = CodexConnector;
static CAPABILITIES: ConnectorCapabilities = ConnectorCapabilities {
    connector_id: "codex-v1",
    agent_id: "codex",
    label: "Codex config.toml",
    adapter_id: "agent-openai-responses",
    base_url_shape: crate::agent_integration::types::BaseUrlShape::OriginV1,
    platforms: &[
        crate::agent_integration::types::Platform::Macos,
        crate::agent_integration::types::Platform::Linux,
        crate::agent_integration::types::Platform::Windows,
        crate::agent_integration::types::Platform::Wsl,
    ],
    config_format: DocumentFormat::Toml,
    config_path_template: "${HOME}/.codex/config.toml",
    // connect_patch also rewrites model to auto and includes it in owned_paths(),
    // so declare it here or ownership metadata, restoration, and display will omit it.
    owned_fields: &[
        "model",
        "model_provider",
        "model_context_window",
        "model_auto_compact_token_limit",
        "model_catalog_json",
        "web_search",
        "model_providers.tokenstation",
    ],
    requires_virtual_key: true,
    restart_required: true,
};

impl Connector for CodexConnector {
    fn capabilities(&self) -> &'static ConnectorCapabilities {
        &CAPABILITIES
    }
    fn connector_id(&self) -> &'static str {
        "codex-v1"
    }

    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex config.toml"
    }

    fn format(&self) -> DocumentFormat {
        DocumentFormat::Toml
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".codex").join("config.toml")
    }

    fn create_dir_error(&self) -> &'static str {
        "建 ~/.codex 失败"
    }

    fn owned_paths(&self) -> Vec<ConfigPath> {
        vec![
            path(&["model"]),
            path(&["model_provider"]),
            path(&["model_context_window"]),
            path(&["model_auto_compact_token_limit"]),
            path(MODEL_CATALOG_JSON),
            path(&["web_search"]),
            path(&["model_providers", "tokenstation"]),
        ]
    }

    fn sensitive_paths(&self) -> Vec<ConfigPath> {
        vec![path(&[
            "model_providers",
            "tokenstation",
            "experimental_bearer_token",
        ])]
    }

    fn projects_model_metadata(&self) -> bool {
        true
    }

    fn refresh_requires_baseline(&self, owned_paths: &[ConfigPath]) -> bool {
        !owned_paths.contains(&path(&["web_search"]))
    }

    fn validate_preconditions(&self, input: &ConnectInput<'_>) -> Result<(), String> {
        if input.adapter_ready {
            input
                .token
                .map(|_| ())
                .ok_or_else(|| "Codex 接入缺少本地虚拟 Key".to_string())
        } else {
            Err(
                "暂不能接入 Codex：网关未加载 agent-openai-responses，/v1/responses \
                 无入站适配器。本次未修改 ~/.codex/config.toml。"
                    .to_string(),
            )
        }
    }

    fn validate_source(&self, document: &ConfigDocument) -> Result<(), String> {
        let ConfigDocument::Toml(document) = document else {
            return Err("Codex 连接器收到错误的配置格式".to_string());
        };
        let root = document.as_table();
        match root.get("model_providers") {
            None => Ok(()),
            Some(item) if item.is_table_like() => Ok(()),
            Some(_) => Err("Codex config.toml 的 model_providers 必须是表".to_string()),
        }
    }

    fn connect_patch(&self, input: &ConnectInput<'_>) -> Result<Vec<PatchOperation>, String> {
        let token = input
            .token
            .ok_or_else(|| "Codex 接入缺少本地虚拟 Key".to_string())?;
        let mut operations = vec![
            replace(&["model"], json!("auto")),
            replace(&["model_provider"], json!("tokenstation")),
            replace(MODEL_CATALOG_JSON, json!(MODEL_CATALOG_RELATIVE_PATH)),
            replace(&["web_search"], json!("disabled")),
            replace(
                &["model_providers", "tokenstation", "base_url"],
                json!(input.base_url),
            ),
        ];
        operations.extend(
            provider_fields(token)
                .into_iter()
                .map(|(field, value)| replace(&["model_providers", "tokenstation", field], value)),
        );
        if let Some((context, _)) = input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits)
        {
            operations.push(replace(&["model_context_window"], json!(context)));
            if let Some(max_input) = input
                .model_metadata
                .and_then(AgentModelMetadata::safe_max_input)
            {
                operations.push(replace(
                    &["model_auto_compact_token_limit"],
                    json!(max_input),
                ));
            }
        }
        // Older connectors wrote env_key. Codex prefers it over the embedded
        // bearer token, so leaving it behind makes a GUI connection depend on
        // an environment variable it never receives.
        operations.push(PatchOperation {
            operation: PatchKind::Remove,
            path: path(&["model_providers", "tokenstation", "env_key"]),
            value: None,
        });
        Ok(operations)
    }

    fn companion_projections(
        &self,
        primary_target: &Path,
        input: &ConnectInput<'_>,
    ) -> Result<Vec<CompanionProjection>, String> {
        let catalog_path = primary_target
            .parent()
            .ok_or_else(|| "Codex config.toml must have a .codex parent directory.".to_string())?
            .join(MODEL_CATALOG_RELATIVE_PATH);
        let source = read_config_source(&catalog_path)?;
        let label = "Codex Token Station model catalog";
        let mut document = parse_source_bytes(
            source.existed.then_some(source.exact_bytes.as_slice()),
            DocumentFormat::Json,
            label,
        )?;
        if !semantic_json(&document)?.is_object() {
            return Err("Codex Token Station model catalog must be a JSON object.".to_string());
        }
        let owned_path = path(MODEL_CATALOG_MODELS);
        let operations = vec![replace(
            MODEL_CATALOG_MODELS,
            json!([token_station_model(
                primary_target.parent().expect("parent checked above"),
                input,
            )]),
        )];
        prepare_owned_paths_for_write(&mut document, std::slice::from_ref(&owned_path))?;
        apply_patch(&mut document, &operations)?;
        let projected_bytes = render_document(&document, label)?.into_bytes();
        Ok(vec![CompanionProjection {
            target_path: catalog_path,
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
        let expected = primary_target.parent()?.join(MODEL_CATALOG_RELATIVE_PATH);
        (companion_target == expected).then_some(DocumentFormat::Json)
    }

    fn refresh_patch_for_document(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        let mut operations = self.connect_patch(input)?;
        let semantic = semantic_json(document)?;
        let web_search_path = path(&["web_search"]);
        if !owned_paths.contains(&web_search_path) {
            return Err(
                "Codex Web Search is not owned by this legacy connection and requires its disconnect baseline before refreshing."
                    .to_string(),
            );
        }
        let context_path = path(&["model_context_window"]);
        let compact_path = path(&["model_auto_compact_token_limit"]);
        if input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits)
            .is_some()
        {
            operations.retain(|operation| {
                if operation.path == context_path {
                    owned_paths.contains(&context_path)
                        || semantic.get("model_context_window").is_none()
                } else if operation.path == compact_path {
                    owned_paths.contains(&compact_path)
                        || semantic.get("model_auto_compact_token_limit").is_none()
                } else {
                    true
                }
            });
        } else {
            if owned_paths.contains(&context_path) {
                operations.push(PatchOperation {
                    operation: PatchKind::Remove,
                    path: context_path,
                    value: None,
                });
            }
            if owned_paths.contains(&compact_path) {
                operations.push(PatchOperation {
                    operation: PatchKind::Remove,
                    path: compact_path,
                    value: None,
                });
            }
        }
        Ok(operations)
    }

    fn refresh_patch_with_baseline(
        &self,
        document: &ConfigDocument,
        baseline: Option<&ConfigDocument>,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<Vec<PatchOperation>, String> {
        let web_search_path = path(&["web_search"]);
        if owned_paths.contains(&web_search_path) {
            return self.refresh_patch_for_document(document, input, owned_paths);
        }

        let baseline = baseline.ok_or_else(|| {
            "Codex Web Search is not owned by this legacy connection and its baseline is unavailable; disconnect and reconnect Codex before refreshing."
                .to_string()
        })?;
        if !same_top_level_toml_field(document, baseline, "web_search")? {
            return Err(
                "Codex Web Search changed after the legacy connection; disconnect and reconnect Codex to preserve the user value."
                    .to_string(),
            );
        }

        let mut migrated_owned_paths = owned_paths.to_vec();
        migrated_owned_paths.push(web_search_path);
        self.refresh_patch_for_document(document, input, &migrated_owned_paths)
    }

    fn validate_refresh_projected(
        &self,
        document: &ConfigDocument,
        input: &ConnectInput<'_>,
        owned_paths: &[ConfigPath],
    ) -> Result<(), String> {
        let owns_metadata = owned_paths.contains(&path(&["model_context_window"]))
            && owned_paths.contains(&path(&["model_auto_compact_token_limit"]));
        let validation_input = ConnectInput {
            base_url: input.base_url,
            token: input.token,
            adapter_ready: input.adapter_ready,
            model_metadata: owns_metadata.then_some(input.model_metadata).flatten(),
        };
        validate_codex_projection(
            document,
            &validation_input,
            owned_paths.contains(&path(&["web_search"])),
        )
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
        validate_codex_projection(document, input, true)
    }

    fn success_message(&self, input: &ConnectInput<'_>) -> String {
        let metadata = if input
            .model_metadata
            .and_then(AgentModelMetadata::safe_limits)
            .is_some()
        {
            "safe context and automatic compaction limits were synchronized"
        } else {
            "route limits are unknown, so the existing top-level context settings were preserved"
        };
        format!(
            "Codex now uses the Responses API at {} (~/.codex/config.toml and the Token Station model catalog are backed up; {}; hosted web search is disabled because dynamic translated routes cannot execute it). Quit and reopen Codex to load Token Station Auto.",
            input.base_url, metadata
        )
    }
}

fn same_top_level_toml_field(
    current: &ConfigDocument,
    baseline: &ConfigDocument,
    field: &str,
) -> Result<bool, String> {
    let ConfigDocument::Toml(current) = current else {
        return Err("Codex current configuration is not TOML".to_string());
    };
    let ConfigDocument::Toml(baseline) = baseline else {
        return Err("Codex baseline configuration is not TOML".to_string());
    };
    match (
        current.as_table().get_key_value(field),
        baseline.as_table().get_key_value(field),
    ) {
        (None, None) => Ok(true),
        (Some((current_key, current_item)), Some((baseline_key, baseline_item))) => Ok(
            current_key.display_repr() == baseline_key.display_repr()
                && current_key.decor() == baseline_key.decor()
                && current_item.to_string() == baseline_item.to_string(),
        ),
        _ => Ok(false),
    }
}

fn validate_codex_projection(
    document: &ConfigDocument,
    input: &ConnectInput<'_>,
    require_web_search_disabled: bool,
) -> Result<(), String> {
    CodexConnector.validate_source(document)?;
    let ConfigDocument::Toml(document) = document else {
        unreachable!();
    };
    let root = document.as_table();
    let provider = root
        .get("model_providers")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|providers| providers.get("tokenstation"))
        .and_then(toml_edit::Item::as_table_like)
        .ok_or_else(|| "Codex 写入前复验缺少 tokenstation provider".to_string())?;
    let valid = root.get("model").and_then(toml_edit::Item::as_str) == Some("auto")
        && root.get("model_provider").and_then(toml_edit::Item::as_str) == Some("tokenstation")
        && root
            .get("model_catalog_json")
            .and_then(toml_edit::Item::as_str)
            == Some(MODEL_CATALOG_RELATIVE_PATH)
        && (!require_web_search_disabled
            || root.get("web_search").and_then(toml_edit::Item::as_str) == Some("disabled"))
        && provider.get("base_url").and_then(toml_edit::Item::as_str) == Some(input.base_url);
    if !valid {
        return Err("Codex 写入前复验失败".to_string());
    }
    let token = input
        .token
        .ok_or_else(|| "Codex 写入前复验缺少本地虚拟 Key".to_string())?;
    for (field, expected) in provider_fields(token) {
        if !item_matches_json(provider.get(field), &expected) {
            return Err(format!("Codex 写入前复验字段 {field} 失败"));
        }
    }
    if provider.get("env_key").is_some() {
        return Err("Codex 写入前复验遗留 env_key".to_string());
    }
    if let Some((expected_context, output)) = input
        .model_metadata
        .and_then(AgentModelMetadata::safe_limits)
    {
        let context = root
            .get("model_context_window")
            .and_then(toml_edit::Item::as_integer);
        let compact = root
            .get("model_auto_compact_token_limit")
            .and_then(toml_edit::Item::as_integer);
        if context != Some(i64::from(expected_context))
            || compact != Some(i64::from(expected_context - output))
        {
            return Err("Codex 写入前复验模型上下文或自动压缩阈值失败".to_string());
        }
    }
    Ok(())
}

fn replace(segments: &[&str], value: serde_json::Value) -> PatchOperation {
    PatchOperation {
        operation: PatchKind::Replace,
        path: path(segments),
        value: Some(value),
    }
}

fn provider_fields(token: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("name", json!("token-station")),
        ("wire_api", json!("responses")),
        ("experimental_bearer_token", json!(token)),
        ("requires_openai_auth", json!(false)),
        ("request_max_retries", json!(0)),
        ("stream_max_retries", json!(0)),
    ]
}

fn token_station_model(codex_home: &Path, input: &ConnectInput<'_>) -> serde_json::Value {
    let (context_window, vision) = input
        .model_metadata
        .and_then(AgentModelMetadata::safe_limits)
        .map(|(context, _)| {
            (
                context,
                input.model_metadata.is_some_and(|metadata| metadata.vision),
            )
        })
        .unwrap_or((FALLBACK_CONTEXT_WINDOW, false));
    let mut model = installed_model_template(codex_home).unwrap_or_else(fallback_model_template);
    let object = model
        .as_object_mut()
        .expect("Codex model templates are JSON objects");
    for (field, value) in [
        ("slug", json!("auto")),
        ("display_name", json!("Token Station Auto")),
        (
            "description",
            json!("Routes each request through the active Token Station Agent route."),
        ),
        ("visibility", json!("list")),
        ("supported_in_api", json!(true)),
        ("priority", json!(0)),
        ("additional_speed_tiers", json!([])),
        ("service_tiers", json!([])),
        ("availability_nux", serde_json::Value::Null),
        ("upgrade", serde_json::Value::Null),
        ("context_window", json!(context_window)),
        ("max_context_window", json!(context_window)),
        (
            "input_modalities",
            if vision {
                json!(["text", "image"])
            } else {
                json!(["text"])
            },
        ),
        ("use_responses_lite", json!(false)),
    ] {
        object.insert(field.to_string(), value);
    }
    model
}

fn installed_model_template(codex_home: &Path) -> Option<serde_json::Value> {
    let source = read_config_source(&codex_home.join("models_cache.json")).ok()?;
    if !source.existed {
        return None;
    }
    let catalog: serde_json::Value = serde_json::from_slice(source.exact_bytes.as_slice()).ok()?;
    catalog
        .get("models")?
        .as_array()?
        .iter()
        .find(|model| {
            model.get("visibility").and_then(serde_json::Value::as_str) == Some("list")
                && model
                    .get("supported_in_api")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                && model
                    .get("supported_reasoning_levels")
                    .and_then(serde_json::Value::as_array)
                    .is_some()
                && model
                    .get("support_verbosity")
                    .and_then(serde_json::Value::as_bool)
                    .is_some()
                && model
                    .get("truncation_policy")
                    .and_then(serde_json::Value::as_object)
                    .is_some()
                && (model
                    .get("base_instructions")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
                    || model
                        .pointer("/model_messages/instructions_template")
                        .and_then(serde_json::Value::as_str)
                        .is_some())
        })
        .cloned()
}

fn fallback_model_template() -> serde_json::Value {
    json!({
        "slug": "auto",
        "display_name": "Token Station Auto",
        "description": "Routes each request through the active Token Station Agent route.",
        "base_instructions": "You are Codex, an AI coding agent. Follow the active developer instructions and collaborate with the user to complete software tasks in the current workspace.",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {
                "effort": "low",
                "description": "Fast responses with lighter reasoning"
            },
            {
                "effort": "medium",
                "description": "Balances speed and reasoning depth"
            },
            {
                "effort": "high",
                "description": "Greater reasoning depth for complex work"
            }
        ],
        "shell_type": "unified_exec",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 0,
        "support_verbosity": false,
        "apply_patch_tool_type": "freeform",
        "include_skills_usage_instructions": true,
        "include_plugin_usage_instructions": true,
        "include_apps_usage_instructions": true,
        "truncation_policy": {
            "mode": "tokens",
            "limit": 10_000
        },
        "context_window": FALLBACK_CONTEXT_WINDOW,
        "max_context_window": FALLBACK_CONTEXT_WINDOW,
        "experimental_supported_tools": [],
        "input_modalities": ["text"],
        "use_responses_lite": false
    })
}

fn item_matches_json(item: Option<&toml_edit::Item>, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::String(value) => item.and_then(toml_edit::Item::as_str) == Some(value),
        serde_json::Value::Bool(value) => item.and_then(toml_edit::Item::as_bool) == Some(*value),
        serde_json::Value::Number(value) => {
            item.and_then(toml_edit::Item::as_integer) == value.as_i64()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_field_matcher_rejects_unsupported_json_shapes() {
        assert!(!item_matches_json(None, &json!(null)));
        assert!(!item_matches_json(None, &json!(["unexpected"])));
    }

    #[test]
    fn codex_connection_embeds_the_sensitive_local_virtual_key_for_gui_clients() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        let operations = CodexConnector.connect_patch(&input).unwrap();
        assert!(operations.iter().any(|operation| {
            operation.path
                == path(&[
                    "model_providers",
                    "tokenstation",
                    "experimental_bearer_token",
                ])
                && operation.value == Some(json!("local-virtual-key"))
        }));
        assert!(operations.iter().any(|operation| {
            operation.operation == PatchKind::Remove
                && operation.path == path(&["model_providers", "tokenstation", "env_key"])
        }));
        assert!(CodexConnector.sensitive_paths().contains(&path(&[
            "model_providers",
            "tokenstation",
            "experimental_bearer_token",
        ])));
        assert!(CodexConnector.capabilities().requires_virtual_key);
    }

    #[test]
    fn codex_connection_disables_hosted_web_search_for_dynamic_routes() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        let operations = CodexConnector.connect_patch(&input).unwrap();

        assert!(CodexConnector
            .owned_paths()
            .contains(&path(&["web_search"])));
        assert!(operations.iter().any(|operation| {
            operation.path == path(&["web_search"]) && operation.value == Some(json!("disabled"))
        }));

        let mut projected = parse_source_bytes(None, DocumentFormat::Toml, "Codex").unwrap();
        apply_patch(&mut projected, &operations).unwrap();
        CodexConnector
            .validate_projected(&projected, &input)
            .unwrap();

        apply_patch(&mut projected, &[replace(&["web_search"], json!("live"))]).unwrap();
        assert!(CodexConnector
            .validate_projected(&projected, &input)
            .is_err());
    }

    #[test]
    fn codex_legacy_refresh_claims_unchanged_web_search_from_baseline() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let source = b"web_search = \"live\"\n";
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        let mut document =
            parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        let mut legacy_operations = CodexConnector.connect_patch(&input).unwrap();
        legacy_operations.retain(|operation| operation.path != path(&["web_search"]));
        apply_patch(&mut document, &legacy_operations).unwrap();
        let legacy_owned_paths = CodexConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["web_search"]))
            .collect::<Vec<_>>();

        let operations = CodexConnector
            .refresh_patch_with_baseline(
                &document,
                Some(&baseline),
                &input,
                &legacy_owned_paths,
            )
            .unwrap();

        assert!(operations.iter().any(|operation| {
            operation.path == path(&["web_search"])
                && operation.value == Some(json!("disabled"))
        }));
        apply_patch(&mut document, &operations).unwrap();
        let mut migrated_owned_paths = legacy_owned_paths;
        migrated_owned_paths.push(path(&["web_search"]));
        CodexConnector
            .validate_refresh_projected(&document, &input, &migrated_owned_paths)
            .unwrap();
        assert_eq!(
            semantic_json(&document).unwrap()["web_search"],
            json!("disabled")
        );
    }

    #[test]
    fn codex_legacy_refresh_rejects_web_search_changed_after_connection() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let source = b"web_search = \"live\"\n";
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        let mut document =
            parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        apply_patch(
            &mut document,
            &CodexConnector.connect_patch(&input).unwrap(),
        )
        .unwrap();
        apply_patch(&mut document, &[replace(&["web_search"], json!("cached"))]).unwrap();
        let legacy_owned_paths = CodexConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["web_search"]))
            .collect::<Vec<_>>();

        let error = match CodexConnector.refresh_patch_with_baseline(
                &document,
                Some(&baseline),
                &input,
                &legacy_owned_paths,
            ) {
            Ok(_) => panic!("a user-edited setting must not be claimed by refresh"),
            Err(error) => error,
        };

        assert!(error.contains("changed after the legacy connection"), "{error}");
    }

    #[test]
    fn codex_legacy_refresh_rejects_web_search_deleted_after_connection() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let source = b"web_search = \"live\"\n";
        let baseline = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        let mut document =
            parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();
        apply_patch(
            &mut document,
            &CodexConnector.connect_patch(&input).unwrap(),
        )
        .unwrap();
        apply_patch(
            &mut document,
            &[PatchOperation {
                operation: PatchKind::Remove,
                path: path(&["web_search"]),
                value: None,
            }],
        )
        .unwrap();
        let legacy_owned_paths = CodexConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["web_search"]))
            .collect::<Vec<_>>();

        let error = match CodexConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &legacy_owned_paths,
        ) {
            Ok(_) => panic!("a deleted user setting must not be reclaimed by refresh"),
            Err(error) => error,
        };

        assert!(error.contains("changed after the legacy connection"), "{error}");
    }

    #[test]
    fn codex_legacy_refresh_rejects_web_search_decoration_changed_after_connection() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let baseline = parse_source_bytes(
            Some(b"web_search = \"live\" # original\n"),
            DocumentFormat::Toml,
            "Codex",
        )
        .unwrap();
        let document = parse_source_bytes(
            Some(b"web_search = \"live\" # user edit\n"),
            DocumentFormat::Toml,
            "Codex",
        )
        .unwrap();
        let legacy_owned_paths = CodexConnector
            .owned_paths()
            .into_iter()
            .filter(|owned| owned != &path(&["web_search"]))
            .collect::<Vec<_>>();

        let error = match CodexConnector.refresh_patch_with_baseline(
            &document,
            Some(&baseline),
            &input,
            &legacy_owned_paths,
        ) {
            Ok(_) => panic!("a decoration-only user edit must not be reclaimed by refresh"),
            Err(error) => error,
        };

        assert!(error.contains("changed after the legacy connection"), "{error}");
    }

    #[test]
    fn codex_connection_projects_a_visible_token_station_model_catalog() {
        let metadata = AgentModelMetadata {
            context: 128_000,
            output: 8_192,
            max_input: 0,
            vision: true,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };

        let operations = CodexConnector.connect_patch(&input).unwrap();
        assert!(operations.iter().any(|operation| {
            operation.path == path(&["model_catalog_json"])
                && operation.value == Some(json!("model-catalogs/tokenstation.json"))
        }));

        let primary = Path::new("/tmp/token-station-codex-test/.codex/config.toml");
        let companions = CodexConnector
            .companion_projections(primary, &input)
            .unwrap();
        assert_eq!(companions.len(), 1);
        assert_eq!(
            companions[0].target_path,
            primary
                .parent()
                .unwrap()
                .join("model-catalogs")
                .join("tokenstation.json")
        );
        let catalog: serde_json::Value =
            serde_json::from_slice(companions[0].projected_bytes.as_slice()).unwrap();
        assert_eq!(catalog["models"].as_array().unwrap().len(), 1);
        assert_eq!(catalog["models"][0]["slug"], "auto");
        assert_eq!(catalog["models"][0]["display_name"], "Token Station Auto");
        assert_eq!(catalog["models"][0]["visibility"], "list");
        assert_eq!(catalog["models"][0]["context_window"], 128_000);
    }

    #[test]
    fn codex_connection_requires_reopening_codex_to_reload_the_catalog() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };

        assert!(CodexConnector.capabilities().restart_required);
        assert!(CodexConnector
            .success_message(&input)
            .contains("reopen Codex"));
    }

    #[test]
    fn codex_catalog_uses_a_practical_compatibility_window_without_route_metadata() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let primary = Path::new("/tmp/token-station-codex-fallback/.codex/config.toml");

        let companions = CodexConnector
            .companion_projections(primary, &input)
            .unwrap();
        let catalog: serde_json::Value =
            serde_json::from_slice(companions[0].projected_bytes.as_slice()).unwrap();

        assert_eq!(catalog["models"][0]["context_window"], 32_000);
        assert_eq!(catalog["models"][0]["max_context_window"], 32_000);
    }

    #[test]
    fn codex_projection_validation_requires_the_managed_catalog_pointer() {
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: None,
        };
        let source = br#"
            model = "auto"
            model_provider = "tokenstation"

            [model_providers.tokenstation]
            name = "token-station"
            base_url = "http://127.0.0.1:8787/agents/codex/v1"
            wire_api = "responses"
            experimental_bearer_token = "local-virtual-key"
            requires_openai_auth = false
            request_max_retries = 0
            stream_max_retries = 0
        "#;
        let document = parse_source_bytes(Some(source), DocumentFormat::Toml, "Codex").unwrap();

        assert!(CodexConnector
            .validate_projected(&document, &input)
            .is_err());
    }

    #[test]
    fn codex_catalog_preserves_the_installed_codex_model_template() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "token-station-codex-template-{}-{nonce}",
            std::process::id()
        ));
        let codex_home = root.join(".codex");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::write(
            codex_home.join("models_cache.json"),
            br#"{
                "models": [{
                    "slug": "installed-model",
                    "display_name": "Installed Model",
                    "description": "Installed description",
                    "model_messages": {
                        "instructions_template": "installed Codex instructions"
                    },
                    "supported_reasoning_levels": [{
                        "effort": "medium",
                        "description": "Installed reasoning"
                    }],
                    "support_verbosity": true,
                    "truncation_policy": {"mode": "tokens", "limit": 1234},
                    "experimental_supported_tools": [],
                    "context_window": 64000,
                    "visibility": "list",
                    "supported_in_api": true,
                    "priority": 4,
                    "installed_template_marker": "preserve"
                }]
            }"#,
        )
        .unwrap();
        let metadata = AgentModelMetadata {
            context: 32_000,
            output: 4_000,
            max_input: 0,
            vision: false,
            tools: true,
            reasoning: true,
            cost: None,
        };
        let input = ConnectInput {
            base_url: "http://127.0.0.1:8787/agents/codex/v1",
            token: Some("local-virtual-key"),
            adapter_ready: true,
            model_metadata: Some(&metadata),
        };

        let companions = CodexConnector
            .companion_projections(&codex_home.join("config.toml"), &input)
            .unwrap();
        let catalog: serde_json::Value =
            serde_json::from_slice(companions[0].projected_bytes.as_slice()).unwrap();
        let model = &catalog["models"][0];
        assert_eq!(model["slug"], "auto");
        assert_eq!(model["display_name"], "Token Station Auto");
        assert_eq!(model["context_window"], 32_000);
        assert_eq!(model["use_responses_lite"], false);
        assert_eq!(model["installed_template_marker"], "preserve");
        assert_eq!(
            model["model_messages"]["instructions_template"],
            "installed Codex instructions"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
