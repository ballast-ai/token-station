use crate::agent_integration::commands::{model_metadata_for_config, runtime_from_app};
use crate::agent_integration::connectors::AgentModelMetadata;
use crate::*;

/// Pool names for the three tier slots shown as the panel's high, middle, and low rows.
pub(crate) const TIER_HIGH: &str = "tier_high";
pub(crate) const TIER_MID: &str = "tier_mid";
pub(crate) const TIER_LOW: &str = "tier_low";

/// Stable ID for each tier's keyword override rule. User keywords enter the
/// rule's keywords_any list and force that tier ahead of complexity scoring at
/// router-core layer 1. IDs remain stable because decision records and audits
/// also store them as matched routing-rule IDs.
pub(crate) const KW_RULE_HIGH: &str = "kw-high";
pub(crate) const KW_RULE_MID: &str = "kw-mid";
pub(crate) const KW_RULE_LOW: &str = "kw-low";

/// Map UI slots to pool names and keyword-rule IDs. Rule order is priority from
/// high to mid to low, so phrases matching multiple tiers move upward safely.
pub(crate) fn tier_pool_and_rule(slot: &str) -> Result<(&'static str, &'static str), String> {
    match slot {
        "high" => Ok((TIER_HIGH, KW_RULE_HIGH)),
        "mid" => Ok((TIER_MID, KW_RULE_MID)),
        "low" => Ok((TIER_LOW, KW_RULE_LOW)),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

/// Three tiers from high to low as UI slot, pool name, and keyword-rule ID; preserve this order in router.rules.
pub(crate) const TIER_ORDER: [(&str, &str, &str); 3] = [
    ("high", TIER_HIGH, KW_RULE_HIGH),
    ("mid", TIER_MID, KW_RULE_MID),
    ("low", TIER_LOW, KW_RULE_LOW),
];

/// Tier thresholds mapping heuristic scores to tiers. Bands descend strictly by
/// at_least, with a final zero fallback. Evaluation will calibrate these defaults later.
pub(crate) const CUT_HIGH: u32 = 55;
pub(crate) const CUT_MID: u32 = 22;

fn transition_model_metadata(
    current: &AgentModelMetadata,
    next: &AgentModelMetadata,
) -> AgentModelMetadata {
    AgentModelMetadata {
        context: current.context.min(next.context),
        output: current.output.min(next.output),
        max_input: current
            .safe_max_input()
            .zip(next.safe_max_input())
            .map_or(0, |(current, next)| current.min(next)),
        vision: current.vision && next.vision,
        tools: current.tools && next.tools,
        reasoning: current.reasoning && next.reasoning,
        cost: (current.cost == next.cost)
            .then(|| current.cost.clone())
            .flatten(),
    }
}

pub(crate) fn pool_key(slot: &str) -> Result<&'static str, String> {
    match slot {
        "high" => Ok(TIER_HIGH),
        "mid" => Ok(TIER_MID),
        "low" => Ok(TIER_LOW),
        other => Err(format!("未知档位 `{other}`(应为 high/mid/low)")),
    }
}

pub(crate) fn ensure_known_agent_id(agent_id: &str) -> Result<(), String> {
    supported_agent_ids()
        .iter()
        .any(|candidate| candidate == agent_id)
        .then_some(())
        .ok_or_else(|| format!("未知 Agent `{agent_id}`"))
}

pub(crate) fn supported_agent_ids() -> Vec<String> {
    AgentRegistry::builtin()
        .expect("built-in Agent Registry must be valid")
        .descriptors()
        .iter()
        .filter(|descriptor| {
            descriptor.admission == AdmissionStatus::Supported
                // Cursor has a verified protocol route and a dedicated SQLite
                // configurator, but intentionally has no generic local Connector.
                || descriptor.agent_id == "cursor"
        })
        .map(|descriptor| descriptor.agent_id.clone())
        .collect()
}

/// Set local-only routing and cloud fallback in the home router so inherited
/// Agents follow automatically. Remove both keys when disabled to preserve
/// ordinary configs and serde's false default.
#[tauri::command]
pub(crate) fn set_local_routing(
    state: State<'_, AppStateManaged>,
    local_only: bool,
    allow_cloud_fallback: bool,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous = inner.draft["router"].clone();
    if local_only {
        inner.draft["router"]["local_only"] = json!(true);
        inner.draft["router"]["allow_cloud_fallback"] = json!(allow_cloud_fallback);
    } else if let Some(router) = inner.draft["router"].as_object_mut() {
        // Cloud fallback is meaningless when local-only is off, so clear both to avoid stale state.
        router.remove("local_only");
        router.remove("allow_cloud_fallback");
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Switch between direct, tiered (difficulty-based), and quota-first
/// (allowance-draining) routing. Agent overrides are always explicit. The
/// top-level host routing contract is authoritative; keep the embedded
/// core router in quota-first only for quota mode; Direct is compiled from the
/// host target and therefore keeps the embedded router in tiered mode.
#[tauri::command]
pub(crate) fn set_routing_mode(
    state: State<'_, AppStateManaged>,
    mode: String,
    agent_id: Option<String>,
) -> Result<StateView, String> {
    if mode != "tiered" && mode != "direct" && mode != "quota_first" {
        return Err(format!("未知路由模式：{mode}"));
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    // Per-Agent switch: pin this one Agent's routing_mode without touching Home
    // or its siblings. Unlike Home, we write BOTH modes explicitly — for an
    // Agent a missing key means "inherit Home", so clearing it on tiered would
    // wrongly re-inherit a quota-first Home. An explicit value is what keeps the
    // Agent independent of later Home changes. ("Restore home routing" is the
    // path back to inheritance; it clears the key.)
    if let Some(agent_id) = agent_id {
        if !supported_agent_ids().contains(&agent_id) {
            return Err(format!("未知 Agent：{agent_id}"));
        }
        let previous = inner.draft["agent_routes"][&agent_id].clone();
        if !inner.draft["agent_routes"][&agent_id].is_object() {
            inner.draft["agent_routes"][&agent_id] = json!({ "mode": "inherit" });
        }
        inner.draft["agent_routes"][&agent_id]["routing_mode"] = json!(mode);
        if let Err(error) = inner.observe_draft() {
            inner.draft["agent_routes"][&agent_id] = previous;
            return Err(error);
        }
        return Ok(inner.snapshot());
    }

    let previous_routing = inner.draft.get("routing").cloned();
    let previous_router = inner.draft["router"].clone();
    if !inner.draft["routing"].is_object() {
        inner.draft["routing"] = json!({});
    }
    inner.draft["routing"]["mode"] = json!(mode);
    inner.draft["router"]["routing_mode"] = json!(if mode == "quota_first" {
        "quota_first"
    } else {
        "tiered"
    });
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous_router;
        if let Some(previous_routing) = previous_routing {
            inner.draft["routing"] = previous_routing;
        } else if let Some(draft) = inner.draft.as_object_mut() {
            draft.remove("routing");
        }
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn set_direct_route(
    state: State<'_, AppStateManaged>,
    upstream: String,
    model: String,
    agent_id: Option<String>,
) -> Result<StateView, String> {
    let upstream = upstream.trim();
    let model = model.trim();
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.validate_route_target(upstream, model)?;

    if let Some(agent_id) = agent_id {
        ensure_known_agent_id(&agent_id)?;
        let previous = inner.draft["agent_routes"][&agent_id].clone();
        if !inner.draft["agent_routes"][&agent_id].is_object() {
            inner.draft["agent_routes"][&agent_id] = json!({ "mode": "inherit" });
        }
        inner.draft["agent_routes"][&agent_id]["direct_target"] =
            json!({ "upstream": upstream, "model": model });
        if let Err(error) = inner.observe_draft() {
            inner.draft["agent_routes"][&agent_id] = previous;
            return Err(error);
        }
    } else {
        let previous = inner.draft.get("routing").cloned();
        if !inner.draft["routing"].is_object() {
            let mode = inner.home_routing_mode().to_owned();
            inner.draft["routing"] = json!({ "mode": mode });
        }
        inner.draft["routing"]["direct_target"] = json!({ "upstream": upstream, "model": model });
        if let Err(error) = inner.observe_draft() {
            if let Some(previous) = previous {
                inner.draft["routing"] = previous;
            } else if let Some(draft) = inner.draft.as_object_mut() {
                draft.remove("routing");
            }
            return Err(error);
        }
    }
    Ok(inner.snapshot())
}

/// Persists the quota-first rotation list (ordered upstream+model accounts). The
/// order is the operator's priority; it lands verbatim in
/// `router.quota_accounts` and drives `Router::route_quota_first`. At least one
/// complete row is required; incomplete rows are rejected without changing the
/// draft. Complete rows are validated against the configured catalog, and exact
/// duplicates are collapsed while keeping first-seen order.
#[tauri::command]
pub(crate) fn set_quota_accounts(
    state: State<'_, AppStateManaged>,
    accounts: Vec<QuotaAccountArg>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;

    if accounts.is_empty() {
        return Err("额度优先至少需要一个完整账户，请选择供应商和模型".to_owned());
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut clean: Vec<Value> = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        let upstream = account.upstream.trim();
        let model = account.model.trim();
        if upstream.is_empty() || model.is_empty() {
            return Err(format!("额度优先账户 #{} 缺少供应商或模型", index + 1));
        }
        inner.validate_route_target(upstream, model)?;
        if seen.insert((upstream.to_owned(), model.to_owned())) {
            clean.push(json!({ "upstream": upstream, "model": model }));
        }
    }

    let previous = inner.draft["router"].clone();
    inner.draft["router"]["quota_accounts"] = Value::Array(clean);
    if let Err(error) = inner.observe_draft() {
        inner.draft["router"] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Declares (or clears) a provider's quota plan for local estimation: one reset
/// window (`len_ms` + `limit`), the unit it counts in, and an optional
/// requests-per-minute rate limit. A zero limit or window clears the plan (the
/// account falls back to non-windowed / authoritative-only). Written under
/// `upstreams[name].quota_plan`, feeding the account ledger's estimate.
#[tauri::command]
pub(crate) fn set_quota_plan(
    state: State<'_, AppStateManaged>,
    upstream: String,
    len_ms: u64,
    limit: u64,
    unit: String,
    rate_limit_per_min: Option<u64>,
) -> Result<StateView, String> {
    let upstream = upstream.trim().to_owned();
    if unit != "tokens" && unit != "requests" {
        return Err(format!("未知额度单位：{unit}"));
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if !inner.draft["upstreams"][&upstream].is_object() {
        return Err(format!("未知供应商 `{upstream}`"));
    }

    let previous = inner.draft["upstreams"][&upstream].clone();
    if limit == 0 || len_ms == 0 {
        if let Some(up) = inner.draft["upstreams"][&upstream].as_object_mut() {
            up.remove("quota_plan");
        }
    } else {
        let mut plan = json!({
            "windows": [{ "len_ms": len_ms, "limit": limit }],
            "unit": unit,
        });
        if let Some(rate) = rate_limit_per_min.filter(|rate| *rate > 0) {
            plan["rate_limit_per_min"] = json!(rate);
        }
        inner.draft["upstreams"][&upstream]["quota_plan"] = plan;
    }
    if let Err(error) = inner.observe_draft() {
        inner.draft["upstreams"][&upstream] = previous;
        return Err(error);
    }
    Ok(inner.snapshot())
}

/// Live quota snapshot for the viewer: queries the running gateway's
/// `/admin/quota` over loopback with the virtual key. Runtime-only — the quota
/// picture lives in the gateway's memory, not the config or the metrics store —
/// so this needs the proxy running.
#[tauri::command]
pub(crate) fn get_quota_snapshot(
    state: State<'_, AppStateManaged>,
) -> Result<serde_json::Value, String> {
    let (listen, key) = {
        let inner = state.0.lock().unwrap();
        let serve = inner.serve_view();
        (serve.listen, serve.virtual_key)
    };
    let Some(key) = key else {
        return Err("代理未运行——启动代理后可查看实时额度".to_owned());
    };
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(3)))
            .build(),
    );
    let response = agent
        .get(format!("http://{listen}/admin/quota"))
        .header("authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| format!("查询额度失败：{error}"))?;
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("读取额度响应失败：{error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("额度响应不是合法 JSON：{error}"))
}

/// Set a tier to a provider-model pair, or pass null to clear it.
#[tauri::command]
pub(crate) fn set_tier(
    state: State<'_, AppStateManaged>,
    slot: String,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StateView, String> {
    let pool = pool_key(&slot)?;
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.set_tier_value(pool, upstream, model))?;
    Ok(inner.snapshot())
}

/// Add a keyword to a high, mid, or low tier; matches force that tier at router-core layer 1.
#[tauri::command]
pub(crate) fn add_keyword(
    state: State<'_, AppStateManaged>,
    slot: String,
    keyword: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.add_tier_keyword(&slot, &keyword))?;
    Ok(inner.snapshot())
}

/// Remove a keyword from a tier.
#[tauri::command]
pub(crate) fn remove_keyword(
    state: State<'_, AppStateManaged>,
    slot: String,
    keyword: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.remove_tier_keyword(&slot, &keyword))?;
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn set_agent_route_mode(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    mode: String,
) -> Result<StateView, String> {
    ensure_known_agent_id(&agent_id)?;
    if mode != "inherit" && mode != "custom" {
        return Err("路由模式必须是 inherit 或 custom".to_string());
    }
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if mode == "custom" {
        inner.begin_agent_route_draft(&agent_id);
        return Ok(inner.snapshot());
    }
    inner.edit_validated_draft(|candidate| {
        candidate.set_agent_inherit_value(&agent_id);
        Ok(())
    })?;
    inner.agent_route_drafts.remove(&agent_id);
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn set_agent_tier(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    slot: String,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.set_agent_route_draft_tier(&agent_id, &slot, upstream, model)?;
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn set_agent_harness_model_route(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    requested_model: String,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    inner.set_agent_harness_model_route(&agent_id, &requested_model, upstream, model)?;
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn save_home_route_as_profile(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.save_home_route_as_profile_value(&name))?;
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn mount_agent_profile(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    profile: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| {
        candidate.mount_agent_profile_value(&agent_id, &profile)
    })?;
    inner.agent_route_drafts.remove(&agent_id);
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn delete_profile(
    state: State<'_, AppStateManaged>,
    name: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.edit_validated_draft(|candidate| candidate.delete_profile_value(&name))?;
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn save_agent_routes(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.promote_agent_route_drafts()?;
    inner.save_draft()?;
    inner.agent_route_drafts.clear();
    Ok(inner.snapshot())
}

/// Save one Agent's route and apply it immediately by hot-swapping just that
/// Agent's router on the running gateway — no full restart, so every other
/// Agent's in-flight and new requests are undisturbed. If the proxy is not
/// running, this behaves like `save_agent_routes` (the route applies on next
/// start). Drives the per-Agent "Save & restart" button.
#[tauri::command]
pub(crate) fn restart_agent_route(
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
    agent_id: String,
) -> Result<StateView, String> {
    let current_runtime = runtime_from_app(state.inner()).ok();
    let mut transition_runtime = current_runtime.clone();
    let snapshot = {
        let mut inner = state.0.lock().unwrap();
        if !supported_agent_ids().contains(&agent_id) {
            return Err(format!("未知 Agent：{agent_id}"));
        }
        if matches!(
            inner.server,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
        ) {
            return Err("apply_in_progress: 配置正在应用，请完成后再保存 Agent 路由".to_owned());
        }
        let applying_direct = inner.draft["agent_routes"][&agent_id]["routing_mode"]
            .as_str()
            .unwrap_or_else(|| inner.home_routing_mode())
            == "direct";
        // Tier editor drafts are a separate axis from Direct routing. Applying a
        // Direct target must neither validate nor silently commit an incomplete
        // hidden tier draft; keep it in memory for when the operator switches back.
        if !applying_direct {
            inner.promote_agent_route_draft(&agent_id)?;
        }
        // Prepare every fallible hot-reload step before persisting. A successful
        // config save must never be followed by a recoverable router-build failure,
        // which would split the durable route from the running Gateway.
        let config = inner.materialize()?;
        let router = config.custom_router_for_agent(&agent_id)?;
        let harness = config.harness_router_for_agent(&agent_id)?;
        let prepared = match &inner.server {
            ServerLifecycle::Running { server, .. } => Some(
                server
                    .prepare_agent_router_reload(&agent_id, router, harness)
                    .map_err(|error| format!("热重启 Agent 路由失败：{error}"))?,
            ),
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Failed { .. }
            | ServerLifecycle::Stopping { .. } => None,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                unreachable!("transitional lifecycles were rejected before editing")
            }
        };

        // Move every connected client to a budget safe for both the old and
        // pending routes before the gateway changes. If either metadata or the
        // external config transaction fails, the route is not applied.
        if let Some(runtime) = transition_runtime.as_mut() {
            let pending_metadata = model_metadata_for_config(&config, &agent_id)?;
            let transition_metadata = runtime
                .model_metadata(&agent_id)
                .zip(pending_metadata.as_ref())
                .map(|(current, next)| transition_model_metadata(current, next))
                .or(pending_metadata);
            runtime.replace_model_metadata(&agent_id, transition_metadata);
            agents
                .refresh_model_metadata(Some(&agent_id), runtime)
                .map_err(|error| {
                    format!(
                        "Agent 路由未应用：无法先写入安全的过渡模型容量：{}",
                        error.message
                    )
                })?;
        }

        if let Err(error) = inner.save_draft() {
            if let Some(runtime) = current_runtime.as_ref() {
                let _ = agents.refresh_model_metadata(Some(&agent_id), runtime);
            }
            return Err(error);
        }
        if !applying_direct {
            inner.agent_route_drafts.remove(&agent_id);
        }
        if let (Some(prepared), ServerLifecycle::Running { server, .. }) =
            (prepared, &mut inner.server)
        {
            server.install_prevalidated_agent_router(prepared);
        }
        inner.snapshot()
    };
    if let Ok(runtime) = runtime_from_app(state.inner()) {
        agents
            .refresh_model_metadata(Some(&agent_id), &runtime)
            .map_err(|error| {
                format!("Agent 路由已应用，但模型元数据刷新失败：{}", error.message)
            })?;
    }
    Ok(snapshot)
}

/// Save one Agent's Harness model mappings without changing its routing mode
/// or tier source. Hot-reload the Agent router when the proxy is running.
#[tauri::command]
pub(crate) fn restart_agent_harness_routes(
    state: State<'_, AppStateManaged>,
    agent_id: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    ensure_known_agent_id(&agent_id)?;
    if matches!(
        inner.server,
        ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
    ) {
        return Err("apply_in_progress: 配置正在应用，请完成后再保存 Harness 映射".to_owned());
    }
    inner.begin_agent_harness_route_draft(&agent_id);
    inner.promote_agent_harness_route_draft(&agent_id)?;
    let config = inner.materialize()?;
    let router = config.custom_router_for_agent(&agent_id)?;
    let harness = config.harness_router_for_agent(&agent_id)?;
    let prepared = match &inner.server {
        ServerLifecycle::Running { server, .. } => Some(
            server
                .prepare_agent_router_reload(&agent_id, router, harness)
                .map_err(|error| format!("热重启 Harness 映射失败：{error}"))?,
        ),
        ServerLifecycle::Stopped { .. }
        | ServerLifecycle::Failed { .. }
        | ServerLifecycle::Stopping { .. } => None,
        ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
            unreachable!("transitional lifecycles were rejected before editing")
        }
    };
    inner.save_draft()?;
    inner.agent_harness_route_drafts.remove(&agent_id);
    if let (Some(prepared), ServerLifecycle::Running { server, .. }) = (prepared, &mut inner.server)
    {
        server.install_prevalidated_agent_router(prepared);
    }
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn apply_home_route_to_all_agents(
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
) -> Result<StateView, String> {
    let agent_ids = supported_agent_ids();
    let current_runtime = runtime_from_app(state.inner()).ok();
    let mut transition_runtime = current_runtime.clone();
    let snapshot = {
        let mut inner = state.0.lock().unwrap();
        if matches!(
            inner.server,
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. }
        ) {
            return Err(
                "apply_in_progress: Finish the current config apply before you apply the Home route."
                    .to_owned(),
            );
        }
        inner.edit_validated_draft(|candidate| {
            for agent_id in &agent_ids {
                candidate.set_agent_inherit_value(agent_id);
            }
            Ok(())
        })?;
        let config = inner.materialize()?;
        let prepared = match &inner.server {
            ServerLifecycle::Running { server, .. } => agent_ids
                .iter()
                .map(|agent_id| {
                    let router = config.custom_router_for_agent(agent_id)?;
                    let harness = config.harness_router_for_agent(agent_id)?;
                    server
                        .prepare_agent_router_reload(agent_id, router, harness)
                        .map_err(|error| format!("Failed to hot-reload the Agent route: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            ServerLifecycle::Stopped { .. }
            | ServerLifecycle::Failed { .. }
            | ServerLifecycle::Stopping { .. } => Vec::new(),
            ServerLifecycle::Starting { .. } | ServerLifecycle::Applying { .. } => {
                unreachable!("transitional lifecycles were rejected before editing")
            }
        };

        if let Some(runtime) = transition_runtime.as_mut() {
            for agent_id in &agent_ids {
                let pending_metadata = model_metadata_for_config(&config, agent_id)?;
                let transition_metadata = runtime
                    .model_metadata(agent_id)
                    .zip(pending_metadata.as_ref())
                    .map(|(current, next)| transition_model_metadata(current, next))
                    .or(pending_metadata);
                runtime.replace_model_metadata(agent_id, transition_metadata);
            }
            agents
                .refresh_model_metadata(None, runtime)
                .map_err(|error| {
                    format!(
                        "Home 路由未应用：无法先写入安全的过渡模型容量：{}",
                        error.message
                    )
                })?;
        }

        if let Err(error) = inner.save_draft() {
            if let Some(runtime) = current_runtime.as_ref() {
                let _ = agents.refresh_model_metadata(None, runtime);
            }
            return Err(error);
        }
        inner.agent_route_drafts.clear();
        if let ServerLifecycle::Running { server, .. } = &mut inner.server {
            for prepared in prepared {
                server.install_prevalidated_agent_router(prepared);
            }
        }
        inner.snapshot()
    };
    if let Ok(runtime) = runtime_from_app(state.inner()) {
        agents
            .refresh_model_metadata(None, &runtime)
            .map_err(|error| {
                format!(
                    "The Home route was applied, but the model metadata refresh failed: {}",
                    error.message
                )
            })?;
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(context: u32, output: u32, max_input: u32) -> AgentModelMetadata {
        AgentModelMetadata {
            context,
            output,
            max_input,
            vision: true,
            tools: true,
            reasoning: true,
            cost: None,
        }
    }

    #[test]
    fn route_transition_uses_limits_safe_for_both_revisions() {
        let current = metadata(1_000_000, 32_768, 967_232);
        let next = metadata(128_000, 16_384, 64_000);

        let transition = transition_model_metadata(&current, &next);

        assert_eq!(transition.context, 128_000);
        assert_eq!(transition.output, 16_384);
        assert_eq!(transition.max_input, 64_000);
    }
}
