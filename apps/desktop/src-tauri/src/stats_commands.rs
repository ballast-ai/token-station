use crate::*;

pub(crate) fn budget_statuses(inner: &AppInner) -> Result<Vec<BudgetStatus>, String> {
    let budgets: std::collections::BTreeMap<String, AgentBudget> = inner
        .draft
        .get("agent_budgets")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Agent 预算配置不合法：{error}"))?
        .unwrap_or_default();
    let db = inner.data_dir().join("metrics.sqlite");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    budgets
        .iter()
        .map(|(agent_id, budget)| {
            let aggregate = if db.exists() {
                stats::collect_range(
                    &db,
                    budget.period_start_ms,
                    budget.period_end_ms,
                    Some(stats::GroupBy::Agent),
                )?
                .groups
                .into_iter()
                .find(|(candidate, _)| candidate == agent_id)
                .map(|(_, aggregate)| aggregate)
                .unwrap_or_default()
            } else {
                stats::Aggregate::default()
            };
            let used_micros = aggregate
                .cost_micros
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or(0);
            Ok(BudgetStatus::evaluate(
                agent_id,
                budget,
                used_micros,
                aggregate.unpriced_requests,
                now_ms,
            ))
        })
        .collect()
}

#[tauri::command]
pub(crate) fn get_agent_budgets(
    state: State<'_, AppStateManaged>,
) -> Result<Vec<BudgetStatus>, String> {
    budget_statuses(&state.0.lock().unwrap())
}

#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps this stable command boundary to named form fields"
)]
pub(crate) fn set_agent_budget(
    state: State<'_, AppStateManaged>,
    agent_id: String,
    limit_micros: u64,
    warning_percent: u8,
    period_start_ms: Option<u64>,
    period_end_ms: Option<u64>,
    expiry_warning_days: u16,
) -> Result<Vec<BudgetStatus>, String> {
    ensure_known_agent_id(&agent_id)?;
    let budget = AgentBudget {
        limit_micros,
        warning_percent,
        period_start_ms,
        period_end_ms,
        expiry_warning_days,
    };
    budget.validate()?;
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    if !inner.draft["agent_budgets"].is_object() {
        inner.draft["agent_budgets"] = json!({});
    }
    inner.draft["agent_budgets"][&agent_id] =
        serde_json::to_value(budget).map_err(|error| error.to_string())?;
    inner.observe_draft()?;
    inner.save_draft()?;
    budget_statuses(&inner)
}

#[tauri::command]
pub(crate) fn remove_agent_budget(
    state: State<'_, AppStateManaged>,
    agent_id: String,
) -> Result<Vec<BudgetStatus>, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let budgets = inner.draft["agent_budgets"]
        .as_object_mut()
        .ok_or_else(|| format!("Agent `{agent_id}` 尚未配置预算"))?;
    if budgets.remove(&agent_id).is_none() {
        return Err(format!("Agent `{agent_id}` 尚未配置预算"));
    }
    inner.observe_draft()?;
    inner.save_draft()?;
    budget_statuses(&inner)
}

/// Read-only usage aggregation. since accepts all, hours, or days; by accepts
/// agent, upstream, model, pool, status, hour, day, or empty. Return empty=true
/// rather than an error when the metrics database does not exist.
#[tauri::command]
pub(crate) fn get_stats(
    state: State<'_, AppStateManaged>,
    since: String,
    by: Option<String>,
    agent_id: Option<String>,
    source: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
) -> Result<StatsView, String> {
    let db = {
        let inner = state.0.lock().unwrap();
        inner.data_dir().join("metrics.sqlite")
    };
    if !db.exists() {
        return Ok(StatsView {
            total: AggView::zero(),
            groups: vec![],
            by,
            empty: true,
        });
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        });
    let cutoff = stats::cutoff_from_since(&since, now_ms)?;
    let group = match by.as_deref() {
        None | Some("") => None,
        Some("agent") => Some(stats::GroupBy::Agent),
        Some("upstream") => Some(stats::GroupBy::Upstream),
        Some("model") => Some(stats::GroupBy::Model),
        Some("pool") => Some(stats::GroupBy::Pool),
        Some("status") => Some(stats::GroupBy::Status),
        Some("hour") => Some(stats::GroupBy::Hour),
        Some("day") => Some(stats::GroupBy::Day),
        Some("engine") => Some(stats::GroupBy::Engine),
        Some("fallback") => Some(stats::GroupBy::Fallback),
        Some(other) => return Err(format!("未知分组 `{other}`")),
    };
    let report = stats::collect_filtered(
        &db,
        cutoff,
        None,
        group,
        stats::StatsFilter {
            agent_id: agent_id.as_deref(),
            source: source.as_deref(),
            upstream: upstream.as_deref(),
            model: model.as_deref(),
        },
    )?;
    Ok(StatsView {
        total: AggView::from(&report.total),
        groups: report
            .groups
            .iter()
            .map(|(k, a)| (k.clone(), AggView::from(a)))
            .collect(),
        by,
        empty: false,
    })
}

/// Return the five most recent body-free Request Receipts for the home page.
/// Return an empty array if the metrics database does not exist, including when
/// metrics are disabled. The read layer enforces the five-item limit.
#[tauri::command]
pub(crate) fn get_recent_receipts(
    state: State<'_, AppStateManaged>,
    limit: usize,
) -> Result<Vec<ReceiptView>, String> {
    let db = {
        let inner = state.0.lock().unwrap();
        inner.data_dir().join("metrics.sqlite")
    };
    SqliteStore::recent_receipts(&db, limit)
}

/// Read the complete body-free Request Receipt ledger with pagination for the usage page.
#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps the dashboard filters to named command fields"
)]
pub(crate) fn get_request_receipts(
    state: State<'_, AppStateManaged>,
    since: String,
    agent_id: Option<String>,
    upstream: Option<String>,
    model: Option<String>,
    status: Option<String>,
    page: usize,
    page_size: usize,
) -> Result<ReceiptPageView, String> {
    let (data_dir, now_ms) = {
        let inner = state.0.lock().unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        (inner.data_dir(), now_ms)
    };
    let since_ms = stats::cutoff_from_since(&since, now_ms)?;
    let bounded_page_size = page_size.clamp(1, 50);
    let bounded_page = page.max(1);
    let offset = bounded_page
        .saturating_sub(1)
        .saturating_mul(bounded_page_size);
    let result = SqliteStore::receipt_page(
        &data_dir.join("metrics.sqlite"),
        &ReceiptQuery {
            since_ms,
            agent_id,
            upstream,
            model,
            status,
        },
        bounded_page_size,
        offset,
    )?;
    let mut plaintext_by_request_id = BTreeMap::new();
    let mut plaintext_errors_by_request_id = BTreeMap::new();
    match BodyLog::open(&data_dir) {
        Ok(body_log) => {
            for receipt in &result.items {
                if !valid_request_id(&receipt.request_id) {
                    continue;
                }
                match body_log.read(&receipt.request_id) {
                    Ok(Some(exchange)) => {
                        plaintext_by_request_id.insert(receipt.request_id.clone(), exchange);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        plaintext_errors_by_request_id.insert(receipt.request_id.clone(), error);
                    }
                }
            }
        }
        Err(error) => {
            for receipt in &result.items {
                if valid_request_id(&receipt.request_id) {
                    plaintext_errors_by_request_id
                        .insert(receipt.request_id.clone(), error.clone());
                }
            }
        }
    }
    Ok(ReceiptPageView {
        items: result.items,
        plaintext_by_request_id,
        plaintext_errors_by_request_id,
        total: result.total,
        page: bounded_page,
        page_size: bounded_page_size,
    })
}

/// Convert the draft's rules, hints, heuristic tiers, and fallback into a read-only routing-table view with no API calls.
#[tauri::command]
pub(crate) fn get_router_table(state: State<'_, AppStateManaged>) -> RouterTableView {
    let inner = state.0.lock().unwrap();
    let r = &inner.draft["router"];

    let rules = r["rules"].as_array().cloned().unwrap_or_default();
    let hint_routes = r["hint_routes"].as_array().cloned().unwrap_or_default();
    let threshold = r["heuristic"]["threshold"].as_u64().map(|v| v as u32);

    let bands = r["heuristic"]["bands"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|b| {
                    let pool = b["pool"].as_str().unwrap_or_default().to_string();
                    let (upstream, model) = inner.pool_member(&pool);
                    BandView {
                        at_least: b["at_least"].as_u64().unwrap_or(0) as u32,
                        pool,
                        upstream,
                        model,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let pools = r["pools"]
        .as_object()
        .map(|obj| {
            obj.keys()
                .map(|pool| {
                    let (upstream, model) = inner.pool_member(pool);
                    PoolView {
                        pool: pool.clone(),
                        upstream,
                        model,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    RouterTableView {
        default_pool: r["default_pool"].as_str().unwrap_or_default().to_string(),
        assumed_context_window: r["assumed_context_window"].as_u64().unwrap_or(0),
        threshold,
        rules,
        hint_routes,
        bands,
        pools,
    }
}

/// Discover the plugin directory and reuse core render_list() for a monospace
/// listing shared with CLI `plugin list`. This works even with incomplete tiers.
#[tauri::command]
pub(crate) fn get_plugins(state: State<'_, AppStateManaged>) -> Result<PluginsView, String> {
    let (plugins_cfg, data_dir) = {
        let inner = state.0.lock().unwrap();
        let cfg: PluginsConfig = serde_json::from_value(inner.draft["plugins"].clone())
            .map_err(|e| format!("plugins 配置不合法: {e}"))?;
        (cfg, inner.data_dir())
    };
    let receipts = Receipts::load(&data_dir)?;
    let registry = PluginRegistry::discover(&plugins_cfg, &receipts)?;
    let approved = south_approved_dialects(&registry);
    {
        let mut inner = state.0.lock().unwrap();
        let current_plugins_dir = inner.draft["plugins"]["dir"].as_str().map(Path::new);
        if current_plugins_dir == Some(plugins_cfg.dir.as_path()) && inner.data_dir() == data_dir {
            inner.south_approved_dialects = approved;
        }
    }
    Ok(PluginsView {
        dir: plugins_cfg.dir.display().to_string(),
        agent: plugins_cfg.effective_agents().join(", "),
        dialects: registry
            .provider_dialects()
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        listing: registry.render_list(),
    })
}
