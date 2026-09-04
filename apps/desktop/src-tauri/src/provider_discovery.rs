use crate::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DiscoveryCredential {
    Explicit(Option<String>),
    Stored { provider: String, slot: String },
}

impl DiscoveryCredential {
    pub(crate) fn is_explicit_secret(&self) -> bool {
        matches!(self, Self::Explicit(Some(_)))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderDiscoveryTarget {
    pub(crate) upstream: Option<Value>,
    pub(crate) upstream_epoch: u64,
    pub(crate) discovery_generation: u64,
}

pub(crate) fn capture_provider_discovery_target(
    inner: &AppInner,
    name: &str,
) -> ProviderDiscoveryTarget {
    ProviderDiscoveryTarget {
        upstream: inner.draft["upstreams"].get(name).cloned(),
        upstream_epoch: inner.upstream_epochs.get(name).copied().unwrap_or_default(),
        discovery_generation: inner
            .discovery_generations
            .get(name)
            .copied()
            .unwrap_or_default(),
    }
}

pub(crate) fn begin_provider_discovery_target(
    inner: &mut AppInner,
    name: &str,
) -> ProviderDiscoveryTarget {
    let generation = inner
        .discovery_generations
        .entry(name.to_owned())
        .or_default();
    *generation = generation.saturating_add(1).max(1);
    capture_provider_discovery_target(inner, name)
}

pub(crate) fn ensure_provider_discovery_target_unchanged(
    inner: &AppInner,
    name: &str,
    expected: &ProviderDiscoveryTarget,
) -> Result<(), String> {
    let current = capture_provider_discovery_target(inner, name);
    if current.upstream != expected.upstream
        || current.upstream_epoch != expected.upstream_epoch
        || current.discovery_generation != expected.discovery_generation
    {
        return Err(format!(
            "Provider `{name}` changed while its model catalog was loading. Retry discovery"
        ));
    }
    Ok(())
}

pub(crate) fn provider_health_uses_south(
    draft: &Value,
    upstream: &Value,
    package_verified: bool,
) -> bool {
    match upstream["provider_call"]
        .as_str()
        .unwrap_or(DEFAULT_PROVIDER_CALL)
    {
        "south_v1_buffered" | "south_v1_buffered_streaming" => {
            south_v1_unavailable_reason(draft, upstream, package_verified).is_none()
        }
        "south_v1_buffered_streaming_header_auth" => {
            south_header_auth_v1_unavailable_reason(draft, upstream, package_verified).is_none()
        }
        _ => false,
    }
}

pub(crate) fn prepare_discovery_credential(
    inner: &AppInner,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<DiscoveryCredential, String> {
    inner.ensure_editable()?;
    ensure_generic_provider_mutation_allowed(inner, name)?;
    if inner.draft["upstreams"]
        .get(name)
        .and_then(|upstream| upstream["provider"].as_str())
        == Some("azure-openai-v1")
    {
        return Err("model_catalog_azure_deployment_manual".to_owned());
    }
    let explicit = api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned);
    if explicit.is_some() {
        return Ok(DiscoveryCredential::Explicit(explicit));
    }
    // OpenRouter's model catalog is public. Avoid an unnecessary Keychain read here:
    // it can trigger a macOS authorization round-trip even though `/models` needs no key.
    if base_url == "https://openrouter.ai/api/v1" {
        return Ok(DiscoveryCredential::Explicit(None));
    }
    if let Some(upstream) = inner.draft["upstreams"].get(name) {
        let configured_base = upstream["base_url"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('/');
        if configured_base != base_url {
            return Err("使用已保存 Key 刷新时，Base URL 必须与供应商配置一致".to_owned());
        }
        return match upstream["auth"]["slot"].as_str() {
            Some(slot) => Ok(DiscoveryCredential::Stored {
                provider: name.to_owned(),
                slot: slot.to_owned(),
            }),
            None => Ok(DiscoveryCredential::Explicit(None)),
        };
    }
    Ok(DiscoveryCredential::Explicit(None))
}

pub(crate) fn catalog_cost_to_model_price(
    cost: &model_catalog::CatalogCostView,
) -> Option<ModelPrice> {
    fn micros(value: f64) -> Option<u64> {
        let scaled = value * 1_000_000.0;
        (scaled.is_finite() && scaled >= 0.0 && scaled <= u64::MAX as f64)
            .then(|| scaled.round() as u64)
    }

    let input_per_mtok = micros(cost.input?)?;
    let cache_write_per_mtok = match cost.cache_write {
        Some(value) => micros(value)?,
        None => input_per_mtok,
    };
    Some(ModelPrice {
        input_per_mtok,
        output_per_mtok: micros(cost.output?)?,
        cache_read_per_mtok: micros(cost.cache_read?)?,
        cache_write_per_mtok,
        reasoning_per_mtok: None,
    })
}

/// Fetch the provider's current model catalog on a blocking worker without
/// blocking the Tauri UI. When using a saved key, require the request URL to
/// match provider configuration so credentials cannot be forwarded elsewhere.
pub(crate) fn apply_discovered_model_capabilities(
    inner: &mut AppInner,
    name: &str,
    catalog: &[model_catalog::CatalogModelView],
) -> Result<bool, String> {
    let Some(upstream) = inner.draft["upstreams"].get(name) else {
        return Ok(false);
    };
    let previous = upstream
        .get("models")
        .filter(|models| models.is_array())
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let facts: std::collections::BTreeMap<&str, &model_catalog::CatalogModelView> = catalog
        .iter()
        .filter(|model| model.catalog_state == model_catalog::CatalogState::Active)
        .filter(|model| {
            model.vision != CapabilityState::Unknown
                || model.context_window.is_some()
                || model.max_output_tokens.is_some()
                || model.cost.is_some()
        })
        .map(|model| (model.model.as_str(), model))
        .collect();
    inner.ensure_editable()?;
    let previous_state = inner.config_state.clone();
    let previous_pricing = inner.draft["pricing"].clone();
    let mut next_pricing = draft_price_table(inner)?;
    let mut pricing_changed = false;
    let selected_models: std::collections::BTreeSet<String> = upstream["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|capability| capability["model"].as_str().map(str::to_owned))
        .collect();
    for (model, fact) in &facts {
        if !selected_models.contains(*model) {
            continue;
        }
        let Some(price) = fact.cost.as_ref().and_then(catalog_cost_to_model_price) else {
            continue;
        };
        let scoped_model = format!("{name}/{model}");
        // Supplier catalogs are useful defaults, but the versioned pricing
        // editor is operator-owned. Without per-entry provenance we cannot
        // distinguish a previous catalog value from an explicit edit, so a
        // refresh may fill an unknown scoped price but must never overwrite an
        // existing one.
        if next_pricing.models.contains_key(&scoped_model) {
            continue;
        }
        next_pricing = next_pricing.next_with_model(&scoped_model, price)?;
        pricing_changed = true;
    }
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str() else {
            continue;
        };
        if let Some(fact) = facts.get(model).copied() {
            if let Some((supported, serialized)) = match fact.vision {
                CapabilityState::Verified => Some((true, "verified")),
                CapabilityState::Unsupported => Some((false, "unsupported")),
                CapabilityState::Declared | CapabilityState::Unknown => None,
            } {
                if capability["vision"].as_bool() != Some(supported)
                    || capability["vision_state"].as_str() != Some(serialized)
                {
                    capability["vision"] = json!(supported);
                    capability["vision_state"] = json!(serialized);
                    changed = true;
                }
            }
            changed |= apply_provider_reported_limits(
                capability,
                fact.context_window,
                fact.max_output_tokens,
            );
            if let Some(cost) = &fact.cost {
                let serialized = serde_json::to_value(cost)
                    .map_err(|error| format!("序列化模型价格失败：{error}"))?;
                if capability["catalog_cost"] != serialized {
                    capability["catalog_cost"] = serialized;
                    changed = true;
                }
            }
        }
    }
    changed |= apply_builtin_model_limits_to_upstream(&mut inner.draft["upstreams"][name]);
    if !changed && !pricing_changed {
        return Ok(false);
    }
    if pricing_changed {
        inner.draft["pricing"] =
            serde_json::to_value(next_pricing).map_err(|error| error.to_string())?;
    }

    let save = inner.observe_draft().and_then(|()| inner.save_draft());
    if let Err(error) = save {
        inner.draft["upstreams"][name]["models"] = previous;
        inner.draft["pricing"] = previous_pricing;
        inner.config_state = previous_state;
        return Err(format!("保存模型目录能力失败：{error}"));
    }
    Ok(true)
}

/// Persist only capacity facts needed by Agent projections. Automatic post-create refreshes use
/// this narrower mutation so optional discovery cannot import prices or unrelated capabilities.
pub(crate) fn apply_discovered_model_limits(
    inner: &mut AppInner,
    name: &str,
    catalog: &[model_catalog::CatalogModelView],
) -> Result<bool, String> {
    let previous = inner.draft["upstreams"][name]["models"]
        .as_array()
        .cloned()
        .ok_or_else(|| format!("供应商 `{name}` 的模型配置无效"))?;
    let facts: std::collections::BTreeMap<&str, &model_catalog::CatalogModelView> = catalog
        .iter()
        .filter(|model| model.catalog_state == model_catalog::CatalogState::Active)
        .filter(|model| model.context_window.is_some() || model.max_output_tokens.is_some())
        .map(|model| (model.model.as_str(), model))
        .collect();
    inner.ensure_editable()?;
    let previous_state = inner.config_state.clone();
    let models = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .expect("model configuration was validated above");
    let mut changed = false;
    for capability in models {
        let Some(model) = capability["model"].as_str() else {
            continue;
        };
        let Some(fact) = facts.get(model).copied() else {
            continue;
        };
        changed |= apply_provider_reported_limits(
            capability,
            fact.context_window,
            fact.max_output_tokens,
        );
    }
    if !changed {
        return Ok(false);
    }
    if let Err(error) = inner.observe_draft().and_then(|()| inner.save_draft()) {
        inner.draft["upstreams"][name]["models"] = json!(previous);
        inner.config_state = previous_state;
        return Err(format!("保存模型目录上限失败：{error}"));
    }
    Ok(true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiscoveryMutation {
    None,
    AllCapabilities,
    LimitsOnly,
}

#[tauri::command]
pub(crate) async fn discover_provider_models(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<ModelDiscoveryView, String> {
    discover_provider_models_impl(
        state,
        name,
        base_url,
        api_key,
        DiscoveryMutation::AllCapabilities,
    )
    .await
}

#[tauri::command]
pub(crate) async fn discover_provider_model_limits(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
) -> Result<ModelDiscoveryView, String> {
    discover_provider_models_impl(
        state,
        name,
        base_url,
        None,
        DiscoveryMutation::LimitsOnly,
    )
    .await
}

#[tauri::command]
pub(crate) async fn verify_enterprise_route(
    state: State<'_, AppStateManaged>,
    base_url: String,
    api_key: String,
) -> Result<ModelDiscoveryView, String> {
    let verification_provider = {
        let inner = state.0.lock().unwrap();
        next_managed_enterprise_provider_id(&inner)?
    };
    discover_provider_models_impl(
        state,
        verification_provider,
        base_url,
        Some(api_key),
        DiscoveryMutation::None,
    )
    .await
}

async fn discover_provider_models_impl(
    state: State<'_, AppStateManaged>,
    name: String,
    base_url: String,
    api_key: Option<String>,
    mutation: DiscoveryMutation,
) -> Result<ModelDiscoveryView, String> {
    let name = name.trim().to_owned();
    let base_url = base_url.trim().trim_end_matches('/').to_owned();
    if name.is_empty() {
        return Err("请先填写供应商名称".to_owned());
    }
    UpstreamRef::new(name.clone()).map_err(|error| format!("供应商名不合法: {error}"))?;
    let endpoint = ProviderEndpoint::try_new(&base_url)
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let base_url = endpoint.as_str();

    let (
        data_dir,
        credential,
        pending_key,
        egress,
        egress_secrets,
        expected_target,
        mutate_derived_state,
    ) = {
        let mut inner = state.0.lock().unwrap();
        if inner.pending_provider_discoveries.contains(&name) {
            return Err(format!(
                "Provider `{name}` model discovery is already in progress"
            ));
        }
        const MAX_CONCURRENT_PROVIDER_DISCOVERIES: usize = 8;
        if inner.pending_provider_discoveries.len() >= MAX_CONCURRENT_PROVIDER_DISCOVERIES {
            return Err("Provider model discovery concurrency limit reached".to_owned());
        }
        let credential =
            prepare_discovery_credential(&inner, &name, &base_url, api_key.as_deref())?;
        // A newly added Provider keeps its key in memory until the draft is saved. Model
        // discovery runs before that save, so prefer the pending value over Keychain for the
        // configured Provider slot. It remains a stored credential semantically, which means a
        // successful discovery may safely persist capabilities for this Provider.
        let pending_key = match &credential {
            DiscoveryCredential::Stored { provider, slot } if slot == "provider_api_key" => {
                inner.pending_provider_keys.get(provider).cloned()
            }
            _ => None,
        };
        let config = inner.materialize()?;
        ensure_credential_transport(&endpoint, &config.egress)?;
        let expected_target = begin_provider_discovery_target(&mut inner, &name);
        let mutate_derived_state = mutation != DiscoveryMutation::None
            && (expected_target.upstream.is_none() || !credential.is_explicit_secret());
        inner.pending_provider_discoveries.insert(name.clone());
        (
            inner.data_dir(),
            credential,
            pending_key,
            config.egress.clone(),
            secrets::SecretStore::from_config(&config, &inner.data_dir()),
            expected_target,
            mutate_derived_state,
        )
    };
    let _discovery_guard = ProviderDiscoveryGuard {
        inner: &state.0,
        provider: name.clone(),
    };

    let task_name = name.clone();
    let task_base_url = base_url.clone();
    let task_data_dir = data_dir.clone();
    let mut result = tauri::async_runtime::spawn_blocking(move || {
        let resolved_key: Option<Zeroizing<String>> = match credential {
            DiscoveryCredential::Explicit(key) => key.map(Zeroizing::new),
            DiscoveryCredential::Stored { provider, slot } => match pending_key {
                Some(key) => Some(key),
                None => Some(Zeroizing::new(egress_secrets.resolve(&provider, &slot)?)),
            },
        };
        model_catalog::discover_candidate_with_cache_egress(
            &task_data_dir,
            &task_name,
            &task_base_url,
            resolved_key.as_ref().map(|key| key.as_str()),
            &egress,
            &egress_secrets,
        )
    })
    .await
    .map_err(|error| format!("模型目录任务异常结束：{error}"))??;
    let mut inner = state.0.lock().unwrap();
    ensure_provider_discovery_target_unchanged(&inner, &name, &expected_target)?;
    if mutate_derived_state {
        if let Err(error) =
            model_catalog::commit_live_discovery_cache(&data_dir, &name, &base_url, &result)
        {
            result.warning = Some(error);
        }
        result.capabilities_updated = match mutation {
            DiscoveryMutation::None => false,
            DiscoveryMutation::AllCapabilities => {
                apply_discovered_model_capabilities(&mut inner, &name, &result.catalog)?
            }
            DiscoveryMutation::LimitsOnly => {
                apply_discovered_model_limits(&mut inner, &name, &result.catalog)?
            }
        };
    }
    if result.source == "none"
        && inner.draft["upstreams"]
            .get(&name)
            .is_some_and(provider_uses_builtin_model_limits)
    {
        result.source = "preset".to_owned();
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn test_provider(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<Vec<ProviderTestResult>, String> {
    let (config, name, tests_south) = {
        let inner = state.0.lock().unwrap();
        let name = name.trim().to_owned();
        let upstream = inner.draft["upstreams"]
            .get(&name)
            .ok_or_else(|| format!("供应商 `{name}` 不存在"))?;
        let provider = upstream["provider"].as_str().unwrap_or_default();
        let package_verified = inner.south_approved_dialects.contains(provider);
        let tests_south = provider_health_uses_south(&inner.draft, upstream, package_verified);
        (inner.materialize()?, name, tests_south)
    };
    let provider_runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
        let gateway =
            Gateway::new_with_provider_runtime(&config, recorder, provider_runtime.clone())?;
        if tests_south {
            let outcomes = provider_runtime.block_on(gateway.probe_south_v1(&name, None))?;
            return Ok(outcomes
                .into_iter()
                .map(|outcome| {
                    let (status, detail, latency_ms) = match outcome.latency_ms {
                        Ok(latency) => (
                            StageStatus::Pass,
                            Some("Configured South engine probe passed".to_owned()),
                            Some(latency),
                        ),
                        Err(error) => (StageStatus::Fail, Some(error), None),
                    };
                    let stages = ["network", "http", "auth", "model", "generation"]
                        .into_iter()
                        .map(|layer| ProviderTestStage {
                            layer: layer.to_owned(),
                            status: if status == StageStatus::Pass || layer == "generation" {
                                status
                            } else {
                                StageStatus::Skipped
                            },
                            detail: detail.clone(),
                            duration_ms: latency_ms,
                            timing_kind: latency_ms.map(|_| "cumulative"),
                        })
                        .collect();
                    ProviderTestResult {
                        model: outcome.model,
                        stages,
                        latency_ms,
                    }
                })
                .collect());
        }
        let probes = gateway.probe_layered(&name, None)?;
        Ok(probes
            .into_iter()
            .map(|probe| {
                let generation_passed = probe
                    .stages
                    .last()
                    .is_some_and(|stage| stage.status == StageStatus::Pass);
                let mut stages: Vec<ProviderTestStage> = probe
                    .stages
                    .into_iter()
                    .map(|stage| ProviderTestStage {
                        layer: match stage.layer {
                            HealthLayer::Network => "network",
                            HealthLayer::Http => "http",
                            HealthLayer::Auth => "auth",
                            HealthLayer::Model => "model",
                            HealthLayer::Generation => "generation",
                        }
                        .to_owned(),
                        status: stage.status,
                        detail: stage.detail,
                        duration_ms: (stage.status != StageStatus::Skipped)
                            .then_some(probe.latency_ms)
                            .flatten(),
                        timing_kind: (stage.status != StageStatus::Skipped).then_some("cumulative"),
                    })
                    .collect();
                if generation_passed {
                    match gateway.probe_features(&name, &probe.model) {
                        Ok(features) => stages.extend(features.stages.into_iter().map(|stage| {
                            ProviderTestStage {
                                layer: match stage.layer {
                                    FeatureLayer::Stream => "stream",
                                    FeatureLayer::Tool => "tool",
                                    FeatureLayer::Json => "json",
                                }
                                .to_owned(),
                                status: stage.status,
                                detail: stage.detail,
                                duration_ms: Some(stage.duration_ms),
                                timing_kind: Some("stage"),
                            }
                        })),
                        Err(error) => stages.extend(["stream", "tool", "json"].map(|layer| {
                            ProviderTestStage {
                                layer: layer.to_owned(),
                                status: StageStatus::Fail,
                                detail: Some(error.clone()),
                                duration_ms: None,
                                timing_kind: None,
                            }
                        })),
                    }
                } else {
                    stages.extend(["stream", "tool", "json"].map(|layer| ProviderTestStage {
                        layer: layer.to_owned(),
                        status: StageStatus::Skipped,
                        detail: Some("基础生成测试未通过".to_owned()),
                        duration_ms: None,
                        timing_kind: None,
                    }));
                }
                ProviderTestResult {
                    model: probe.model,
                    stages,
                    latency_ms: probe.latency_ms,
                }
            })
            .collect())
    })
    .await
    .map_err(|error| format!("Provider 测试任务异常结束：{error}"))?
}
