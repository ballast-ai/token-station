use crate::*;

/// Derive required desktop inbound adapters from Connector capabilities.
/// Deduplicate adapters while preserving build-time registry order so adding a
/// Connector no longer requires changes here.
pub(crate) fn desktop_agents() -> Vec<&'static str> {
    let mut agents = Vec::new();
    for connector in agent_integration::connectors::builtin_connectors() {
        let adapter = connector.capabilities().adapter_id;
        if !agents.contains(&adapter) {
            agents.push(adapter);
        }
    }
    agents
}

/// New-config template. Empty upstreams and an unset Direct target form a valid
/// editing draft until the user chooses a provider-model pair. Tauri injects
/// runtime directories.
pub(crate) fn template(data_dir: &std::path::Path, plugins_dir: &std::path::Path) -> Value {
    let pricing = serde_json::to_value(PriceTable::builtin())
        .expect("the built-in price table always serializes");
    json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:8787", "auth": true },
        "data": { "dir": data_dir, "metrics": true },
        "plugins": {
            "dir": plugins_dir,
            "agents": desktop_agents(),
            "providers": { "openai-compatible": "provider-openai-compatible-v2" }
        },
        "upstreams": {},
        "pricing": pricing,
        "routing": {
            "mode": "direct"
        },
        "router": {
            "version": 1,
            "routing_mode": "tiered",
            "pools": {},
            "rules": [],
            "hint_routes": [],
            "default_pool": "",
            "assumed_context_window": 8192
        }
    })
}

pub(crate) fn seed_builtin_pricing(draft: &mut Value) -> Result<bool, String> {
    let current: PriceTable = draft
        .get("pricing")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("定价表配置不合法：{error}"))?
        .unwrap_or_default();
    if current.version != 0 || !current.models.is_empty() {
        return Ok(false);
    }
    draft["pricing"] =
        serde_json::to_value(PriceTable::builtin()).map_err(|error| error.to_string())?;
    Ok(true)
}

/// Upgrade a CLI-era single-Chat inbound config into the desktop three-inbound
/// draft and anchor relative runtime paths to the config directory. Change only
/// the in-memory draft until the user saves.
pub(crate) fn prepare_desktop_draft(mut draft: Value, config_dir: &std::path::Path) -> Value {
    let agents = draft["plugins"]["agents"].as_array();
    let legacy_alias = agents.is_none_or(Vec::is_empty)
        && draft["plugins"]["agent"].as_str() == Some("agent-openai");
    let legacy_desktop_list = agents.is_some_and(|agents| {
        agents.len() == 1
            && agents[0].as_str() == Some("agent-openai")
            && draft["plugins"]["agent"].is_null()
    });
    if legacy_alias || legacy_desktop_list {
        if let Some(plugins) = draft["plugins"].as_object_mut() {
            plugins.remove("agent");
        }
        draft["plugins"]["agents"] = json!(desktop_agents());
    }

    // Ensure agents contains every built-in connector adapter. Legacy configs
    // captured a fixed snapshot, so newer adapters such as agent-gemini would be
    // missing and their Agents rejected because the gateway did not load them.
    // Add missing adapters while preserving existing order and custom entries.
    if !draft["plugins"]["agents"].is_array() {
        draft["plugins"]["agents"] = json!([]);
    }
    if let Some(agents) = draft["plugins"]["agents"].as_array_mut() {
        for adapter in desktop_agents() {
            if !agents.iter().any(|value| value.as_str() == Some(adapter)) {
                agents.push(json!(adapter));
            }
        }
    }

    fn anchor(path: &mut Value, config_dir: &std::path::Path) {
        let Some(raw) = path.as_str() else {
            return;
        };
        let value = PathBuf::from(raw);
        if value.is_relative() {
            *path = json!(config_dir.join(value));
        }
    }
    anchor(&mut draft["plugins"]["dir"], config_dir);
    anchor(&mut draft["data"]["dir"], config_dir);

    // Capability migration: promote tool_state and json_schema_state from
    // unknown to declared. Early add_provider versions wrote unknown, while
    // tool routing fails closed and rejected every tool-using Agent. Catalog
    // entries are OpenAI-compatible chat providers whose contract includes tools
    // and structured output. Keep vision unchanged because it varies by model,
    // and never overwrite explicit unsupported or verified states.
    if let Some(upstreams) = draft["upstreams"].as_object_mut() {
        for upstream in upstreams.values_mut() {
            if upstream["access_tier"].as_str() == Some("free") {
                continue;
            }
            let Some(models) = upstream["models"].as_array_mut() else {
                continue;
            };
            for model in models.iter_mut() {
                if model["tool_state"] == json!("unknown") {
                    model["tool_state"] = json!("declared");
                    model["tool"] = json!(true);
                }
                if model["json_schema_state"] == json!("unknown") {
                    model["json_schema_state"] = json!("declared");
                    model["json_schema"] = json!(true);
                }
            }
            apply_builtin_model_limits_to_upstream(upstream);
        }
    }

    // Remove dangling references after provider or model deletion and migration.
    // Agent-specific routes and profiles created before validation may retain
    // missing targets. Reset them to unselected so the UI does not show stale
    // choices. Run only when upstreams is a valid object to avoid treating every
    // target in a damaged config as dangling.
    if draft["upstreams"].is_object() {
        let valid: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = draft
            ["upstreams"]
            .as_object()
            .map(|upstreams| {
                upstreams
                    .iter()
                    .map(|(name, upstream)| {
                        let models = upstream["models"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|model| model["model"].as_str().map(str::to_owned))
                            .collect();
                        (name.clone(), models)
                    })
                    .collect()
            })
            .unwrap_or_default();

        fn prune_dangling_tier(
            tier: &mut Value,
            valid: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        ) {
            let Some(upstream) = tier["upstream"].as_str().map(str::to_owned) else {
                return;
            };
            match valid.get(&upstream) {
                // The provider was removed; reset the entire tier to unselected.
                None => {
                    tier["upstream"] = Value::Null;
                    tier["model"] = Value::Null;
                }
                // The provider remains but the model was removed; keep the provider for reselection.
                Some(models) => {
                    if tier["model"]
                        .as_str()
                        .is_some_and(|model| !models.contains(model))
                    {
                        tier["model"] = Value::Null;
                    }
                }
            }
        }

        fn prune_dangling_direct_target(
            target: &mut Value,
            valid: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
            preserve_explicit_empty: bool,
        ) {
            if !target.is_object() {
                return;
            }
            prune_dangling_tier(target, valid);
            if target["upstream"].is_null() && !preserve_explicit_empty {
                *target = Value::Null;
            }
        }

        if let Some(routing) = draft.get_mut("routing").and_then(Value::as_object_mut) {
            if let Some(target) = routing.get_mut("direct_target") {
                prune_dangling_direct_target(target, &valid, false);
            }
        }
        if let Some(router) = draft.get_mut("router").and_then(Value::as_object_mut) {
            if let Some(accounts) = router
                .get_mut("quota_accounts")
                .and_then(Value::as_array_mut)
            {
                accounts.retain(|account| {
                    let Some(upstream) = account["upstream"].as_str() else {
                        return false;
                    };
                    let Some(model) = account["model"].as_str() else {
                        return false;
                    };
                    valid
                        .get(upstream)
                        .is_some_and(|models| models.contains(model))
                });
            }
        }

        if let Some(agent_routes) = draft.get_mut("agent_routes").and_then(Value::as_object_mut) {
            for route in agent_routes.values_mut() {
                if let Some(target) = route
                    .as_object_mut()
                    .and_then(|route| route.get_mut("direct_target"))
                {
                    // Keep an explicit empty object as a tombstone. `null`
                    // means "inherit Home" for an Agent, which would silently
                    // route traffic to a different target after deletion.
                    prune_dangling_direct_target(target, &valid, true);
                }
                for slot in ["high", "mid", "low"] {
                    if route["custom_route"][slot].is_object() {
                        prune_dangling_tier(&mut route["custom_route"][slot], &valid);
                    }
                }
            }
        }
        if let Some(profiles) = draft.get_mut("profiles").and_then(Value::as_object_mut) {
            for profile in profiles.values_mut() {
                for slot in ["high", "mid", "low"] {
                    if profile[slot].is_object() {
                        prune_dangling_tier(&mut profile[slot], &valid);
                    }
                }
            }
        }
    }

    draft
}

/// Validate a structurally sound configuration after removing only the known
/// stale provider/model references that the desktop editor can repair safely.
///
/// The returned configuration is an audit projection only: Direct modes whose
/// target was just removed are temporarily treated as Tiered so every unrelated
/// semantic constraint can still run. The projection is never shown, saved, or
/// served. The real draft keeps Direct selected with an empty target and remains
/// dirty until the operator chooses a replacement.
pub(crate) fn validate_dangling_route_recovery(config: &ClientConfig) -> Result<(), String> {
    let mut audit = config.clone();
    let valid: BTreeMap<String, BTreeSet<String>> = audit
        .upstreams
        .iter()
        .map(|(name, upstream)| {
            let models = upstream
                .models
                .iter()
                .map(|capability| capability.model.clone())
                .collect();
            (name.clone(), models)
        })
        .collect();
    let target_exists = |target: &UpstreamModel| {
        valid
            .get(target.upstream.as_str())
            .is_some_and(|models| models.contains(&target.model))
    };

    let mut repaired_references = 0_usize;
    let mut home_target_removed = false;
    if let Some(routing) = audit.routing.as_mut() {
        if routing
            .direct_target
            .as_ref()
            .is_some_and(|target| !target_exists(target))
        {
            routing.direct_target = None;
            home_target_removed = true;
            repaired_references += 1;
        }
    }

    let quota_before = audit.router.quota_accounts.len();
    audit
        .router
        .quota_accounts
        .retain(|target| target_exists(target));
    repaired_references += quota_before - audit.router.quota_accounts.len();

    let mut agent_targets_removed = BTreeSet::new();
    for (agent_id, route) in &mut audit.agent_routes {
        if route
            .direct_target
            .as_ref()
            .is_some_and(|target| !target_exists(target))
        {
            route.direct_target = None;
            agent_targets_removed.insert(agent_id.clone());
            repaired_references += 1;
        }
    }

    if repaired_references == 0 {
        return Err("配置不包含可安全清理的悬空 Direct 或额度路由引用".to_owned());
    }

    // A target that was present but became stale is an editable selection, not
    // permission to accept a Direct configuration that was already missing its
    // required target. Only suppress the derivative missing-target error when
    // this exact recovery removed the target.
    if home_target_removed && audit.effective_home_routing_mode() == HostRoutingMode::Direct {
        let routing = audit
            .routing
            .as_mut()
            .expect("only top-level routing can encode host Direct mode");
        routing.mode = HostRoutingMode::Tiered;
    }
    let home_mode = audit.effective_home_routing_mode();
    let home_target = audit.effective_home_direct_target().cloned();
    for (agent_id, route) in &mut audit.agent_routes {
        let effective_mode = route.routing_mode.unwrap_or(home_mode);
        let has_effective_target = route
            .direct_target
            .as_ref()
            .or(home_target.as_ref())
            .is_some();
        if effective_mode == HostRoutingMode::Direct
            && !has_effective_target
            && (home_target_removed || agent_targets_removed.contains(agent_id))
        {
            route.routing_mode = Some(HostRoutingMode::Tiered);
        }
    }

    audit.validate()
}

/// Existing configs must pass complete CLI loading, defaulting, and structural
/// validation. On failure, return a safe display template with a read-only gate
/// that blocks saving and starting to protect the damaged file.
#[cfg(test)]
pub(crate) fn load_draft(
    config_path: &std::path::Path,
    root: &std::path::Path,
) -> (Value, Option<String>) {
    let (draft, _saved, error) = load_draft_state(
        config_path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );
    (draft, error)
}

pub(crate) fn load_draft_state(
    config_path: &std::path::Path,
    data_dir: &std::path::Path,
    plugins_dir: &std::path::Path,
) -> (Value, Value, Option<String>) {
    if !config_path.exists() {
        let draft = template(data_dir, plugins_dir);
        return (draft.clone(), draft, None);
    }
    match ClientConfig::load(config_path) {
        Ok(config) => {
            let saved = serde_json::to_value(config).expect("ClientConfig always serializes");
            let config_dir = config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            (
                prepare_desktop_draft(saved.clone(), config_dir),
                saved,
                None,
            )
        }
        Err(error) => {
            let recovered = std::fs::read_to_string(config_path)
                .map_err(|read_error| read_error.to_string())
                .and_then(|source| {
                    ClientConfig::parse_with_load_migrations(&source)
                        .map_err(|parse_error| parse_error.to_string())
                })
                .and_then(|config| {
                    validate_dangling_route_recovery(&config)?;
                    Ok(config)
                });
            if let Ok(config) = recovered {
                let saved = serde_json::to_value(config).expect("ClientConfig always serializes");
                let config_dir = config_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                return (
                    prepare_desktop_draft(saved.clone(), config_dir),
                    saved,
                    None,
                );
            }

            let draft = template(data_dir, plugins_dir);
            (
                draft.clone(),
                draft,
                Some(format!(
                    "现有配置无法读取，已进入只读保护；请先修复或移走 {}：{error}",
                    config_path.display()
                )),
            )
        }
    }
}

/// Full ten-dimensional heuristic weights that make content-driven tiers effective even for short difficult prompts.
pub(crate) fn default_weights() -> Value {
    json!({
        "tokens_per_point": 100,
        "per_tool": 20,
        "json_schema": 10,
        "image": 15,
        "per_code_block": 8,
        "per_extra_turn": 3,
        "per_reasoning_marker": 10,
        "per_technical_term": 8,
        "per_code_keyword": 6,
        "per_math_term": 12,
        "per_creative_term": 6,
        "per_multi_step_point": 3,
        "per_question": 2,
        "system_format": 10,
        "per_simple_indicator": 8
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct SettingsCommandError {
    pub(crate) field: String,
    pub(crate) reason_code: String,
    pub(crate) message: String,
}

pub(crate) fn settings_error(
    field: impl Into<String>,
    reason_code: impl Into<String>,
    message: impl Into<String>,
) -> SettingsCommandError {
    SettingsCommandError {
        field: field.into(),
        reason_code: reason_code.into(),
        message: message.into(),
    }
}

// ---- helpers ------------------------------------------------------------------

impl AppInner {
    #[cfg(test)]
    pub(crate) fn new(config_path: PathBuf, draft: Value, load_error: Option<String>) -> Self {
        Self::new_with_saved(config_path, draft.clone(), draft, load_error)
    }

    pub(crate) fn new_with_saved(
        config_path: PathBuf,
        mut draft: Value,
        saved: Value,
        mut load_error: Option<String>,
    ) -> Self {
        let mut config_state = ConfigState::load(&config_path, &saved).unwrap_or_else(|error| {
            load_error
                .get_or_insert_with(|| format!("配置版本状态无法持久化，已进入只读保护：{error}"));
            ConfigState::read_only(&config_path, &saved)
        });
        if load_error.is_none() {
            if let Err(error) = config_state.observe_draft(&draft) {
                load_error = Some(format!("配置版本状态无法持久化，已进入只读保护：{error}"));
                draft = config_state.draft().clone();
            }
        }
        let south_approved_dialects = south_approved_dialects_for_draft(&draft);
        Self {
            config_path,
            draft,
            load_error,
            config_state,
            agent_route_drafts: BTreeMap::new(),
            server: ServerLifecycle::stopped(),
            pending_free_providers: BTreeSet::new(),
            pending_provider_keys: BTreeMap::new(),
            pending_provider_key_removals: BTreeSet::new(),
            pending_provider_discoveries: BTreeSet::new(),
            south_approved_dialects,
            upstream_epochs: BTreeMap::new(),
            discovery_generations: BTreeMap::new(),
        }
    }

    pub(crate) fn observe_draft(&mut self) -> Result<(), String> {
        let previous = self.config_state.draft().clone();
        let draft = self.draft.clone();
        if let Err(error) = self.config_state.observe_draft(&draft) {
            self.draft = self.config_state.draft().clone();
            return Err(error);
        }
        record_changed_upstream_epochs(&previous, &draft, &mut self.upstream_epochs);
        Ok(())
    }

    pub(crate) fn bump_upstream_epoch(&mut self, name: &str) {
        let epoch = self.upstream_epochs.entry(name.to_owned()).or_default();
        *epoch = epoch.saturating_add(1).max(1);
    }

    /// Build a candidate config under the lock and replace the authoritative draft
    /// only after materialization and revision recording succeed. The callback may
    /// edit only the config draft; update other AppInner state after commit.
    pub(crate) fn edit_validated_draft<T>(
        &mut self,
        edit: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.ensure_editable()?;
        let previous = self.draft.clone();
        let result = match edit(self) {
            Ok(result) => result,
            Err(error) => {
                self.draft = previous.clone();
                return Err(error);
            }
        };
        if let Err(error) = self.materialize() {
            self.draft = previous.clone();
            return Err(error);
        }
        let candidate = self.draft.clone();
        self.draft = previous.clone();
        let mut candidate_state = self.config_state.clone();
        candidate_state.observe_draft(&candidate)?;
        self.draft = candidate;
        self.config_state = candidate_state;
        record_changed_upstream_epochs(&previous, &self.draft, &mut self.upstream_epochs);
        Ok(result)
    }

    pub(crate) fn save_draft(&mut self) -> Result<u64, String> {
        self.ensure_editable()?;
        let config = self.materialize()?;
        let draft = self.draft.clone();
        let revision = self.config_state.prepare_save(&draft)?;
        let data_dir = self.data_dir();
        let mut applied_keys: Vec<(String, Option<String>)> = Vec::new();
        for (upstream, value) in &self.pending_provider_keys {
            let previous = secrets::store_get(&data_dir, upstream, "provider_api_key").ok();
            if let Err(error) = secrets::store_set(&data_dir, upstream, "provider_api_key", value) {
                for (applied, old_value) in applied_keys.iter().rev() {
                    restore_provider_key(
                        &data_dir,
                        applied,
                        "provider_api_key",
                        old_value.as_deref(),
                    )
                    .ok();
                }
                return Err(error);
            }
            applied_keys.push((upstream.clone(), previous));
        }
        if let Err(error) = config.save(&self.config_path) {
            let mut rollback_errors = Vec::new();
            for (upstream, previous) in applied_keys.iter().rev() {
                if let Err(rollback_error) = restore_provider_key(
                    &data_dir,
                    upstream,
                    "provider_api_key",
                    previous.as_deref(),
                ) {
                    rollback_errors.push(rollback_error);
                }
            }
            let mut message = format!("写配置失败: {error}");
            if !rollback_errors.is_empty() {
                message.push_str(&format!(
                    "；同时恢复 Provider 凭据失败：{}",
                    rollback_errors.join("；")
                ));
            }
            return Err(message);
        }
        let removable_keys = self
            .pending_provider_key_removals
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for upstream in removable_keys {
            match secrets::store_remove(&data_dir, &upstream, "provider_api_key") {
                Ok(()) => {
                    self.pending_provider_key_removals.remove(&upstream);
                    self.bump_upstream_epoch(&upstream);
                }
                Err(error) => {
                    eprintln!(
                        "configuration saved but legacy Provider credential cleanup failed for `{upstream}`: {error}"
                    );
                }
            }
        }
        if let Err(error) = self.config_state.finish_save(&draft) {
            // The config committed atomically. The pending journal will be
            // promoted on next startup, so do not misreport a trailing state-write
            // failure as a failed config save.
            eprintln!("configuration saved but revision finalization failed: {error}");
        }
        if let Err(error) =
            SqliteStore::backfill_unknown_costs(&data_dir.join("metrics.sqlite"), &config.pricing)
        {
            // The configuration is already atomically committed. Keep save
            // semantics truthful and retry this idempotent backfill on the next
            // save or startup instead of reporting a rollback that did not occur.
            eprintln!("configuration saved but historical cost backfill failed: {error}");
        }
        let committed_key_upstreams = self
            .pending_provider_keys
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for upstream in &committed_key_upstreams {
            if let Err(error) = provider_tombstones::discard(&self.data_dir(), upstream) {
                eprintln!(
                    "configuration saved but free Provider tombstone cleanup failed: {error}"
                );
            }
        }
        for upstream in committed_key_upstreams {
            // The credential value is absent from ClientConfig, so publishing
            // it must advance the same identity used by running/draft Gateway
            // reuse even when the Provider definition itself did not change.
            self.bump_upstream_epoch(&upstream);
        }
        self.pending_provider_keys.clear();
        Ok(revision)
    }

    pub(crate) fn ensure_editable(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub(crate) fn upstreams(&self) -> Vec<ProviderView> {
        let Some(map) = self.draft["upstreams"].as_object() else {
            return vec![];
        };
        map.iter()
            .map(|(name, up)| {
                let model_values = up["models"].as_array().cloned().unwrap_or_default();
                let models = model_values
                    .iter()
                    .filter_map(|model| model["model"].as_str().map(str::to_owned))
                    .collect();
                let configured_capabilities: Vec<ModelCapability> = model_values
                    .into_iter()
                    .filter_map(|model| serde_json::from_value::<ModelCapability>(model).ok())
                    .collect();
                let model_capabilities = configured_capabilities
                    .iter()
                    .map(|capability| {
                        let tool = capability.tool_state();
                        let vision = capability.vision_state();
                        let json_schema = capability.json_schema_state();
                        ModelCapabilityView {
                            model: capability.model.clone(),
                            tool,
                            vision,
                            json_schema,
                            context_window: capability.context_window,
                            max_output_tokens: capability.max_output_tokens,
                            context_window_source: model_limit_source(
                                capability,
                                CONTEXT_WINDOW_SOURCE_KEY,
                            ),
                            max_output_tokens_source: model_limit_source(
                                capability,
                                MAX_OUTPUT_TOKENS_SOURCE_KEY,
                            ),
                        }
                    })
                    .collect();
                let base_url = up["base_url"].as_str().unwrap_or_default().to_string();
                let access_tier = up
                    .get("access_tier")
                    .and_then(Value::as_str)
                    .unwrap_or("paid");
                let (catalog_revision, catalog) = model_catalog::catalog_for_provider(
                    &self.data_dir(),
                    name,
                    &base_url,
                    &configured_capabilities,
                );
                let package_verified = self
                    .south_approved_dialects
                    .contains(up["provider"].as_str().unwrap_or_default());
                let south_v1_unavailable_reason =
                    south_v1_unavailable_reason(&self.draft, up, package_verified);
                let south_header_auth_v1_unavailable_reason =
                    south_header_auth_v1_unavailable_reason(&self.draft, up, package_verified);
                ProviderView {
                    name: name.clone(),
                    brand_id: provider_brand_id(name, &base_url, access_tier),
                    provider: up["provider"].as_str().unwrap_or_default().to_string(),
                    base_url,
                    models,
                    model_capabilities,
                    catalog_revision,
                    catalog,
                    has_auth: up.get("auth").map(|a| !a.is_null()).unwrap_or(false),
                    credential_source: if up["auth"]["store"].as_bool() == Some(true) {
                        "store"
                    } else if up["auth"]["env"].is_string() {
                        "env"
                    } else if up["auth"]["file"].is_string() {
                        "file"
                    } else {
                        "none"
                    }
                    .to_owned(),
                    credential_reference: up["auth"]["env"]
                        .as_str()
                        .or_else(|| up["auth"]["file"].as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    provider_call: up
                        .get("provider_call")
                        .and_then(Value::as_str)
                        .unwrap_or(DEFAULT_PROVIDER_CALL)
                        .to_owned(),
                    south_v1_available: south_v1_unavailable_reason.is_none(),
                    south_v1_unavailable_reason,
                    south_header_auth_v1_available: south_header_auth_v1_unavailable_reason
                        .is_none(),
                    south_header_auth_v1_unavailable_reason,
                    local: up.get("local").and_then(Value::as_bool).unwrap_or(false),
                    managed_route: up
                        .get("managed_route")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    access_tier: access_tier.to_owned(),
                    quota_plan: up["quota_plan"]["windows"][0].as_object().map(|window| {
                        QuotaPlanView {
                            len_ms: window["len_ms"].as_u64().unwrap_or(0),
                            limit: window["limit"].as_u64().unwrap_or(0),
                            unit: up["quota_plan"]["unit"]
                                .as_str()
                                .unwrap_or("tokens")
                                .to_owned(),
                            rate_limit_per_min: up["quota_plan"]["rate_limit_per_min"].as_u64(),
                        }
                    }),
                }
            })
            .collect()
    }

    pub(crate) fn tier(&self, pool: &str) -> TierView {
        let member = self.draft["router"]["pools"][pool]
            .as_array()
            .and_then(|arr| arr.first());
        match member {
            Some(m) => TierView {
                upstream: m["upstream"].as_str().map(str::to_string),
                model: m["model"].as_str().map(str::to_string),
            },
            None => TierView {
                upstream: None,
                model: None,
            },
        }
    }

    pub(crate) fn home_tiers(&self) -> std::collections::BTreeMap<String, TierView> {
        let mut tiers = std::collections::BTreeMap::new();
        tiers.insert("high".to_string(), self.tier(TIER_HIGH));
        tiers.insert("mid".to_string(), self.tier(TIER_MID));
        tiers.insert("low".to_string(), self.tier(TIER_LOW));
        tiers
    }

    /// Whether a tier pool has members. Keywords require this or their rule would target an empty pool.
    pub(crate) fn pool_present(&self, pool: &str) -> bool {
        self.draft["router"]["pools"][pool]
            .as_array()
            .is_some_and(|members| !members.is_empty())
    }

    /// Read the current keywords_any list for a keyword-rule ID.
    pub(crate) fn rule_keywords(&self, rule_id: &str) -> Vec<String> {
        self.draft["router"]["rules"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|rule| rule["id"].as_str() == Some(rule_id))
            .and_then(|rule| rule["when"]["keywords_any"].as_array())
            .map(|words| {
                words
                    .iter()
                    .filter_map(|w| w.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keyword libraries for high, mid, and low tiers, exposed to the frontend.
    pub(crate) fn home_keywords(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        TIER_ORDER
            .iter()
            .map(|(slot, _pool, rule_id)| ((*slot).to_string(), self.rule_keywords(rule_id)))
            .collect()
    }

    /// Current mapping from tier slots to keyword lists, used as the pre-write snapshot.
    pub(crate) fn keyword_map(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        self.home_keywords()
    }

    /// Rewrite router.rules from the supplied tier-keyword map in high-to-low
    /// priority order. Emit rules only for tiers with keywords and configured
    /// pools. Preserve operator-authored non-keyword rules afterward. Empty or
    /// unconfigured tiers emit no rule, avoiding references to missing pools.
    pub(crate) fn apply_keyword_map(
        &mut self,
        map: &std::collections::BTreeMap<String, Vec<String>>,
    ) {
        let mut rules: Vec<Value> = Vec::new();
        for (slot, pool, rule_id) in TIER_ORDER {
            let words = map.get(slot).cloned().unwrap_or_default();
            if words.is_empty() || !self.pool_present(pool) {
                continue;
            }
            rules.push(json!({
                "id": rule_id,
                "when": { "keywords_any": words },
                "route_to": pool,
            }));
        }
        // Preserve existing rules not managed by this module and append them afterward.
        let managed = [KW_RULE_HIGH, KW_RULE_MID, KW_RULE_LOW];
        if let Some(existing) = self.draft["router"]["rules"].as_array() {
            for rule in existing {
                let is_managed = rule["id"].as_str().is_some_and(|id| managed.contains(&id));
                if !is_managed {
                    rules.push(rule.clone());
                }
            }
        }
        self.draft["router"]["rules"] = Value::Array(rules);
    }

    /// Normalize keywords by trimming whitespace. Deduplicate case-insensitively
    /// to match core keywords_any behavior while preserving original case for display.
    pub(crate) fn add_tier_keyword(&mut self, slot: &str, keyword: &str) -> Result<(), String> {
        let (pool, _rule_id) = tier_pool_and_rule(slot)?;
        if !self.pool_present(pool) {
            return Err("请先为该档配置供应商和模型,再添加关键词".to_string());
        }
        let word = keyword.trim();
        if word.is_empty() {
            return Err("关键词不能为空".to_string());
        }
        if word.chars().count() > 64 {
            return Err("单个关键词过长(最多 64 字)".to_string());
        }
        let mut map = self.keyword_map();
        let list = map.entry(slot.to_string()).or_default();
        if list
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(word))
        {
            return Err(format!("关键词「{word}」已在该档"));
        }
        if list.len() >= 100 {
            return Err("单档关键词过多(最多 100 个)".to_string());
        }
        list.push(word.to_string());
        self.apply_keyword_map(&map);
        Ok(())
    }

    pub(crate) fn remove_tier_keyword(&mut self, slot: &str, keyword: &str) -> Result<(), String> {
        tier_pool_and_rule(slot)?;
        let mut map = self.keyword_map();
        if let Some(list) = map.get_mut(slot) {
            list.retain(|existing| !existing.eq_ignore_ascii_case(keyword.trim()));
        }
        self.apply_keyword_map(&map);
        Ok(())
    }

    /// Remove a pool's keyword rule when clearing it so route_to cannot reference an empty pool.
    pub(crate) fn drop_keyword_rule_for_pool(&mut self, pool: &str) {
        let Some(rule_id) = TIER_ORDER
            .iter()
            .find(|(_, p, _)| *p == pool)
            .map(|(_, _, id)| *id)
        else {
            return;
        };
        if let Some(rules) = self.draft["router"]["rules"].as_array_mut() {
            rules.retain(|rule| rule["id"].as_str() != Some(rule_id));
        }
    }

    pub(crate) fn agent_route_mode(&self, agent_id: &str) -> &str {
        self.draft["agent_routes"][agent_id]["mode"]
            .as_str()
            .unwrap_or("inherit")
    }

    pub(crate) fn agent_route_view_mode(&self, agent_id: &str) -> &str {
        if self.agent_route_drafts.contains_key(agent_id) {
            "custom"
        } else {
            self.agent_route_mode(agent_id)
        }
    }

    pub(crate) fn agent_tier(&self, agent_id: &str, slot: &str) -> TierView {
        if let Some(tier) = self
            .agent_route_drafts
            .get(agent_id)
            .and_then(|tiers| tiers.get(slot))
        {
            return tier.clone();
        }
        let target = match self.agent_route_mode(agent_id) {
            "custom" => &self.draft["agent_routes"][agent_id]["custom_route"][slot],
            "profile" => {
                let name = self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .unwrap_or_default();
                &self.draft["profiles"][name][slot]
            }
            _ => return self.tier(pool_key(slot).expect("known UI tier slot")),
        };
        TierView {
            upstream: target["upstream"].as_str().map(str::to_string),
            model: target["model"].as_str().map(str::to_string),
        }
    }

    pub(crate) fn agent_profile(&self, agent_id: &str) -> Option<String> {
        (!self.agent_route_drafts.contains_key(agent_id)
            && self.agent_route_mode(agent_id) == "profile")
            .then(|| {
                self.draft["agent_routes"][agent_id]["profile"]
                    .as_str()
                    .map(str::to_string)
            })
            .flatten()
    }

    pub(crate) fn profile_names(&self) -> Vec<String> {
        self.draft["profiles"]
            .as_object()
            .map(|profiles| profiles.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn agent_routes_view(&self) -> std::collections::BTreeMap<String, AgentRouteView> {
        let home_mode = self.home_routing_mode();
        supported_agent_ids()
            .into_iter()
            .map(|agent_id| {
                let mode = self.agent_route_view_mode(&agent_id).to_string();
                let stored_route = &self.draft["agent_routes"][&agent_id];
                let inherits_global = !self.agent_route_drafts.contains_key(&agent_id)
                    && self.agent_route_mode(&agent_id) == "inherit"
                    && stored_route["routing_mode"].is_null()
                    && stored_route["direct_target"].is_null();
                let routing_mode = self.draft["agent_routes"][&agent_id]["routing_mode"]
                    .as_str()
                    .unwrap_or(home_mode)
                    .to_string();
                let tiers = ["high", "mid", "low"]
                    .into_iter()
                    .map(|slot| (slot.to_string(), self.agent_tier(&agent_id, slot)))
                    .collect();
                let direct_target = self.agent_direct_target_view(&agent_id);
                let direct_target_incomplete = direct_target
                    .as_ref()
                    .and_then(|target| target.model.as_ref())
                    .is_none();
                let config_error = if routing_mode == "direct" && direct_target_incomplete {
                    Some(format!("Agent `{agent_id}` 的单独路由缺少供应商和模型"))
                } else if mode == "custom" || mode == "profile" {
                    ["high", "mid", "low"].into_iter().find_map(|slot| {
                        let tier = self.agent_tier(&agent_id, slot);
                        (tier.upstream.is_none() || tier.model.is_none())
                            .then(|| format!("Agent `{agent_id}` 的 {slot} 档缺少供应商和模型"))
                    })
                } else {
                    None
                };
                (
                    agent_id.clone(),
                    AgentRouteView {
                        mode,
                        inherits_global,
                        tiers,
                        config_error,
                        profile: self.agent_profile(&agent_id),
                        routing_mode,
                        direct_target,
                    },
                )
            })
            .collect()
    }

    pub(crate) fn direct_target_view(value: &Value) -> Option<DirectTargetView> {
        Some(DirectTargetView {
            upstream: value["upstream"].as_str()?.to_owned(),
            model: value["model"].as_str().map(str::to_owned),
        })
    }

    pub(crate) fn home_direct_target_view(&self) -> Option<DirectTargetView> {
        Self::direct_target_view(&self.draft["routing"]["direct_target"])
    }

    pub(crate) fn home_routing_mode(&self) -> &str {
        self.draft["routing"]["mode"]
            .as_str()
            .or_else(|| self.draft["router"]["routing_mode"].as_str())
            .unwrap_or("tiered")
    }

    pub(crate) fn agent_direct_target_view(&self, agent_id: &str) -> Option<DirectTargetView> {
        let target = &self.draft["agent_routes"][agent_id]["direct_target"];
        if target.is_object() {
            Self::direct_target_view(target)
        } else {
            self.home_direct_target_view()
        }
    }

    pub(crate) fn quota_accounts_view(&self) -> Vec<QuotaAccountView> {
        self.draft["router"]["quota_accounts"]
            .as_array()
            .map(|accounts| {
                accounts
                    .iter()
                    .filter_map(|account| {
                        Some(QuotaAccountView {
                            upstream: account["upstream"].as_str()?.to_owned(),
                            model: account["model"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Rebuild tier-pool references, heuristic bands, and default from configured
    /// tiers. Include only tiers with a selected upstream-model pair.
    pub(crate) fn rebuild_routing(&mut self) {
        // Collect configured tiers from high to low.
        let present: Vec<(&str, u32)> =
            [(TIER_HIGH, CUT_HIGH), (TIER_MID, CUT_MID), (TIER_LOW, 0u32)]
                .into_iter()
                .filter(|(pool, _)| {
                    self.draft["router"]["pools"][*pool]
                        .as_array()
                        .map(|a| !a.is_empty())
                        .unwrap_or(false)
                })
                .collect();

        if present.is_empty() {
            // With no configured tiers, clear heuristic and default so saving reports the empty-pool error.
            self.draft["router"]["heuristic"] = Value::Null;
            self.draft["router"]["default_pool"] = json!("");
            return;
        }

        // `present` is high to low; force the last band's at_least to zero so no request is missed.
        let last = present.len() - 1;
        let bands: Vec<Value> = present
            .iter()
            .enumerate()
            .map(|(i, (pool, cut))| {
                let at_least = if i == last { 0 } else { *cut };
                json!({ "at_least": at_least, "pool": pool })
            })
            .collect();

        let highest = present.first().unwrap().0;
        let lowest = present.last().unwrap().0;

        self.draft["router"]["heuristic"] = json!({
            "weights": default_weights(),
            "threshold": CUT_MID,
            "above": highest,
            "below": lowest,
            "bands": bands
        });
        self.draft["router"]["default_pool"] = json!(lowest);
    }

    /// Materialize and validate the draft as ClientConfig, returning a human-readable error on failure.
    pub(crate) fn materialize(&self) -> Result<ClientConfig, String> {
        if let Some(upstreams) = self.draft["upstreams"].as_object() {
            for (name, provider) in upstreams {
                if provider["access_tier"].as_str() == Some("free") {
                    free_provider_catalog::validate_stored_provider(name, provider)?;
                }
            }
        }
        serde_json::from_value::<ClientConfig>(self.draft.clone())
            .map_err(|e| format!("配置结构不合法: {e}"))
    }

    pub(crate) fn config_error(&self) -> Option<String> {
        self.load_error.clone().or_else(|| self.materialize().err())
    }

    pub(crate) fn serve_view(&self) -> ServeView {
        let model_test_uses_running_gateway = self
            .materialize()
            .ok()
            .is_some_and(|config| reusable_model_test_server(self, &config).is_some());
        match &self.server {
            ServerLifecycle::Stopped { .. } => ServeView {
                phase: ServePhase::Stopped,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: self.draft["server"]["listen"]
                    .as_str()
                    .unwrap_or("127.0.0.1:8787")
                    .to_string(),
                virtual_key: None,
                error: None,
                model_test_uses_running_gateway,
            },
            ServerLifecycle::Starting { listen, .. } => ServeView {
                phase: ServePhase::Starting,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
                model_test_uses_running_gateway,
            },
            ServerLifecycle::Applying { old, .. } => {
                let alive = old.is_task_alive();
                let reachable = alive && old.listener_reachable();
                ServeView {
                    phase: ServePhase::Starting,
                    app_runtime: if alive {
                        AppRuntime::Running
                    } else {
                        AppRuntime::Stopped
                    },
                    listener_reachable: reachable,
                    agent_connected: false,
                    running_revision: alive.then(|| old.running_revision()),
                    instance_id: alive.then(|| old.instance_id().to_owned()),
                    listen: old.listen().to_owned(),
                    virtual_key: old.virtual_key().map(str::to_string),
                    error: None,
                    model_test_uses_running_gateway,
                }
            }
            ServerLifecycle::Stopping { listen, .. } => ServeView {
                phase: ServePhase::Stopping,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: None,
                model_test_uses_running_gateway,
            },
            ServerLifecycle::Running {
                server,
                apply_error,
                ..
            } => {
                let alive = server.is_task_alive();
                let reachable = alive && server.listener_reachable();
                ServeView {
                    phase: if alive {
                        ServePhase::Running
                    } else {
                        ServePhase::Error
                    },
                    app_runtime: if alive {
                        AppRuntime::Running
                    } else {
                        AppRuntime::Stopped
                    },
                    listener_reachable: reachable,
                    agent_connected: false,
                    running_revision: alive.then(|| server.running_revision()),
                    instance_id: alive.then(|| server.instance_id().to_owned()),
                    listen: server.listen().to_string(),
                    virtual_key: server.virtual_key().map(str::to_string),
                    error: if alive {
                        apply_error.clone()
                    } else {
                        Some("serve_task_exited: 代理任务已退出".to_owned())
                    },
                    model_test_uses_running_gateway,
                }
            }
            ServerLifecycle::Failed { listen, error, .. } => ServeView {
                phase: ServePhase::Error,
                app_runtime: AppRuntime::Stopped,
                listener_reachable: false,
                agent_connected: false,
                running_revision: None,
                instance_id: None,
                listen: listen.clone(),
                virtual_key: None,
                error: Some(error.clone()),
                model_test_uses_running_gateway,
            },
        }
    }

    pub(crate) fn snapshot(&self) -> StateView {
        let (deleted_providers, provider_recovery_error) =
            match provider_tombstones::list(&self.data_dir()) {
                Ok(providers) => (providers, None),
                Err(error) => (Vec::new(), Some(error)),
            };
        StateView {
            providers: self.upstreams(),
            deleted_providers,
            provider_recovery_error,
            tiers: self.home_tiers(),
            agent_routes: self.agent_routes_view(),
            profiles: self.profile_names(),
            keywords: self.home_keywords(),
            local_only: self.draft["router"]["local_only"]
                .as_bool()
                .unwrap_or(false),
            allow_cloud_fallback: self.draft["router"]["allow_cloud_fallback"]
                .as_bool()
                .unwrap_or(false),
            routing_mode: self.home_routing_mode().to_string(),
            direct_target: self.home_direct_target_view(),
            quota_accounts: self.quota_accounts_view(),
            serve: self.serve_view(),
            draft_revision: self.config_state.draft_revision(),
            saved_revision: self.config_state.saved_revision(),
            config_dirty: self.config_state.is_dirty(),
            config_error: self.config_error(),
            settings: self.settings_view(),
        }
    }

    pub(crate) fn settings_view(&self) -> SettingsView {
        let d = &self.draft;
        SettingsView {
            listen: d["server"]["listen"]
                .as_str()
                .unwrap_or("127.0.0.1:8787")
                .to_string(),
            auth: d["server"]["auth"].as_bool().unwrap_or(true),
            metrics: d["data"]["metrics"].as_bool().unwrap_or(true),
            data_dir: d["data"]["dir"].as_str().unwrap_or_default().to_string(),
            plugins_dir: d["plugins"]["dir"].as_str().unwrap_or_default().to_string(),
            agent: agents_display(&d["plugins"]),
            version: env!("CARGO_PKG_VERSION").to_string(),
            desktop_version: env!("CARGO_PKG_VERSION").to_string(),
            core_version: upgrade::CURRENT_VERSION.to_string(),
            egress_mode: d["egress"]["mode"].as_str().unwrap_or("direct").to_string(),
            egress_proxy_url: d["egress"]["proxy_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            egress_no_proxy: d["egress"]["no_proxy"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            egress_auth_username: d["egress"]["auth"]["username"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            egress_auth_slot: d["egress"]["auth"]["credential"]["slot"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Absolute data directory from the draft, anchoring stats, receipts, and plugins.
    pub(crate) fn data_dir(&self) -> PathBuf {
        PathBuf::from(self.draft["data"]["dir"].as_str().unwrap_or_default())
    }

    /// Resolve a pool's first member as provider and model for routing-table and band display.
    pub(crate) fn pool_member(&self, pool: &str) -> (Option<String>, Option<String>) {
        let m = self.draft["router"]["pools"][pool]
            .as_array()
            .and_then(|a| a.first());
        match m {
            Some(m) => (
                m["upstream"].as_str().map(str::to_string),
                m["model"].as_str().map(str::to_string),
            ),
            None => (None, None),
        }
    }

    pub(crate) fn set_tier_value(
        &mut self,
        pool: &str,
        upstream: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        match (upstream, model) {
            (Some(upstream), Some(model)) => {
                let configured = self.draft["upstreams"][&upstream]
                    .as_object()
                    .ok_or_else(|| format!("未知供应商 `{upstream}`"))?;
                let model_exists = configured["models"].as_array().is_some_and(|models| {
                    models
                        .iter()
                        .any(|entry| entry["model"].as_str() == Some(model.as_str()))
                });
                if !model_exists {
                    return Err(format!("供应商 `{upstream}` 未配置模型 `{model}`"));
                }
                self.draft["router"]["pools"][pool] =
                    json!([{ "upstream": upstream, "model": model }]);
            }
            (None, None) => {
                if let Some(pools) = self.draft["router"]["pools"].as_object_mut() {
                    pools.remove(pool);
                }
                // Remove the keyword rule with the pool so it cannot target an empty pool and break saving.
                self.drop_keyword_rule_for_pool(pool);
            }
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_string()),
        }
        self.rebuild_routing();
        Ok(())
    }

    pub(crate) fn validate_route_target(&self, upstream: &str, model: &str) -> Result<(), String> {
        let configured = self.draft["upstreams"][upstream]
            .as_object()
            .ok_or_else(|| format!("未知供应商 `{upstream}`"))?;
        let model_exists = configured["models"].as_array().is_some_and(|models| {
            models
                .iter()
                .any(|entry| entry["model"].as_str() == Some(model))
        });
        model_exists
            .then_some(())
            .ok_or_else(|| format!("供应商 `{upstream}` 未配置模型 `{model}`"))
    }

    pub(crate) fn begin_agent_route_draft(&mut self, agent_id: &str) {
        if self.agent_route_drafts.contains_key(agent_id) {
            return;
        }
        let stored = &self.draft["agent_routes"][agent_id]["custom_route"];
        let tiers = ["high", "mid", "low"]
            .into_iter()
            .map(|slot| {
                let target = &stored[slot];
                let tier = if stored.is_object() {
                    TierView {
                        upstream: target["upstream"].as_str().map(str::to_owned),
                        model: target["model"].as_str().map(str::to_owned),
                    }
                } else {
                    self.tier(pool_key(slot).expect("known UI tier slot"))
                };
                (slot.to_owned(), tier)
            })
            .collect();
        self.agent_route_drafts.insert(agent_id.to_owned(), tiers);
    }

    pub(crate) fn set_agent_route_draft_tier(
        &mut self,
        agent_id: &str,
        slot: &str,
        upstream: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        pool_key(slot)?;
        let tier = match (upstream, model) {
            (Some(upstream), Some(model)) => {
                self.validate_route_target(&upstream, &model)?;
                TierView {
                    upstream: Some(upstream),
                    model: Some(model),
                }
            }
            (None, None) => TierView {
                upstream: None,
                model: None,
            },
            _ => return Err("档位必须同时提供供应商和模型，或同时清空".to_owned()),
        };
        self.begin_agent_route_draft(agent_id);
        self.agent_route_drafts
            .get_mut(agent_id)
            .expect("route draft was just initialized")
            .insert(slot.to_owned(), tier);
        Ok(())
    }

    pub(crate) fn complete_agent_route_draft(
        agent_id: &str,
        tiers: &BTreeMap<String, TierView>,
    ) -> Result<Value, String> {
        let mut route = serde_json::Map::new();
        for slot in ["high", "mid", "low"] {
            let tier = tiers.get(slot);
            let upstream = tier.and_then(|tier| tier.upstream.as_deref());
            let model = tier.and_then(|tier| tier.model.as_deref());
            let target = match (upstream, model) {
                (Some(upstream), Some(model)) => {
                    json!({ "upstream": upstream, "model": model })
                }
                (None, None) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少供应商和模型"));
                }
                (None, Some(_)) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少供应商"));
                }
                (Some(_), None) => {
                    return Err(format!("Agent `{agent_id}` 的 {slot} 档缺少模型"));
                }
            };
            route.insert(slot.to_owned(), target);
        }
        Ok(Value::Object(route))
    }

    pub(crate) fn promote_agent_route_drafts(&mut self) -> Result<(), String> {
        let routes: BTreeMap<String, Value> = self
            .agent_route_drafts
            .iter()
            .map(|(agent_id, tiers)| {
                Self::complete_agent_route_draft(agent_id, tiers)
                    .map(|route| (agent_id.clone(), route))
            })
            .collect::<Result<_, _>>()?;
        if routes.is_empty() {
            return Ok(());
        }
        self.edit_validated_draft(|inner| {
            if !inner.draft["agent_routes"].is_object() {
                inner.draft["agent_routes"] = json!({});
            }
            for (agent_id, route) in &routes {
                if !inner.draft["agent_routes"][agent_id].is_object() {
                    inner.draft["agent_routes"][agent_id] = json!({});
                }
                if let Some(agent_route) = inner.draft["agent_routes"][agent_id].as_object_mut() {
                    agent_route.remove("profile");
                }
                inner.draft["agent_routes"][agent_id]["mode"] = json!("custom");
                inner.draft["agent_routes"][agent_id]["custom_route"] = route.clone();
            }
            Ok(())
        })
    }

    pub(crate) fn set_agent_inherit_value(&mut self, agent_id: &str) {
        if !self.draft["agent_routes"].is_object() {
            self.draft["agent_routes"] = json!({});
        }
        if !self.draft["agent_routes"][agent_id].is_object() {
            self.draft["agent_routes"][agent_id] = json!({});
        }
        if let Some(route) = self.draft["agent_routes"][agent_id].as_object_mut() {
            route.remove("profile");
            route.remove("routing_mode");
            route.remove("direct_target");
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!("inherit");
    }

    pub(crate) fn save_home_route_as_profile_value(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err("策略组名称无效".to_string());
        }
        let mut tiers = serde_json::Map::new();
        for (slot, pool) in [("high", TIER_HIGH), ("mid", TIER_MID), ("low", TIER_LOW)] {
            let tier = self.tier(pool);
            let (Some(upstream), Some(model)) = (tier.upstream, tier.model) else {
                return Err(format!("{slot} 档尚未配置，无法另存为策略组"));
            };
            self.validate_route_target(&upstream, &model)?;
            tiers.insert(
                slot.to_string(),
                json!({ "upstream": upstream, "model": model }),
            );
        }
        if !self.draft["profiles"].is_object() {
            self.draft["profiles"] = json!({});
        }
        self.draft["profiles"][name] = Value::Object(tiers);
        Ok(())
    }

    pub(crate) fn mount_agent_profile_value(
        &mut self,
        agent_id: &str,
        profile: &str,
    ) -> Result<(), String> {
        ensure_known_agent_id(agent_id)?;
        if !self.draft["profiles"][profile].is_object() {
            return Err(format!("策略组 `{profile}` 不存在"));
        }
        if !self.draft["agent_routes"][agent_id].is_object() {
            self.draft["agent_routes"][agent_id] = json!({});
        }
        self.draft["agent_routes"][agent_id]["mode"] = json!("profile");
        self.draft["agent_routes"][agent_id]["profile"] = json!(profile);
        Ok(())
    }

    pub(crate) fn delete_profile_value(&mut self, name: &str) -> Result<(), String> {
        let mounted: Vec<_> = supported_agent_ids()
            .into_iter()
            .filter(|agent_id| self.agent_profile(agent_id).as_deref() == Some(name))
            .collect();
        if !mounted.is_empty() {
            return Err(format!("策略组 `{name}` 仍被挂载：{}", mounted.join(", ")));
        }
        let profiles = self.draft["profiles"]
            .as_object_mut()
            .ok_or_else(|| format!("策略组 `{name}` 不存在"))?;
        if profiles.remove(name).is_none() {
            return Err(format!("策略组 `{name}` 不存在"));
        }
        Ok(())
    }
}

/// Validate and write atomically. Return validation errors without writing, matching config edit semantics.
#[tauri::command]
pub(crate) fn save_config(state: State<'_, AppStateManaged>) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let tiered = inner.home_routing_mode() == "tiered";
    if tiered
        && inner.draft["router"]["pools"]
            .as_object()
            .map(|p| p.is_empty())
            .unwrap_or(true)
    {
        return Err("请至少配置一档(供应商 + 模型)再保存".into());
    }
    inner.save_draft()?;
    Ok(inner.snapshot())
}

/// Display inbound adapters from the comma-joined agents list, falling back to the single agent value.
pub(crate) fn agents_display(plugins: &Value) -> String {
    let list: Vec<&str> = plugins["agents"]
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if list.is_empty() {
        plugins["agent"].as_str().unwrap_or_default().to_string()
    } else {
        list.join(", ")
    }
}

// ---- Subpage commands (#5) -------------------------------------------------

pub(crate) fn classify_settings_error(message: String) -> SettingsCommandError {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("proxy")
        || normalized.contains("egress")
        || normalized.contains("socks")
        || normalized.contains("url")
    {
        settings_error(
            "egress_proxy_url",
            "invalid_proxy_url",
            "代理地址无效；HTTP 代理请使用 http:// 或 https://，SOCKS5 请使用 socks5:// 或 socks5h://",
        )
    } else {
        settings_error("settings", "validation_failed", message)
    }
}

/// Settings page command for server.auth and data.metrics. Persist after
/// successful materialization, matching config set; otherwise keep draft-only
/// changes until a full save. Running serve instances require a proxy restart.
#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "Tauri maps this stable command boundary to named frontend arguments"
)]
pub(crate) fn set_settings(
    state: State<'_, AppStateManaged>,
    auth: bool,
    metrics: bool,
    egress_mode: String,
    egress_proxy_url: String,
    egress_no_proxy: Vec<String>,
    egress_auth_username: String,
    egress_auth_slot: String,
) -> Result<StateView, SettingsCommandError> {
    let mut inner = state.0.lock().unwrap();
    inner
        .ensure_editable()
        .map_err(|message| settings_error("settings", "read_only", message))?;
    let previous_draft = inner.draft.clone();
    let previous_config_state = inner.config_state.clone();
    let edit_result = inner.edit_validated_draft(|candidate| {
        candidate.draft["server"]["auth"] = json!(auth);
        candidate.draft["data"]["metrics"] = json!(metrics);
        candidate.draft["egress"] = if egress_mode == "direct" {
            json!({ "mode": "direct" })
        } else {
            let mut egress = json!({
                "mode": egress_mode,
                "proxy_url": egress_proxy_url,
                "no_proxy": egress_no_proxy,
            });
            if !egress_auth_username.is_empty() || !egress_auth_slot.is_empty() {
                egress["auth"] = json!({
                    "username": egress_auth_username,
                    "credential": { "slot": egress_auth_slot, "store": true }
                });
            }
            egress
        };
        candidate.materialize()?.validate()?;
        Ok(())
    });
    if let Err(message) = edit_result {
        return Err(classify_settings_error(message));
    }
    if let Err(message) = inner.save_draft() {
        inner.draft = previous_draft;
        inner.config_state = previous_config_state;
        return Err(settings_error("settings", "save_failed", message));
    }
    Ok(inner.snapshot())
}

#[tauri::command]
pub(crate) fn get_egress(state: State<'_, AppStateManaged>) -> Result<Value, String> {
    let inner = state.0.lock().unwrap();
    let config = inner.materialize()?;
    let mut routes = Vec::new();
    for (upstream, entry) in &config.upstreams {
        let target = String::from(entry.base_url.clone());
        let bypassed = config.egress.bypasses_proxy(&target)?;
        let route =
            if config.egress.mode == token_station_cli::config::EgressMode::Direct || bypassed {
                "direct"
            } else {
                "proxy"
            };
        for request_class in ["provider_request", "model_catalog", "health_probe"] {
            routes.push(json!({
                "request_class": request_class,
                "upstream": upstream,
                "target": target,
                "route": route,
                "matched_no_proxy": bypassed && config.egress.mode != token_station_cli::config::EgressMode::Direct,
            }));
        }
    }
    Ok(json!({
        "mode": config.egress.mode,
        "proxy_url": config.egress.proxy_url,
        "no_proxy": config.egress.no_proxy,
        "auth_slot": config.egress.auth.map(|auth| auth.credential.slot),
        "routes": routes,
        "fixed_direct_classes": ["update_check"],
    }))
}
