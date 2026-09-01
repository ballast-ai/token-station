use crate::*;

pub(crate) fn draft_price_table(inner: &AppInner) -> Result<PriceTable, String> {
    inner
        .draft
        .get("pricing")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("定价表配置不合法：{error}"))
        .map(Option::unwrap_or_default)
}

/// Remove catalog-derived prices bound to one Provider identity in a single
/// table revision. A Base URL or credential change may select another account;
/// carrying the old account's scoped prices across that boundary would make
/// both settlement and Agent `Spent` metadata factually wrong.
pub(crate) fn clear_provider_scoped_prices(
    inner: &mut AppInner,
    name: &str,
) -> Result<bool, String> {
    clear_provider_scoped_prices_for(inner, std::slice::from_ref(&name))
}

/// Permanently retire prices for several Provider identities in one table
/// revision so a recycle-bin purge cannot make a reused name inherit any of
/// their account-scoped settlement data.
pub(crate) fn clear_provider_scoped_prices_for<T: AsRef<str>>(
    inner: &mut AppInner,
    names: &[T],
) -> Result<bool, String> {
    let mut pricing = draft_price_table(inner)?;
    let before = pricing.models.len();
    pricing.models.retain(|model, _| {
        !names
            .iter()
            .any(|name| model.starts_with(&format!("{}/", name.as_ref())))
    });
    if pricing.models.len() == before {
        return Ok(false);
    }
    pricing.version = pricing
        .version
        .checked_add(1)
        .ok_or_else(|| "pricing version exhausted; start a new configuration".to_owned())?;
    inner.draft["pricing"] = serde_json::to_value(pricing).map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn get_price_table(state: State<'_, AppStateManaged>) -> Result<PriceTable, String> {
    draft_price_table(&state.0.lock().unwrap())
}

#[tauri::command]
pub(crate) async fn list_public_provider_models(
    state: State<'_, AppStateManaged>,
    provider_ids: Vec<String>,
) -> Result<PublicProviderModelsView, String> {
    if provider_ids.is_empty() || provider_ids.len() > 128 {
        return Err("Request 1 to 128 Provider IDs.".to_owned());
    }
    let mut requested = BTreeSet::new();
    for provider_id in provider_ids {
        let provider_id = provider_id.trim().to_owned();
        if provider_id.is_empty()
            || provider_id.len() > 128
            || provider_id.chars().any(char::is_control)
        {
            return Err(
                "Each Provider ID must be 1 to 128 bytes and contain no control characters."
                    .to_owned(),
            );
        }
        requested.insert(provider_id);
    }
    let requested: Vec<String> = requested.into_iter().collect();
    let (data_dir, egress, egress_secrets) = {
        let inner = state.0.lock().unwrap();
        let egress = draft_egress_config(&inner.draft)
            .map_err(|error| format!("The egress configuration is invalid: {error}"))?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::list_public_provider_models_with_cache_egress(
            &data_dir,
            &requested,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("The public model catalog task ended unexpectedly: {error}"))?
}

#[tauri::command]
pub(crate) async fn suggest_model_price(
    state: State<'_, AppStateManaged>,
    provider_id: Option<String>,
    model_id: String,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let model_id = model_id.trim().to_owned();
    if model_id.is_empty() || model_id.len() > 256 {
        return Err("模型 ID 必须是 1–256 个字符".to_owned());
    }
    let provider_id = provider_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let (data_dir, egress, egress_secrets) = {
        let inner = state.0.lock().unwrap();
        let egress = draft_egress_config(&inner.draft)?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::suggest_with_cache_egress(
            &data_dir,
            provider_id.as_deref(),
            &model_id,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("公开价格目录任务异常结束：{error}"))?
}

pub(crate) fn configured_upstream_models(
    inner: &AppInner,
    name: &str,
) -> Result<BTreeSet<String>, String> {
    let upstream = inner.draft["upstreams"]
        .get(name)
        .ok_or_else(|| format!("Provider `{name}` is not configured"))?;
    upstream["models"]
        .as_array()
        .ok_or_else(|| format!("Provider `{name}` has invalid model configuration"))?
        .iter()
        .map(|capability| {
            capability["model"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("Provider `{name}` has an invalid model entry"))
        })
        .collect()
}

pub(crate) fn configured_public_price_provider_id(
    inner: &AppInner,
    name: &str,
) -> Result<String, String> {
    let upstream = inner.draft["upstreams"]
        .get(name)
        .ok_or_else(|| format!("Provider `{name}` is not configured"))?;
    if upstream["provider"].as_str() != Some("openai-compatible") {
        return Err(format!(
            "Provider `{name}` does not support automatic public price import"
        ));
    }
    let base_url = upstream["base_url"]
        .as_str()
        .ok_or_else(|| format!("Provider `{name}` has no valid Base URL"))?;
    let endpoint = ProviderEndpoint::try_new(base_url).map_err(|error| error.to_string())?;
    if endpoint.is_loopback() {
        return Err("Local Provider prices must be configured explicitly".to_owned());
    }
    let access_tier = upstream["access_tier"].as_str().unwrap_or_default();
    let brand_id = provider_brand_id(name, &endpoint.as_str(), access_tier).ok_or_else(|| {
        format!("Provider `{name}` has no authoritative public price catalog mapping")
    })?;
    Ok(pricing_catalog::normalize_provider_id(brand_id))
}

#[derive(Clone, Debug)]
pub(crate) struct PriceImportTarget {
    pub(crate) upstream: Value,
    pub(crate) upstream_epoch: u64,
    pub(crate) price_version: u32,
}

pub(crate) fn capture_price_import_target(
    inner: &AppInner,
    upstream_name: &str,
) -> Result<PriceImportTarget, String> {
    let upstream = inner.draft["upstreams"]
        .get(upstream_name)
        .cloned()
        .ok_or_else(|| format!("Provider `{upstream_name}` is not configured"))?;
    Ok(PriceImportTarget {
        upstream,
        upstream_epoch: inner
            .upstream_epochs
            .get(upstream_name)
            .copied()
            .unwrap_or_default(),
        price_version: draft_price_table(inner)?.version,
    })
}

pub(crate) fn ensure_price_import_target_unchanged(
    inner: &AppInner,
    upstream_name: &str,
    expected: &PriceImportTarget,
) -> Result<(), String> {
    let current = capture_price_import_target(inner, upstream_name)?;
    if current.upstream != expected.upstream
        || current.upstream_epoch != expected.upstream_epoch
        || current.price_version != expected.price_version
    {
        return Err(format!(
            "Provider `{upstream_name}` or its price table changed while public prices were loading. Retry the import"
        ));
    }
    Ok(())
}

pub(crate) fn ensure_automatic_price_suggestions_fresh(
    suggestions: &[RequestedModelPriceSuggestion],
) -> Result<(), String> {
    if suggestions
        .iter()
        .any(|value| value.suggestion.catalog_source != "live")
    {
        return Err(
            "Automatic price import requires a live public catalog; cached prices are advisory only"
                .to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn apply_public_model_prices(
    inner: &mut AppInner,
    upstream_name: &str,
    requested_models: &BTreeSet<String>,
    suggestions: Vec<RequestedModelPriceSuggestion>,
) -> Result<(usize, usize, Vec<String>, u32), String> {
    let configured = configured_upstream_models(inner, upstream_name)?;
    if let Some(model) = requested_models
        .iter()
        .find(|model| !configured.contains(*model))
    {
        return Err(format!(
            "Model `{model}` is not configured for Provider `{upstream_name}`"
        ));
    }

    let current = draft_price_table(inner)?;
    let suggestions: BTreeMap<String, ModelPriceSuggestionView> = suggestions
        .into_iter()
        .map(|value| (value.requested_model_id, value.suggestion))
        .collect();
    let mut additions = BTreeMap::new();
    let mut existing = 0;
    let mut missing_model_ids = Vec::new();
    for model in requested_models {
        let scoped_model = format!("{upstream_name}/{model}");
        if current.models.contains_key(&scoped_model) {
            existing += 1;
            continue;
        }
        let Some(suggestion) = suggestions.get(model) else {
            missing_model_ids.push(model.clone());
            continue;
        };
        additions.insert(
            scoped_model,
            ModelPrice {
                input_per_mtok: suggestion.input_per_mtok,
                output_per_mtok: suggestion.output_per_mtok,
                cache_read_per_mtok: suggestion.cache_read_per_mtok,
                cache_write_per_mtok: suggestion.cache_write_per_mtok,
                reasoning_per_mtok: suggestion.reasoning_per_mtok,
            },
        );
    }

    let imported = additions.len();
    if additions.is_empty() {
        return Ok((0, existing, missing_model_ids, current.version));
    }
    let next = current.next_with_models(additions)?;
    let next_version = next.version;
    let previous_pricing = inner.draft["pricing"].clone();
    let previous_state = inner.config_state.clone();
    inner.draft["pricing"] = serde_json::to_value(next).map_err(|error| error.to_string())?;
    if let Err(error) = inner.observe_draft() {
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(error);
    }
    Ok((imported, existing, missing_model_ids, next_version))
}

#[tauri::command]
pub(crate) async fn import_model_prices_for_provider(
    state: State<'_, AppStateManaged>,
    upstream_name: String,
    model_ids: Vec<String>,
) -> Result<ModelPriceImportResultView, String> {
    let upstream_name = upstream_name.trim().to_owned();
    if upstream_name.is_empty() {
        return Err("Provider name is required for a price import".to_owned());
    }
    let mut requested_models = BTreeSet::new();
    for model in model_ids {
        let model = model.trim().to_owned();
        if model.is_empty() || model.len() > 512 || model.chars().any(char::is_control) {
            return Err(
                "Model IDs must be 1-512 bytes and contain no control characters".to_owned(),
            );
        }
        requested_models.insert(model);
    }
    if requested_models.is_empty() || requested_models.len() > 512 {
        return Err("A price import must contain 1-512 unique models".to_owned());
    }

    let (data_dir, egress, egress_secrets, expected_target, catalog_provider_id) = {
        let inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        let configured = configured_upstream_models(&inner, &upstream_name)?;
        let catalog_provider_id = configured_public_price_provider_id(&inner, &upstream_name)?;
        if let Some(model) = requested_models
            .iter()
            .find(|model| !configured.contains(*model))
        {
            return Err(format!(
                "Model `{model}` is not configured for Provider `{upstream_name}`"
            ));
        }
        let egress = draft_egress_config(&inner.draft)?;
        let expected_target = capture_price_import_target(&inner, &upstream_name)?;
        (
            inner.data_dir(),
            egress.clone(),
            secrets::SecretStore::from_egress_config(&egress, &inner.data_dir()),
            expected_target,
            catalog_provider_id,
        )
    };
    let requested_for_catalog: Vec<String> = requested_models.iter().cloned().collect();
    let suggestions = tauri::async_runtime::spawn_blocking(move || {
        pricing_catalog::suggest_many_live_with_egress(
            &data_dir,
            Some(&catalog_provider_id),
            &requested_for_catalog,
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("Public price catalog task ended unexpectedly: {error}"))??;
    ensure_automatic_price_suggestions_fresh(&suggestions)?;

    let mut inner = state.0.lock().unwrap();
    ensure_price_import_target_unchanged(&inner, &upstream_name, &expected_target)?;
    let (imported, existing, missing_model_ids, price_version) =
        apply_public_model_prices(&mut inner, &upstream_name, &requested_models, suggestions)?;
    Ok(ModelPriceImportResultView {
        state: inner.snapshot(),
        imported,
        existing,
        missing_model_ids,
        price_version,
    })
}

#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps the five price classes and expected version to named form fields"
)]
pub(crate) fn set_model_price(
    state: State<'_, AppStateManaged>,
    model: String,
    input_per_mtok: u64,
    output_per_mtok: u64,
    cache_read_per_mtok: u64,
    cache_write_per_mtok: u64,
    reasoning_per_mtok: Option<u64>,
    expected_version: u32,
) -> Result<PriceTable, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let current = draft_price_table(&inner)?;
    if current.version != expected_version {
        return Err(format!(
            "定价表版本冲突：当前为 v{}，页面基于 v{expected_version}；请刷新后重试",
            current.version
        ));
    }
    let next = current.next_with_model(
        &model,
        ModelPrice {
            input_per_mtok,
            output_per_mtok,
            cache_read_per_mtok,
            cache_write_per_mtok,
            reasoning_per_mtok,
        },
    )?;
    inner.draft["pricing"] = serde_json::to_value(&next).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    Ok(next)
}

#[tauri::command]
pub(crate) fn remove_model_price(
    state: State<'_, AppStateManaged>,
    model: String,
    expected_version: u32,
) -> Result<PriceTable, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let current = draft_price_table(&inner)?;
    if current.version != expected_version {
        return Err(format!(
            "定价表版本冲突：当前为 v{}，页面基于 v{expected_version}；请刷新后重试",
            current.version
        ));
    }
    let next = current.next_without_model(&model)?;
    inner.draft["pricing"] = serde_json::to_value(&next).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    Ok(next)
}
