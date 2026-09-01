use crate::*;

pub(crate) const MANAGED_ENTERPRISE_PROVIDER_ID: &str = "tokenstation";
const PROVIDER_PURGE_PENDING_VERSION: u32 = 1;
const PROVIDER_PURGE_PENDING_FILE: &str = "provider-purge-pending.json";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderPurgePending {
    version: u32,
    providers: Vec<String>,
}

fn provider_purge_pending_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PROVIDER_PURGE_PENDING_FILE)
}

fn load_pending_provider_purge(data_dir: &Path) -> Result<Option<ProviderPurgePending>, String> {
    let path = provider_purge_pending_path(data_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Provider 凭据待清理记录不可读：{error}")),
    };
    let pending: ProviderPurgePending = serde_json::from_str(&text)
        .map_err(|error| format!("Provider 凭据待清理记录损坏，已拒绝复用名称：{error}"))?;
    if pending.version != PROVIDER_PURGE_PENDING_VERSION {
        return Err(format!(
            "Provider 凭据待清理记录版本 {} 不受支持，已拒绝复用名称",
            pending.version
        ));
    }
    if pending.providers.is_empty() {
        return Err("Provider 凭据待清理记录为空，已拒绝复用名称".to_owned());
    }
    let unique = pending
        .providers
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != pending.providers.len()
        || pending.providers.iter().any(|name| name.trim().is_empty())
    {
        return Err("Provider 凭据待清理记录包含无效或重复名称，已拒绝复用".to_owned());
    }
    Ok(Some(pending))
}

fn persist_pending_provider_purge(data_dir: &Path, providers: &[String]) -> Result<(), String> {
    let pending = ProviderPurgePending {
        version: PROVIDER_PURGE_PENDING_VERSION,
        providers: providers.to_vec(),
    };
    let mut rendered = serde_json::to_string_pretty(&pending)
        .map_err(|error| format!("序列化 Provider 凭据待清理记录失败：{error}"))?;
    rendered.push('\n');
    crate::agent_integration::safe_fs::write_atomic_private(
        &provider_purge_pending_path(data_dir),
        rendered.as_bytes(),
    )
    .map_err(|error| format!("保存 Provider 凭据待清理记录失败：{error}"))
}

fn remove_pending_provider_purge(data_dir: &Path) -> Result<(), String> {
    let path = provider_purge_pending_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => crate::agent_integration::safe_fs::sync_parent(data_dir)
            .map_err(|error| format!("同步 Provider 凭据待清理记录目录失败：{error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("移除 Provider 凭据待清理记录失败：{error}")),
    }
}

fn recover_pending_provider_purge(inner: &mut AppInner) -> Result<(), String> {
    let data_dir = inner.data_dir();
    let Some(pending) = load_pending_provider_purge(&data_dir)? else {
        return Ok(());
    };
    let tombstoned = provider_tombstones::names(&data_dir)?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let pending_names = pending
        .providers
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let retained = pending_names.intersection(&tombstoned).count();
    if retained == pending_names.len() {
        return remove_pending_provider_purge(&data_dir);
    }
    if retained != 0 {
        return Err("Provider 回收站与凭据待清理记录状态不一致，已拒绝自动删除凭据".to_owned());
    }
    let active_draft = pending
        .providers
        .iter()
        .filter(|name| inner.draft["upstreams"].get(name.as_str()).is_some())
        .cloned()
        .collect::<Vec<_>>();
    if !active_draft.is_empty() {
        return Err(format!(
            "当前草稿已复用待清理 Provider {}，已拒绝删除其凭据",
            active_draft.join("、")
        ));
    }
    if inner.config_path.exists() {
        let persisted = ClientConfig::load(&inner.config_path).map_err(|error| {
            format!("无法核对磁盘配置中的待清理 Provider，已拒绝删除凭据：{error}")
        })?;
        let active_persisted = pending
            .providers
            .iter()
            .filter(|name| persisted.upstreams.contains_key(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !active_persisted.is_empty() {
            return Err(format!(
                "磁盘配置已复用待清理 Provider {}，已拒绝删除其凭据",
                active_persisted.join("、")
            ));
        }
    }
    if inner.config_state.is_dirty() {
        return Err("Provider 永久删除已提交，但凭据和定价清理待重试；请先保存当前配置".to_owned());
    }
    secrets::store_remove_provider_api_keys(&data_dir, &pending.providers)?;
    let previous_pricing = inner.draft["pricing"].clone();
    let previous_state = inner.config_state.clone();
    let pricing_changed = clear_provider_scoped_prices_for(inner, &pending.providers)?;
    if pricing_changed {
        if let Err(error) = inner.observe_draft().and_then(|()| inner.save_draft()) {
            inner.draft["pricing"] = previous_pricing;
            inner.config_state = previous_state;
            return Err(format!(
                "Provider 永久删除已提交，但保存定价清理失败，已记录待重试：{error}"
            ));
        }
    }
    remove_pending_provider_purge(&data_dir)
}

pub(crate) fn recover_pending_provider_purge_on_startup(
    inner: &mut AppInner,
) -> Result<(), String> {
    recover_pending_provider_purge(inner)
}

fn is_managed_enterprise_provider_id(name: &str) -> bool {
    name == MANAGED_ENTERPRISE_PROVIDER_ID
        || name.strip_prefix("tokenstation_").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub(crate) const PROVIDER_BRANDS_BY_BASE_URL: &[(&str, &str)] = &[
    ("https://api.openai.com/v1", "openai"),
    ("https://api.anthropic.com/v1", "anthropic"),
    (
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "gemini",
    ),
    ("https://api.deepseek.com/v1", "deepseek"),
    ("https://open.bigmodel.cn/api/paas/v4", "glm_cn"),
    ("https://api.z.ai/api/paas/v4", "glm"),
    ("https://api.z.ai/api/coding/paas/v4", "glm_coding"),
    ("https://api.moonshot.cn/v1", "kimi"),
    ("https://api.moonshot.ai/v1", "kimi_global"),
    ("https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen"),
    (
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        "qwen_singapore",
    ),
    (
        "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
        "qwen_us",
    ),
    ("https://api.minimaxi.com/v1", "minimax_cn"),
    ("https://api.minimax.io/v1", "minimax_global"),
    ("https://api.groq.com/openai/v1", "groq"),
    ("https://integrate.api.nvidia.com/v1", "nvidia_nim"),
    ("https://api.mistral.ai/v1", "mistral"),
    ("https://api.x.ai/v1", "xai"),
    ("https://ark.cn-beijing.volces.com/api/v3", "volcengine_ark"),
    (
        "https://ark.cn-beijing.volces.com/api/coding/v3",
        "volcengine_ark_coding",
    ),
    (
        "https://ark.ap-southeast.bytepluses.com/api/v3",
        "byteplus_ark",
    ),
    (
        "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
        "byteplus_ark_coding",
    ),
    ("https://api.siliconflow.cn/v1", "siliconflow"),
    ("https://api.siliconflow.com/v1", "siliconflow_global"),
    ("https://api.together.ai/v1", "together"),
    ("https://api.fireworks.ai/inference/v1", "fireworks"),
    ("https://api.deepinfra.com/v1/openai", "deepinfra"),
    ("https://api.cerebras.ai/v1", "cerebras"),
    ("https://api.sambanova.ai/v1", "sambanova"),
    ("https://api.cohere.ai/compatibility/v1", "cohere"),
    ("https://models.github.ai/inference", "github_models"),
    ("https://qianfan.baidubce.com/v2", "qianfan"),
    ("https://api.hunyuan.cloud.tencent.com/v1", "hunyuan"),
    ("https://api.stepfun.com/v1", "stepfun"),
    ("https://api.stepfun.com/step_plan/v1", "stepfun_plan"),
    ("https://api.xiaomimimo.com/v1", "xiaomi_mimo"),
    ("https://api.perplexity.ai", "perplexity"),
    ("https://api.novita.ai/v3/openai", "novita"),
    ("https://api.hyperbolic.xyz/v1", "hyperbolic"),
    ("https://api.studio.nebius.com/v1", "nebius"),
    ("http://127.0.0.1:11434/v1", "ollama"),
    ("http://localhost:11434/v1", "ollama"),
    ("https://openrouter.ai/api/v1", "openrouter"),
];

pub(crate) fn provider_brand_id(
    upstream_name: &str,
    base_url: &str,
    access_tier: &str,
) -> Option<&'static str> {
    let normalized = base_url.trim().trim_end_matches('/');
    if access_tier == "free" {
        if let Some(preset) = free_provider_catalog::presets().iter().find(|preset| {
            preset.upstream_name == upstream_name
                && preset.base_url.trim_end_matches('/') == normalized
        }) {
            return Some(preset.id);
        }
    }
    PROVIDER_BRANDS_BY_BASE_URL
        .iter()
        .find_map(|(known_url, brand_id)| (*known_url == normalized).then_some(*brand_id))
}

/// The official OpenAI-compatible South component, by its builtin manifest
/// name or by the package directory the staged copy lives in.
pub(crate) fn is_official_openai_compatible_package(package: &str) -> bool {
    matches!(
        package,
        "provider-openai-compatible" | "provider-openai-compatible-v2"
    )
}

/// The engine an upstream runs on when its config names none: the full South
/// transport. Mirrors `ProviderCallEngine::default()` in the CLI crate.
pub(crate) const DEFAULT_PROVIDER_CALL: &str = "south_v1_buffered_streaming_header_auth";

pub(crate) fn south_v1_unavailable_reason(
    draft: &Value,
    upstream: &Value,
    package_verified: bool,
) -> Option<&'static str> {
    if upstream
        .get("api_dialect")
        .and_then(Value::as_str)
        .is_some_and(|dialect| dialect != "translated")
    {
        return Some("api_dialect");
    }
    if !package_verified
        || upstream["provider"].as_str() != Some("openai-compatible")
        || draft["plugins"]["providers"]["openai-compatible"]
            .as_str()
            .is_some_and(|package| !is_official_openai_compatible_package(package))
    {
        return Some("provider_package");
    }

    if draft["egress"]["mode"]
        .as_str()
        .is_some_and(|mode| mode != "direct")
    {
        return Some("egress");
    }
    if upstream["auth"]["store"].as_bool() != Some(true) && !upstream["auth"]["env"].is_string() {
        return Some("auth");
    }
    None
}

pub(crate) fn south_header_auth_v1_unavailable_reason(
    draft: &Value,
    upstream: &Value,
    package_verified: bool,
) -> Option<&'static str> {
    let provider = upstream["provider"].as_str().unwrap_or_default();
    if upstream
        .get("api_dialect")
        .and_then(Value::as_str)
        .is_some_and(|dialect| dialect != "translated")
    {
        return Some("api_dialect");
    }
    if !package_verified
        || !matches!(provider, "openai-compatible" | "azure-openai-v1")
        || draft["plugins"]["providers"][provider]
            .as_str()
            .is_some_and(|package| !is_official_openai_compatible_package(package))
    {
        return Some("provider_package");
    }

    if draft["egress"]["mode"]
        .as_str()
        .is_some_and(|mode| mode != "direct")
    {
        return Some("egress");
    }
    if upstream["auth"]["store"].as_bool() != Some(true) && !upstream["auth"]["env"].is_string() {
        return Some("auth");
    }
    None
}

pub(crate) fn south_approved_dialects(registry: &PluginRegistry) -> BTreeSet<String> {
    // Every bound dialect. Provenance is no longer judged here: a component is
    // admitted at Gateway startup — source trust, compatibility handshake, Wasm
    // gates, identity — and a package that fails any of them fails startup
    // rather than quietly losing South. Re-deciding it in a settings view could
    // only produce a second, weaker opinion.
    registry
        .provider_dialects()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(crate) fn south_approved_dialects_for_draft(draft: &Value) -> BTreeSet<String> {
    let Ok(plugins) = serde_json::from_value::<PluginsConfig>(draft["plugins"].clone()) else {
        return BTreeSet::new();
    };
    let Some(data_dir) = draft["data"]["dir"].as_str().map(PathBuf::from) else {
        return BTreeSet::new();
    };
    let Ok(receipts) = Receipts::load(&data_dir) else {
        return BTreeSet::new();
    };
    PluginRegistry::discover(&plugins, &receipts)
        .map(|registry| south_approved_dialects(&registry))
        .unwrap_or_default()
}

pub(crate) fn record_changed_upstream_epochs(
    before: &Value,
    after: &Value,
    epochs: &mut BTreeMap<String, u64>,
) {
    let before = before.get("upstreams").and_then(Value::as_object);
    let after = after.get("upstreams").and_then(Value::as_object);
    let names = before
        .into_iter()
        .flat_map(|values| values.keys())
        .chain(after.into_iter().flat_map(|values| values.keys()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let previous = before.and_then(|values| values.get(&name));
        let current = after.and_then(|values| values.get(&name));
        if previous != current {
            let epoch = epochs.entry(name).or_default();
            *epoch = epoch.saturating_add(1).max(1);
        }
    }
}

/// Preview the provider URL selected by each inbound protocol before saving.
#[tauri::command]
pub(crate) fn preview_provider_endpoints(
    base_url: String,
) -> Result<ProviderEndpointPreview, String> {
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    Ok(ProviderEndpointPreview {
        chat: endpoint.resolve(ProviderApi::ChatCompletions),
        responses: endpoint.resolve(ProviderApi::Responses),
        messages: endpoint.resolve(ProviderApi::Messages),
        loopback: endpoint.is_loopback(),
    })
}

pub(crate) fn ensure_credential_transport(
    endpoint: &ProviderEndpoint,
    egress: &EgressConfig,
) -> Result<(), String> {
    if !endpoint.uses_https() && !endpoint.is_loopback() {
        return Err("Remote Provider endpoints must use HTTPS".to_owned());
    }
    if !endpoint.uses_https() && !egress.bypasses_proxy(&endpoint.as_str())? {
        return Err(
            "Plaintext loopback Provider endpoints must bypass the configured proxy".to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn draft_egress_config(draft: &Value) -> Result<EgressConfig, String> {
    match draft.get("egress").filter(|value| !value.is_null()) {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("出站配置不合法：{error}")),
        None => Ok(EgressConfig::default()),
    }
}

pub(crate) fn ensure_generic_provider_mutation_allowed(
    inner: &AppInner,
    name: &str,
) -> Result<(), String> {
    if inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("access_tier"))
        .and_then(Value::as_str)
        == Some("free")
    {
        return Err(format!(
            "免费供应商 `{name}` 由内置目录管理，不能通过通用 Provider 接口修改"
        ));
    }
    Ok(())
}

pub(crate) fn normalize_provider_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > model_catalog::MAX_MODELS_PER_PROVIDER {
        return Err(format!(
            "A Provider may configure at most {} models",
            model_catalog::MAX_MODELS_PER_PROVIDER
        ));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if model.len() > model_catalog::MAX_MODEL_ID_BYTES || model.chars().any(char::is_control) {
            return Err(format!(
                "Model IDs must be 1-{} bytes and contain no control characters",
                model_catalog::MAX_MODEL_ID_BYTES
            ));
        }
        if seen.insert(model.to_owned()) {
            normalized.push(model.to_owned());
        }
    }
    if normalized.is_empty() {
        return Err("请至少填一个模型".to_owned());
    }
    Ok(normalized)
}

pub(crate) fn provider_auth_value(
    source: &str,
    reference: Option<&str>,
) -> Result<Option<Value>, String> {
    let reference = reference.map(str::trim).filter(|value| !value.is_empty());
    match source {
        "none" => Ok(None),
        "store" => Ok(Some(json!({ "slot": "provider_api_key", "store": true }))),
        "env" => {
            let name = reference.ok_or("环境变量凭据需要填写变量名")?;
            if name.len() > 128
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || name.as_bytes().first().is_some_and(u8::is_ascii_digit)
            {
                return Err("环境变量名只能包含字母、数字和下划线，且不能以数字开头".to_owned());
            }
            Ok(Some(json!({ "slot": "provider_api_key", "env": name })))
        }
        "file" => {
            let path = std::path::Path::new(reference.ok_or("文件凭据需要填写绝对路径")?);
            if !path.is_absolute() {
                return Err("文件凭据必须使用绝对路径".to_owned());
            }
            Ok(Some(json!({
                "slot": "provider_api_key",
                "file": path.to_string_lossy()
            })))
        }
        other => Err(format!("未知凭据来源 `{other}`")),
    }
}

pub(crate) fn is_free_provider_value(provider: &Value) -> bool {
    provider["access_tier"].as_str() == Some("free")
}

#[tauri::command]
pub(crate) fn list_free_provider_presets() -> Vec<free_provider_catalog::FreeProviderPreset> {
    free_provider_catalog::presets().to_vec()
}

#[tauri::command]
pub(crate) async fn add_free_provider(
    state: State<'_, AppStateManaged>,
    preset_id: String,
    selected_models: Vec<String>,
    api_key: String,
    guard_confirmed: bool,
) -> Result<StateView, String> {
    let preset = free_provider_catalog::find(preset_id.trim())
        .ok_or_else(|| format!("未知免费供应商 `{}`", preset_id.trim()))?;
    let api_key = api_key.trim().to_owned();
    if api_key.is_empty() {
        return Err("请填写 API Key".to_owned());
    }
    if preset.overage_policy == free_provider_catalog::OveragePolicy::UserMustEnableGuard
        && !guard_confirmed
    {
        return Err("请先确认已在供应商控制台启用免费额度保护".to_owned());
    }

    let allowed: std::collections::BTreeMap<&str, &free_provider_catalog::FreeModelPreset> = preset
        .models
        .iter()
        .map(|model| (model.id, model))
        .collect();
    let mut selected = Vec::new();
    for raw in selected_models {
        let model = raw.trim();
        if model.is_empty() || selected.iter().any(|existing: &String| existing == model) {
            continue;
        }
        if !allowed.contains_key(model) {
            return Err(format!("模型 `{model}` 不在该供应商的免费目录中"));
        }
        selected.push(model.to_owned());
    }
    if selected.is_empty() {
        return Err("请至少选择一个免费模型".to_owned());
    }

    let (egress, egress_secrets, expected_revision, expected_tombstone) = {
        let mut inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        if inner.draft["upstreams"].get(preset.upstream_name).is_some() {
            return Err(format!(
                "免费供应商 `{}` 已存在，请先在主页管理该实例",
                preset.upstream_name
            ));
        }
        let tombstone = provider_tombstones::get(&inner.data_dir(), preset.upstream_name)?;
        if tombstone
            .as_ref()
            .is_some_and(|provider| !is_free_provider_value(provider))
        {
            return Err(format!(
                "Provider 回收站中已有同名普通供应商 `{}`，请先恢复或彻底处理该实例",
                preset.upstream_name
            ));
        }
        let egress = draft_egress_config(&inner.draft)?;
        if inner.pending_free_providers.contains(preset.upstream_name) {
            return Err(format!(
                "免费供应商 `{}` 正在验证，请等待当前请求完成",
                preset.upstream_name
            ));
        }
        const MAX_CONCURRENT_FREE_VALIDATIONS: usize = 2;
        if inner.pending_free_providers.len() >= MAX_CONCURRENT_FREE_VALIDATIONS {
            return Err("免费供应商验证任务已达并发上限，请稍后重试".to_owned());
        }
        inner
            .pending_free_providers
            .insert(preset.upstream_name.to_owned());
        (
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
            inner.config_state.draft_revision(),
            tombstone,
        )
    };
    let _validation_guard = FreeProviderValidationGuard {
        inner: &state.0,
        upstream: preset.upstream_name.to_owned(),
    };

    let validate_preset = *preset;
    let validate_models = selected.clone();
    let validate_key = api_key.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for model in validate_models {
            free_provider_catalog::validate_chat_completion(
                &validate_preset,
                &model,
                &validate_key,
                &egress,
                &egress_secrets,
            )
            .map_err(|error| format!("模型 `{model}` 验证失败：{error}"))?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| format!("免费模型验证任务异常结束：{error}"))??;

    let model_objs: Vec<Value> = selected
        .iter()
        .filter_map(|id| allowed.get(id.as_str()).copied())
        .map(|model| {
            let capability_bool = |state: CapabilityState| {
                matches!(state, CapabilityState::Verified | CapabilityState::Declared)
            };
            json!({
                "model": model.id,
                "tool": capability_bool(model.tool),
                "vision": capability_bool(model.vision),
                "json_schema": capability_bool(model.json_schema),
                "tool_state": model.tool,
                "vision_state": model.vision,
                "json_schema_state": model.json_schema,
                "context_window": model.context_window,
            })
        })
        .collect();

    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if inner.config_state.draft_revision() != expected_revision {
        return Err("验证期间配置已变化，请按当前出站设置重新验证".to_owned());
    }
    if inner.draft["upstreams"].get(preset.upstream_name).is_some() {
        return Err(format!("免费供应商 `{}` 已存在", preset.upstream_name));
    }
    let data_dir = inner.data_dir();
    let current_tombstone = provider_tombstones::get(&data_dir, preset.upstream_name)?;
    if current_tombstone != expected_tombstone {
        return Err(format!(
            "验证期间 Provider `{}` 的回收状态已变化，请重试",
            preset.upstream_name
        ));
    }
    let previous_draft = inner.draft.clone();
    let previous_config_state = inner.config_state.clone();
    inner.draft["upstreams"][preset.upstream_name] = json!({
        "provider": "openai-compatible",
        "base_url": preset.base_url,
        "access_tier": "free",
        "auth": { "slot": "provider_api_key", "store": true },
        "models": model_objs,
    });
    if let Err(error) = inner.observe_draft() {
        inner.draft = previous_draft;
        inner.config_state = previous_config_state;
        return Err(error);
    }
    inner
        .pending_provider_keys
        .insert(preset.upstream_name.to_owned(), Zeroizing::new(api_key));
    inner
        .pending_provider_key_removals
        .remove(preset.upstream_name);
    Ok(inner.snapshot())
}

/// Infer context windows from explicit size markers in model IDs such as
/// `moonshot-v1-128k`, `glm-5.2[1m]`, and `qwen-turbo-1m`. Use the largest
/// numeric k or m marker within 8k to 10M, avoiding version numbers like `glm-4.6`.
pub(crate) fn context_window_from_marker(name: &str) -> Option<u64> {
    let bytes = name.as_bytes();
    let mut best: Option<u64> = None;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && (bytes[i] == b'k' || bytes[i] == b'm') {
            if let Ok(n) = name[start..i].parse::<u64>() {
                let unit = if bytes[i] == b'm' { 1_000_000 } else { 1_000 };
                let window = n.saturating_mul(unit);
                if (8_000..=10_000_000).contains(&window) {
                    best = Some(best.map_or(window, |current| current.max(window)));
                }
            }
            i += 1;
        }
    }
    best
}

/// A best-effort real context window for a freshly added model, so it is not
/// stuck at a blanket default that under-reports big-context models (the user's
/// `glm-5.2[1m]` is 1M, not 128k). An explicit size marker in the id wins;
/// otherwise a small family table; otherwise 128k. Only a starting value — the
/// operator can override it per model, and routing forwards over-context
/// requests rather than refusing them, so an imperfect guess never hard-fails.
pub(crate) fn known_context_window(model: &str) -> u64 {
    let name = model.to_ascii_lowercase();
    if let Some(window) = context_window_from_marker(&name) {
        return window;
    }
    if name.contains("gemini") {
        return 1_000_000;
    }
    if name.contains("claude") {
        return 200_000;
    }
    128_000
}

pub(crate) const CONTEXT_WINDOW_SOURCE_KEY: &str = "x-token-station-context-window-source";
pub(crate) const MAX_OUTPUT_TOKENS_SOURCE_KEY: &str = "x-token-station-max-output-tokens-source";
pub(crate) const LIMIT_SOURCE_PROVIDER: &str = "provider";
pub(crate) const LIMIT_SOURCE_BUILTIN_PRESET: &str = "builtin_preset";
pub(crate) const LIMIT_SOURCE_OPERATOR: &str = "operator";
pub(crate) const LIMIT_SOURCE_HEURISTIC: &str = "heuristic";

#[derive(Clone, Copy)]
pub(crate) struct BuiltinModelLimits {
    pub(crate) context_window: u32,
    pub(crate) max_output_tokens: u32,
}

pub(crate) fn builtin_model_limits(base_url: &str, model: &str) -> Option<BuiltinModelLimits> {
    let endpoint = base_url.trim().trim_end_matches('/');
    if !matches!(
        endpoint,
        "https://api.moonshot.cn/v1" | "https://api.moonshot.ai/v1"
    ) {
        return None;
    }
    match model {
        "kimi-k2.6" => Some(BuiltinModelLimits {
            context_window: 262_144,
            max_output_tokens: 262_144,
        }),
        "kimi-k3" => Some(BuiltinModelLimits {
            context_window: 1_048_576,
            max_output_tokens: 131_072,
        }),
        _ => None,
    }
}

pub(crate) fn model_limit_source(capability: &ModelCapability, key: &str) -> Option<String> {
    capability
        .extensions
        .get(key)
        .and_then(Value::as_str)
        .filter(|source| {
            matches!(
                *source,
                LIMIT_SOURCE_PROVIDER
                    | LIMIT_SOURCE_BUILTIN_PRESET
                    | LIMIT_SOURCE_OPERATOR
                    | LIMIT_SOURCE_HEURISTIC
            )
        })
        .map(str::to_owned)
}

pub(crate) fn json_limit_source<'a>(capability: &'a Value, key: &str) -> Option<&'a str> {
    capability.get(key).and_then(Value::as_str)
}

pub(crate) fn source_is_default(source: Option<&str>) -> bool {
    matches!(
        source,
        Some(LIMIT_SOURCE_BUILTIN_PRESET | LIMIT_SOURCE_HEURISTIC)
    )
}

pub(crate) fn apply_builtin_model_limits_to_upstream(upstream: &mut Value) -> bool {
    let base_url = upstream["base_url"].as_str().unwrap_or_default().to_owned();
    let Some(models) = upstream["models"].as_array_mut() else {
        return false;
    };
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(preset) = builtin_model_limits(&base_url, &model) else {
            continue;
        };
        let context = capability["context_window"].as_u64().unwrap_or_default();
        let output = capability["max_output_tokens"].as_u64().unwrap_or_default();
        let context_source =
            json_limit_source(capability, CONTEXT_WINDOW_SOURCE_KEY).map(str::to_owned);
        let output_source =
            json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY).map(str::to_owned);
        let legacy_heuristic = output == 0
            && output_source.is_none()
            && context == known_context_window(&model)
            && context_source
                .as_deref()
                .is_none_or(|source| source == LIMIT_SOURCE_HEURISTIC);

        if (context == 0 || legacy_heuristic || source_is_default(context_source.as_deref()))
            && (context != u64::from(preset.context_window)
                || context_source.as_deref() != Some(LIMIT_SOURCE_BUILTIN_PRESET))
        {
            capability["context_window"] = json!(preset.context_window);
            capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
            changed = true;
        }

        let effective_context = capability["context_window"].as_u64().unwrap_or_default();
        if (output == 0 || source_is_default(output_source.as_deref()))
            && u64::from(preset.max_output_tokens) <= effective_context
            && (output != u64::from(preset.max_output_tokens)
                || output_source.as_deref() != Some(LIMIT_SOURCE_BUILTIN_PRESET))
        {
            capability["max_output_tokens"] = json!(preset.max_output_tokens);
            capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
            changed = true;
        }
    }
    changed
}

pub(crate) fn provider_uses_builtin_model_limits(upstream: &Value) -> bool {
    upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|capability| {
            json_limit_source(capability, CONTEXT_WINDOW_SOURCE_KEY)
                == Some(LIMIT_SOURCE_BUILTIN_PRESET)
                || json_limit_source(capability, MAX_OUTPUT_TOKENS_SOURCE_KEY)
                    == Some(LIMIT_SOURCE_BUILTIN_PRESET)
        })
}

/// Add an OpenAI-compatible upstream provider, storing its key in the system keychain when present.
#[tauri::command]
pub(crate) fn add_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
) -> Result<StateView, String> {
    let source = if api_key.as_deref().is_some_and(|key| !key.trim().is_empty()) {
        "store"
    } else {
        "none"
    };
    add_provider_impl(
        state,
        name,
        base_url,
        models,
        api_key,
        local,
        source,
        None,
        "openai-compatible",
        false,
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_provider_with_credential(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
    credential_source: String,
    credential_reference: Option<String>,
    provider_dialect: Option<String>,
) -> Result<StateView, String> {
    let provider_dialect = provider_dialect.as_deref().unwrap_or("openai-compatible");
    add_provider_impl(
        state,
        name,
        base_url,
        models,
        api_key,
        local,
        credential_source.trim(),
        credential_reference.as_deref(),
        provider_dialect,
        false,
    )
}

#[tauri::command]
pub(crate) fn add_managed_enterprise_route(
    state: State<'_, AppStateManaged>,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<StateView, String> {
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    let model = model.trim().to_owned();
    if model.is_empty() {
        return Err("企业路由模型不能为空".to_owned());
    }
    loop {
        let (existing_provider, new_provider) = {
            let inner = state.0.lock().unwrap();
            let existing_provider = inner.draft["upstreams"]
                .as_object()
                .into_iter()
                .flat_map(|upstreams| upstreams.iter())
                .find(|(name, upstream)| {
                    is_managed_enterprise_provider_id(name)
                        && upstream["managed_route"].as_bool() == Some(true)
                        && upstream["base_url"].as_str() == Some(base_url.as_str())
                })
                .map(|(name, _)| name.clone());
            let new_provider = if existing_provider.is_none() {
                Some(next_managed_enterprise_provider_id(&inner)?)
            } else {
                None
            };
            (existing_provider, new_provider)
        };
        if let Some(provider) = existing_provider {
            let result = extend_managed_enterprise_route(
                state.clone(),
                provider.clone(),
                base_url.clone(),
                api_key.clone(),
                model.clone(),
            );
            if result.is_ok() {
                return result;
            }
            let still_matches = {
                let inner = state.0.lock().unwrap();
                inner.draft["upstreams"]
                    .get(&provider)
                    .is_some_and(|upstream| {
                        upstream["managed_route"].as_bool() == Some(true)
                            && upstream["base_url"].as_str() == Some(base_url.as_str())
                    })
            };
            if still_matches {
                return result;
            }
            continue;
        }
        let provider = new_provider.expect("a new managed Provider Channel id was allocated");
        let result = add_provider_impl(
            state.clone(),
            provider.clone(),
            base_url.clone(),
            vec![model.clone()],
            Some(api_key.clone()),
            false,
            "store",
            None,
            "openai-compatible",
            true,
        );
        if result.is_ok() {
            return result;
        }
        let allocation_was_claimed = {
            let inner = state.0.lock().unwrap();
            inner.draft["upstreams"].get(&provider).is_some()
                || provider_tombstones::contains(&inner.data_dir(), &provider)?
        };
        if !allocation_was_claimed {
            return result;
        }
    }
}

pub(crate) fn next_managed_enterprise_provider_id(inner: &AppInner) -> Result<String, String> {
    let data_dir = inner.data_dir();
    let mut reserved = provider_tombstones::names(&data_dir)?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(pending) = load_pending_provider_purge(&data_dir)? {
        reserved.extend(pending.providers);
    }
    for index in 1..=10_000 {
        let candidate = if index == 1 {
            MANAGED_ENTERPRISE_PROVIDER_ID.to_owned()
        } else {
            format!("{MANAGED_ENTERPRISE_PROVIDER_ID}_{index}")
        };
        if inner.draft["upstreams"].get(&candidate).is_none() && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err("企业供应商数量已超过支持的上限".to_owned())
}

fn extend_managed_enterprise_route(
    state: State<'_, AppStateManaged>,
    provider: String,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<StateView, String> {
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    let api_key = api_key.trim().to_owned();
    if api_key.is_empty() {
        return Err("本地凭据存储需要填写 API Key".to_owned());
    }
    let model = normalize_provider_model_ids(vec![model])?
        .into_iter()
        .next()
        .expect("one normalized enterprise model");

    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    recover_pending_provider_purge(&mut inner)?;
    let egress = draft_egress_config(&inner.draft)?;
    ensure_credential_transport(&endpoint, &egress)?;

    let previous_upstream = inner.draft["upstreams"]
        .get(&provider)
        .cloned()
        .ok_or_else(|| "托管企业供应商不存在".to_owned())?;
    if previous_upstream["managed_route"].as_bool() != Some(true) {
        return Err(format!("供应商 `{provider}` 已存在，但不是托管企业供应商"));
    }
    let configured_base_url = previous_upstream["base_url"]
        .as_str()
        .ok_or_else(|| "托管企业供应商缺少 Base URL".to_owned())?;
    if configured_base_url != base_url {
        return Err("企业供应商 Base URL 与已配置地址不一致".to_owned());
    }

    let mut models = previous_upstream["models"]
        .as_array()
        .cloned()
        .ok_or_else(|| "托管企业供应商的模型配置无效".to_owned())?;
    let already_configured = models
        .iter()
        .any(|candidate| candidate["model"].as_str() == Some(model.as_str()));
    if !already_configured {
        models.push(provider_model_capability(&base_url, &model, true));
    }

    let previous_routing = inner.draft.get("routing").cloned();
    let previous_router = inner.draft["router"].clone();
    let previous_state = inner.config_state.clone();
    inner.draft["upstreams"][&provider]["models"] = json!(models);
    inner.draft["upstreams"][&provider]["auth"] =
        json!({ "slot": "provider_api_key", "store": true });
    if !inner.draft["routing"].is_object() {
        inner.draft["routing"] = json!({});
    }
    inner.draft["routing"]["mode"] = json!("direct");
    inner.draft["routing"]["direct_target"] = json!({
        "upstream": provider,
        "model": model
    });
    inner.draft["router"]["routing_mode"] = json!("tiered");
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&provider] = previous_upstream;
        restore_managed_route_mutation(&mut inner.draft, &previous_routing, &previous_router);
        inner.config_state = previous_state;
        return Err(error);
    }

    inner
        .pending_provider_keys
        .insert(provider.clone(), Zeroizing::new(api_key));
    inner.pending_provider_key_removals.remove(&provider);
    Ok(inner.snapshot())
}

pub(crate) fn restore_managed_route_mutation(
    draft: &mut Value,
    previous_routing: &Option<Value>,
    previous_router: &Value,
) {
    draft["router"] = previous_router.clone();
    if let Some(previous_routing) = previous_routing {
        draft["routing"] = previous_routing.clone();
    } else if let Some(root) = draft.as_object_mut() {
        root.remove("routing");
    }
}

fn provider_model_capability(base_url: &str, model: &str, managed_route: bool) -> Value {
    // OpenAI Chat Completions includes tools and structured output, and catalog
    // entries are compatible chat providers, so declare support by default.
    // Ordinary models keep vision unknown because support varies. A managed
    // route declares pass-through support because the service selects the model.
    let mut capability = json!({
        "model": model,
        "tool": true,
        "vision": false,
        "json_schema": true,
        "tool_state": "declared",
        "vision_state": "unknown",
        "json_schema_state": "declared",
        "context_window": known_context_window(model)
    });
    if managed_route {
        capability["vision"] = json!(true);
        capability["vision_state"] = json!("declared");
        capability["supported_parameters"] = json!(["reasoning_effort"]);
    }
    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_HEURISTIC);
    if let Some(preset) = builtin_model_limits(base_url, model) {
        capability["context_window"] = json!(preset.context_window);
        capability["max_output_tokens"] = json!(preset.max_output_tokens);
        capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
        capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
    }
    capability
}

fn restore_legacy_managed_upstreams(draft: &mut Value, legacy: &[(String, Value)]) {
    for (name, upstream) in legacy {
        draft["upstreams"][name] = upstream.clone();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_provider_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    models: Vec<String>,
    api_key: Option<String>,
    local: bool,
    credential_source: &str,
    credential_reference: Option<&str>,
    provider_dialect: &str,
    managed_route: bool,
) -> Result<StateView, String> {
    if !matches!(provider_dialect, "openai-compatible" | "azure-openai-v1") {
        return Err("Provider dialect 不受支持".to_owned());
    }
    if name.trim().is_empty() {
        return Err("供应商名不能为空".into());
    }
    let name = name.trim().to_string();
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    if provider_dialect == "azure-openai-v1" && !azure_openai_v1_base_url_is_exact(&base_url) {
        return Err("Azure OpenAI v1 的 Base URL 路径必须精确为 `/openai/v1`".to_owned());
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    recover_pending_provider_purge(&mut inner)?;
    let egress = draft_egress_config(&inner.draft)?;
    ensure_credential_transport(&endpoint, &egress)?;
    if inner.draft["upstreams"].get(&name).is_some() {
        return Err(format!("供应商 `{name}` 已存在，请在 Provider 详情中编辑"));
    }
    let data_dir = inner.data_dir();
    if provider_tombstones::contains(&data_dir, &name)? {
        return Err(format!(
            "Provider 回收站中已有 `{name}`，请先恢复它，再在详情中编辑"
        ));
    }

    let has_modern_managed_upstream = inner.draft["upstreams"]
        .as_object()
        .into_iter()
        .flat_map(|upstreams| upstreams.iter())
        .any(|(upstream_name, upstream)| {
            is_managed_enterprise_provider_id(upstream_name)
                && upstream["managed_route"].as_bool() == Some(true)
        });
    let legacy_managed_upstreams = if managed_route && !has_modern_managed_upstream {
        let candidates = inner.draft["upstreams"]
            .as_object()
            .into_iter()
            .flat_map(|upstreams| upstreams.iter())
            .filter(|(legacy_name, upstream)| {
                legacy_name.as_str() != name
                    && !is_managed_enterprise_provider_id(legacy_name)
                    && upstream["managed_route"].as_bool() == Some(true)
            })
            .map(|(legacy_name, upstream)| (legacy_name.clone(), upstream.clone()))
            .collect::<Vec<_>>();
        let direct_target = inner.draft["routing"]["direct_target"]["upstream"].as_str();
        if let Some(index) = candidates
            .iter()
            .position(|(candidate, _)| Some(candidate.as_str()) == direct_target)
        {
            vec![candidates[index].clone()]
        } else if candidates.len() <= 1 {
            Vec::new()
        } else {
            return Err(format!(
                "存在多个旧版托管 Provider {}，但当前直连路由未指向其中一个，无法确定迁移目标",
                candidates
                    .iter()
                    .map(|(candidate, _)| candidate.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
    } else {
        Vec::new()
    };
    let models = normalize_provider_model_ids(models)?;
    let managed_model = managed_route.then(|| models[0].clone());
    let model_objs: Vec<Value> = models
        .iter()
        .map(|model| provider_model_capability(&base_url, model, managed_route))
        .collect();
    // A previous interrupted removal may have left only derived catalog data.
    // New Provider identity must never inherit it, even with the same name/URL.
    model_catalog::remove_provider(&data_dir, &name)?;
    for (legacy_name, _) in &legacy_managed_upstreams {
        model_catalog::remove_provider(&data_dir, legacy_name)?;
    }

    let mut up = json!({
        "provider": provider_dialect,
        "base_url": base_url,
        "models": model_objs,
    });
    // Write the local key only when marked local, preserving ordinary cloud
    // provider configs in line with serde skip_serializing_if. local_only uses it
    // to keep traffic on the machine.
    if local {
        up["local"] = json!(true);
    }
    if managed_route {
        up["managed_route"] = json!(true);
    }
    // Store a key in the keychain and point auth to its slot; omit auth when no key exists, as with local Ollama.
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    let auth = provider_auth_value(credential_source, credential_reference)?;
    if credential_source == "store" && api_key.is_none() {
        return Err("本地凭据存储需要填写 API Key".to_owned());
    }
    if credential_source != "store" && api_key.is_some() {
        return Err("env/file 凭据只保存引用，不能同时提交 API Key 明文".to_owned());
    }
    if let Some(auth) = auth {
        up["auth"] = auth;
    }

    let previous_routing = if managed_route {
        inner.draft.get("routing").cloned()
    } else {
        None
    };
    let previous_router = inner.draft["router"].clone();
    for (legacy_name, _) in &legacy_managed_upstreams {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(legacy_name);
    }
    inner.draft["upstreams"][&name] = up;
    if managed_route {
        if !inner.draft["routing"].is_object() {
            inner.draft["routing"] = json!({});
        }
        inner.draft["routing"]["mode"] = json!("direct");
        inner.draft["routing"]["direct_target"] = json!({
            "upstream": name,
            "model": managed_model.expect("managed routes have one normalized model")
        });
        inner.draft["router"]["routing_mode"] = json!("tiered");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(&name);
        if managed_route {
            restore_legacy_managed_upstreams(&mut inner.draft, &legacy_managed_upstreams);
            restore_managed_route_mutation(&mut inner.draft, &previous_routing, &previous_router);
        }
        return Err(error);
    }
    if credential_source == "store" {
        let Some(key) = api_key else {
            unreachable!("store source was validated above");
        };
        inner
            .pending_provider_keys
            .insert(name.clone(), Zeroizing::new(key));
        inner.pending_provider_key_removals.remove(&name);
        for (legacy_name, _) in &legacy_managed_upstreams {
            inner.pending_provider_keys.remove(legacy_name);
            inner
                .pending_provider_key_removals
                .insert(legacy_name.clone());
        }
    }
    Ok(inner.snapshot())
}

pub(crate) fn azure_openai_v1_base_url_is_exact(base_url: &str) -> bool {
    base_url
        .split_once("://")
        .and_then(|(_, authority_and_path)| {
            authority_and_path
                .find('/')
                .map(|path_start| &authority_and_path[path_start..])
        })
        == Some("/openai/v1")
}

#[tauri::command]
pub(crate) fn edit_provider(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<StateView, String> {
    edit_provider_impl(state, name, base_url, api_key, None, None, None)
}

#[tauri::command]
pub(crate) fn edit_provider_with_credential(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: String,
    credential_reference: Option<String>,
    provider_call: Option<String>,
) -> Result<StateView, String> {
    edit_provider_impl(
        state,
        name,
        base_url,
        api_key,
        Some(credential_source.trim()),
        credential_reference.as_deref(),
        provider_call.as_deref().map(str::trim),
    )
}

pub(crate) fn edit_provider_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    credential_source: Option<&str>,
    credential_reference: Option<&str>,
    provider_call: Option<&str>,
) -> Result<StateView, String> {
    let name = name.trim().to_owned();
    let endpoint = ProviderEndpoint::try_new(base_url.trim())
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(&inner, &name)?;
    let previous = inner.draft["upstreams"]
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    if previous["provider"].as_str() == Some("azure-openai-v1")
        && !azure_openai_v1_base_url_is_exact(&base_url)
    {
        return Err("Azure OpenAI v1 的 Base URL 路径必须精确为 `/openai/v1`".to_owned());
    }
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    let auth = credential_source
        .map(|source| provider_auth_value(source, credential_reference))
        .transpose()?;
    if credential_source.is_some_and(|source| source != "store") && api_key.is_some() {
        return Err("env/file 凭据只保存引用，不能同时提交 API Key 明文".to_owned());
    }
    // An engine is written exactly as named, `legacy` included: South is the
    // default, so an explicit legacy choice must survive in the document.
    let provider_call = match provider_call {
        None => None,
        Some(
            engine @ ("legacy"
            | "south_v1_buffered"
            | "south_v1_buffered_streaming"
            | "south_v1_buffered_streaming_header_auth"),
        ) => Some(engine),
        Some(_) => return Err("Provider call engine 不受支持".to_owned()),
    };
    let previous_auth = previous.get("auth").filter(|value| !value.is_null());
    let egress = draft_egress_config(&inner.draft)?;
    ensure_credential_transport(&endpoint, &egress)?;
    let auth_changed = auth
        .as_ref()
        .is_some_and(|next| next.as_ref() != previous_auth);
    let identity_changed = previous["base_url"].as_str() != Some(base_url.as_str())
        || api_key.is_some()
        || auth_changed;
    let previous_pricing = inner.draft["pricing"].clone();
    let previous_state = inner.config_state.clone();
    if identity_changed {
        // A URL or credential change may select a different Provider account.
        // Invalidate first: losing derived cache on a later rollback is safe;
        // presenting the old account's catalog as trusted is not.
        model_catalog::remove_provider(&inner.data_dir(), &name)?;
        clear_provider_scoped_prices(&mut inner, &name)?;
    }
    if let Some(provider_call) = provider_call {
        inner.draft["upstreams"][&name]["provider_call"] = json!(provider_call);
    }
    inner.draft["upstreams"][&name]["base_url"] = json!(base_url);
    if let Some(auth) = auth {
        match auth {
            Some(value) => inner.draft["upstreams"][&name]["auth"] = value,
            None => {
                inner.draft["upstreams"][&name]
                    .as_object_mut()
                    .expect("upstream is an object")
                    .remove("auth");
            }
        }
    } else if api_key.is_some() {
        inner.draft["upstreams"][&name]["auth"] =
            json!({ "slot": "provider_api_key", "store": true });
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&name] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(error);
    }
    if let Some(key) = api_key {
        if let Err(key_error) =
            secrets::store_set(&inner.data_dir(), &name, "provider_api_key", &key)
        {
            inner.draft["upstreams"][&name] = previous;
            inner.draft["pricing"] = previous_pricing;
            inner.config_state = previous_state;
            return match inner.observe_draft() {
                Ok(()) => Err(key_error),
                Err(rollback_error) => Err(format!(
                    "{key_error}；同时回滚 Provider 草稿失败：{rollback_error}"
                )),
            };
        }
        // Secret-store contents are intentionally absent from the serialized
        // Provider definition, so a successful key rotation needs its own epoch.
        inner.bump_upstream_epoch(&name);
    }
    Ok(inner.snapshot())
}

/// Update an existing provider's model set while protecting models referenced by routing tiers.
pub(crate) fn replace_provider_models(
    inner: &mut AppInner,
    name: &str,
    models: Vec<String>,
) -> Result<(), String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
    let normalized = normalize_provider_model_ids(models)?;

    let upstream = inner.draft["upstreams"]
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    let base_url = upstream
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let configured_capabilities: Vec<ModelCapability> = upstream
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| serde_json::from_value(model.clone()).ok())
        .collect();
    let (_, discovered_catalog) = model_catalog::catalog_for_provider(
        &inner.data_dir(),
        name,
        &base_url,
        &configured_capabilities,
    );
    let discovered_by_model: std::collections::BTreeMap<&str, &model_catalog::CatalogModelView> =
        discovered_catalog
            .iter()
            .filter(|entry| entry.catalog_state == model_catalog::CatalogState::Active)
            .map(|entry| (entry.model.as_str(), entry))
            .collect();
    let discovered_prices: Vec<(String, ModelPrice)> = normalized
        .iter()
        .filter_map(|model| {
            discovered_by_model
                .get(model.as_str())
                .and_then(|entry| entry.cost.as_ref())
                .and_then(catalog_cost_to_model_price)
                .map(|price| (format!("{name}/{model}"), price))
        })
        .collect();
    let removed_reference = |target: &Value| {
        target["upstream"].as_str() == Some(name)
            && target["model"]
                .as_str()
                .is_some_and(|model| !normalized.iter().any(|candidate| candidate == model))
    };
    let mut direct_and_quota_blocked = Vec::new();
    if removed_reference(&inner.draft["routing"]["direct_target"]) {
        direct_and_quota_blocked.push("主页/单独路由".to_owned());
    }
    for (index, account) in inner.draft["router"]["quota_accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if removed_reference(account) {
            direct_and_quota_blocked.push(format!("主页/额度优先#{}", index + 1));
        }
    }
    if let Some(agent_routes) = inner.draft["agent_routes"].as_object() {
        for (agent_id, route) in agent_routes {
            if removed_reference(&route["direct_target"]) {
                direct_and_quota_blocked.push(format!("Agent/{agent_id}/单独路由"));
            }
        }
    }
    direct_and_quota_blocked.sort();
    direct_and_quota_blocked.dedup();
    if !direct_and_quota_blocked.is_empty() {
        return Err(format!(
            "不能移除 {} 正在使用的模型，请先调整对应路由",
            direct_and_quota_blocked.join("、")
        ));
    }
    let blocked: Vec<&str> = [(TIER_HIGH, "上档"), (TIER_MID, "中档"), (TIER_LOW, "下档")]
        .into_iter()
        .filter_map(|(pool, label)| {
            let member = inner.draft["router"]["pools"][pool]
                .as_array()
                .and_then(|members| members.first());
            let refers_to_provider = member
                .and_then(|item| item["upstream"].as_str())
                .is_some_and(|upstream| upstream == name);
            let retained = member
                .and_then(|item| item["model"].as_str())
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            (refers_to_provider && !retained).then_some(label)
        })
        .collect();
    if !blocked.is_empty() {
        return Err(format!(
            "不能移除 {} 正在使用的模型，请先调整对应档位",
            blocked.join("、")
        ));
    }

    let mut agent_blocked = Vec::new();
    for agent_id in supported_agent_ids() {
        for slot in ["high", "mid", "low"] {
            let target = &inner.draft["agent_routes"][&agent_id]["custom_route"][slot];
            let refers_to_provider = target["upstream"].as_str() == Some(name);
            let retained = target["model"]
                .as_str()
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            if refers_to_provider && !retained {
                agent_blocked.push(format!("{agent_id}/{slot}"));
            }
        }
    }
    for (agent_id, tiers) in &inner.agent_route_drafts {
        for (slot, target) in tiers {
            let refers_to_provider = target.upstream.as_deref() == Some(name);
            let retained = target
                .model
                .as_deref()
                .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
            if refers_to_provider && !retained {
                agent_blocked.push(format!("{agent_id}/{slot}"));
            }
        }
    }
    agent_blocked.sort();
    agent_blocked.dedup();
    if !agent_blocked.is_empty() {
        return Err(format!(
            "不能移除 Agent 独立路由 {} 正在使用的模型，请先调整对应档位",
            agent_blocked.join("、")
        ));
    }

    // Strategy groups (profiles) pin a provider+model per tier; a model still used
    // by one must not be silently removed, or the profile is left dangling.
    let mut profile_blocked = Vec::new();
    if let Some(profiles) = inner.draft["profiles"].as_object() {
        for (profile_name, tiers) in profiles {
            for slot in ["high", "mid", "low"] {
                let target = &tiers[slot];
                let refers_to_provider = target["upstream"].as_str() == Some(name);
                let retained = target["model"]
                    .as_str()
                    .is_some_and(|model| normalized.iter().any(|candidate| candidate == model));
                if refers_to_provider && !retained {
                    profile_blocked.push(format!("{profile_name}/{slot}"));
                }
            }
        }
    }
    profile_blocked.sort();
    profile_blocked.dedup();
    if !profile_blocked.is_empty() {
        return Err(format!(
            "不能移除策略组 {} 正在使用的模型，请先调整对应档位",
            profile_blocked.join("、")
        ));
    }

    let existing: std::collections::BTreeMap<String, Value> = upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item["model"]
                .as_str()
                .map(|model| (model.to_owned(), item.clone()))
        })
        .collect();
    let model_objects: Vec<Value> = normalized
        .into_iter()
        .map(|model| {
            existing.get(&model).cloned().unwrap_or_else(|| {
                let mut capability = json!({
                    // As in add_provider, OpenAI-compatible chat declares tools and structured output by default.
                    "model": model,
                    "tool": true,
                    "vision": false,
                    "json_schema": true,
                    "tool_state": "declared",
                    "vision_state": "unknown",
                    "json_schema_state": "declared",
                    "context_window": known_context_window(&model)
                });
                if let Some(preset) = builtin_model_limits(&base_url, &model) {
                    capability["context_window"] = json!(preset.context_window);
                    capability["max_output_tokens"] = json!(preset.max_output_tokens);
                    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
                    capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_BUILTIN_PRESET);
                }
                if let Some(discovered) = discovered_by_model.get(model.as_str()) {
                    if let Some(context) = discovered.context_window {
                        capability["context_window"] = json!(context);
                        capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                    }
                    if let Some(output) = discovered.max_output_tokens {
                        capability["max_output_tokens"] = json!(output);
                        capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_PROVIDER);
                    }
                    if discovered.vision != CapabilityState::Unknown {
                        let supported = discovered.vision.is_supported();
                        capability["vision"] = json!(supported);
                        capability["vision_state"] = json!(match discovered.vision {
                            CapabilityState::Verified => "verified",
                            CapabilityState::Declared => "declared",
                            CapabilityState::Unsupported => "unsupported",
                            CapabilityState::Unknown => "unknown",
                        });
                    }
                }
                capability
            })
        })
        .collect();

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let previous_pricing = inner.draft["pricing"].clone();
    let next_pricing = if discovered_prices.is_empty() {
        None
    } else {
        let mut pricing = draft_price_table(inner)?;
        let mut changed = false;
        for (model, price) in discovered_prices {
            // Model selection may fill a previously unknown supplier-scoped
            // price, but cannot replace an operator-owned versioned value.
            if pricing.models.contains_key(&model) {
                continue;
            }
            pricing = pricing.next_with_model(&model, price)?;
            changed = true;
        }
        changed
            .then(|| serde_json::to_value(pricing).map_err(|error| error.to_string()))
            .transpose()?
    };
    inner.draft["upstreams"][name]["models"] = json!(model_objects);
    if let Some(pricing) = next_pricing {
        inner.draft["pricing"] = pricing;
    }
    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(format!("保存供应商模型失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn update_provider_models(
    state: State<'_, AppStateManaged>,
    name: String,
    models: Vec<String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_models(&mut inner, name.trim(), models)?;
    Ok(inner.snapshot())
}

pub(crate) fn replace_provider_model_vision(
    inner: &mut AppInner,
    name: &str,
    model: &str,
    supported: bool,
) -> Result<(), String> {
    inner.ensure_editable()?;
    let name = name.trim();
    let model = model.trim();
    if name.is_empty() || model.is_empty() {
        return Err("供应商和模型 ID 不能为空".to_owned());
    }
    ensure_generic_provider_mutation_allowed(inner, name)?;

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let capability = models
        .iter_mut()
        .find(|candidate| candidate["model"].as_str() == Some(model))
        .ok_or_else(|| format!("供应商 `{name}` 未配置模型 `{model}`"))?;
    capability["vision"] = json!(supported);
    capability["vision_state"] = json!(if supported { "declared" } else { "unsupported" });

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.config_state = previous_state;
        return Err(format!("保存模型视觉能力失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn set_provider_model_vision(
    state: State<'_, AppStateManaged>,
    name: String,
    model: String,
    supported: bool,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_model_vision(&mut inner, &name, &model, supported)?;
    Ok(inner.snapshot())
}

pub(crate) fn replace_provider_model_limits(
    inner: &mut AppInner,
    name: &str,
    model: &str,
    context_window: u32,
    max_output_tokens: u32,
) -> Result<(), String> {
    inner.ensure_editable()?;
    let name = name.trim();
    let model = model.trim();
    if name.is_empty() || model.is_empty() {
        return Err("供应商和模型 ID 不能为空".to_owned());
    }
    if context_window == 0 || max_output_tokens == 0 {
        return Err("上下文上限和最大输出 Token 必须大于 0".to_owned());
    }
    if max_output_tokens > context_window {
        return Err("最大输出 Token 不能大于上下文上限".to_owned());
    }
    ensure_generic_provider_mutation_allowed(inner, name)?;

    let previous = inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream.get("models"))
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 不存在或模型配置无效"))?;
    let capability = models
        .iter_mut()
        .find(|candidate| candidate["model"].as_str() == Some(model))
        .ok_or_else(|| format!("供应商 `{name}` 未配置模型 `{model}`"))?;
    capability["context_window"] = json!(context_window);
    capability["max_output_tokens"] = json!(max_output_tokens);
    capability[CONTEXT_WINDOW_SOURCE_KEY] = json!(LIMIT_SOURCE_OPERATOR);
    capability[MAX_OUTPUT_TOKENS_SOURCE_KEY] = json!(LIMIT_SOURCE_OPERATOR);

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.config_state = previous_state;
        return Err(format!("保存模型限制失败：{error}"));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn set_provider_model_limits(
    state: State<'_, AppStateManaged>,
    name: String,
    model: String,
    context_window: u32,
    max_output_tokens: u32,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    replace_provider_model_limits(&mut inner, &name, &model, context_window, max_output_tokens)?;
    Ok(inner.snapshot())
}

pub(crate) fn provider_references(inner: &AppInner, name: &str) -> Vec<String> {
    let mut references = Vec::new();
    if inner.draft["routing"]["direct_target"]["upstream"].as_str() == Some(name) {
        references.push("主页/单独路由".to_owned());
    }
    for (index, account) in inner.draft["router"]["quota_accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if account["upstream"].as_str() == Some(name) {
            references.push(format!("主页/额度优先#{}", index + 1));
        }
    }
    if let Some(agent_routes) = inner.draft["agent_routes"].as_object() {
        for (agent_id, route) in agent_routes {
            if route["direct_target"]["upstream"].as_str() == Some(name) {
                references.push(format!("Agent/{agent_id}/单独路由"));
            }
        }
    }
    for (pool, label) in [
        (TIER_HIGH, "主页/上档"),
        (TIER_MID, "主页/中档"),
        (TIER_LOW, "主页/下档"),
    ] {
        for (index, member) in inner.draft["router"]["pools"][pool]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if member["upstream"].as_str() == Some(name) {
                references.push(format!("{label}#{}", index + 1));
            }
        }
    }
    for agent_id in supported_agent_ids() {
        for slot in ["high", "mid", "low"] {
            if inner.draft["agent_routes"][&agent_id]["custom_route"][slot]["upstream"].as_str()
                == Some(name)
            {
                references.push(format!("Agent/{agent_id}/{slot}"));
            }
        }
    }
    for (agent_id, tiers) in &inner.agent_route_drafts {
        for (slot, target) in tiers {
            if target.upstream.as_deref() == Some(name) {
                references.push(format!("Agent/{agent_id}/{slot}"));
            }
        }
    }
    // Saved strategy groups (profiles) reference providers by name too; without
    // this scan a provider used only by a profile would pass the removal gate and
    // leave that profile pointing at a deleted upstream, causing stale-option residue.
    if let Some(profiles) = inner.draft["profiles"].as_object() {
        for (profile_name, tiers) in profiles {
            for slot in ["high", "mid", "low"] {
                if tiers[slot]["upstream"].as_str() == Some(name) {
                    references.push(format!("策略组/{profile_name}/{slot}"));
                }
            }
        }
    }
    references.sort();
    references.dedup();
    references
}

#[tauri::command]
pub(crate) fn preview_provider_removal(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<ProviderRemovalPreview, String> {
    let inner = state.0.lock().unwrap();
    let name = name.trim();
    if inner.draft["upstreams"].get(name).is_none() {
        return Err(format!("供应商 `{name}` 不存在"));
    }
    let references = provider_references(&inner, name);
    Ok(ProviderRemovalPreview {
        name: name.to_owned(),
        can_remove: references.is_empty(),
        references,
    })
}

#[tauri::command]
pub(crate) fn remove_provider(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    recover_pending_provider_purge(&mut inner)?;
    let name = name.trim();
    let references = provider_references(&inner, name);
    if !references.is_empty() {
        return Err(format!(
            "供应商仍被引用，不能删除：{}。请先调整这些路由",
            references.join("、")
        ));
    }
    let provider = inner.draft["upstreams"]
        .get(name)
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
    let data_dir = inner.data_dir();
    model_catalog::remove_provider(&data_dir, name)?;
    provider_tombstones::archive(&data_dir, name, &provider)?;
    let pending_key = inner.pending_provider_keys.remove(name);
    inner.draft["upstreams"]
        .as_object_mut()
        .expect("upstreams is an object")
        .remove(name);
    inner.rebuild_routing();
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][name] = provider;
        if let Some(key) = pending_key {
            inner.pending_provider_keys.insert(name.to_owned(), key);
        }
        provider_tombstones::discard(&data_dir, name).ok();
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn restore_provider(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    recover_pending_provider_purge(&mut inner)?;
    let name = name.trim();
    if inner.draft["upstreams"].get(name).is_some() {
        return Err(format!("同名供应商 `{name}` 已存在，不能覆盖恢复"));
    }
    let data_dir = inner.data_dir();
    let archived = provider_tombstones::get(&data_dir, name)?
        .ok_or_else(|| format!("Provider 回收站中没有 `{name}`"))?;
    if is_free_provider_value(&archived) {
        return Err(format!(
            "免费供应商 `{name}` 必须从免费目录重新验证，不能恢复旧目录快照"
        ));
    }
    model_catalog::remove_provider(&data_dir, name)?;
    let provider = provider_tombstones::take(&data_dir, name)?
        .ok_or_else(|| format!("Provider 回收站中没有 `{name}`"))?;
    inner.draft["upstreams"][name] = provider.clone();
    inner.rebuild_routing();
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"]
            .as_object_mut()
            .expect("upstreams is an object")
            .remove(name);
        provider_tombstones::archive(&data_dir, name, &provider).ok();
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn purge_deleted_providers(
    state: State<'_, AppStateManaged>,
) -> Result<StateView, String> {
    purge_deleted_providers_with_discard(state, provider_tombstones::discard_all)
}

pub(crate) fn purge_deleted_providers_with_discard(
    state: State<'_, AppStateManaged>,
    discard: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    recover_pending_provider_purge(&mut inner)?;
    if inner.config_state.is_dirty() {
        return Err("Provider 回收站只能在配置已保存后清空；请先保存当前 Provider 删除".to_owned());
    }
    let data_dir = inner.data_dir();
    let names = provider_tombstones::names(&data_dir)?;
    let still_active = names
        .iter()
        .filter(|name| inner.draft["upstreams"].get(name.as_str()).is_some())
        .cloned()
        .collect::<Vec<_>>();
    if !still_active.is_empty() {
        return Err(format!(
            "当前草稿仍包含回收站 Provider {}；已拒绝删除活动 Provider 的恢复点",
            still_active.join("、")
        ));
    }
    if inner.config_path.exists() {
        let persisted = ClientConfig::load(&inner.config_path)
            .map_err(|error| format!("无法核对磁盘配置中的 Provider，已拒绝清空回收站：{error}"))?;
        let still_persisted = names
            .iter()
            .filter(|name| persisted.upstreams.contains_key(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !still_persisted.is_empty() {
            return Err(format!(
                "磁盘配置仍包含待删除 Provider {}；请先保存 Provider 删除后再清空回收站",
                still_persisted.join("、")
            ));
        }
    }
    let inactive_names = names;
    if inactive_names.is_empty() {
        return Ok(inner.snapshot());
    }
    persist_pending_provider_purge(&data_dir, &inactive_names)?;
    if let Err(error) = discard(&data_dir) {
        let tombstoned = provider_tombstones::names(&data_dir).map_err(|state_error| {
            format!(
                "{error}；且无法确认 Provider 回收站是否已提交，待清理记录已保留：{state_error}"
            )
        })?;
        let retained = inactive_names
            .iter()
            .filter(|name| tombstoned.contains(name))
            .count();
        if retained == inactive_names.len() {
            return match remove_pending_provider_purge(&data_dir) {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(format!(
                    "{error}；回收站未提交，但移除待清理记录失败：{cleanup_error}"
                )),
            };
        }
        if retained == 0 {
            return Err(format!(
                "Provider 回收站删除已提交，但持久化确认失败；凭据与定价待清理记录已保留：{error}"
            ));
        }
        return Err(format!(
            "{error}；Provider 回收站仅删除了部分待清理项，记录已保留并拒绝自动删除凭据"
        ));
    }
    if let Err(error) = recover_pending_provider_purge(&mut inner) {
        return Err(format!(
            "Provider 永久删除已提交，后续清理已记录待重试：{error}"
        ));
    }

    for name in inactive_names {
        inner.pending_provider_keys.remove(&name);
        inner.pending_provider_key_removals.remove(&name);
        inner.bump_upstream_epoch(&name);
    }
    Ok(inner.snapshot())
}
