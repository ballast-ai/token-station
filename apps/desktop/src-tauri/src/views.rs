use crate::*;

// ---- Frontend view types ----------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ProviderView {
    pub(crate) name: String,
    pub(crate) brand_id: Option<&'static str>,
    pub(crate) provider: String,
    pub(crate) base_url: String,
    pub(crate) models: Vec<String>,
    pub(crate) model_capabilities: Vec<ModelCapabilityView>,
    pub(crate) catalog_revision: u64,
    pub(crate) catalog: Vec<model_catalog::CatalogModelView>,
    pub(crate) has_auth: bool,
    pub(crate) credential_source: String,
    pub(crate) credential_reference: String,
    pub(crate) provider_call: String,
    pub(crate) south_v1_available: bool,
    pub(crate) south_v1_unavailable_reason: Option<&'static str>,
    pub(crate) south_header_auth_v1_available: bool,
    pub(crate) south_header_auth_v1_unavailable_reason: Option<&'static str>,
    /// This upstream runs on the local machine; `local_only` routing keeps to it.
    pub(crate) local: bool,
    /// This upstream was created through the managed enterprise connection flow.
    pub(crate) managed_route: bool,
    pub(crate) access_tier: String,
    /// The declared quota plan (window + limit + unit) used for local estimation
    /// in quota-first mode, if the user set one. `None` ⇒ non-windowed / metered.
    pub(crate) quota_plan: Option<QuotaPlanView>,
}

/// A provider's declared quota plan, flattened to its primary reset window for
/// the UI (the common case is one window, e.g. a token allowance per 5 hours).
#[derive(Serialize)]
pub(crate) struct QuotaPlanView {
    pub(crate) len_ms: u64,
    pub(crate) limit: u64,
    pub(crate) unit: String,
    pub(crate) rate_limit_per_min: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ModelCapabilityView {
    pub(crate) model: String,
    pub(crate) tool: CapabilityState,
    pub(crate) vision: CapabilityState,
    pub(crate) json_schema: CapabilityState,
    pub(crate) context_window: u32,
    pub(crate) max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_window_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_tokens_source: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ProviderEndpointPreview {
    pub(crate) chat: String,
    pub(crate) responses: String,
    pub(crate) messages: String,
    pub(crate) loopback: bool,
}

#[derive(Serialize)]
pub(crate) struct ProviderRemovalPreview {
    pub(crate) name: String,
    pub(crate) references: Vec<String>,
    pub(crate) can_remove: bool,
}

#[derive(Serialize)]
pub(crate) struct ProviderTestStage {
    pub(crate) layer: String,
    pub(crate) status: StageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timing_kind: Option<&'static str>,
}

#[derive(Serialize)]
pub(crate) struct ProviderTestResult {
    pub(crate) model: String,
    pub(crate) stages: Vec<ProviderTestStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) latency_ms: Option<u64>,
}

#[derive(Clone, Serialize)]
pub(crate) struct TierView {
    pub(crate) upstream: Option<String>,
    pub(crate) model: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AgentRouteView {
    pub(crate) mode: String,
    /// True only when every per-Agent routing axis is absent and the Agent
    /// therefore follows the complete Home configuration.
    pub(crate) inherits_global: bool,
    pub(crate) tiers: std::collections::BTreeMap<String, TierView>,
    pub(crate) config_error: Option<String>,
    pub(crate) profile: Option<String>,
    /// Effective routing philosophy for this Agent: its own override if set,
    /// otherwise the Home default. Drives the per-Agent top-bar toggle and which
    /// page body (three-tier vs quota-first) the Agent renders.
    pub(crate) routing_mode: String,
    pub(crate) direct_target: Option<DirectTargetView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DirectTargetView {
    pub(crate) upstream: String,
    pub(crate) model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ModelOfferingView {
    pub(crate) upstream: String,
    pub(crate) model: String,
}

#[derive(Serialize)]
pub(crate) struct RouteContextView {
    pub(crate) model_offerings: Vec<ModelOfferingView>,
}

/// One account (upstream + model) in the quota-first rotation, in priority
/// order. Shared across every scope in quota mode — the pool of allowances to
/// drain is global; only the per-Agent *mode* is independent.
#[derive(Serialize)]
pub(crate) struct QuotaAccountView {
    pub(crate) upstream: String,
    pub(crate) model: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct QuotaAccountArg {
    pub(crate) upstream: String,
    pub(crate) model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServePhase {
    Stopped,
    Starting,
    Stopping,
    Running,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppRuntime {
    Stopped,
    Running,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServeView {
    pub(crate) phase: ServePhase,
    pub(crate) app_runtime: AppRuntime,
    pub(crate) listener_reachable: bool,
    pub(crate) agent_connected: bool,
    pub(crate) running_revision: Option<u64>,
    pub(crate) instance_id: Option<String>,
    pub(crate) listen: String,
    pub(crate) virtual_key: Option<String>,
    pub(crate) error: Option<String>,
    /// Whether Home model tests share this live Gateway's mutable state.
    pub(crate) model_test_uses_running_gateway: bool,
}

#[derive(Serialize)]
pub(crate) struct StateView {
    pub(crate) providers: Vec<ProviderView>,
    pub(crate) deleted_providers: Vec<String>,
    pub(crate) provider_recovery_error: Option<String>,
    pub(crate) tiers: std::collections::BTreeMap<String, TierView>,
    pub(crate) agent_routes: std::collections::BTreeMap<String, AgentRouteView>,
    pub(crate) profiles: Vec<String>,
    /// Per-tier user keyword libraries that provide direct routing control through router.rules keywords_any.
    pub(crate) keywords: std::collections::BTreeMap<String, Vec<String>>,
    /// Local-only routing uses providers marked local and keeps requests on the machine.
    pub(crate) local_only: bool,
    /// Whether local_only can use cloud fallback when no local target is available; false is strict local routing.
    pub(crate) allow_cloud_fallback: bool,
    /// Routing mode: direct, tiered intelligent routing, or quota_first.
    pub(crate) routing_mode: String,
    pub(crate) direct_target: Option<DirectTargetView>,
    /// Complete Home routing candidates. Tiered mode includes every pool member.
    pub(crate) route_context: RouteContextView,
    /// Globally shared quota-first rotation accounts, provider plus model, in priority order.
    pub(crate) quota_accounts: Vec<QuotaAccountView>,
    pub(crate) serve: ServeView,
    pub(crate) draft_revision: u64,
    pub(crate) saved_revision: u64,
    pub(crate) config_dirty: bool,
    /// Whether the draft materializes as a valid config and can be saved or started.
    pub(crate) config_error: Option<String>,
    /// Settings read model: switches, egress policy, and read-only environment information.
    pub(crate) settings: SettingsView,
}

#[derive(Serialize)]
pub(crate) struct ModelPriceImportResultView {
    pub(crate) state: StateView,
    pub(crate) imported: usize,
    pub(crate) existing: usize,
    pub(crate) missing_model_ids: Vec<String>,
    pub(crate) price_version: u32,
}

/// Settings view for proxy switches, egress policy, and read-only environment information.
#[derive(Serialize)]
pub(crate) struct SettingsView {
    pub(crate) listen: String,
    pub(crate) auth: bool,
    pub(crate) metrics: bool,
    pub(crate) data_dir: String,
    pub(crate) plugins_dir: String,
    pub(crate) agent: String,
    /// Backward-compatible alias for the desktop package version.
    pub(crate) version: String,
    pub(crate) desktop_version: String,
    pub(crate) core_version: String,
    pub(crate) egress_mode: String,
    pub(crate) egress_proxy_url: String,
    pub(crate) egress_no_proxy: Vec<String>,
    pub(crate) egress_auth_username: String,
    pub(crate) egress_auth_slot: String,
}

// ---- Subpage view types for full-capability subpages (#5) ------------------

/// Serializable mirror of stats::Aggregate for one tier or group.
#[derive(Serialize)]
pub(crate) struct AggView {
    pub(crate) requests: u64,
    pub(crate) errors: u64,
    pub(crate) p50_latency_ms: u64,
    pub(crate) p95_latency_ms: u64,
    pub(crate) input_tokens: u64,
    pub(crate) legacy_input_requests: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) cost_micros: Option<i64>,
    pub(crate) priced_requests: u64,
    pub(crate) unpriced_requests: u64,
}

impl AggView {
    pub(crate) fn zero() -> Self {
        Self {
            requests: 0,
            errors: 0,
            p50_latency_ms: 0,
            p95_latency_ms: 0,
            input_tokens: 0,
            legacy_input_requests: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_micros: None,
            priced_requests: 0,
            unpriced_requests: 0,
        }
    }
    pub(crate) fn from(a: &stats::Aggregate) -> Self {
        Self {
            requests: a.requests,
            errors: a.errors,
            p50_latency_ms: a.p50_latency_ms,
            p95_latency_ms: a.p95_latency_ms,
            input_tokens: a.input_tokens,
            legacy_input_requests: a.legacy_input_requests,
            output_tokens: a.output_tokens,
            cache_read_tokens: a.cache_read_tokens,
            cache_write_tokens: a.cache_write_tokens,
            reasoning_tokens: a.reasoning_tokens,
            cost_micros: a.cost_micros,
            priced_requests: a.priced_requests,
            unpriced_requests: a.unpriced_requests,
        }
    }
}

/// Usage-page view. empty=true means the metrics database does not exist because
/// serve never ran with metrics enabled, so the frontend shows guidance instead of an empty table.
#[derive(Serialize)]
pub(crate) struct StatsView {
    pub(crate) total: AggView,
    pub(crate) groups: Vec<(String, AggView)>,
    pub(crate) by: Option<String>,
    pub(crate) empty: bool,
}

#[derive(Serialize)]
pub(crate) struct ReceiptPageView {
    pub(crate) items: Vec<ReceiptView>,
    pub(crate) plaintext_by_request_id: BTreeMap<String, PlaintextExchange>,
    pub(crate) plaintext_errors_by_request_id: BTreeMap<String, String>,
    pub(crate) total: u64,
    pub(crate) page: usize,
    pub(crate) page_size: usize,
}

/// Four-layer routing-table view in order: rules, hint routes, heuristic bands,
/// then default-pool fallback. It reads only the draft and performs no API calls.
#[derive(Serialize)]
pub(crate) struct RouterTableView {
    pub(crate) default_pool: String,
    pub(crate) assumed_context_window: u64,
    pub(crate) threshold: Option<u32>,
    pub(crate) rules: Vec<Value>,
    pub(crate) hint_routes: Vec<Value>,
    pub(crate) bands: Vec<BandView>,
    pub(crate) pools: Vec<PoolView>,
}

/// One heuristic band: scores at or above at_least select its pool and current provider-model pair.
#[derive(Serialize)]
pub(crate) struct BandView {
    pub(crate) at_least: u32,
    pub(crate) pool: String,
    pub(crate) upstream: Option<String>,
    pub(crate) model: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct PoolView {
    pub(crate) pool: String,
    pub(crate) upstream: Option<String>,
    pub(crate) model: Option<String>,
}

/// Plugins-page view whose monospace listing reuses core render_list(), shared with CLI `plugin list`.
#[derive(Serialize)]
pub(crate) struct PluginsView {
    pub(crate) dir: String,
    pub(crate) agent: String,
    pub(crate) dialects: Vec<String>,
    pub(crate) listing: String,
}

#[tauri::command]
pub(crate) fn get_state(state: State<'_, AppStateManaged>) -> StateView {
    state.0.lock().unwrap().snapshot()
}

#[tauri::command]
pub(crate) fn get_runtime_state(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
    agents: State<'_, AgentCommandState>,
) -> ServeView {
    // Agent config inspection is file I/O and therefore intentionally outside
    // the App lock. Revalidate the immutable instance identity afterwards so
    // a concurrent publish can never combine old Agent facts with a new
    // running_revision/instance_id.
    for _ in 0..3 {
        let Ok(runtime) = runtime_from_app(state.inner()) else {
            let view = state.0.lock().unwrap().serve_view();
            desktop_shell::update_proxy_menu(&app);
            return view;
        };
        let identity = runtime.instance_id().to_owned();
        let agent_connected = agents.any_connected_to(&runtime).unwrap_or(false);
        let mut view = state.0.lock().unwrap().serve_view();
        if view.instance_id.as_deref() == Some(identity.as_str()) {
            view.agent_connected = agent_connected;
            desktop_shell::update_proxy_menu(&app);
            return view;
        }
    }
    // Continuous handoffs are rare; if all snapshots raced, return a truthful
    // current runtime view with the conservative independent Agent fact.
    let view = state.0.lock().unwrap().serve_view();
    desktop_shell::update_proxy_menu(&app);
    view
}
