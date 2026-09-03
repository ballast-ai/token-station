//! Structured Tauri IPC for Agent discovery and configuration transactions.
//!
//! Client input is deliberately limited to opaque IDs, a path that must match
//! the latest server-side scan exactly, and a short-lived confirmation token.
//! Target paths, patches, configuration bytes and executable commands are
//! never accepted from the renderer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use ring::hmac;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State, WebviewWindow};
use zeroize::Zeroizing;

use super::compatibility::{evaluate_discovery, CatalogSource, CompatibilityCatalog};
use super::config_codec::{
    apply_patch, parse_rendered, parse_source_bytes, prepare_owned_paths_for_write,
    render_document, DocumentFormat,
};
use super::connectors::{
    builtin_connectors, find_connector, owned_paths_with_legacy, validate_patch_ownership,
    AgentModelCost, AgentModelMetadata, ConnectInput, Connector,
};
use super::discovery::DiscoveryScanner;
use super::drift::analyze_drift;
use super::ownership::{FileOwnershipStore, OwnershipStore};
use super::plan::{
    attach_disconnect_companions, attach_restore_companions, build_connection_plan,
    build_disconnect_plan, build_metadata_refresh_plan, build_metadata_refresh_plan_with_baseline,
    build_snapshot_restore_plan, companion_document_format, generate_operation_id,
    read_config_source, ConfigSource, PreparedChangePlan, COMPANION_OWNED_VALUES_CHANGED,
    OWNED_VALUES_CHANGED,
};
use super::registry::AgentRegistry;
use super::snapshot::{FileMasterKeyStore, FileSnapshotStore, MasterKeyStore, SnapshotStore};
use super::transaction::{
    AtomicConfigWriter, Clock, ConfirmedOperation, FsAtomicConfigWriter, ParseOnlyVerifier,
    RecoveryStatus, RuntimeAdmission, SystemClock, TransactionCoordinator, TransactionEngine,
    TransactionFailure, TransactionOutcome, TransactionStage,
};
use super::types::{
    AgentDriftView, AgentUiMetadata, CompatibilityDecision, CompatibilityStatus, ConfigChangePlan,
    ConfigPath, Diagnostic, DiscoveryRecord, DriftStatus, PatchOperation, PlanIntent, ReasonCode,
    SnapshotRecord,
};
use crate::{AgentIntegrationPaths, AppStateManaged};

const MAX_PENDING_PLANS: usize = 64;
const MAX_SESSION_LABEL_BYTES: usize = 128;
const CONFIRMATION_TOKEN_BYTES: usize = 32;

#[tauri::command]
pub(crate) fn get_agent_backup_directory(paths: State<'_, AgentIntegrationPaths>) -> String {
    paths.snapshot_root.display().to_string()
}

#[tauri::command]
pub(crate) fn open_agent_backup_directory(
    paths: State<'_, AgentIntegrationPaths>,
) -> Result<String, String> {
    super::safe_fs::ensure_private_dir(&paths.snapshot_root)
        .map_err(|error| format!("创建 Agent 备份目录失败：{error}"))?;
    tauri_plugin_opener::open_path(&paths.snapshot_root, None::<&str>)
        .map_err(|error| format!("打开 Agent 备份目录失败：{error}"))?;
    Ok(paths.snapshot_root.display().to_string())
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInstallationView {
    pub discovery: DiscoveryRecord,
    pub compatibility: CompatibilityDecision,
    pub adapter_ready: Option<bool>,
    pub connection_issue: Option<AgentConnectionIssueView>,
    pub managed: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConnectionIssueView {
    pub code: String,
    pub message: String,
    pub target: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentView {
    pub metadata: AgentUiMetadata,
    pub installations: Vec<AgentInstallationView>,
    pub status: CompatibilityStatus,
    pub catalog_sequence: u64,
    pub catalog_expires_at_ms: Option<u64>,
    pub catalog_source: &'static str,
    pub catalog_warning: Option<String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPlanView {
    #[serde(flatten)]
    pub plan: ConfigChangePlan,
    pub confirmation_token: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSourceView {
    Encrypted,
    LegacyBackup,
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotView {
    pub snapshot_id: String,
    pub agent_id: String,
    pub target_config_path: String,
    pub created_at_ms: u64,
    pub connector_id: String,
    pub app_version: String,
    pub original_existed: bool,
    pub pinned: bool,
    pub source: SnapshotSourceView,
    pub restorable: bool,
    pub maintenance_warning: Option<String>,
}

impl From<SnapshotRecord> for SnapshotView {
    fn from(record: SnapshotRecord) -> Self {
        Self {
            snapshot_id: record.snapshot_id,
            agent_id: record.agent_id,
            target_config_path: record.target_config_path,
            created_at_ms: record.created_at_ms,
            connector_id: record.connector_id,
            app_version: record.app_version,
            original_existed: record.original_existed,
            pinned: record.pinned,
            source: SnapshotSourceView::Encrypted,
            restorable: true,
            maintenance_warning: None,
        }
    }
}

fn legacy_backup_path(target: &Path) -> PathBuf {
    let mut backup = target.to_path_buf();
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.token-station.bak"))
        .unwrap_or_else(|| "token-station.bak".to_string());
    backup.set_extension(extension);
    backup
}

fn legacy_snapshot_view(agent_id: &str, target: &Path) -> Option<SnapshotView> {
    let backup = legacy_backup_path(target);
    let metadata = std::fs::symlink_metadata(&backup).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let created_at_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .unwrap_or(0);
    let mut hash = Sha256::new();
    hash.update(b"token-station-legacy-backup-view-v1\0");
    hash_field(&mut hash, agent_id.as_bytes());
    hash_field(&mut hash, target.as_os_str().as_encoded_bytes());
    Some(SnapshotView {
        snapshot_id: format!("legacy-{}", lower_hex(hash.finalize().as_ref())),
        agent_id: agent_id.to_string(),
        target_config_path: target.to_string_lossy().into_owned(),
        created_at_ms,
        connector_id: "legacy-read-only".to_string(),
        app_version: "旧版备份".to_string(),
        original_existed: true,
        pinned: false,
        source: SnapshotSourceView::LegacyBackup,
        restorable: false,
        maintenance_warning: None,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandError {
    pub code: String,
    pub message: String,
    pub target: Option<String>,
    pub stage: Option<TransactionStage>,
    pub recovery: Option<RecoveryStatus>,
    pub recovery_reason_code: Option<String>,
}

impl AgentCommandError {
    fn boundary(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            target: None,
            stage: None,
            recovery: None,
            recovery_reason_code: None,
        }
    }

    fn internal(message: String) -> Self {
        Self::boundary("agent_operation_rejected", message)
    }

    fn plan(message: String) -> Self {
        match message.as_str() {
            OWNED_VALUES_CHANGED => Self::boundary(
                OWNED_VALUES_CHANGED,
                "受管配置已被其他工具修改；本次未写入，请重新扫描并确认",
            ),
            COMPANION_OWNED_VALUES_CHANGED => Self::boundary(
                COMPANION_OWNED_VALUES_CHANGED,
                "受管关联配置已被其他工具修改；本次未写入，请重新扫描并确认",
            ),
            _ => Self::internal(message),
        }
    }

    fn connection_issue(issue: &AgentConnectionIssueView) -> Self {
        Self {
            code: issue.code.clone(),
            message: issue.message.clone(),
            target: issue.target.clone(),
            stage: None,
            recovery: None,
            recovery_reason_code: None,
        }
    }
}

impl From<TransactionFailure> for AgentCommandError {
    fn from(value: TransactionFailure) -> Self {
        Self {
            code: value.reason_code.clone(),
            message: "Agent 配置事务未完成，请重新扫描并预览".to_string(),
            target: None,
            stage: Some(value.stage),
            recovery: Some(value.recovery),
            recovery_reason_code: value.recovery_reason_code,
        }
    }
}

#[derive(Clone)]
struct ScanSnapshot {
    catalog: CompatibilityCatalog,
    source: CatalogSource,
    warning: Option<String>,
    records: Vec<DiscoveryRecord>,
}

fn exact_installation_selection(record: &DiscoveryRecord) -> DiscoveryRecord {
    let mut selected = record.clone();
    selected.conflict_group = None;
    selected
}

impl ScanSnapshot {
    fn selected(
        &self,
        registry: &AgentRegistry,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<(DiscoveryRecord, CompatibilityDecision), AgentCommandError> {
        validate_short_identifier(agent_id, "agent_id")?;
        validate_installation_lookup(installation_path)?;
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == agent_id)
            .ok_or_else(|| AgentCommandError::boundary("unknown_agent", "未知 Agent"))?;
        let matches: Vec<_> = self
            .records
            .iter()
            .filter(|record| {
                record.agent_id == agent_id && record.canonical_path == installation_path
            })
            .cloned()
            .collect();
        if matches.len() != 1 {
            return Err(AgentCommandError::boundary(
                "unknown_or_stale_installation",
                "安装实例不属于最近一次服务端扫描，请重新扫描",
            ));
        }
        let selected = matches.into_iter().next().expect("length checked");
        // Every row in a multi-install scan carries the conflict group. Exact
        // lookup above is the only trusted transition to a selected instance.
        let selected = exact_installation_selection(&selected);
        let decision = evaluate_discovery(&self.catalog, descriptor, &selected);
        Ok((selected, decision))
    }
}

#[derive(Clone, Eq, PartialEq)]
struct DiscoveryBinding {
    agent_id: String,
    canonical_path: String,
    version_normalized: Option<String>,
    config_fingerprint: Option<String>,
    runnable: bool,
}

impl From<&DiscoveryRecord> for DiscoveryBinding {
    fn from(value: &DiscoveryRecord) -> Self {
        Self {
            agent_id: value.agent_id.clone(),
            canonical_path: value.canonical_path.clone(),
            version_normalized: value.version_normalized.clone(),
            config_fingerprint: value.config_fingerprint.clone(),
            runnable: value.runnable,
        }
    }
}

struct StoredPlan {
    prepared: PreparedChangePlan,
    confirmation_tag: [u8; 32],
    view_hash: [u8; 32],
    session_label: String,
    discovery: DiscoveryBinding,
    proxy_binding: Option<[u8; 32]>,
}

struct TakenPlan {
    prepared: PreparedChangePlan,
    discovery: DiscoveryBinding,
    proxy_binding: Option<[u8; 32]>,
}

#[derive(Default)]
struct CommandSession {
    scan: Option<ScanSnapshot>,
    plans: HashMap<String, StoredPlan>,
}

pub struct AgentProxyRuntime {
    instance_id: String,
    connector_base_urls: BTreeMap<String, String>,
    connector_adapter_ready: BTreeMap<String, bool>,
    virtual_key: Zeroizing<String>,
    model_metadata: BTreeMap<String, AgentModelMetadata>,
    connection_issues: BTreeMap<String, AgentConnectionIssueView>,
}

impl AgentProxyRuntime {
    fn new(
        instance_id: String,
        gateway_origin: &str,
        virtual_key: String,
        adapter_readiness: BTreeMap<String, bool>,
        model_metadata: BTreeMap<String, AgentModelMetadata>,
        connection_issues: BTreeMap<String, AgentConnectionIssueView>,
    ) -> Self {
        let mut connector_base_urls = BTreeMap::new();
        let mut connector_adapter_ready = BTreeMap::new();
        for connector in builtin_connectors() {
            let capability = connector.capabilities();
            let suffix = match capability.base_url_shape {
                super::types::BaseUrlShape::Origin => "",
                super::types::BaseUrlShape::OriginV1 => "/v1",
            };
            connector_base_urls.insert(
                capability.connector_id.to_string(),
                format!("{gateway_origin}/agents/{}{suffix}", capability.agent_id),
            );
            let ready = adapter_readiness
                .get(capability.adapter_id)
                .copied()
                .unwrap_or(false);
            connector_adapter_ready.insert(capability.connector_id.to_string(), ready);
        }
        Self {
            instance_id,
            connector_base_urls,
            connector_adapter_ready,
            virtual_key: Zeroizing::new(virtual_key),
            model_metadata,
            connection_issues,
        }
    }

    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn virtual_key(&self) -> &str {
        self.virtual_key.as_str()
    }

    pub(crate) fn gateway_origin(&self) -> Result<String, AgentCommandError> {
        let base =
            self.connector_base_urls.values().next().ok_or_else(|| {
                AgentCommandError::boundary("proxy_not_running", "代理地址不可用")
            })?;
        let marker = "/agents/";
        let origin = base
            .split_once(marker)
            .map(|(origin, _)| origin)
            .unwrap_or(base)
            .to_string();
        Ok(origin)
    }

    fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"token-station-agent-proxy-binding-v1\0");
        hash_field(&mut hash, self.instance_id.as_bytes());
        for (connector_id, base_url) in &self.connector_base_urls {
            hash_field(&mut hash, connector_id.as_bytes());
            hash_field(&mut hash, base_url.as_bytes());
        }
        hash_field(&mut hash, self.virtual_key.as_bytes());
        for (connector_id, ready) in &self.connector_adapter_ready {
            hash_field(&mut hash, connector_id.as_bytes());
            hash.update([u8::from(*ready)]);
        }
        for (agent_id, metadata) in &self.model_metadata {
            hash_field(&mut hash, agent_id.as_bytes());
            let serialized =
                serde_json::to_vec(metadata).expect("Agent model metadata is always serializable");
            hash_field(&mut hash, &serialized);
        }
        for (connector_id, issue) in &self.connection_issues {
            hash_field(&mut hash, connector_id.as_bytes());
            let serialized =
                serde_json::to_vec(issue).expect("Agent connection issue is always serializable");
            hash_field(&mut hash, &serialized);
        }
        hash.finalize().into()
    }

    fn input_for<'a>(&'a self, connector_id: &str) -> Result<ConnectInput<'a>, AgentCommandError> {
        let connector = find_connector(connector_id).ok_or_else(|| {
            AgentCommandError::boundary("unsupported_connector", "兼容目录引用了未实现的 Connector")
        })?;
        let base_url = self.connector_base_urls.get(connector_id).ok_or_else(|| {
            AgentCommandError::boundary("unsupported_connector", "Connector 缺少运行时 URL 投影")
        })?;
        Ok(ConnectInput {
            base_url,
            token: connector
                .capabilities()
                .requires_virtual_key
                .then_some(self.virtual_key.as_str()),
            adapter_ready: self
                .connector_adapter_ready
                .get(connector_id)
                .copied()
                .unwrap_or(false),
            model_metadata: self.model_metadata.get(connector.agent_id()),
        })
    }

    fn connection_issue(&self, connector_id: &str) -> Option<&AgentConnectionIssueView> {
        self.connection_issues.get(connector_id)
    }
}

#[cfg(test)]
fn agent_model_metadata(
    config: &token_station_cli::config::ClientConfig,
    agent_id: &str,
) -> Result<Option<AgentModelMetadata>, String> {
    let router = configured_router_for_agent(config, agent_id)?;
    agent_model_metadata_for_router(config, &router)
}

fn configured_router_for_agent(
    config: &token_station_cli::config::ClientConfig,
    agent_id: &str,
) -> Result<token_station_router_core::RouterConfig, String> {
    match config.custom_router_for_agent(agent_id)? {
        Some(router) => Ok(router),
        None => config.home_router_config(),
    }
}

fn agent_model_metadata_for_router(
    config: &token_station_cli::config::ClientConfig,
    router: &token_station_router_core::RouterConfig,
) -> Result<Option<AgentModelMetadata>, String> {
    use token_station_router_core::RoutingMode;

    let mut candidates = BTreeSet::new();
    if router.routing_mode == RoutingMode::QuotaFirst && !router.quota_accounts.is_empty() {
        candidates.extend(router.quota_accounts.iter().cloned());
    } else if router.routing_mode == RoutingMode::QuotaFirst {
        for (upstream, entry) in &config.upstreams {
            let reference = token_station_router_core::UpstreamRef::new(upstream.clone())
                .map_err(|error| error.to_string())?;
            candidates.extend(entry.models.iter().map(|capability| {
                token_station_router_core::UpstreamModel::new(
                    reference.clone(),
                    capability.model.clone(),
                )
            }));
        }
    } else {
        candidates.extend(router.pools.values().flatten().cloned());
    }
    if candidates.is_empty() {
        return Ok(None);
    }

    let mut context = Some(u32::MAX);
    let mut output = Some(u32::MAX);
    let mut vision = true;
    let mut tools = true;
    let mut reasoning = true;
    let mut costs = Vec::new();
    for candidate in &candidates {
        let capability = config
            .upstreams
            .get(candidate.upstream.as_str())
            .and_then(|upstream| {
                upstream
                    .models
                    .iter()
                    .find(|capability| capability.model == candidate.model)
            });
        let Some(capability) = capability else {
            return Ok(None);
        };
        let max_output = (capability.max_output_tokens > 0).then_some(capability.max_output_tokens);
        context = context.and_then(|current| {
            (capability.context_window > 0).then_some(current.min(capability.context_window))
        });
        output = output.and_then(|current| max_output.map(|value| current.min(value)));
        vision &= capability.vision_state().is_supported();
        tools &= capability.tool_state().is_supported();
        reasoning &= capability.supported_parameters.contains("reasoning_effort");

        let catalog_cost = capability
            .extensions
            .get("catalog_cost")
            .and_then(|value| serde_json::from_value::<AgentModelCost>(value.clone()).ok())
            .filter(AgentModelCost::is_valid);
        let configured_cost = config.pricing.models.get(&candidate.model).map(|price| {
            // Price display is decimal USD per million tokens; sub-cent rounding is acceptable here.
            #[allow(clippy::cast_precision_loss)]
            let dollars = |micros: u64| micros as f64 / 1_000_000.0;
            AgentModelCost {
                input: dollars(price.input_per_mtok),
                output: dollars(price.output_per_mtok),
                cache_read: Some(dollars(price.cache_read_per_mtok)),
                cache_write: Some(dollars(price.cache_write_per_mtok)),
            }
        });
        costs.push(catalog_cost.or(configured_cost));
    }

    let cost = costs.first().cloned().flatten().filter(|first| {
        costs
            .iter()
            .all(|candidate| candidate.as_ref() == Some(first))
    });
    let context = context.unwrap_or(0);
    let output = output.unwrap_or(0);
    Ok(Some(AgentModelMetadata {
        context,
        output,
        vision,
        tools,
        reasoning,
        cost,
    }))
}

fn connection_issue(
    code: &str,
    message: impl Into<String>,
    target: Option<String>,
) -> AgentConnectionIssueView {
    AgentConnectionIssueView {
        code: code.to_owned(),
        message: message.into(),
        target,
    }
}

fn opencode_connection_issue(
    config: &token_station_cli::config::ClientConfig,
    router: &token_station_router_core::RouterConfig,
) -> Option<AgentConnectionIssueView> {
    if router.honor_exact_model {
        return Some(connection_issue(
            "model_contract_exact_routing_unsupported",
            "OpenCode 固定使用 tokenstation/auto，与当前精确模型路由不兼容；请改用分层、额度优先或单独路由",
            None,
        ));
    }

    let mut targets = if router.routing_mode == token_station_router_core::RoutingMode::QuotaFirst
        && !router.quota_accounts.is_empty()
    {
        router.quota_accounts.clone()
    } else if router.routing_mode == token_station_router_core::RoutingMode::QuotaFirst {
        config
            .upstreams
            .iter()
            .flat_map(|(upstream, entry)| {
                entry.models.iter().filter_map(move |capability| {
                    token_station_router_core::UpstreamRef::new(upstream.clone())
                        .ok()
                        .map(|upstream| {
                            token_station_router_core::UpstreamModel::new(
                                upstream,
                                capability.model.clone(),
                            )
                        })
                })
            })
            .collect()
    } else {
        let mut reachable_pools = BTreeSet::from([router.default_pool.clone()]);
        reachable_pools.extend(router.rules.iter().map(|rule| rule.route_to.clone()));
        reachable_pools.extend(
            router
                .hint_routes
                .iter()
                .map(|route| route.route_to.clone()),
        );
        if let Some(heuristic) = &router.heuristic {
            reachable_pools.insert(heuristic.above.clone());
            reachable_pools.insert(heuristic.below.clone());
            reachable_pools.extend(heuristic.bands.iter().map(|band| band.pool.clone()));
        }
        if let token_station_router_core::RecoveryPolicy::Ordered { pools } = &router.recovery {
            reachable_pools.extend(pools.iter().cloned());
        }
        reachable_pools
            .into_iter()
            .flat_map(|pool| router.pools.get(&pool).into_iter().flatten().cloned())
            .collect()
    };
    if router.local_only && !router.allow_cloud_fallback {
        targets.retain(|target| {
            config
                .upstreams
                .get(target.upstream.as_str())
                .is_some_and(|upstream| upstream.local)
        });
    }
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        return Some(connection_issue(
            "model_contract_no_reachable_model",
            "OpenCode 当前路由没有可达模型，无法证明自动压缩所需的输入预算",
            Some("opencode".to_owned()),
        ));
    }

    for target in targets {
        let Some(upstream) = config.upstreams.get(target.upstream.as_str()) else {
            return Some(connection_issue(
                "model_contract_unknown_provider",
                format!("OpenCode 路由引用了未知供应商 `{}`", target.upstream),
                Some(target.upstream.to_string()),
            ));
        };
        let Some(capability) = upstream
            .models
            .iter()
            .find(|capability| capability.model == target.model)
        else {
            return Some(connection_issue(
                "model_contract_unknown_model",
                format!("OpenCode 路由引用了未知模型 `{target}`"),
                Some(target.to_string()),
            ));
        };
        if capability.context_window == 0 {
            return Some(connection_issue(
                "model_contract_missing_context_window",
                format!(
                    "模型 `{target}` 缺少可信 context window；不会使用路由假定值代替供应商事实"
                ),
                Some(target.to_string()),
            ));
        }
        let projected_output = if capability.max_output_tokens == 0 {
            super::connectors::OPENCODE_SAFE_DEFAULT_OUTPUT_TOKENS
        } else {
            capability.max_output_tokens
        };
        if projected_output >= capability.context_window {
            return Some(connection_issue(
                "model_contract_invalid_limits",
                format!(
                    "模型 `{target}` 的 OpenCode 输出预算 {projected_output} 必须小于 context window"
                ),
                Some(target.to_string()),
            ));
        }
    }
    None
}

fn serving_router_for_agent(
    server: &crate::serve_lifecycle::RunningServer,
    agent_id: &str,
) -> Result<token_station_router_core::RouterConfig, String> {
    let config = server.serving_config();
    match server.agent_router_override(agent_id) {
        Some(Some(router)) => Ok(router.clone()),
        Some(None) => config.home_router_config(),
        None => configured_router_for_agent(config, agent_id),
    }
}

fn opencode_issue_from_inner(
    inner: &crate::AppInner,
) -> Result<Option<AgentConnectionIssueView>, String> {
    match &inner.server {
        crate::ServerLifecycle::Running { server, .. } => {
            let router = serving_router_for_agent(server, "opencode")?;
            Ok(opencode_connection_issue(server.serving_config(), &router))
        }
        crate::ServerLifecycle::Starting { .. }
        | crate::ServerLifecycle::Applying { .. }
        | crate::ServerLifecycle::Stopping { .. } => Ok(Some(connection_issue(
            "agent_runtime_transition",
            "代理正在切换运行实例，完成后将重新检查 OpenCode 接入条件",
            None,
        ))),
        crate::ServerLifecycle::Stopped { .. } | crate::ServerLifecycle::Failed { .. } => {
            let config = inner.materialize()?;
            let router = configured_router_for_agent(&config, "opencode")?;
            Ok(opencode_connection_issue(&config, &router))
        }
    }
}

pub struct AgentCommandState {
    registry: AgentRegistry,
    #[cfg(test)]
    paths: AgentIntegrationPaths,
    keys: Arc<dyn MasterKeyStore>,
    snapshots: FileSnapshotStore<Arc<dyn MasterKeyStore>>,
    snapshot_migration_warnings: BTreeMap<String, String>,
    ownership: FileOwnershipStore,
    transaction_coordinator: Arc<TransactionCoordinator>,
    token_key: hmac::Key,
    session: Mutex<CommandSession>,
    scan_in_progress: AtomicBool,
    clock: SystemClock,
}

struct ScanInFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for ScanInFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

fn snapshot_master_key_path(paths: &AgentIntegrationPaths) -> PathBuf {
    // Store beside snapshots under the agent-integration data root with private 0600 permissions.
    paths
        .snapshot_root
        .parent()
        .unwrap_or(paths.snapshot_root.as_path())
        .join("snapshot-master.key")
}

/// Ensure the 0600 snapshot master-key file exists, generating it when absent.
/// The OS keychain is no longer used; the key lives only in this private local
/// file. This is non-destructive and never removes snapshots or ownership data.
/// Snapshots from older keychain-only versions cannot be decrypted, but
/// development re-signing had already invalidated that keychain key.
fn ensure_master_key_file(key_path: &Path) {
    if key_path.exists() {
        return;
    }
    let _ = FileMasterKeyStore::new(key_path.to_path_buf()).load_or_create(true);
}

struct PreparedForceStrip {
    target: PathBuf,
    rendered: Zeroizing<Vec<u8>>,
    permissions: Option<u32>,
    original_owner: Option<String>,
    expected_before_hash: String,
    expected_after_hash: String,
    format: DocumentFormat,
    label: &'static str,
}

/// Project `removals` entirely in memory. Force-forget prepares every primary
/// and companion before any file is changed, so parse/patch/reconnect failures
/// cannot leave a partially stripped multi-file installation behind.
#[cfg(test)]
fn prepare_force_strip_owned(
    target: &Path,
    format: DocumentFormat,
    label: &'static str,
    removals: &[PatchOperation],
    reconnect_connector: Option<&dyn Connector>,
) -> Result<Option<PreparedForceStrip>, AgentCommandError> {
    let source = read_config_source(target).map_err(AgentCommandError::internal)?;
    if !source.existed {
        return Ok(None);
    }
    let document = parse_source_bytes(Some(source.exact_bytes.as_slice()), format, label)
        .map_err(AgentCommandError::internal)?;
    prepare_force_strip_projection(
        target,
        source,
        document,
        format,
        label,
        removals,
        reconnect_connector,
    )
    .map(Some)
}

/// Read a Connector's primary document exactly once, then derive its dynamic
/// disconnect patch, projected bytes, and revision guard from that same
/// snapshot. WorkBuddy's array replacement must never be computed from an
/// earlier read than the one later accepted by the commit CAS.
fn prepare_connector_force_strip_owned(
    target: &Path,
    connector: &dyn Connector,
) -> Result<Option<PreparedForceStrip>, AgentCommandError> {
    let source = read_config_source(target).map_err(AgentCommandError::internal)?;
    if !source.existed {
        return Ok(None);
    }
    let document = parse_source_bytes(
        Some(source.exact_bytes.as_slice()),
        connector.format(),
        connector.label(),
    )
    .map_err(AgentCommandError::internal)?;
    let removals = connector
        .disconnect_patch_for_document(&document)
        .map_err(AgentCommandError::internal)?;
    validate_patch_ownership(&removals, &owned_paths_with_legacy(connector))
        .map_err(AgentCommandError::internal)?;
    prepare_force_strip_projection(
        target,
        source,
        document,
        connector.format(),
        connector.label(),
        &removals,
        Some(connector),
    )
    .map(Some)
}

/// Read and project one companion from a single revision. Connectors may
/// dynamically filter a shared collection (for example Claude Desktop's
/// metadata entries) instead of deleting the entire persisted owned path.
#[allow(clippy::too_many_arguments)]
fn prepare_connector_companion_force_strip_owned(
    primary_target: &Path,
    companion_target: &Path,
    connector: &dyn Connector,
    format: DocumentFormat,
    label: &'static str,
    owned_paths: &[ConfigPath],
) -> Result<Option<PreparedForceStrip>, AgentCommandError> {
    let source = read_config_source(companion_target).map_err(AgentCommandError::internal)?;
    if !source.existed {
        return Ok(None);
    }
    let document = parse_source_bytes(Some(source.exact_bytes.as_slice()), format, label)
        .map_err(AgentCommandError::internal)?;
    let removals = connector
        .disconnect_companion_patch_for_document(
            primary_target,
            companion_target,
            &document,
            owned_paths,
        )
        .map_err(AgentCommandError::internal)?;
    validate_patch_ownership(&removals, owned_paths).map_err(AgentCommandError::internal)?;
    if removals.is_empty() {
        return Ok(None);
    }
    prepare_force_strip_projection(
        companion_target,
        source,
        document,
        format,
        label,
        &removals,
        None,
    )
    .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn prepare_force_strip_projection(
    target: &Path,
    source: ConfigSource,
    mut document: super::config_codec::ConfigDocument,
    format: DocumentFormat,
    label: &'static str,
    removals: &[PatchOperation],
    reconnect_connector: Option<&dyn Connector>,
) -> Result<PreparedForceStrip, AgentCommandError> {
    apply_patch(&mut document, removals).map_err(AgentCommandError::internal)?;
    if let Some(connector) = reconnect_connector {
        validate_force_forget_reconnect(connector, &document)
            .map_err(AgentCommandError::internal)?;
    }
    let rendered = render_document(&document, label).map_err(AgentCommandError::internal)?;
    let expected_before_hash =
        super::plan::file_revision_hash(target, &source).map_err(AgentCommandError::internal)?;
    let projected = ConfigSource::existing(
        rendered.as_bytes().to_vec(),
        source.original_permissions,
        source.original_owner.clone(),
    );
    let expected_after_hash =
        super::plan::file_revision_hash(target, &projected).map_err(AgentCommandError::internal)?;
    Ok(PreparedForceStrip {
        target: target.to_path_buf(),
        rendered: Zeroizing::new(rendered.into_bytes()),
        permissions: source.original_permissions,
        original_owner: source.original_owner.clone(),
        expected_before_hash,
        expected_after_hash,
        format,
        label,
    })
}

/// Fail closed before force-forget writes a primary config that the same
/// Connector cannot safely connect again. The check is purely in memory and
/// uses fixed non-secret fixture values; it performs no network request and
/// never reads the runtime virtual key.
fn validate_force_forget_reconnect(
    connector: &dyn Connector,
    disconnected: &super::config_codec::ConfigDocument,
) -> Result<(), String> {
    const RECONNECT_BASE_URL: &str =
        "http://127.0.0.1:8787/agents/token-station-reconnect-check/v1";
    const RECONNECT_TOKEN: &str = "token-station-reconnect-check";

    let rendered = render_document(disconnected, connector.label())?;
    let mut projected = parse_rendered(&rendered, connector.format(), connector.label())?;
    let owned_paths = connector.owned_paths();
    prepare_owned_paths_for_write(&mut projected, &owned_paths)?;
    connector.validate_source(&projected)?;
    let metadata = AgentModelMetadata {
        context: 131_072,
        output: 8_192,
        vision: true,
        tools: true,
        reasoning: true,
        cost: None,
    };
    let input = ConnectInput {
        base_url: RECONNECT_BASE_URL,
        token: Some(RECONNECT_TOKEN),
        adapter_ready: true,
        model_metadata: Some(&metadata),
    };
    let reconnect = connector.connect_patch_for_document(&projected, &input)?;
    validate_patch_ownership(&reconnect, &owned_paths)?;
    apply_patch(&mut projected, &reconnect)?;
    connector.validate_projected(&projected, &input)?;
    let reparsed = parse_rendered(
        &render_document(&projected, connector.label())?,
        connector.format(),
        connector.label(),
    )?;
    connector.validate_projected(&reparsed, &input)
}

impl AgentCommandState {
    pub fn new(paths: AgentIntegrationPaths) -> Result<Self, String> {
        // Store the snapshot master key in a private local 0600 file instead of
        // the OS keychain. Development re-signing invalidated keychain entries and
        // could permanently block restoration or disconnect with
        // agent_operation_rejected. Snapshots are encrypted copies of config that
        // already exists as plaintext on disk, so file storage does not weaken it.
        let key_path = snapshot_master_key_path(&paths);
        ensure_master_key_file(&key_path);
        Self::new_with_master_key(paths, Arc::new(FileMasterKeyStore::new(key_path)))
    }

    fn new_with_master_key(
        paths: AgentIntegrationPaths,
        keys: Arc<dyn MasterKeyStore>,
    ) -> Result<Self, String> {
        let registry = AgentRegistry::builtin()?;
        let mut process_key = [0_u8; 32];
        getrandom::fill(&mut process_key)
            .map_err(|_| "初始化 Agent IPC 会话密钥失败".to_string())?;
        let snapshots = FileSnapshotStore::new(paths.snapshot_root.clone(), keys.clone());
        let snapshot_migration_warnings = snapshots
            .organize_legacy_layout()?
            .warnings
            .into_iter()
            .map(|warning| (warning.snapshot_id, warning.message))
            .collect();
        let ownership = FileOwnershipStore::new(paths.ownership_root.clone());
        Ok(Self {
            registry,
            #[cfg(test)]
            paths,
            keys,
            snapshots,
            snapshot_migration_warnings,
            ownership,
            transaction_coordinator: Arc::new(TransactionCoordinator::default()),
            token_key: hmac::Key::new(hmac::HMAC_SHA256, &process_key),
            session: Mutex::new(CommandSession::default()),
            scan_in_progress: AtomicBool::new(false),
            clock: SystemClock,
        })
    }

    fn registry_metadata(&self) -> Vec<AgentUiMetadata> {
        self.registry.ui_metadata()
    }

    /// Projects the last completed discovery scan into a path-free status-menu
    /// view. Ownership is the durable "connected through Token Station" fact;
    /// unlike runtime validation it remains truthful while the proxy is stopped.
    pub(crate) fn managed_agent_menu_entries(&self) -> Vec<(String, String, u16)> {
        let records = self
            .session
            .lock()
            .ok()
            .and_then(|session| session.scan.as_ref().map(|scan| scan.records.clone()))
            .unwrap_or_default();
        self.registry
            .ui_metadata()
            .into_iter()
            .filter(|metadata| {
                records
                    .iter()
                    .filter(|record| record.agent_id == metadata.agent_id)
                    .any(|record| {
                        self.ownership
                            .list_agent_installation(&record.agent_id, &record.canonical_path)
                            .is_ok_and(|ownership| !ownership.is_empty())
                    })
            })
            .map(|metadata| (metadata.agent_id, metadata.display_name, metadata.ui_order))
            .collect()
    }

    fn begin_scan(&self) -> Result<ScanInFlightGuard<'_>, AgentCommandError> {
        self.scan_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                AgentCommandError::boundary(
                    "scan_in_progress",
                    "Agent 扫描正在进行，请等待当前扫描完成",
                )
            })?;
        Ok(ScanInFlightGuard {
            flag: &self.scan_in_progress,
        })
    }

    #[cfg(test)]
    fn scan(&self) -> Result<Vec<AgentView>, AgentCommandError> {
        self.scan_with_runtime(None, None)
    }

    fn scan_with_runtime(
        &self,
        runtime: Option<&AgentProxyRuntime>,
        opencode_issue: Option<&AgentConnectionIssueView>,
    ) -> Result<Vec<AgentView>, AgentCommandError> {
        let _scan_guard = self.begin_scan()?;
        let snapshot = self.perform_scan()?;
        let views = self.views(&snapshot, runtime, opencode_issue)?;
        self.session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan = Some(snapshot);
        Ok(views)
    }

    fn cached_views_with_runtime(
        &self,
        runtime: Option<&AgentProxyRuntime>,
        opencode_issue: Option<&AgentConnectionIssueView>,
    ) -> Result<Vec<AgentView>, AgentCommandError> {
        let snapshot = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan
            .clone()
            .ok_or_else(|| {
                AgentCommandError::boundary("scan_required", "请先执行服务端 Agent 扫描")
            })?;
        self.views(&snapshot, runtime, opencode_issue)
    }

    /// Re-reads managed Agent files and validates their projected values
    /// against the currently published proxy. Ownership only locates files; it
    /// is never accepted as proof that an Agent is still connected.
    pub(crate) fn any_connected_to(
        &self,
        runtime: &AgentProxyRuntime,
    ) -> Result<bool, AgentCommandError> {
        let records = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan
            .as_ref()
            .map(|snapshot| snapshot.records.clone())
            .unwrap_or_default();
        for record in records {
            if self.installation_connected(&record, runtime)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn perform_scan(&self) -> Result<ScanSnapshot, AgentCommandError> {
        let catalog =
            CompatibilityCatalog::builtin(&self.registry).map_err(AgentCommandError::internal)?;
        let records = DiscoveryScanner::from_process(&self.registry).scan_registry(&self.registry);
        Ok(ScanSnapshot {
            catalog,
            source: CatalogSource::Builtin,
            warning: None,
            records,
        })
    }

    fn refresh_scan(&self) -> Result<(), AgentCommandError> {
        let _scan_guard = self.begin_scan()?;
        let snapshot = self.perform_scan()?;
        self.session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan = Some(snapshot);
        Ok(())
    }

    /// Refresh route-derived model limits and capabilities in every managed
    /// connector that projects them. The restore transaction updates the
    /// active ownership revision but preserves the original disconnect
    /// baseline snapshot.
    pub(crate) fn refresh_model_metadata(
        &self,
        agent_id: Option<&str>,
        runtime: &AgentProxyRuntime,
    ) -> Result<usize, AgentCommandError> {
        if let Some(agent_id) = agent_id {
            validate_short_identifier(agent_id, "agent_id")?;
        }
        let snapshot = self.perform_scan()?;
        let mut refreshed = 0;
        for record in snapshot
            .records
            .iter()
            .filter(|record| agent_id.is_none_or(|selected| record.agent_id == selected))
        {
            let descriptor = self
                .registry
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.agent_id == record.agent_id)
                .ok_or_else(|| AgentCommandError::boundary("unknown_agent", "未知 Agent"))?;
            let ownership = self
                .ownership
                .list_agent_installation(&record.agent_id, &record.canonical_path)
                .map_err(AgentCommandError::internal)?;
            for owned in ownership {
                // Durable ownership resolves a multi-install discovery conflict
                // to this exact canonical installation, just as an explicit UI
                // selection does. Other discovered installations remain untouched.
                let selected = exact_installation_selection(record);
                let decision = evaluate_discovery(&snapshot.catalog, descriptor, &selected);
                if decision.status != CompatibilityStatus::DetectedVerified
                    || decision.connector_id.as_deref() != Some(owned.connector_id.as_str())
                {
                    continue;
                }
                let connector = connector_for(&owned.connector_id)?;
                if !connector.refreshes_managed_configuration() {
                    continue;
                }
                if let Some(issue) = runtime.connection_issue(&owned.connector_id) {
                    return Err(AgentCommandError::connection_issue(issue));
                }
                let target = Path::new(&owned.target_config_path);
                let source = read_config_source(target).map_err(AgentCommandError::internal)?;
                let input = runtime.input_for(&owned.connector_id)?;
                let now_ms = self.clock.now_ms();
                let operation_id = generate_operation_id().map_err(AgentCommandError::internal)?;
                let restores_legacy_paths = connector
                    .legacy_owned_paths()
                    .iter()
                    .any(|path| owned.owned_paths.contains(path));
                let prepared = if restores_legacy_paths {
                    let baseline = self
                        .snapshots
                        .load(&owned.baseline_snapshot_id)
                        .map_err(AgentCommandError::internal)?;
                    if baseline.record.snapshot_id != owned.baseline_snapshot_id
                        || baseline.record.agent_id != owned.agent_id
                        || baseline.record.target_config_path != owned.target_config_path
                        || baseline.record.connector_id != owned.connector_id
                    {
                        return Err(AgentCommandError::internal(
                            "legacy Connector baseline snapshot binding is invalid".to_string(),
                        ));
                    }
                    let baseline_source = if baseline.record.original_existed {
                        ConfigSource::existing(
                            baseline.exact_bytes.to_vec(),
                            baseline.record.original_permissions,
                            baseline.record.original_owner.clone(),
                        )
                    } else {
                        ConfigSource::missing()
                    };
                    build_metadata_refresh_plan_with_baseline(
                        connector,
                        &selected,
                        &decision,
                        target,
                        &source,
                        &baseline_source,
                        &input,
                        &owned,
                        snapshot.catalog.sequence,
                        snapshot.catalog.expires_at_ms,
                        now_ms,
                        operation_id,
                    )
                } else {
                    build_metadata_refresh_plan(
                        connector,
                        &selected,
                        &decision,
                        target,
                        &source,
                        &input,
                        &owned,
                        snapshot.catalog.sequence,
                        snapshot.catalog.expires_at_ms,
                        now_ms,
                        operation_id,
                    )
                }
                .map_err(AgentCommandError::internal)?;
                let confirmation = ConfirmedOperation {
                    operation_id: prepared.view.operation_id.clone(),
                    confirmed_at_ms: now_ms,
                    confirmations: prepared
                        .view
                        .required_confirmations
                        .iter()
                        .copied()
                        .collect(),
                };
                let admission = RuntimeAdmission {
                    compatibility_sequence: snapshot.catalog.sequence,
                    status: decision.status,
                };
                TransactionEngine::with_coordinator(
                    &self.snapshots,
                    &self.ownership,
                    self.keys.as_ref(),
                    &FsAtomicConfigWriter,
                    &ParseOnlyVerifier,
                    &self.clock,
                    Arc::clone(&self.transaction_coordinator),
                )
                .apply_snapshot_restore(&prepared, &confirmation, &admission, now_ms)
                .map_err(AgentCommandError::from)?;
                refreshed += 1;
            }
        }
        self.session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan = Some(snapshot);
        Ok(refreshed)
    }

    fn views(
        &self,
        snapshot: &ScanSnapshot,
        runtime: Option<&AgentProxyRuntime>,
        opencode_issue: Option<&AgentConnectionIssueView>,
    ) -> Result<Vec<AgentView>, AgentCommandError> {
        let metadata = self.registry.ui_metadata();
        self.registry
            .descriptors()
            .iter()
            .zip(metadata)
            .map(|(descriptor, metadata)| {
                let installations: Result<Vec<_>, AgentCommandError> = snapshot
                    .records
                    .iter()
                    .filter(|record| record.agent_id == descriptor.agent_id)
                    .map(|record| {
                        let mut record = record.clone();
                        let ownership = self
                            .ownership
                            .list_agent_installation(&record.agent_id, &record.canonical_path);
                        if ownership.is_err() {
                            record.diagnostics.push(Diagnostic {
                                reason_code: ReasonCode::ReadOnlyPreflightFailed,
                                message: "Token Station 接管索引不可用；已保留只读安装发现，接入和恢复暂时禁用"
                                    .to_string(),
                            });
                        }
                        let compatibility =
                            evaluate_discovery(&snapshot.catalog, descriptor, &record);
                        let connector_id = compatibility.connector_id.as_deref().or_else(|| {
                            (descriptor.local_connector_ids.len() == 1)
                                .then(|| descriptor.local_connector_ids[0].as_str())
                        });
                        let adapter_ready = runtime.and_then(|runtime| {
                            connector_id
                                .and_then(|connector_id| runtime.input_for(connector_id).ok())
                                .map(|input| input.adapter_ready)
                        });
                        let connection_issue = runtime
                            .and_then(|runtime| {
                                connector_id.and_then(|id| runtime.connection_issue(id))
                            })
                            .or_else(|| {
                                (connector_id == Some("opencode-v1"))
                                    .then_some(opencode_issue)
                                    .flatten()
                            })
                            .cloned();
                        let ownership_available = ownership.is_ok();
                        let managed = ownership.is_ok_and(|records| !records.is_empty());
                        let connected = ownership_available && runtime.is_some_and(|runtime| {
                            self.installation_connected(&record, runtime)
                                .unwrap_or(false)
                        });
                        Ok(AgentInstallationView {
                            discovery: record,
                            compatibility,
                            adapter_ready,
                            connection_issue,
                            managed,
                            connected,
                        })
                    })
                    .collect();
                let installations = installations?;
                let status = installations
                    .iter()
                    .find(|installation| installation.connected)
                    .map_or_else(
                        || {
                            installations
                                .first()
                                .map_or(CompatibilityStatus::NotDetected, |installation| {
                                    installation.compatibility.status
                                })
                        },
                        |_| CompatibilityStatus::Connected,
                    );
                Ok(AgentView {
                    metadata,
                    installations,
                    status,
                    catalog_sequence: snapshot.catalog.sequence,
                    catalog_expires_at_ms: snapshot.catalog.expires_at_ms,
                    catalog_source: match snapshot.source {
                        CatalogSource::Builtin => "builtin",
                    },
                    catalog_warning: snapshot.warning.clone(),
                })
            })
            .collect()
    }

    fn installation_connected(
        &self,
        record: &DiscoveryRecord,
        runtime: &AgentProxyRuntime,
    ) -> Result<bool, AgentCommandError> {
        let ownership = self
            .ownership
            .list_agent_installation(&record.agent_id, &record.canonical_path)
            .map_err(AgentCommandError::internal)?;
        for owned in ownership {
            let Ok(connector) = connector_for(&owned.connector_id) else {
                continue;
            };
            let Ok(input) = runtime.input_for(&owned.connector_id) else {
                continue;
            };
            if !input.adapter_ready {
                continue;
            }
            let Ok(source) = read_config_source(Path::new(&owned.target_config_path)) else {
                continue;
            };
            let bytes = source.existed.then_some(source.exact_bytes.as_slice());
            let Ok(document) = parse_source_bytes(bytes, connector.format(), connector.label())
            else {
                continue;
            };
            if connector.validate_projected(&document, &input).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn selected(
        &self,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<(DiscoveryRecord, CompatibilityDecision, u64, Option<u64>), AgentCommandError> {
        let session = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?;
        let scan = session.scan.as_ref().ok_or_else(|| {
            AgentCommandError::boundary("scan_required", "请先执行服务端 Agent 扫描")
        })?;
        let (record, decision) = scan.selected(&self.registry, agent_id, installation_path)?;
        Ok((
            record,
            decision,
            scan.catalog.sequence,
            scan.catalog.expires_at_ms,
        ))
    }

    fn plan_connection(
        &self,
        agent_id: &str,
        installation_path: &str,
        expected_version: Option<&str>,
        session_label: &str,
        runtime: &AgentProxyRuntime,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        validate_session_label(session_label)?;
        let (record, decision, sequence, catalog_expiry) =
            self.selected(agent_id, installation_path)?;
        if !self
            .ownership
            .list_agent_installation(agent_id, installation_path)
            .map_err(AgentCommandError::internal)?
            .is_empty()
        {
            return Err(AgentCommandError::boundary(
                "ownership_repair_required",
                "该安装已有 Token Station 接管记录，但当前运行态不一致；请先恢复 Agent 原始配置，再重新接入",
            ));
        }
        if record.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.reason_code,
                ReasonCode::ReadOnlyPreflightFailed
                    | ReasonCode::ConfigReadFailed
                    | ReasonCode::ConfigParseFailed
                    | ReasonCode::InvalidEnvironmentOverride
            )
        }) {
            return Err(AgentCommandError::boundary(
                "read_only_preflight_failed",
                "只读配置预检失败，禁止生成写入计划",
            ));
        }
        let connector_id = decision
            .connector_id
            .as_deref()
            .ok_or_else(|| AgentCommandError::boundary("not_admitted", "当前版本不能接入"))?;
        match (record.version_normalized.as_deref(), expected_version) {
            (Some(_), None) => {
                return Err(AgentCommandError::boundary(
                    "expected_version_required",
                    "接入前必须绑定扫描到的 Agent 版本",
                ));
            }
            (Some(current), Some(expected)) if expected.len() <= 128 && current == expected => {}
            (None, None) => {}
            _ => {
                return Err(AgentCommandError::boundary(
                    "discovery_changed_before_plan",
                    "Agent 版本已变化，请重新扫描并确认当前版本",
                ));
            }
        }
        let connector = connector_for(connector_id)?;
        if let Some(issue) = runtime.connection_issue(connector_id) {
            return Err(AgentCommandError::connection_issue(issue));
        }
        if !connector.supports_platform(record.environment) {
            return Err(AgentCommandError::boundary(
                "unsupported_platform",
                format!(
                    "{} 不支持当前 {:?} 平台；未修改任何配置",
                    connector.label(),
                    record.environment
                ),
            ));
        }
        let target = server_target(&record)?;
        let source = read_config_source(target).map_err(AgentCommandError::internal)?;
        let input = runtime.input_for(connector_id)?;
        let now_ms = self.clock.now_ms();
        let prepared = build_connection_plan(
            connector,
            &record,
            &decision,
            target,
            &source,
            &input,
            sequence,
            catalog_expiry,
            now_ms,
            generate_operation_id().map_err(AgentCommandError::internal)?,
        )
        .map_err(AgentCommandError::internal)?;
        self.issue_plan(
            prepared,
            &record,
            session_label,
            Some(runtime.fingerprint()),
        )
    }

    fn plan_disconnect(
        &self,
        agent_id: &str,
        installation_path: &str,
        session_label: &str,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        validate_session_label(session_label)?;
        let (record, decision, sequence, catalog_expiry) =
            self.selected(agent_id, installation_path)?;
        let owned = self
            .ownership
            .list_agent_installation(agent_id, installation_path)
            .map_err(AgentCommandError::internal)?;
        if owned.len() != 1 {
            return Err(AgentCommandError::boundary(
                "ownership_missing_or_ambiguous",
                "该安装实例没有唯一的 Token Station 归属记录",
            ));
        }
        let ownership = owned.into_iter().next().expect("length checked");
        let connector = connector_for(&ownership.connector_id)?;
        let baseline = self
            .snapshots
            .load(&ownership.baseline_snapshot_id)
            .map_err(AgentCommandError::internal)?;
        let baseline_source = decrypted_source(&baseline.record, baseline.exact_bytes);
        let target = Path::new(&ownership.target_config_path);
        let current = read_config_source(target).map_err(AgentCommandError::internal)?;
        let key = self.keys.load().map_err(AgentCommandError::internal)?;
        let mut prepared = build_disconnect_plan(
            connector,
            &record,
            &decision,
            target,
            &current,
            &ownership,
            &baseline.record,
            &baseline_source,
            &key,
            sequence,
            catalog_expiry,
            self.clock.now_ms(),
            generate_operation_id().map_err(AgentCommandError::internal)?,
        )
        .map_err(AgentCommandError::plan)?;
        attach_disconnect_companions(&mut prepared, connector, &ownership, &self.snapshots, &key)
            .map_err(AgentCommandError::plan)?;
        self.issue_plan(prepared, &record, session_label, None)
    }

    /// Force-disconnect fallback that does not require the keychain or baseline
    /// snapshot. Remove Token Station-managed fields according to ownership,
    /// clear ownership, and unpin the baseline snapshot. This recovers when a lost
    /// key makes snapshots unreadable and normal restoration is rejected, but it
    /// cannot reconstruct overwritten original values exactly.
    fn force_forget(
        &self,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<(), AgentCommandError> {
        validate_short_identifier(agent_id, "agent_id")?;
        let owned = self
            .ownership
            .list_agent_installation(agent_id, installation_path)
            .map_err(AgentCommandError::internal)?;
        if owned.is_empty() {
            return Err(AgentCommandError::boundary(
                "ownership_missing",
                "该安装实例没有可清除的接管记录",
            ));
        }
        if owned.len() != 1 {
            return Err(AgentCommandError::boundary(
                "ownership_ambiguous",
                "该安装实例存在多条归属记录，必须使用逐项快照恢复",
            ));
        }

        // Parse every companion format before writing anything. If the connector's
        // explicit contract cannot confirm a legacy ownership format, force
        // disconnect must fail closed before changing the main config.
        let companion_formats = owned
            .iter()
            .map(|ownership| {
                let connector = connector_for(&ownership.connector_id)?;
                ownership
                    .companion_files
                    .iter()
                    .map(|companion| {
                        companion_document_format(connector, ownership, companion)
                            .map_err(AgentCommandError::internal)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, AgentCommandError>>()?;

        let mut prepared_strips = Vec::new();
        for (ownership, companion_formats) in owned.iter().zip(&companion_formats) {
            let connector = connector_for(&ownership.connector_id)?;
            let target = Path::new(&ownership.target_config_path);
            // Main config: regular connectors use fixed Remove operations, while
            // WorkBuddy filters dynamically by model ID so force disconnect does
            // not remove models the user added later.
            if let Some(prepared) = prepare_connector_force_strip_owned(target, connector)? {
                prepared_strips.push(prepared);
            }
            // Companion config: parse the persisted format or a connector's
            // explicit legacy contract, then remove owned_paths.
            for (companion, document_format) in ownership
                .companion_files
                .iter()
                .zip(companion_formats.iter().copied())
            {
                if let Some(prepared) = prepare_connector_companion_force_strip_owned(
                    target,
                    Path::new(&companion.target_config_path),
                    connector,
                    document_format,
                    "companion 配置",
                    &companion.owned_paths,
                )? {
                    prepared_strips.push(prepared);
                }
            }
        }

        let written_strips =
            commit_force_strips(&prepared_strips, &FsAtomicConfigWriter, &read_config_source)?;

        for ownership in &owned {
            if let Err(error) = self.ownership.remove(&ownership.key(), ownership.revision) {
                rollback_force_strips(&prepared_strips, &written_strips, &FsAtomicConfigWriter);
                return Err(AgentCommandError::internal(error));
            }
        }
        for ownership in owned {
            for companion in &ownership.companion_files {
                let _ = self
                    .snapshots
                    .set_pinned(&companion.baseline_snapshot_id, false);
            }
            let _ = self
                .snapshots
                .set_pinned(&ownership.baseline_snapshot_id, false);
        }
        Ok(())
    }

    fn list_snapshots(&self, agent_id: &str) -> Result<Vec<SnapshotView>, AgentCommandError> {
        validate_short_identifier(agent_id, "agent_id")?;
        if !self
            .registry
            .descriptors()
            .iter()
            .any(|descriptor| descriptor.agent_id == agent_id)
        {
            return Err(AgentCommandError::boundary("unknown_agent", "未知 Agent"));
        }
        let mut views = self
            .snapshots
            .list_agent(agent_id)
            .map(|records| {
                records
                    .into_iter()
                    .map(SnapshotView::from)
                    .collect::<Vec<_>>()
            })
            .map_err(AgentCommandError::internal)?;
        for view in &mut views {
            if let Some(warning) = self.snapshot_migration_warnings.get(&view.snapshot_id) {
                view.restorable = false;
                view.maintenance_warning = Some(warning.clone());
            }
        }
        let legacy_targets = {
            let session = self.session.lock().map_err(|_| {
                AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用")
            })?;
            session
                .scan
                .as_ref()
                .map(|scan| {
                    scan.records
                        .iter()
                        .filter(|record| record.agent_id == agent_id)
                        .flat_map(|record| record.config_candidates.iter())
                        .cloned()
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default()
        };
        views.extend(
            legacy_targets
                .iter()
                .filter_map(|target| legacy_snapshot_view(agent_id, Path::new(target))),
        );
        views.sort_by_key(|view| std::cmp::Reverse(view.created_at_ms));
        Ok(views)
    }

    fn drift(
        &self,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<Vec<AgentDriftView>, AgentCommandError> {
        let (_record, _, _, _) = self.selected(agent_id, installation_path)?;
        let ownership = self
            .ownership
            .list_agent_installation(agent_id, installation_path)
            .map_err(AgentCommandError::internal)?;
        let checked_at_ms = self.clock.now_ms();
        ownership
            .into_iter()
            .map(|owned| {
                let failure = |status: DriftStatus, current_hash: Option<String>, message: &str| {
                    AgentDriftView {
                        agent_id: owned.agent_id.clone(),
                        installation_path: owned.installation_path.clone(),
                        target_config_path: owned.target_config_path.clone(),
                        connector_id: owned.connector_id.clone(),
                        status,
                        baseline_hash: owned.before_hash.clone(),
                        managed_hash: owned.managed_after_hash.clone(),
                        current_hash,
                        checked_at_ms,
                        changes: Vec::new(),
                        truncated: false,
                        message: message.to_string(),
                    }
                };
                let connector = match connector_for(&owned.connector_id) {
                    Ok(connector) => connector,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unreadable,
                            None,
                            "归属记录引用的 Connector 不可用",
                        ));
                    }
                };
                let baseline = match self.snapshots.load(&owned.baseline_snapshot_id) {
                    Ok(snapshot) => snapshot,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unreadable,
                            None,
                            "接管前加密快照不可读",
                        ));
                    }
                };
                let baseline_bytes = baseline
                    .record
                    .original_existed
                    .then_some(baseline.exact_bytes.as_slice());
                let baseline_document =
                    match parse_source_bytes(baseline_bytes, connector.format(), connector.label())
                    {
                        Ok(document) => document,
                        Err(_) => {
                            return Ok(failure(
                                DriftStatus::Unreadable,
                                None,
                                "接管前加密快照无法结构化解析",
                            ));
                        }
                    };
                let target = Path::new(&owned.target_config_path);
                let current = match read_config_source(target) {
                    Ok(source) => source,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unreadable,
                            None,
                            "当前 Agent 配置不可读",
                        ));
                    }
                };
                if !current.existed {
                    return Ok(failure(
                        DriftStatus::Missing,
                        None,
                        "当前 Agent 配置文件已不存在",
                    ));
                }
                let current_hash = match super::plan::file_revision_hash(target, &current) {
                    Ok(hash) => hash,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unreadable,
                            None,
                            "无法计算当前 Agent 配置指纹",
                        ));
                    }
                };
                let current_document = match parse_source_bytes(
                    Some(current.exact_bytes.as_slice()),
                    connector.format(),
                    connector.label(),
                ) {
                    Ok(document) => document,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unparseable,
                            Some(current_hash),
                            "当前 Agent 配置已无法结构化解析",
                        ));
                    }
                };
                let key = match self.keys.load() {
                    Ok(key) => key,
                    Err(_) => {
                        return Ok(failure(
                            DriftStatus::Unreadable,
                            Some(current_hash),
                            "配置对账主密钥不可用",
                        ));
                    }
                };
                analyze_drift(
                    &owned,
                    &baseline_document,
                    &current_document,
                    &current_hash,
                    &key,
                    checked_at_ms,
                )
                .map_err(AgentCommandError::internal)
            })
            .collect::<Result<Vec<_>, _>>()
    }

    fn plan_restore(
        &self,
        snapshot_id: &str,
        session_label: &str,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        validate_session_label(session_label)?;
        if let Some(warning) = self.snapshot_migration_warnings.get(snapshot_id) {
            return Err(AgentCommandError::boundary(
                "snapshot_migration_failed",
                format!("The snapshot needs maintenance before restore: {warning}"),
            ));
        }
        let snapshot = self
            .snapshots
            .load(snapshot_id)
            .map_err(AgentCommandError::internal)?;
        let matching: Vec<_> = {
            let session = self.session.lock().map_err(|_| {
                AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用")
            })?;
            let scan = session.scan.as_ref().ok_or_else(|| {
                AgentCommandError::boundary("scan_required", "请先执行服务端 Agent 扫描")
            })?;
            scan.records
                .iter()
                .filter(|record| record.agent_id == snapshot.record.agent_id)
                .filter_map(|record| {
                    let key = super::ownership::OwnershipKey {
                        agent_id: record.agent_id.clone(),
                        installation_path: record.canonical_path.clone(),
                        target_config_path: snapshot.record.target_config_path.clone(),
                    };
                    self.ownership
                        .load(&key)
                        .ok()
                        .flatten()
                        .map(|ownership| (record.clone(), ownership))
                })
                .collect()
        };
        if matching.len() != 1 {
            return Err(AgentCommandError::boundary(
                "snapshot_ownership_missing_or_ambiguous",
                "快照无法唯一绑定当前安装实例和归属记录",
            ));
        }
        let (mut record, ownership) = matching.into_iter().next().expect("length checked");
        record.conflict_group = None;
        let descriptor = self
            .registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == record.agent_id)
            .expect("snapshot agent was validated by Registry when created");
        let (decision, sequence, catalog_expiry) = {
            let session = self.session.lock().map_err(|_| {
                AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用")
            })?;
            let scan = session.scan.as_ref().expect("checked above");
            (
                evaluate_discovery(&scan.catalog, descriptor, &record),
                scan.catalog.sequence,
                scan.catalog.expires_at_ms,
            )
        };
        let connector = connector_for(&ownership.connector_id)?;
        let target = Path::new(&ownership.target_config_path);
        let current = read_config_source(target).map_err(AgentCommandError::internal)?;
        let source = decrypted_source(&snapshot.record, snapshot.exact_bytes);
        let key = self.keys.load().map_err(AgentCommandError::internal)?;
        let mut prepared = build_snapshot_restore_plan(
            connector,
            &record,
            &decision,
            target,
            &current,
            &ownership,
            &snapshot.record,
            &source,
            &key,
            sequence,
            catalog_expiry,
            self.clock.now_ms(),
            generate_operation_id().map_err(AgentCommandError::internal)?,
        )
        .map_err(AgentCommandError::plan)?;
        attach_restore_companions(
            &mut prepared,
            connector,
            &ownership,
            &snapshot.record,
            &self.snapshots,
            &key,
        )
        .map_err(AgentCommandError::plan)?;
        self.issue_plan(prepared, &record, session_label, None)
    }

    fn issue_plan(
        &self,
        prepared: PreparedChangePlan,
        discovery: &DiscoveryRecord,
        session_label: &str,
        proxy_binding: Option<[u8; 32]>,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        if prepared.view.expires_at_ms <= self.clock.now_ms() {
            return Err(AgentCommandError::boundary(
                "plan_already_expired",
                "计划在签发确认令牌前已过期",
            ));
        }
        let mut token = [0_u8; CONFIRMATION_TOKEN_BYTES];
        getrandom::fill(&mut token)
            .map_err(|_| AgentCommandError::boundary("random_failed", "生成确认令牌失败"))?;
        let confirmation_token = lower_hex(&token);
        let rendered_view = serde_json::to_vec(&prepared.view)
            .map_err(|_| AgentCommandError::boundary("plan_encoding_failed", "计划编码失败"))?;
        let view_hash: [u8; 32] = Sha256::digest(&rendered_view).into();
        let payload = confirmation_payload(
            &prepared.view.operation_id,
            &view_hash,
            session_label,
            prepared.view.expires_at_ms,
            &token,
        );
        let confirmation_tag: [u8; 32] = hmac::sign(&self.token_key, &payload)
            .as_ref()
            .try_into()
            .expect("HMAC-SHA256 has 32 bytes");
        let operation_id = prepared.view.operation_id.clone();
        let view = prepared.view.clone();
        let mut session = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?;
        prune_expired(&mut session.plans, self.clock.now_ms());
        if session.plans.len() >= MAX_PENDING_PLANS {
            return Err(AgentCommandError::boundary(
                "too_many_pending_plans",
                "待确认计划过多，请等待旧计划过期后重新预览",
            ));
        }
        if session.plans.contains_key(&operation_id) {
            return Err(AgentCommandError::boundary(
                "duplicate_operation_id",
                "operation ID 冲突，请重新预览",
            ));
        }
        session.plans.insert(
            operation_id,
            StoredPlan {
                prepared,
                confirmation_tag,
                view_hash,
                session_label: session_label.to_string(),
                discovery: DiscoveryBinding::from(discovery),
                proxy_binding,
            },
        );
        Ok(ConfigPlanView {
            plan: view,
            confirmation_token,
        })
    }

    fn plan_intent(&self, operation_id: &str) -> Result<PlanIntent, AgentCommandError> {
        validate_operation_id(operation_id)?;
        let session = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?;
        session
            .plans
            .get(operation_id)
            .map(|stored| stored.prepared.view.intent)
            .ok_or_else(|| AgentCommandError::boundary("unknown_operation", "计划不存在或已消费"))
    }

    fn take_plan(
        &self,
        operation_id: &str,
        confirmation_token: &str,
        session_label: &str,
        expected_intents: &[PlanIntent],
    ) -> Result<TakenPlan, AgentCommandError> {
        validate_operation_id(operation_id)?;
        validate_session_label(session_label)?;
        let token = decode_token(confirmation_token)?;
        let now_ms = self.clock.now_ms();
        let mut session = self
            .session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?;
        prune_expired(&mut session.plans, now_ms);
        let stored = session.plans.get(operation_id).ok_or_else(|| {
            AgentCommandError::boundary(
                "unknown_or_expired_operation",
                "计划不存在、已过期或已消费",
            )
        })?;
        if stored.session_label != session_label
            || !expected_intents.contains(&stored.prepared.view.intent)
        {
            return Err(AgentCommandError::boundary(
                "operation_session_or_intent_mismatch",
                "计划与当前窗口会话或操作入口不匹配",
            ));
        }
        let payload = confirmation_payload(
            operation_id,
            &stored.view_hash,
            session_label,
            stored.prepared.view.expires_at_ms,
            &token,
        );
        hmac::verify(
            &self.token_key,
            &payload,
            stored.confirmation_tag.as_slice(),
        )
        .map_err(|_| {
            AgentCommandError::boundary("confirmation_token_mismatch", "确认令牌不匹配")
        })?;
        let stored = session
            .plans
            .remove(operation_id)
            .expect("plan remained present while lock was held");
        Ok(TakenPlan {
            prepared: stored.prepared,
            discovery: stored.discovery,
            proxy_binding: stored.proxy_binding,
        })
    }

    fn apply(
        &self,
        operation_id: &str,
        confirmation_token: &str,
        session_label: &str,
        expected_intents: &[PlanIntent],
        runtime: Option<&AgentProxyRuntime>,
    ) -> Result<TransactionOutcome, AgentCommandError> {
        self.refresh_scan()?;
        self.apply_from_cached_scan(
            operation_id,
            confirmation_token,
            session_label,
            expected_intents,
            runtime,
        )
    }

    fn discard_plan(
        &self,
        operation_id: &str,
        confirmation_token: &str,
        session_label: &str,
    ) -> Result<(), AgentCommandError> {
        let _ = self.take_plan(
            operation_id,
            confirmation_token,
            session_label,
            &[
                PlanIntent::Connect,
                PlanIntent::Disconnect,
                PlanIntent::Restore,
            ],
        )?;
        Ok(())
    }

    /// Apply a confirmed plan against the scan snapshot already stored in this
    /// command state. Production callers refresh immediately before reaching
    /// this boundary; tests inject an isolated scan so the full confirmation,
    /// discovery-binding and transaction path can run without touching real
    /// Agent installations.
    fn apply_from_cached_scan(
        &self,
        operation_id: &str,
        confirmation_token: &str,
        session_label: &str,
        expected_intents: &[PlanIntent],
        runtime: Option<&AgentProxyRuntime>,
    ) -> Result<TransactionOutcome, AgentCommandError> {
        let taken = self.take_plan(
            operation_id,
            confirmation_token,
            session_label,
            expected_intents,
        )?;
        if taken.proxy_binding != runtime.map(AgentProxyRuntime::fingerprint) {
            return Err(AgentCommandError::boundary(
                "proxy_runtime_changed",
                "代理运行态已变化，请重新预览",
            ));
        }
        let (current, decision, sequence, _) = self.selected(
            &taken.prepared.view.agent_id,
            &taken.prepared.view.installation_path,
        )?;
        if DiscoveryBinding::from(&current) != taken.discovery {
            return Err(AgentCommandError::boundary(
                "discovery_changed_after_plan",
                "Agent 版本、配置指纹或安装状态已变化，请重新预览",
            ));
        }
        if taken.prepared.view.intent == PlanIntent::Connect
            && decision.connector_id.as_deref() != Some(taken.prepared.view.connector_id.as_str())
        {
            return Err(AgentCommandError::boundary(
                "connector_changed_after_plan",
                "Connector 兼容绑定已变化，请重新预览",
            ));
        }
        let confirmations = taken
            .prepared
            .view
            .required_confirmations
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let confirmation = ConfirmedOperation {
            operation_id: operation_id.to_string(),
            confirmed_at_ms: self.clock.now_ms(),
            confirmations,
        };
        let admission = RuntimeAdmission {
            compatibility_sequence: sequence,
            status: decision.status,
        };
        let engine = TransactionEngine::with_coordinator(
            &self.snapshots,
            &self.ownership,
            self.keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &self.clock,
            Arc::clone(&self.transaction_coordinator),
        );
        let result = match taken.prepared.view.intent {
            PlanIntent::Connect => engine.apply_connection(
                &taken.prepared,
                &confirmation,
                &admission,
                self.clock.now_ms(),
            ),
            PlanIntent::Disconnect => engine.apply_disconnect(
                &taken.prepared,
                &confirmation,
                &admission,
                self.clock.now_ms(),
            ),
            PlanIntent::Restore => engine.apply_snapshot_restore(
                &taken.prepared,
                &confirmation,
                &admission,
                self.clock.now_ms(),
            ),
        };
        result.map_err(AgentCommandError::from)
    }
}

fn commit_force_strips(
    prepared: &[PreparedForceStrip],
    writer: &dyn AtomicConfigWriter,
    read_source: &dyn Fn(&Path) -> Result<ConfigSource, String>,
) -> Result<Vec<(usize, ConfigSource)>, AgentCommandError> {
    let mut written_strips: Vec<(usize, ConfigSource)> = Vec::new();
    for (index, strip) in prepared.iter().enumerate() {
        let current = match read_source(&strip.target) {
            Ok(current) => current,
            Err(error) => {
                rollback_force_strips(prepared, &written_strips, writer);
                return Err(AgentCommandError::internal(error));
            }
        };
        let current_hash = match super::plan::file_revision_hash(&strip.target, &current) {
            Ok(hash) => hash,
            Err(error) => {
                rollback_force_strips(prepared, &written_strips, writer);
                return Err(AgentCommandError::internal(error));
            }
        };
        if current_hash != strip.expected_before_hash {
            rollback_force_strips(prepared, &written_strips, writer);
            return Err(AgentCommandError::boundary(
                "target_changed_before_force_forget",
                "配置在恢复前已变化，请重新扫描后再试",
            ));
        }
        if let Err(failure) = writer.replace(
            &strip.target,
            strip.rendered.as_slice(),
            strip.permissions,
            strip.original_owner.as_deref(),
            &strip.expected_before_hash,
            false,
        ) {
            if failure.target_replaced {
                written_strips.push((index, current));
            }
            rollback_force_strips(prepared, &written_strips, writer);
            return Err(AgentCommandError::internal(format!(
                "原子替换配置失败（{:?}）",
                failure.stage
            )));
        }
        let written = match read_source(&strip.target) {
            Ok(written) => written,
            Err(error) => {
                written_strips.push((index, current));
                rollback_force_strips(prepared, &written_strips, writer);
                return Err(AgentCommandError::internal(error));
            }
        };
        let written_hash = match super::plan::file_revision_hash(&strip.target, &written) {
            Ok(hash) => hash,
            Err(error) => {
                written_strips.push((index, current));
                rollback_force_strips(prepared, &written_strips, writer);
                return Err(AgentCommandError::internal(error));
            }
        };
        if written_hash != strip.expected_after_hash {
            written_strips.push((index, current));
            rollback_force_strips(prepared, &written_strips, writer);
            return Err(AgentCommandError::internal(
                "恢复后的配置 revision 与计划不一致".to_string(),
            ));
        }
        let reparsed = match parse_source_bytes(
            written.existed.then_some(written.exact_bytes.as_slice()),
            strip.format,
            strip.label,
        ) {
            Ok(reparsed) => reparsed,
            Err(error) => {
                written_strips.push((index, current));
                rollback_force_strips(prepared, &written_strips, writer);
                return Err(AgentCommandError::internal(error));
            }
        };
        if render_document(&reparsed, strip.label).is_err() {
            written_strips.push((index, current));
            rollback_force_strips(prepared, &written_strips, writer);
            return Err(AgentCommandError::internal(
                "恢复后的配置无法通过写后复验".to_string(),
            ));
        }
        written_strips.push((index, current));
    }
    Ok(written_strips)
}

fn rollback_force_strips(
    prepared: &[PreparedForceStrip],
    written: &[(usize, ConfigSource)],
    writer: &dyn AtomicConfigWriter,
) {
    for (index, source) in written.iter().rev() {
        let strip = &prepared[*index];
        let _ = writer.replace(
            &strip.target,
            source.exact_bytes.as_slice(),
            source.original_permissions,
            source.original_owner.as_deref(),
            &strip.expected_after_hash,
            false,
        );
    }
}

fn connector_for(connector_id: &str) -> Result<&'static dyn Connector, AgentCommandError> {
    find_connector(connector_id).ok_or_else(|| {
        AgentCommandError::boundary(
            "unsupported_connector",
            "Connector 不在本机构建的准入列表中",
        )
    })
}

fn server_target(record: &DiscoveryRecord) -> Result<&Path, AgentCommandError> {
    record
        .config_candidates
        .first()
        .map(Path::new)
        .ok_or_else(|| {
            AgentCommandError::boundary(
                "config_target_unavailable",
                "服务端无法从 Registry 和扫描环境推导配置目标",
            )
        })
}

fn decrypted_source(record: &SnapshotRecord, exact_bytes: Zeroizing<Vec<u8>>) -> ConfigSource {
    ConfigSource {
        existed: record.original_existed,
        exact_bytes,
        original_permissions: record.original_permissions,
        original_owner: record.original_owner.clone(),
    }
}

fn confirmation_payload(
    operation_id: &str,
    view_hash: &[u8; 32],
    session_label: &str,
    expires_at_ms: u64,
    token: &[u8; CONFIRMATION_TOKEN_BYTES],
) -> Vec<u8> {
    let mut payload = b"token-station-agent-confirmation-v1\0".to_vec();
    for field in [
        operation_id.as_bytes(),
        view_hash.as_slice(),
        session_label.as_bytes(),
        expires_at_ms.to_be_bytes().as_slice(),
        token.as_slice(),
    ] {
        payload.extend_from_slice(&(field.len() as u64).to_be_bytes());
        payload.extend_from_slice(field);
    }
    payload
}

fn prune_expired(plans: &mut HashMap<String, StoredPlan>, now_ms: u64) {
    plans.retain(|_, stored| stored.prepared.view.expires_at_ms >= now_ms);
}

fn validate_short_identifier(value: &str, label: &str) -> Result<(), AgentCommandError> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AgentCommandError::boundary(
            "invalid_identifier",
            format!("{label} 格式无效"),
        ));
    }
    Ok(())
}

fn validate_installation_lookup(value: &str) -> Result<(), AgentCommandError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(AgentCommandError::boundary(
            "invalid_installation_lookup",
            "安装实例查找键无效",
        ));
    }
    Ok(())
}

fn validate_session_label(value: &str) -> Result<(), AgentCommandError> {
    if value.is_empty()
        || value.len() > MAX_SESSION_LABEL_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(AgentCommandError::boundary(
            "invalid_session",
            "窗口会话标识无效",
        ));
    }
    Ok(())
}

fn validate_operation_id(value: &str) -> Result<(), AgentCommandError> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(AgentCommandError::boundary(
            "invalid_operation_id",
            "operation ID 无效",
        ));
    }
    Ok(())
}

fn decode_token(value: &str) -> Result<[u8; CONFIRMATION_TOKEN_BYTES], AgentCommandError> {
    if value.len() != CONFIRMATION_TOKEN_BYTES * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(AgentCommandError::boundary(
            "invalid_confirmation_token",
            "确认令牌格式无效",
        ));
    }
    let mut output = [0_u8; CONFIRMATION_TOKEN_BYTES];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(output)
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => 0,
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("String writes cannot fail");
    }
    output
}

fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

pub(crate) fn runtime_from_app(
    state: &AppStateManaged,
) -> Result<AgentProxyRuntime, AgentCommandError> {
    let inner = state
        .0
        .lock()
        .map_err(|_| AgentCommandError::boundary("app_state_poisoned", "应用状态不可用"))?;
    inner
        .ensure_editable()
        .map_err(AgentCommandError::internal)?;
    let serve = inner.serve_view();
    if serve.app_runtime != crate::AppRuntime::Running || !serve.listener_reachable {
        return Err(AgentCommandError::boundary(
            "proxy_not_running",
            "请先启动代理再生成或应用连接计划",
        ));
    }
    let origin = format!("http://{}", serve.listen)
        .trim_end_matches('/')
        .to_string();
    let serving = match &inner.server {
        crate::ServerLifecycle::Running { server, .. } => server,
        crate::ServerLifecycle::Applying { old, .. } => old,
        _ => {
            return Err(AgentCommandError::boundary(
                "proxy_not_running",
                "请先启动代理再生成或应用连接计划",
            ));
        }
    };
    let adapter_readiness = builtin_connectors()
        .iter()
        .map(|connector| connector.capabilities().adapter_id)
        .map(|adapter_id| {
            (
                adapter_id.to_string(),
                serving.agent_adapter_ready(adapter_id),
            )
        })
        .collect();
    let config = serving.serving_config();
    let mut model_metadata = BTreeMap::new();
    let mut connection_issues = BTreeMap::new();
    if !config.home_route_is_unconfigured() {
        for connector in builtin_connectors() {
            let agent_id = connector.agent_id();
            if model_metadata.contains_key(agent_id) {
                continue;
            }
            let router =
                serving_router_for_agent(serving, agent_id).map_err(AgentCommandError::internal)?;
            if let Some(metadata) = agent_model_metadata_for_router(config, &router)
                .map_err(AgentCommandError::internal)?
            {
                model_metadata.insert(agent_id.to_string(), metadata);
            }
        }
        let opencode_router =
            serving_router_for_agent(serving, "opencode").map_err(AgentCommandError::internal)?;
        if let Some(issue) = opencode_connection_issue(config, &opencode_router) {
            connection_issues.insert("opencode-v1".to_owned(), issue);
        }
    }
    let virtual_key = serve.virtual_key.ok_or_else(|| {
        AgentCommandError::boundary(
            "local_auth_disabled",
            "Agent 接入需要稳定的本地虚拟 Key；请先在设置中开启本地鉴权并重启代理",
        )
    })?;
    Ok(AgentProxyRuntime::new(
        serve.instance_id.ok_or_else(|| {
            AgentCommandError::boundary("proxy_not_running", "代理运行实例身份不可用")
        })?,
        &origin,
        virtual_key,
        adapter_readiness,
        model_metadata,
        connection_issues,
    ))
}

#[tauri::command]
pub(crate) fn list_agent_registry(state: State<'_, AgentCommandState>) -> Vec<AgentUiMetadata> {
    state.registry_metadata()
}

#[tauri::command(async)]
pub(crate) fn scan_agents(
    app: AppHandle,
    state: State<'_, AgentCommandState>,
    app_state: State<'_, AppStateManaged>,
) -> Result<Vec<AgentView>, AgentCommandError> {
    let runtime = runtime_from_app(app_state.inner()).ok();
    let opencode_issue = app_state
        .inner()
        .0
        .lock()
        .ok()
        .and_then(|inner| opencode_issue_from_inner(&inner).ok().flatten());
    let views = state.scan_with_runtime(runtime.as_ref(), opencode_issue.as_ref())?;
    crate::desktop_shell::update_agent_menu(&app);
    Ok(views)
}

#[tauri::command(async)]
pub(crate) fn get_cached_agent_views(
    state: State<'_, AgentCommandState>,
    app_state: State<'_, AppStateManaged>,
) -> Result<Vec<AgentView>, AgentCommandError> {
    let runtime = runtime_from_app(app_state.inner()).ok();
    let opencode_issue = app_state
        .inner()
        .0
        .lock()
        .ok()
        .and_then(|inner| opencode_issue_from_inner(&inner).ok().flatten());
    state.cached_views_with_runtime(runtime.as_ref(), opencode_issue.as_ref())
}

#[tauri::command(async)]
pub(crate) fn plan_agent_connection(
    state: State<'_, AgentCommandState>,
    app_state: State<'_, AppStateManaged>,
    window: WebviewWindow,
    agent_id: String,
    installation_path: String,
    expected_version: Option<String>,
) -> Result<ConfigPlanView, AgentCommandError> {
    let runtime = runtime_from_app(&app_state)?;
    state.refresh_scan()?;
    state.plan_connection(
        &agent_id,
        &installation_path,
        expected_version.as_deref(),
        window.label(),
        &runtime,
    )
}

#[tauri::command(async)]
pub(crate) fn apply_agent_plan(
    app: AppHandle,
    state: State<'_, AgentCommandState>,
    app_state: State<'_, AppStateManaged>,
    window: WebviewWindow,
    operation_id: String,
    confirmation_token: String,
) -> Result<TransactionOutcome, AgentCommandError> {
    let intent = state.plan_intent(&operation_id)?;
    let runtime = if intent == PlanIntent::Connect {
        Some(runtime_from_app(&app_state)?)
    } else {
        None
    };
    let outcome = state.apply(
        &operation_id,
        &confirmation_token,
        window.label(),
        &[PlanIntent::Connect, PlanIntent::Disconnect],
        runtime.as_ref(),
    )?;
    crate::desktop_shell::update_agent_menu(&app);
    Ok(outcome)
}

#[tauri::command(async)]
pub(crate) fn discard_agent_plan(
    state: State<'_, AgentCommandState>,
    window: WebviewWindow,
    operation_id: String,
    confirmation_token: String,
) -> Result<(), AgentCommandError> {
    state.discard_plan(&operation_id, &confirmation_token, window.label())
}

#[tauri::command(async)]
pub(crate) fn plan_agent_disconnect(
    state: State<'_, AgentCommandState>,
    window: WebviewWindow,
    agent_id: String,
    installation_path: String,
) -> Result<ConfigPlanView, AgentCommandError> {
    state.refresh_scan()?;
    state.plan_disconnect(&agent_id, &installation_path, window.label())
}

/// Force-disconnect fallback: remove managed fields and ownership when a lost key blocks normal snapshot restoration.
#[tauri::command(async)]
pub(crate) fn force_forget_agent(
    app: AppHandle,
    state: State<'_, AgentCommandState>,
    agent_id: String,
    installation_path: String,
) -> Result<(), AgentCommandError> {
    state.refresh_scan()?;
    state.force_forget(&agent_id, &installation_path)?;
    crate::desktop_shell::update_agent_menu(&app);
    Ok(())
}

#[tauri::command]
pub(crate) fn list_agent_snapshots(
    state: State<'_, AgentCommandState>,
    agent_id: String,
) -> Result<Vec<SnapshotView>, AgentCommandError> {
    state.list_snapshots(&agent_id)
}

#[tauri::command]
pub(crate) fn get_agent_drift(
    state: State<'_, AgentCommandState>,
    agent_id: String,
    installation_path: String,
) -> Result<Vec<AgentDriftView>, AgentCommandError> {
    state.drift(&agent_id, &installation_path)
}

#[tauri::command(async)]
pub(crate) fn plan_snapshot_restore(
    state: State<'_, AgentCommandState>,
    window: WebviewWindow,
    snapshot_id: String,
) -> Result<ConfigPlanView, AgentCommandError> {
    state.refresh_scan()?;
    state.plan_restore(&snapshot_id, window.label())
}

#[tauri::command(async)]
pub(crate) fn apply_snapshot_restore(
    state: State<'_, AgentCommandState>,
    window: WebviewWindow,
    operation_id: String,
    confirmation_token: String,
) -> Result<TransactionOutcome, AgentCommandError> {
    state.apply(
        &operation_id,
        &confirmation_token,
        window.label(),
        &[PlanIntent::Restore],
        None,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::agent_integration::config_codec::semantic_json;
    use crate::agent_integration::connectors::ConnectInput;
    use crate::agent_integration::plan::build_connection_plan;
    use crate::agent_integration::types::{
        AllowedAction, BinarySource, ConfigPath, ConfirmationKind, Diagnostic, DiscoveryEvidence,
        DiscoverySource, DriftScope, DriftStatus, PatchKind, Platform,
    };

    struct MemoryMasterKey;

    #[test]
    fn force_forget_reconnect_validator_covers_every_builtin_connector() {
        let fixtures = [
            ("claude-code-v1", DocumentFormat::Json, r#"{"env":{}}"#),
            ("claude-desktop-3p-v1", DocumentFormat::Json, "{}"),
            ("codex-v1", DocumentFormat::Toml, "[model_providers]\n"),
            ("gemini-cli-v1", DocumentFormat::Dotenv, ""),
            ("grok-build-v1", DocumentFormat::Toml, ""),
            ("kimi-code-v1", DocumentFormat::Toml, ""),
            ("deepseek-harness-v1", DocumentFormat::Yaml, "{}\n"),
            ("hermes-v1", DocumentFormat::Yaml, "model:\n"),
            (
                "openclaw-v1",
                DocumentFormat::Json5,
                "{ models: { providers: {} }, agents: { defaults: {} } }",
            ),
            ("opencode-v1", DocumentFormat::Json5, r#"{"provider":{}}"#),
            (
                "workbuddy-v1",
                DocumentFormat::Json,
                r#"{"models":[],"availableModels":[]}"#,
            ),
        ];

        for (connector_id, format, source) in fixtures {
            let connector = connector_for(connector_id).unwrap();
            let document = parse_rendered(source, format, connector.label()).unwrap();
            validate_force_forget_reconnect(connector, &document)
                .unwrap_or_else(|error| panic!("{connector_id}: {error}"));
        }
    }

    impl MasterKeyStore for MemoryMasterKey {
        fn load_or_create(&self, _allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
            self.load()
        }

        fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
            Ok(Zeroizing::new([23_u8; 32]))
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).unwrap();
        std::env::temp_dir().join(format!(
            "token-station-commands-{label}-{}-{}",
            std::process::id(),
            lower_hex(&random)
        ))
    }

    fn state(label: &str) -> AgentCommandState {
        let root = scratch(label);
        AgentCommandState::new_with_master_key(
            AgentIntegrationPaths {
                snapshot_root: root.join("snapshots"),
                ownership_root: root.join("ownership"),
            },
            Arc::new(MemoryMasterKey),
        )
        .unwrap()
    }

    #[test]
    fn command_state_starts_and_disables_restore_for_failed_legacy_migration() {
        let root = scratch("legacy-migration-startup");
        let paths = AgentIntegrationPaths {
            snapshot_root: root.join("snapshots"),
            ownership_root: root.join("ownership"),
        };
        let keys: Arc<dyn MasterKeyStore> = Arc::new(MemoryMasterKey);
        let store = FileSnapshotStore::new(paths.snapshot_root.clone(), keys.clone());
        let source = ConfigSource::existing(b"legacy".to_vec(), Some(0o600), None);
        let record = store
            .create(crate::agent_integration::snapshot::SnapshotRequest {
                operation_id: "legacy-migration-operation",
                agent_id: "claude-code",
                target_config_path: "/tmp/.claude/settings.json",
                before_hash: crate::agent_integration::plan::file_revision_hash(
                    Path::new("/tmp/.claude/settings.json"),
                    &source,
                )
                .unwrap(),
                source: &source,
                created_at_ms: 1,
                connector_id: "claude-code-v1",
                app_version: "test",
                pinned: true,
            })
            .unwrap()
            .record;
        let current = std::fs::read_dir(paths.snapshot_root.join("claude-code"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let legacy = paths
            .snapshot_root
            .join(format!("{}.snapshot.json", record.snapshot_id));
        std::fs::rename(&current, &legacy).unwrap();
        let mut damaged = std::fs::read(&legacy).unwrap();
        let damaged_index = damaged.len() / 2;
        damaged[damaged_index] ^= 1;
        crate::agent_integration::safe_fs::write_atomic_private(&legacy, &damaged).unwrap();

        let state = AgentCommandState::new_with_master_key(paths, keys)
            .expect("one damaged legacy snapshot must not abort desktop setup");
        let snapshots = state.list_snapshots("claude-code").unwrap();
        let damaged_view = snapshots
            .iter()
            .find(|view| view.snapshot_id == record.snapshot_id)
            .unwrap();
        assert!(!damaged_view.restorable);
        assert!(damaged_view
            .maintenance_warning
            .as_deref()
            .unwrap()
            .contains("哈希不匹配"));
        let error = match state.plan_restore(&record.snapshot_id, "main") {
            Ok(_) => panic!("a failed legacy migration must disable restore"),
            Err(error) => error,
        };
        assert_eq!(error.code, "snapshot_migration_failed");
        std::fs::remove_dir_all(root).ok();
    }

    fn record(target: &Path, conflict: bool) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: "claude-code".to_string(),
            executable_path: "/opt/claude".to_string(),
            canonical_path: "/opt/claude".to_string(),
            binary_source: BinarySource::Path,
            modified_at_ms: None,
            binary_sha256: None,
            upgrade_command: None,
            version_raw: Some("2.1.211".to_string()),
            version_normalized: Some("2.1.211".to_string()),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: "/opt/claude".to_string(),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: vec![target.to_str().unwrap().to_string()],
            config_fingerprint: None,
            conflict_group: conflict.then(|| "fixture-conflict".to_string()),
            diagnostics: Vec::<Diagnostic>::new(),
            scanned_at_ms: 1,
        }
    }

    fn decision() -> CompatibilityDecision {
        CompatibilityDecision {
            agent_id: "claude-code".to_string(),
            installation_path: Some("/opt/claude".to_string()),
            status: CompatibilityStatus::DetectedVerified,
            reason_code: ReasonCode::DefaultAdmission,
            message: "admitted".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("claude-code-v1".to_string()),
            allowed_actions: BTreeSet::from([AllowedAction::PreviewConnect]),
        }
    }

    fn prepared(target: &Path, secret: &str, now_ms: u64) -> PreparedChangePlan {
        let source = read_config_source(target).unwrap();
        build_connection_plan(
            connector_for("claude-code-v1").unwrap(),
            &record(target, false),
            &decision(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some(secret),
                adapter_ready: true,
                model_metadata: None,
            },
            1,
            None,
            now_ms,
            generate_operation_id().unwrap(),
        )
        .unwrap()
    }

    fn runtime(token: &str) -> AgentProxyRuntime {
        let adapter_readiness = builtin_connectors()
            .iter()
            .map(|connector| (connector.capabilities().adapter_id.to_string(), true))
            .collect();
        AgentProxyRuntime::new(
            "fixture-runtime".to_string(),
            "http://127.0.0.1:8787",
            token.to_string(),
            adapter_readiness,
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    #[test]
    fn owned_exact_installation_resolves_multi_install_refresh_selection() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();
        let target = Path::new("/tmp/token-station-owned-claude/settings.json");
        let conflicted = record(target, true);
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();

        assert_eq!(
            evaluate_discovery(&catalog, descriptor, &conflicted).status,
            CompatibilityStatus::MultipleInstallations
        );
        let selected = exact_installation_selection(&conflicted);
        let decision = evaluate_discovery(&catalog, descriptor, &selected);

        assert_eq!(selected.canonical_path, conflicted.canonical_path);
        assert_eq!(decision.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(decision.connector_id.as_deref(), Some("claude-code-v1"));
    }

    #[test]
    fn agent_projection_uses_each_effective_route_safe_limits_and_uniform_costs() {
        let root = scratch("opencode-model-metadata");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        for (name, context, output) in [
            ("provider_a", 257_550, 32_768),
            ("provider_b", 128_000, 16_384),
        ] {
            draft["upstreams"][name] = json!({
                "provider": "openai-compatible",
                "base_url": format!("https://{name}.example/v1"),
                "models": [{
                    "model": "glm-5.2",
                    "tool": true,
                    "vision": true,
                    "supported_parameters": ["reasoning_effort"],
                    "context_window": context,
                    "max_output_tokens": output,
                    "catalog_cost": {"input": 0.2, "output": 0.6, "cache_read": 0.04}
                }]
            });
        }
        draft["router"]["pools"] = json!({
            "tier_high": [{"upstream": "provider_a", "model": "glm-5.2"}],
            "tier_low": [{"upstream": "provider_b", "model": "glm-5.2"}]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft.clone()).unwrap();

        let metadata = agent_model_metadata(&config, "opencode").unwrap().unwrap();
        assert_eq!(metadata.context, 128_000);
        assert_eq!(metadata.output, 16_384);
        assert!(metadata.vision);
        assert!(metadata.tools);
        assert!(metadata.reasoning);
        assert_eq!(metadata.cost.as_ref().map(|cost| cost.input), Some(0.2));

        draft["agent_routes"]["workbuddy"] = json!({
            "mode": "custom",
            "routing_mode": "tiered",
            "custom_route": {
                "high": {"upstream": "provider_a", "model": "glm-5.2"},
                "mid": {"upstream": "provider_a", "model": "glm-5.2"},
                "low": {"upstream": "provider_a", "model": "glm-5.2"}
            }
        });
        let workbuddy_config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft.clone()).unwrap();
        let workbuddy = agent_model_metadata(&workbuddy_config, "workbuddy")
            .unwrap()
            .unwrap();
        assert_eq!(workbuddy.context, 257_550);
        assert_eq!(workbuddy.output, 32_768);

        draft["upstreams"]["provider_b"]["models"][0]["catalog_cost"]["output"] = json!(0.7);
        draft["upstreams"]["provider_b"]["models"][0]["vision"] = json!(false);
        draft["upstreams"]["provider_b"]["models"][0]["tool"] = json!(false);
        draft["upstreams"]["provider_b"]["models"][0]["supported_parameters"] = json!([]);
        let mixed: token_station_cli::config::ClientConfig = serde_json::from_value(draft).unwrap();
        let mixed = agent_model_metadata(&mixed, "opencode").unwrap().unwrap();
        assert_eq!(mixed.cost, None);
        assert!(!mixed.vision);
        assert!(!mixed.tools);
        assert!(!mixed.reasoning);
    }

    #[test]
    fn agent_projection_falls_back_to_complete_configured_price_for_partial_catalog_costs() {
        let root = scratch("partial-catalog-price-fallback");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        for name in ["provider_a", "provider_b"] {
            draft["upstreams"][name] = json!({
                "provider": "openai-compatible",
                "base_url": format!("https://{name}.example/v1"),
                "models": [{
                    "model": "glm-5.2",
                    "tool": true,
                    "vision": true,
                    "context_window": 257550,
                    "max_output_tokens": 32768,
                    "catalog_cost": {"input": 99.0}
                }]
            });
        }
        draft["router"]["pools"] = json!({
            "tier_low": [
                {"upstream": "provider_a", "model": "glm-5.2"},
                {"upstream": "provider_b", "model": "glm-5.2"}
            ]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        draft["pricing"] = json!({
            "version": 1,
            "models": {
                "glm-5.2": {
                    "input_per_mtok": 200000,
                    "output_per_mtok": 600000,
                    "cache_read_per_mtok": 40000,
                    "cache_write_per_mtok": 0
                }
            }
        });
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        let metadata = agent_model_metadata(&config, "opencode").unwrap().unwrap();
        let cost = metadata
            .cost
            .expect("the complete configured price is used");
        assert_eq!(cost.input, 0.2);
        assert_eq!(cost.output, 0.6);
        assert_eq!(cost.cache_read, Some(0.04));
        assert_eq!(cost.cache_write, Some(0.0));
    }

    #[test]
    fn incomplete_limits_do_not_discard_verified_capabilities_or_price() {
        let root = scratch("partial-limits-keep-capabilities");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        draft["upstreams"]["provider_a"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://provider-a.example/v1",
            "models": [{
                "model": "glm-5.2",
                "tool": true,
                "vision": true,
                "supported_parameters": ["reasoning_effort"],
                "context_window": 257550,
                "catalog_cost": {"input": 0.2, "output": 0.6}
            }]
        });
        draft["router"]["pools"] = json!({
            "tier_low": [{"upstream": "provider_a", "model": "glm-5.2"}]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        let metadata = agent_model_metadata(&config, "opencode")
            .unwrap()
            .expect("independently verified metadata remains available");
        assert_eq!(metadata.safe_limits(), None);
        assert!(metadata.vision);
        assert!(metadata.tools);
        assert!(metadata.reasoning);
        assert_eq!(metadata.cost.as_ref().map(|cost| cost.output), Some(0.6));
    }

    #[test]
    fn opencode_refuses_assumed_context_and_reports_the_exact_missing_target() {
        let root = scratch("opencode-fail-closed-limits");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://provider.example/v1",
            "models": [{
                "model": "unknown-context",
                "context_window": 0,
                "max_output_tokens": 1
            }]
        });
        draft["router"]["assumed_context_window"] = json!(128_000);
        draft["router"]["pools"] = json!({
            "tier_low": [{"upstream": "provider", "model": "unknown-context"}]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        let issue = opencode_connection_issue(&config, &config.router)
            .expect("an assumed context is not a trustworthy provider fact");

        assert_eq!(issue.code, "model_contract_missing_context_window");
        assert_eq!(issue.target.as_deref(), Some("provider/unknown-context"));
        assert!(issue.message.contains("不会使用路由假定值"));
    }

    #[test]
    fn opencode_accepts_a_trusted_context_with_the_safe_default_output() {
        let root = scratch("opencode-safe-default-output");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://provider.example/v1",
            "models": [{
                "model": "known-context",
                "context_window": 128_000
            }]
        });
        draft["router"]["pools"] = json!({
            "tier_low": [{"upstream": "provider", "model": "known-context"}]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        assert_eq!(opencode_connection_issue(&config, &config.router), None);
        let metadata = agent_model_metadata(&config, "opencode")
            .unwrap()
            .expect("trusted context remains available");
        assert_eq!(metadata.safe_limits(), None);
        assert_eq!(metadata.opencode_limits(), Some((128_000, 8_192)));
    }

    #[test]
    fn kimi_inherited_direct_route_projects_its_configured_context_window() {
        let root = scratch("kimi-direct-route-context");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["upstreams"]["deepseek"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://deepseek.example/v1",
            "models": [{
                "model": "deepseek-v4-flash",
                "context_window": 128_000,
                "tool": true,
                "vision": false
            }]
        });
        draft["routing"] = json!({
            "mode": "direct",
            "direct_target": {
                "upstream": "deepseek",
                "model": "deepseek-v4-flash"
            }
        });
        draft["agent_routes"]["kimi-code"] = json!({
            "mode": "inherit",
            "routing_mode": "direct"
        });
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        let metadata = agent_model_metadata(&config, "kimi-code")
            .unwrap()
            .expect("the inherited direct route has one configured model");

        assert_eq!(metadata.context, 128_000);
    }

    #[test]
    fn opencode_rejects_the_safe_default_when_it_leaves_no_input_budget() {
        let root = scratch("opencode-default-output-too-large");
        let mut draft = crate::template(&root.join("data"), &root.join("plugins"));
        draft["routing"]["mode"] = json!("tiered");
        draft["upstreams"]["provider"] = json!({
            "provider": "openai-compatible",
            "base_url": "https://provider.example/v1",
            "models": [{
                "model": "tiny-context",
                "context_window": 8_192
            }]
        });
        draft["router"]["pools"] = json!({
            "tier_low": [{"upstream": "provider", "model": "tiny-context"}]
        });
        draft["router"]["default_pool"] = json!("tier_low");
        let config: token_station_cli::config::ClientConfig =
            serde_json::from_value(draft).unwrap();

        let issue = opencode_connection_issue(&config, &config.router)
            .expect("the default output must leave a positive input budget");

        assert_eq!(issue.code, "model_contract_invalid_limits");
        assert_eq!(issue.target.as_deref(), Some("provider/tiny-context"));
        assert!(issue.message.contains("8192"));
    }

    struct LifecycleFileFixture {
        path: PathBuf,
        baseline: &'static [u8],
        format: DocumentFormat,
        marker: &'static str,
    }

    struct LifecycleCase {
        label: &'static str,
        agent_id: &'static str,
        connector_id: &'static str,
        version: &'static str,
        installation_path: String,
        primary: LifecycleFileFixture,
        companions: Vec<LifecycleFileFixture>,
    }

    fn non_codex_lifecycle_cases(root: &Path) -> Vec<LifecycleCase> {
        let claude_desktop_support = root.join("claude-desktop/Application Support");
        let claude_desktop_library = claude_desktop_support
            .join("Claude-3p")
            .join("configLibrary");
        let claude_desktop_primary =
            claude_desktop_library.join("7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2.json");

        vec![
            LifecycleCase {
                label: "claude-code",
                agent_id: "claude-code",
                connector_id: "claude-code-v1",
                version: "2.1.211",
                installation_path: root
                    .join("install/claude")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.claude/settings.json"),
                    baseline: br#"{"env":null,"keep":"claude-code"}"#,
                    format: DocumentFormat::Json,
                    marker: "claude-code",
                },
                companions: Vec::new(),
            },
            LifecycleCase {
                label: "claude-desktop",
                agent_id: "claude-desktop",
                connector_id: "claude-desktop-3p-v1",
                version: "1.0.0",
                installation_path: root
                    .join("install/Claude.app/Contents/MacOS/Claude")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: claude_desktop_primary,
                    baseline: br#"{"keep":"claude-desktop"}"#,
                    format: DocumentFormat::Json,
                    marker: "claude-desktop",
                },
                companions: vec![
                    LifecycleFileFixture {
                        path: claude_desktop_library.join("_meta.json"),
                        baseline: br#"{"appliedId":"user-profile","entries":[{"id":"user-profile","name":"User"}],"keep":"desktop-meta"}"#,
                        format: DocumentFormat::Json,
                        marker: "user-profile",
                    },
                    LifecycleFileFixture {
                        path: claude_desktop_support.join("Claude/config.json"),
                        baseline: br#"{"keep":"desktop-official"}"#,
                        format: DocumentFormat::Json,
                        marker: "desktop-official",
                    },
                    LifecycleFileFixture {
                        path: claude_desktop_support.join("Claude-3p/config.json"),
                        baseline: br#"{"keep":"desktop-3p"}"#,
                        format: DocumentFormat::Json,
                        marker: "desktop-3p",
                    },
                ],
            },
            LifecycleCase {
                label: "gemini-cli",
                agent_id: "gemini-cli",
                connector_id: "gemini-cli-v1",
                version: "1.0.0",
                installation_path: root
                    .join("install/gemini")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.gemini/.env"),
                    baseline: b"UNOWNED=gemini\n",
                    format: DocumentFormat::Dotenv,
                    marker: "UNOWNED=gemini",
                },
                companions: vec![LifecycleFileFixture {
                    path: root.join("home/.gemini/settings.json"),
                    baseline: br#"{"security":null,"keep":"gemini-settings"}"#,
                    format: DocumentFormat::Json,
                    marker: "gemini-settings",
                }],
            },
            LifecycleCase {
                label: "hermes",
                agent_id: "nous-hermes-agent",
                connector_id: "hermes-v1",
                version: "0.18.0",
                installation_path: root
                    .join("install/hermes")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.hermes/config.yaml"),
                    baseline: b"# keep Hermes comment\nmodel:\nfallback_providers: []\n",
                    format: DocumentFormat::Yaml,
                    marker: "fallback_providers",
                },
                companions: Vec::new(),
            },
            LifecycleCase {
                label: "openclaw",
                agent_id: "openclaw",
                connector_id: "openclaw-v1",
                version: "1.0.0",
                installation_path: root
                    .join("install/openclaw")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.openclaw/openclaw.json"),
                    baseline: b"{ // keep OpenClaw comment\n models: null, agents: null, keep: 'openclaw'\n}\n",
                    format: DocumentFormat::Json5,
                    marker: "openclaw",
                },
                companions: Vec::new(),
            },
            LifecycleCase {
                label: "opencode",
                agent_id: "opencode",
                connector_id: "opencode-v1",
                version: "1.18.2",
                installation_path: root
                    .join("install/opencode")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.config/opencode/opencode.json"),
                    baseline: br#"{"provider":null,"keep":"opencode"}"#,
                    format: DocumentFormat::Json5,
                    marker: "opencode",
                },
                companions: Vec::new(),
            },
            LifecycleCase {
                label: "workbuddy",
                agent_id: "workbuddy",
                connector_id: "workbuddy-v1",
                version: "1.0.0",
                installation_path: root
                    .join("install/WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy")
                    .to_string_lossy()
                    .into_owned(),
                primary: LifecycleFileFixture {
                    path: root.join("home/.workbuddy/models.json"),
                    baseline: br#"{"models":[{"id":"user-model","name":"User"}],"availableModels":["user-model"],"keep":"workbuddy"}"#,
                    format: DocumentFormat::Json,
                    marker: "user-model",
                },
                companions: Vec::new(),
            },
        ]
    }

    fn seed_lifecycle_case(case: &LifecycleCase) {
        for file in std::iter::once(&case.primary).chain(&case.companions) {
            std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
            std::fs::write(&file.path, file.baseline).unwrap();
        }
    }

    fn lifecycle_record(case: &LifecycleCase) -> DiscoveryRecord {
        let mut installation = record(&case.primary.path, false);
        installation.agent_id = case.agent_id.to_string();
        installation.executable_path = case.installation_path.clone();
        installation.canonical_path = case.installation_path.clone();
        installation.version_raw = Some(case.version.to_string());
        installation.version_normalized = Some(case.version.to_string());
        installation.evidence[0].observed_path = case.installation_path.clone();
        installation
    }

    fn apply_lifecycle_connection(
        state: &AgentCommandState,
        case: &LifecycleCase,
        runtime: &AgentProxyRuntime,
        session_label: &str,
    ) -> ConfigPlanView {
        let plan = state
            .plan_connection(
                case.agent_id,
                &case.installation_path,
                Some(case.version),
                session_label,
                runtime,
            )
            .unwrap_or_else(|error| panic!("{} plan failed: {}", case.label, error.message));
        assert_eq!(plan.plan.connector_id, case.connector_id);
        assert_eq!(
            plan.plan.projection.files.len(),
            1 + case.companions.len(),
            "{} must include every companion in the public projection",
            case.label
        );
        let public_plan = serde_json::to_string(&plan.plan).unwrap();
        assert!(public_plan.contains(runtime.virtual_key()));
        state
            .apply_from_cached_scan(
                &plan.plan.operation_id,
                &plan.confirmation_token,
                session_label,
                &[PlanIntent::Connect],
                Some(runtime),
            )
            .unwrap_or_else(|error| panic!("{} apply failed: {}", case.label, error.message));
        plan
    }

    fn lifecycle_files(case: &LifecycleCase) -> impl Iterator<Item = &LifecycleFileFixture> {
        std::iter::once(&case.primary).chain(&case.companions)
    }

    fn assert_lifecycle_files(
        case: &LifecycleCase,
        forbidden_tokens: &[&str],
        expected_token: Option<&str>,
    ) {
        let connector = connector_for(case.connector_id).unwrap();
        for file in lifecycle_files(case) {
            let bytes = std::fs::read(&file.path).unwrap_or_else(|error| {
                panic!(
                    "{} cannot read {}: {error}",
                    case.label,
                    file.path.display()
                )
            });
            parse_source_bytes(Some(&bytes), file.format, case.label).unwrap_or_else(|error| {
                panic!(
                    "{} cannot parse {}: {error}",
                    case.label,
                    file.path.display()
                )
            });
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                text.contains(file.marker),
                "{} must preserve marker {} in {}",
                case.label,
                file.marker,
                file.path.display()
            );
            for token in forbidden_tokens {
                assert!(
                    !text.contains(token),
                    "{} leaked an obsolete credential into {}",
                    case.label,
                    file.path.display()
                );
            }
            #[cfg(unix)]
            if expected_token.is_some() {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{} must keep managed files private",
                    case.label
                );
            }
        }

        if let Some(token) = expected_token {
            let primary = std::fs::read(&case.primary.path).unwrap();
            let document =
                parse_source_bytes(Some(&primary), case.primary.format, case.label).unwrap();
            let fixture_runtime = runtime(token);
            connector
                .validate_projected(
                    &document,
                    &fixture_runtime.input_for(case.connector_id).unwrap(),
                )
                .unwrap_or_else(|error| {
                    panic!("{} projected validation failed: {error}", case.label)
                });
            assert!(String::from_utf8_lossy(&primary).contains(token));
        }
    }

    fn assert_lifecycle_ownership(
        state: &AgentCommandState,
        case: &LifecycleCase,
        expected: usize,
    ) {
        let ownership = state
            .ownership
            .list_agent_installation(case.agent_id, &case.installation_path)
            .unwrap();
        assert_eq!(ownership.len(), expected, "{} ownership count", case.label);
        if let Some(record) = ownership.first() {
            assert_eq!(record.connector_id, case.connector_id);
            assert_eq!(record.companion_files.len(), case.companions.len());
        }
    }

    fn clean_lifecycle_case(state: &AgentCommandState, root: &Path) {
        std::fs::remove_dir_all(root).ok();
        if let Some(state_root) = state.paths.snapshot_root.parent() {
            std::fs::remove_dir_all(state_root).ok();
        }
    }

    // Only these three tests use it, and they are off under this feature.
    #[cfg(not(feature = "bundled-plugins"))]
    fn running_app_with_adapters(
        label: &str,
        installed_agents: &[&str],
        configured_agents: &[&str],
    ) -> (PathBuf, AppStateManaged) {
        let root = scratch(label);
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        let plugin_fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("plugins-dist");
        for (package, wasm_file) in installed_agents
            .iter()
            .map(|agent| (*agent, "adapter.wasm"))
            .chain([("provider-openai-compatible-v2", "component.wasm")])
        {
            let source = plugin_fixtures.join(package);
            let target = plugins_dir.join(package);
            std::fs::create_dir_all(&target).unwrap();
            for file in ["manifest.json", wasm_file] {
                std::fs::copy(source.join(file), target.join(file)).unwrap();
            }
        }

        let data_dir = root.join("data");
        let mut draft = crate::template(&data_dir, &plugins_dir);
        draft
            .as_object_mut()
            .expect("config fixture is an object")
            .remove("routing");
        draft["server"]["listen"] = json!("127.0.0.1:0");
        draft["server"]["auth"] = json!(true);
        draft["data"]["metrics"] = json!(false);
        draft["plugins"]["agents"] = json!(configured_agents);
        draft["upstreams"]["local"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:11434/v1",
            "models": [{ "model": "small" }]
        });
        draft["router"]["pools"] = json!({
            "tier_low": [{ "upstream": "local", "model": "small" }]
        });
        draft["router"]["default_pool"] = json!("tier_low");

        let config =
            serde_json::from_value::<token_station_cli::config::ClientConfig>(draft.clone())
                .unwrap();
        let running = crate::serve_lifecycle::prepare_server(config)
            .unwrap()
            .bind()
            .unwrap()
            .publish(7, std::collections::BTreeMap::new())
            .unwrap();
        let mut inner = crate::AppInner::new(root.join("token-station.json"), draft, None);
        inner.server = crate::ServerLifecycle::Running {
            generation: 1,
            server: running,
            apply_error: None,
        };
        (root, AppStateManaged(Mutex::new(inner)))
    }

    // Only these three tests use it, and they are off under this feature.
    #[cfg(not(feature = "bundled-plugins"))]
    fn running_app_with_skipped_openai_adapter(label: &str) -> (PathBuf, AppStateManaged) {
        running_app_with_adapters(
            label,
            &["agent-anthropic"],
            &["agent-anthropic", "agent-openai"],
        )
    }

    // Only these three tests use it, and they are off under this feature.
    #[cfg(not(feature = "bundled-plugins"))]
    fn stop_running_app(state: &AppStateManaged) {
        let running = {
            let mut inner = state.0.lock().unwrap();
            let lifecycle = std::mem::replace(
                &mut inner.server,
                crate::ServerLifecycle::Stopped { generation: 2 },
            );
            match lifecycle {
                crate::ServerLifecycle::Running { server, .. } => server,
                crate::ServerLifecycle::Applying { old, .. } => old,
                _ => panic!("fixture runtime must be serving"),
            }
        };
        running.drain_and_shutdown();
    }

    fn install_scan(
        state: &AgentCommandState,
        catalog: CompatibilityCatalog,
        records: Vec<DiscoveryRecord>,
    ) {
        state.session.lock().unwrap().scan = Some(ScanSnapshot {
            catalog,
            source: CatalogSource::Builtin,
            warning: None,
            records,
        });
    }

    #[test]
    fn commands_plan_token_is_session_bound_one_shot_and_exposes_local_confirmation_values() {
        let state = state("token");
        let target = scratch("target").join("settings.json");
        let now_ms = state.clock.now_ms();
        let prepared = prepared(&target, "vk-command-secret", now_ms);
        let view = state
            .issue_plan(prepared, &record(&target, false), "main", Some([7_u8; 32]))
            .unwrap();
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(encoded.contains("vk-command-secret"));
        assert!(!encoded.contains("projected_bytes"));

        let wrong_session = state
            .take_plan(
                &view.plan.operation_id,
                &view.confirmation_token,
                "other-window",
                &[PlanIntent::Connect],
            )
            .err()
            .expect("cross-window token is rejected without consuming the plan");
        assert_eq!(wrong_session.code, "operation_session_or_intent_mismatch");
        let wrong_token = state
            .take_plan(
                &view.plan.operation_id,
                &"00".repeat(CONFIRMATION_TOKEN_BYTES),
                "main",
                &[PlanIntent::Connect],
            )
            .err()
            .expect("wrong token is rejected without consuming the plan");
        assert_eq!(wrong_token.code, "confirmation_token_mismatch");
        let taken = state
            .take_plan(
                &view.plan.operation_id,
                &view.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .unwrap();
        assert_eq!(taken.proxy_binding, Some([7_u8; 32]));
        assert!(state
            .take_plan(
                &view.plan.operation_id,
                &view.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .is_err());
    }

    #[test]
    fn commands_discard_consumes_only_the_exact_pending_plan() {
        let state = state("discard");
        let target = scratch("discard-target").join("settings.json");
        let prepared = prepared(&target, "vk-discard-secret", state.clock.now_ms());
        let view = state
            .issue_plan(prepared, &record(&target, false), "main", None)
            .unwrap();

        let wrong_token = state
            .discard_plan(
                &view.plan.operation_id,
                &"00".repeat(CONFIRMATION_TOKEN_BYTES),
                "main",
            )
            .unwrap_err();
        assert_eq!(wrong_token.code, "confirmation_token_mismatch");
        assert!(state.plan_intent(&view.plan.operation_id).is_ok());

        state
            .discard_plan(&view.plan.operation_id, &view.confirmation_token, "main")
            .unwrap();
        assert!(state.plan_intent(&view.plan.operation_id).is_err());
    }

    #[test]
    fn commands_token_cannot_authorize_another_plan_or_wrong_endpoint() {
        let state = state("cross-plan");
        let target = scratch("cross-plan-target").join("settings.json");
        let now_ms = state.clock.now_ms();
        let first = state
            .issue_plan(
                prepared(&target, "vk-first", now_ms),
                &record(&target, false),
                "main",
                None,
            )
            .unwrap();
        let mut second_plan = prepared(&target, "vk-second", now_ms);
        second_plan.view.intent = PlanIntent::Restore;
        let second = state
            .issue_plan(second_plan, &record(&target, false), "main", None)
            .unwrap();
        assert!(state
            .take_plan(
                &second.plan.operation_id,
                &first.confirmation_token,
                "main",
                &[PlanIntent::Restore],
            )
            .is_err());
        assert!(state
            .take_plan(
                &second.plan.operation_id,
                &second.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .is_err());
        assert!(state
            .take_plan(
                &second.plan.operation_id,
                &second.confirmation_token,
                "main",
                &[PlanIntent::Restore],
            )
            .is_ok());
    }

    #[test]
    fn commands_installation_path_is_only_an_exact_scan_lookup_key() {
        let state = state("lookup");
        let registry_metadata = state.registry_metadata();
        assert_eq!(registry_metadata.len(), 12);
        assert_eq!(registry_metadata[0].agent_id, "claude-code");
        let target = scratch("lookup-target").join("settings.json");
        let registry = AgentRegistry::builtin().unwrap();
        let snapshot = ScanSnapshot {
            catalog: CompatibilityCatalog::builtin(&registry).unwrap(),
            source: CatalogSource::Builtin,
            warning: None,
            records: vec![record(&target, true)],
        };
        let (selected, selected_decision) = snapshot
            .selected(&registry, "claude-code", "/opt/claude")
            .unwrap();
        assert!(selected.conflict_group.is_none());
        assert_eq!(
            selected_decision.status,
            CompatibilityStatus::DetectedVerified
        );
        for injected in [
            "/opt/claude/../arbitrary",
            "/tmp/not-scanned",
            "../../etc/passwd",
        ] {
            assert!(snapshot
                .selected(&registry, "claude-code", injected)
                .is_err());
        }
        let empty = ScanSnapshot {
            catalog: CompatibilityCatalog::builtin(&state.registry).unwrap(),
            source: CatalogSource::Builtin,
            warning: None,
            records: Vec::new(),
        };
        let views = state.views(&empty, None, None).unwrap();
        assert_eq!(views.len(), 12);
        assert!(views.iter().all(|view| {
            view.status == CompatibilityStatus::NotDetected && view.installations.is_empty()
        }));
    }

    #[test]
    fn commands_display_scan_is_single_flight() {
        let state = state("single-flight");
        state
            .scan_in_progress
            .store(true, std::sync::atomic::Ordering::Release);

        let error = match state.scan() {
            Ok(_) => panic!("a duplicate scan must fail closed"),
            Err(error) => error,
        };

        assert_eq!(error.code, "scan_in_progress");
    }

    #[test]
    fn commands_cached_views_require_a_snapshot_and_do_not_start_a_scan() {
        let state = state("cached-views");
        let missing = state
            .cached_views_with_runtime(None, None)
            .err()
            .expect("cached views must fail before the startup scan");
        assert_eq!(missing.code, "scan_required");

        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, Vec::new());
        state
            .scan_in_progress
            .store(true, std::sync::atomic::Ordering::Release);

        let views = state.cached_views_with_runtime(None, None).unwrap();

        assert_eq!(views.len(), 12);
        assert!(views.iter().all(|view| view.installations.is_empty()));
        assert!(state.scan_in_progress.load(Ordering::Acquire));
    }

    #[test]
    fn commands_keep_discovery_visible_when_ownership_state_is_unreadable() {
        let state = state("ownership-read-degrade");
        super::super::safe_fs::ensure_private_dir(&state.paths.ownership_root).unwrap();
        super::super::safe_fs::write_atomic_private(
            &state.paths.ownership_root.join("ownership-index.json"),
            b"not-json",
        )
        .unwrap();
        let target = scratch("ownership-read-degrade-target").join("settings.json");
        let snapshot = ScanSnapshot {
            catalog: CompatibilityCatalog::builtin(&state.registry).unwrap(),
            source: CatalogSource::Builtin,
            warning: None,
            records: vec![record(&target, false)],
        };

        let views = state
            .views(&snapshot, Some(&runtime("vk-ownership-read-degrade")), None)
            .expect("read-only discovery remains available");
        let installation = &views
            .iter()
            .find(|view| view.metadata.agent_id == "claude-code")
            .unwrap()
            .installations[0];
        assert_eq!(
            installation.compatibility.status,
            CompatibilityStatus::DetectedUnknown
        );
        assert_eq!(
            installation.compatibility.reason_code,
            ReasonCode::ReadOnlyPreflightFailed
        );
        assert!(!installation
            .compatibility
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));
        assert!(!installation.managed);
        assert!(!installation.connected);
    }

    #[test]
    fn commands_apply_keeps_the_plan_when_refresh_cannot_start() {
        let state = state("apply-refresh-failure");
        let target = scratch("apply-refresh-failure-target").join("settings.json");
        let runtime = runtime("vk-refresh-failure");
        let plan = state
            .issue_plan(
                prepared(&target, "vk-refresh-failure", state.clock.now_ms()),
                &record(&target, false),
                "main",
                Some(runtime.fingerprint()),
            )
            .unwrap();
        state.scan_in_progress.store(true, Ordering::Release);

        let error = state
            .apply(
                &plan.plan.operation_id,
                &plan.confirmation_token,
                "main",
                &[PlanIntent::Connect],
                Some(&runtime),
            )
            .unwrap_err();

        assert_eq!(error.code, "scan_in_progress");
        assert_eq!(
            state.plan_intent(&plan.plan.operation_id).unwrap(),
            PlanIntent::Connect
        );
        state.scan_in_progress.store(false, Ordering::Release);
    }

    #[test]
    fn commands_rejects_expired_plan_before_issuing_a_token() {
        let state = state("expired");
        let target = scratch("expired-target").join("settings.json");
        let mut plan = prepared(&target, "vk-expired", state.clock.now_ms());
        plan.view.expires_at_ms = state.clock.now_ms().saturating_sub(1);
        let error = state
            .issue_plan(plan, &record(&target, false), "main", None)
            .err()
            .expect("expired plan is rejected");
        assert_eq!(error.code, "plan_already_expired");
    }

    #[test]
    fn commands_reject_reconnect_plan_when_ownership_exists_for_an_old_runtime() {
        let state = state("owned-old-runtime");
        let root = scratch("owned-old-runtime-target");
        let target = root.join("opencode.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&target, br#"{"unowned":"keep"}"#).unwrap();

        let mut installation = record(&target, false);
        installation.agent_id = "opencode".to_string();
        installation.executable_path = "/opt/opencode".to_string();
        installation.canonical_path = "/opt/opencode".to_string();
        installation.version_raw = Some("1.18.2".to_string());
        installation.version_normalized = Some("1.18.2".to_string());
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![installation]);

        let old_runtime = runtime("vk-opencode-old-runtime");
        let connection = state
            .plan_connection(
                "opencode",
                "/opt/opencode",
                Some("1.18.2"),
                "main",
                &old_runtime,
            )
            .unwrap();
        let taken = state
            .take_plan(
                &connection.plan.operation_id,
                &connection.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .unwrap();
        let now_ms = state.clock.now_ms();
        let engine = TransactionEngine::new(
            &state.snapshots,
            &state.ownership,
            state.keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &state.clock,
        );
        engine
            .apply_connection(
                &taken.prepared,
                &ConfirmedOperation {
                    operation_id: taken.prepared.view.operation_id.clone(),
                    confirmed_at_ms: now_ms,
                    confirmations: taken
                        .prepared
                        .view
                        .required_confirmations
                        .iter()
                        .copied()
                        .collect(),
                },
                &RuntimeAdmission {
                    compatibility_sequence: taken.prepared.view.compatibility_sequence,
                    status: CompatibilityStatus::DetectedVerified,
                },
                now_ms,
            )
            .unwrap();

        let changed_runtime = runtime("vk-opencode-new-runtime");
        let error = state
            .plan_connection(
                "opencode",
                "/opt/opencode",
                Some("1.18.2"),
                "main",
                &changed_runtime,
            )
            .err()
            .expect("active ownership must block a second connect plan");
        assert_eq!(error.code, "ownership_repair_required");
    }

    #[test]
    fn commands_plan_is_memory_only_blocked_is_rejected_and_version_binding_is_conditional() {
        let state = state("plan-boundary");
        let target = scratch("plan-boundary-target").join("missing/settings.json");
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog.clone(), vec![record(&target, false)]);
        let plan = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("2.1.211"),
                "main",
                &runtime("vk-plan-memory-only"),
            )
            .unwrap();
        assert_eq!(plan.plan.target_config_path, target.to_str().unwrap());
        assert!(!target.exists());
        assert!(!state.paths.snapshot_root.exists());
        assert!(!state.paths.ownership_root.exists());
        assert!(serde_json::to_string(&plan)
            .unwrap()
            .contains("vk-plan-memory-only"));

        let mut blocked_catalog = catalog.clone();
        let entry = blocked_catalog
            .entries
            .iter_mut()
            .find(|entry| entry.agent_id == "claude-code")
            .unwrap();
        entry
            .blocked
            .push(super::super::compatibility::BlockedRule {
                version_requirement: "=2.1.211".to_string(),
                reason: "fixture blocked".to_string(),
            });
        install_scan(&state, blocked_catalog, vec![record(&target, false)]);
        let blocked = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("2.1.211"),
                "main",
                &runtime("vk-blocked"),
            )
            .err()
            .expect("blocked version cannot produce a plan");
        assert_eq!(blocked.code, "not_admitted");

        let mut unknown_record = record(&target, false);
        unknown_record.version_raw = Some("99.0.0".to_string());
        unknown_record.version_normalized = Some("99.0.0".to_string());
        install_scan(&state, catalog, vec![unknown_record]);
        let missing = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                None,
                "main",
                &runtime("vk-missing-version"),
            )
            .err()
            .expect("a discovered version must be bound into the plan");
        assert_eq!(missing.code, "expected_version_required");
        let changed = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("98.0.0"),
                "main",
                &runtime("vk-stale-confirmation"),
            )
            .err()
            .expect("the confirmed version must still match before planning");
        assert_eq!(changed.code, "discovery_changed_before_plan");
        let admitted = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("99.0.0"),
                "main",
                &runtime("vk-unknown"),
            )
            .expect("an unblocked version should produce a normal plan");
        assert_eq!(
            admitted.plan.compatibility_evidence.status,
            CompatibilityStatus::DetectedVerified
        );
        assert_eq!(
            admitted.plan.required_confirmations,
            vec![
                ConfirmationKind::Installation,
                ConfirmationKind::TargetConfig,
                ConfirmationKind::ConfigurationDiff,
            ]
        );
        assert!(admitted
            .plan
            .compatibility_evidence
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));

        let mut no_version = record(&target, false);
        no_version.version_raw = Some("Claude nightly".to_string());
        no_version.version_normalized = None;
        no_version.diagnostics.push(Diagnostic {
            reason_code: ReasonCode::VersionOutputUnparseable,
            message: "no semver".to_string(),
        });
        install_scan(
            &state,
            CompatibilityCatalog::builtin(&state.registry).unwrap(),
            vec![no_version],
        );
        let no_version = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                None,
                "main",
                &runtime("vk-no-version"),
            )
            .expect("an unparseable version must skip version binding");
        assert_eq!(
            no_version.plan.compatibility_evidence.status,
            CompatibilityStatus::DetectedVerified
        );
        assert!(!target.exists());
    }

    #[test]
    fn claude_desktop_linux_returns_typed_unsupported_before_any_write() {
        let state = state("claude-desktop-linux");
        let target = scratch("claude-desktop-linux-target")
            .join("configLibrary/7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2.json");
        let mut installation = record(&target, false);
        installation.agent_id = "claude-desktop".to_string();
        installation.executable_path = "/opt/Claude".to_string();
        installation.canonical_path = "/opt/Claude".to_string();
        installation.environment = Platform::Linux;
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![installation]);

        let error = state
            .plan_connection(
                "claude-desktop",
                "/opt/Claude",
                Some("2.1.211"),
                "main",
                &runtime("vk-must-not-be-written"),
            )
            .err()
            .expect("Claude Desktop has no supported Linux 3P profile path");

        assert_eq!(error.code, "unsupported_platform");
        assert!(!target.exists());
        assert!(!target.with_file_name("_meta.json").exists());
        assert!(!state.paths.snapshot_root.exists());
        assert!(!state.paths.ownership_root.exists());
    }

    #[test]
    fn commands_snapshot_view_exposes_metadata_without_crypto_or_owner_fields() {
        let view = SnapshotView::from(SnapshotRecord {
            schema_version: 1,
            snapshot_id: "ab".repeat(16),
            operation_id: "cd".repeat(16),
            agent_id: "claude-code".to_string(),
            target_config_path: "/tmp/settings.json".to_string(),
            envelope_hash: "ef".repeat(32),
            before_hash: "12".repeat(32),
            original_existed: true,
            original_permissions: Some(0o600),
            original_owner: Some("501:20".to_string()),
            created_at_ms: 1,
            connector_id: "claude-code-v1".to_string(),
            app_version: "0.1.0".to_string(),
            pinned: true,
        });
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(encoded.contains("\"source\":\"encrypted\""));
        assert!(encoded.contains("\"restorable\":true"));
        for forbidden in [
            "envelope_hash",
            "before_hash",
            "original_owner",
            "original_permissions",
            "ciphertext",
            "exact_bytes",
        ] {
            assert!(!encoded.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn commands_list_legacy_backup_as_read_only_without_reading_or_mutating_it() {
        let state = state("legacy-backup-view");
        let root = scratch("legacy-backup-target");
        let target = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        let backup = legacy_backup_path(&target);
        let marker = b"legacy backup bytes are not parsed";
        std::fs::write(&backup, marker).unwrap();
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![record(&target, false)]);

        let views = state.list_snapshots("claude-code").unwrap();

        assert_eq!(views.len(), 1);
        let encoded = serde_json::to_value(&views[0]).unwrap();
        assert_eq!(encoded["source"], "legacy_backup");
        assert_eq!(encoded["restorable"], false);
        assert_eq!(encoded["target_config_path"], target.to_str().unwrap());
        assert_eq!(std::fs::read(&backup).unwrap(), marker);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn commands_runtime_connector_and_input_boundary_matrix_is_fail_closed() {
        let runtime_view = runtime("vk-runtime-matrix");
        let fingerprint = runtime_view.fingerprint();
        for (connector_id, expected_base) in [
            ("claude-code-v1", "http://127.0.0.1:8787/agents/claude-code"),
            (
                "claude-desktop-3p-v1",
                "http://127.0.0.1:8787/agents/claude-desktop",
            ),
            ("codex-v1", "http://127.0.0.1:8787/agents/codex/v1"),
            ("gemini-cli-v1", "http://127.0.0.1:8787/agents/gemini-cli"),
            (
                "grok-build-v1",
                "http://127.0.0.1:8787/agents/grok-build/v1",
            ),
            ("kimi-code-v1", "http://127.0.0.1:8787/agents/kimi-code/v1"),
            (
                "deepseek-harness-v1",
                "http://127.0.0.1:8787/agents/deepseek-harness/v1",
            ),
            ("opencode-v1", "http://127.0.0.1:8787/agents/opencode/v1"),
            ("openclaw-v1", "http://127.0.0.1:8787/agents/openclaw/v1"),
            ("workbuddy-v1", "http://127.0.0.1:8787/agents/workbuddy/v1"),
            (
                "hermes-v1",
                "http://127.0.0.1:8787/agents/nous-hermes-agent/v1",
            ),
        ] {
            let connector = connector_for(connector_id).unwrap();
            let input = runtime_view.input_for(connector_id).unwrap();
            assert_eq!(input.base_url, expected_base);
            assert_eq!(connector.connector_id(), connector_id);
            if connector_id == "opencode-v1" {
                assert_eq!(input.token, Some("vk-runtime-matrix"));
                assert!(connector.connect_patch(&input).is_ok());
            }
        }
        assert!(runtime_view.input_for("future-v1").is_err());
        assert!(connector_for("future-v1").is_err());
        assert_ne!(fingerprint, runtime("different-key").fingerprint());

        for value in ["", "bad/id", &"a".repeat(81)] {
            assert_eq!(
                validate_short_identifier(value, "agent_id")
                    .unwrap_err()
                    .code,
                "invalid_identifier"
            );
        }
        assert!(validate_short_identifier("agent.ok-1", "agent_id").is_ok());
        for value in ["", "bad\0path", &"x".repeat(4097)] {
            assert_eq!(
                validate_installation_lookup(value).unwrap_err().code,
                "invalid_installation_lookup"
            );
        }
        assert!(validate_installation_lookup("/opt/agent").is_ok());
        for value in ["", "bad\nwindow", &"w".repeat(MAX_SESSION_LABEL_BYTES + 1)] {
            assert_eq!(
                validate_session_label(value).unwrap_err().code,
                "invalid_session"
            );
        }
        assert!(validate_session_label("main-window").is_ok());
        for value in ["", "AA", &"g".repeat(32)] {
            assert_eq!(
                validate_operation_id(value).unwrap_err().code,
                "invalid_operation_id"
            );
        }
        assert!(validate_operation_id(&"ab".repeat(16)).is_ok());
        for value in ["", &"00".repeat(31), &"GG".repeat(32)] {
            assert_eq!(
                decode_token(value).unwrap_err().code,
                "invalid_confirmation_token"
            );
        }
        assert_eq!(decode_token(&"af".repeat(32)).unwrap(), [0xaf; 32]);

        let targetless = DiscoveryRecord {
            config_candidates: Vec::new(),
            ..record(Path::new("/tmp/unused"), false)
        };
        assert_eq!(
            server_target(&targetless).unwrap_err().code,
            "config_target_unavailable"
        );
        let failure = TransactionFailure {
            operation_id: "ab".repeat(16),
            stage: TransactionStage::TargetWrite,
            reason_code: "write_failed".to_string(),
            recovery: RecoveryStatus::RepairRequired,
            recovery_reason_code: Some("restore_failed".to_string()),
        };
        let boundary = AgentCommandError::from(failure);
        assert_eq!(boundary.code, "write_failed");
        assert_eq!(boundary.stage, Some(TransactionStage::TargetWrite));
        assert_eq!(boundary.recovery, Some(RecoveryStatus::RepairRequired));
    }

    #[test]
    fn commands_views_publish_runtime_adapter_readiness() {
        let state = state("adapter-readiness-view");
        let target = scratch("adapter-readiness-view-target").join("settings.json");
        let snapshot = ScanSnapshot {
            catalog: CompatibilityCatalog::builtin(&state.registry).unwrap(),
            source: CatalogSource::Builtin,
            warning: None,
            records: vec![record(&target, false)],
        };
        let mut adapter_readiness = builtin_connectors()
            .iter()
            .map(|connector| (connector.capabilities().adapter_id.to_string(), true))
            .collect::<BTreeMap<_, _>>();
        adapter_readiness.insert("agent-anthropic".to_string(), false);
        let runtime = AgentProxyRuntime::new(
            "fixture-runtime".to_string(),
            "http://127.0.0.1:8787",
            "vk-readiness-view".to_string(),
            adapter_readiness,
            BTreeMap::new(),
            BTreeMap::new(),
        );

        let views = state.views(&snapshot, Some(&runtime), None).unwrap();
        let installation = &views
            .iter()
            .find(|view| view.metadata.agent_id == "claude-code")
            .unwrap()
            .installations[0];

        assert_eq!(installation.adapter_ready, Some(false));
        assert!(!installation.connected);
        std::fs::remove_dir_all(target.parent().unwrap()).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn commands_runtime_from_app_rejects_readonly_and_stopped_states() {
        for (label, load_error, expected_code) in [
            (
                "runtime-readonly",
                Some("existing config is invalid".to_string()),
                "agent_operation_rejected",
            ),
            ("runtime-stopped", None, "proxy_not_running"),
        ] {
            let root = scratch(label);
            let state = AppStateManaged(Mutex::new(crate::AppInner::new(
                root.join("token-station.json"),
                crate::template(&root.join("data"), &root.join("plugins")),
                load_error,
            )));

            let error = runtime_from_app(&state)
                .err()
                .expect("runtime must fail closed");

            assert_eq!(error.code, expected_code);
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn commands_runtime_accepts_a_running_gateway_that_is_waiting_for_a_route() {
        let root = scratch("runtime-waiting-route");
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("plugins-dist");
        let mut draft = crate::template(&root.join("data"), &plugins_dir);
        draft["server"]["listen"] = json!("127.0.0.1:0");
        draft["server"]["auth"] = json!(true);
        draft["data"]["metrics"] = json!(false);
        let config =
            serde_json::from_value::<token_station_cli::config::ClientConfig>(draft.clone())
                .unwrap();
        let running = crate::serve_lifecycle::prepare_server(config)
            .unwrap()
            .bind()
            .unwrap()
            .publish(7, BTreeMap::new())
            .unwrap();
        let mut inner = crate::AppInner::new(root.join("token-station.json"), draft, None);
        inner.server = crate::ServerLifecycle::Running {
            generation: 1,
            server: running,
            apply_error: None,
        };
        let state = AppStateManaged(Mutex::new(inner));

        let runtime = runtime_from_app(&state)
            .expect("a waiting gateway remains a valid Agent connection runtime");
        assert!(runtime
            .input_for("claude-code-v1")
            .unwrap()
            .model_metadata
            .is_none());

        let running = {
            let mut inner = state.0.lock().unwrap();
            let lifecycle = std::mem::replace(
                &mut inner.server,
                crate::ServerLifecycle::Stopped { generation: 2 },
            );
            let crate::ServerLifecycle::Running { server, .. } = lifecycle else {
                panic!("fixture runtime must be serving");
            };
            server
        };
        running.drain_and_shutdown();
        std::fs::remove_dir_all(root).ok();
    }

    // Not under `bundled-plugins`. This test needs `agent-openai` to be a
    // *skipped* adapter, which it arranges by leaving the package out of the
    // fixture's plugin directory. With the adapters compiled in, the builtin
    // tier wins across tiers (see `plugins.rs`), so the package loads anyway
    // and the scenario cannot exist. The feature's own comment says ordinary
    // unit tests keep it disabled; saying so here turns three confusing
    // failures into a skip for anyone who runs the suite with it on.
    #[cfg(not(feature = "bundled-plugins"))]
    #[test]
    fn commands_runtime_readiness_comes_from_loaded_adapters_not_configured_names() {
        let (root, state) = running_app_with_skipped_openai_adapter("runtime-readiness");

        let runtime = runtime_from_app(&state).unwrap();
        let loaded = runtime.input_for("claude-code-v1").unwrap().adapter_ready;
        let skipped_input = runtime.input_for("opencode-v1").unwrap();
        let skipped = skipped_input.adapter_ready;
        let plan_rejection = find_connector("opencode-v1")
            .unwrap()
            .validate_preconditions(&skipped_input)
            .expect_err("a skipped runtime adapter must block a connection plan");
        stop_running_app(&state);
        std::fs::remove_dir_all(root).ok();

        assert!(
            loaded,
            "the successfully loaded Anthropic adapter remains ready"
        );
        assert!(
            !skipped,
            "agent-openai is configured but missing on disk, so the running Gateway skipped it"
        );
        assert!(plan_rejection.contains("未加载 agent-openai"));
    }

    // Not under `bundled-plugins`. This test needs `agent-openai` to be a
    // *skipped* adapter, which it arranges by leaving the package out of the
    // fixture's plugin directory. With the adapters compiled in, the builtin
    // tier wins across tiers (see `plugins.rs`), so the package loads anyway
    // and the scenario cannot exist. The feature's own comment says ordinary
    // unit tests keep it disabled; saying so here turns three confusing
    // failures into a skip for anyone who runs the suite with it on.
    #[cfg(not(feature = "bundled-plugins"))]
    #[test]
    fn commands_runtime_readiness_during_apply_comes_from_the_serving_old_instance() {
        let (root, state) = running_app_with_skipped_openai_adapter("applying-readiness");
        {
            let mut inner = state.0.lock().unwrap();
            let lifecycle = std::mem::replace(
                &mut inner.server,
                crate::ServerLifecycle::Stopped { generation: 2 },
            );
            let crate::ServerLifecycle::Running { server, .. } = lifecycle else {
                panic!("fixture runtime must be running");
            };
            inner.server = crate::ServerLifecycle::Applying {
                generation: 2,
                revision: 8,
                old: server,
            };
        }

        let runtime = runtime_from_app(&state).unwrap();
        let skipped = runtime.input_for("opencode-v1").unwrap().adapter_ready;
        stop_running_app(&state);
        std::fs::remove_dir_all(root).ok();

        assert!(
            !skipped,
            "an applying candidate must not replace the old instance's actual adapter readiness"
        );
    }

    // Not under `bundled-plugins`. This test needs `agent-openai` to be a
    // *skipped* adapter, which it arranges by leaving the package out of the
    // fixture's plugin directory. With the adapters compiled in, the builtin
    // tier wins across tiers (see `plugins.rs`), so the package loads anyway
    // and the scenario cannot exist. The feature's own comment says ordinary
    // unit tests keep it disabled; saying so here turns three confusing
    // failures into a skip for anyone who runs the suite with it on.
    #[cfg(not(feature = "bundled-plugins"))]
    #[test]
    fn commands_runtime_readiness_tracks_each_replacement_running_instance() {
        let (old_root, old_state) =
            running_app_with_skipped_openai_adapter("old-running-readiness");
        let old_runtime = runtime_from_app(&old_state).unwrap();
        assert!(
            old_runtime
                .input_for("claude-code-v1")
                .unwrap()
                .adapter_ready
        );
        assert!(!old_runtime.input_for("opencode-v1").unwrap().adapter_ready);

        let (new_root, new_state) = running_app_with_adapters(
            "new-running-readiness",
            &["agent-openai"],
            &["agent-anthropic", "agent-openai"],
        );
        let new_runtime = runtime_from_app(&new_state).unwrap();
        assert!(
            !new_runtime
                .input_for("claude-code-v1")
                .unwrap()
                .adapter_ready
        );
        assert!(new_runtime.input_for("opencode-v1").unwrap().adapter_ready);

        stop_running_app(&old_state);
        stop_running_app(&new_state);
        std::fs::remove_dir_all(old_root).ok();
        std::fs::remove_dir_all(new_root).ok();
    }

    #[test]
    fn commands_scan_views_and_plan_rejections_cover_stale_and_preflight_states() {
        let state = state("scan-views");
        let target = scratch("scan-view-target").join("settings.json");
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        let snapshot = ScanSnapshot {
            catalog: catalog.clone(),
            source: CatalogSource::Builtin,
            warning: None,
            records: vec![record(&target, false)],
        };
        let views = state.views(&snapshot, None, None).unwrap();
        let claude = views
            .iter()
            .find(|view| view.metadata.agent_id == "claude-code")
            .unwrap();
        assert_eq!(claude.catalog_source, "builtin");
        assert_eq!(claude.catalog_warning, None);
        assert_eq!(claude.installations.len(), 1);
        assert_eq!(claude.status, CompatibilityStatus::DetectedVerified);

        assert_eq!(
            state
                .selected("claude-code", "/opt/claude")
                .unwrap_err()
                .code,
            "scan_required"
        );
        install_scan(&state, catalog.clone(), vec![record(&target, false)]);
        assert_eq!(
            state.selected("unknown", "/opt/claude").unwrap_err().code,
            "unknown_agent"
        );
        assert_eq!(
            state.selected("claude-code", "/missing").unwrap_err().code,
            "unknown_or_stale_installation"
        );
        assert_eq!(
            state
                .list_snapshots("unknown")
                .err()
                .expect("unknown Agent is rejected")
                .code,
            "unknown_agent"
        );
        assert!(state.list_snapshots("bad/id").is_err());

        let mut broken = record(&target, false);
        broken.diagnostics.push(Diagnostic {
            reason_code: ReasonCode::ConfigParseFailed,
            message: "fixture parse failure".to_string(),
        });
        install_scan(&state, catalog, vec![broken]);
        let error = state
            .plan_connection("claude-code", "/opt/claude", None, "main", &runtime("vk"))
            .err()
            .expect("preflight diagnostic blocks planning");
        assert_eq!(error.code, "read_only_preflight_failed");

        let scanned = state.scan().unwrap();
        assert_eq!(scanned.len(), 12);
        assert!(state.session.lock().unwrap().scan.is_some());
    }

    #[test]
    fn commands_pending_plan_limits_duplicates_intent_and_expiry_are_enforced() {
        let state = state("pending-limits");
        let target = scratch("pending-target").join("settings.json");
        let now_ms = state.clock.now_ms();
        let first = state
            .issue_plan(
                prepared(&target, "vk-first", now_ms),
                &record(&target, false),
                "main",
                None,
            )
            .unwrap();
        assert_eq!(
            state.plan_intent(&first.plan.operation_id).unwrap(),
            PlanIntent::Connect
        );
        assert_eq!(
            state.plan_intent(&"cd".repeat(16)).unwrap_err().code,
            "unknown_operation"
        );
        assert!(state.plan_intent("invalid").is_err());

        let mut duplicate = prepared(&target, "vk-duplicate", now_ms);
        duplicate.view.operation_id = first.plan.operation_id.clone();
        assert_eq!(
            state
                .issue_plan(duplicate, &record(&target, false), "main", None)
                .err()
                .expect("duplicate operation ID is rejected")
                .code,
            "duplicate_operation_id"
        );

        for index in 1..MAX_PENDING_PLANS {
            state
                .issue_plan(
                    prepared(&target, &format!("vk-{index}"), now_ms),
                    &record(&target, false),
                    "main",
                    None,
                )
                .unwrap();
        }
        assert_eq!(state.session.lock().unwrap().plans.len(), MAX_PENDING_PLANS);
        assert_eq!(
            state
                .issue_plan(
                    prepared(&target, "vk-overflow", now_ms),
                    &record(&target, false),
                    "main",
                    None,
                )
                .err()
                .expect("pending plan capacity is enforced")
                .code,
            "too_many_pending_plans"
        );

        {
            let mut session = state.session.lock().unwrap();
            let stored = session.plans.get_mut(&first.plan.operation_id).unwrap();
            stored.prepared.view.expires_at_ms = now_ms.saturating_sub(1);
        }
        assert_eq!(
            state
                .take_plan(
                    &first.plan.operation_id,
                    &first.confirmation_token,
                    "main",
                    &[PlanIntent::Connect],
                )
                .err()
                .expect("expired plan is pruned")
                .code,
            "unknown_or_expired_operation"
        );
        assert_eq!(
            state.session.lock().unwrap().plans.len(),
            MAX_PENDING_PLANS - 1
        );
    }

    #[test]
    fn force_forget_strips_owned_fields_and_clears_ownership_without_keychain() {
        let state = state("force-forget");
        let root = scratch("force-forget-target");
        let target = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&target, br#"{"unowned":"keep"}"#).unwrap();
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![record(&target, false)]);

        // Establish management by writing managed fields, ownership records, and a snapshot.
        let runtime = runtime("vk-force-forget");
        let connection = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("2.1.211"),
                "main",
                &runtime,
            )
            .unwrap();
        let taken = state
            .take_plan(
                &connection.plan.operation_id,
                &connection.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .unwrap();
        let now_ms = state.clock.now_ms();
        let engine = TransactionEngine::new(
            &state.snapshots,
            &state.ownership,
            state.keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &state.clock,
        );
        engine
            .apply_connection(
                &taken.prepared,
                &ConfirmedOperation {
                    operation_id: taken.prepared.view.operation_id.clone(),
                    confirmed_at_ms: now_ms,
                    confirmations: taken
                        .prepared
                        .view
                        .required_confirmations
                        .iter()
                        .copied()
                        .collect(),
                },
                &RuntimeAdmission {
                    compatibility_sequence: taken.prepared.view.compatibility_sequence,
                    status: CompatibilityStatus::DetectedVerified,
                },
                now_ms,
            )
            .unwrap();

        let after_connect = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(
            after_connect.contains("ANTHROPIC_BASE_URL"),
            "接管应写入受管字段"
        );
        assert_eq!(
            state
                .ownership
                .list_agent_installation("claude-code", "/opt/claude")
                .unwrap()
                .len(),
            1
        );
        let menu_entries = state.managed_agent_menu_entries();
        assert_eq!(menu_entries.len(), 1);
        assert_eq!(menu_entries[0].0, "claude-code");

        // force_forget does not access the keychain; it removes managed fields only according to ownership.
        state.force_forget("claude-code", "/opt/claude").unwrap();

        let after_forget = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(
            !after_forget.contains("ANTHROPIC_BASE_URL"),
            "受管字段应被删除"
        );
        assert!(after_forget.contains("keep"), "用户自己的字段必须保留");
        assert!(
            state
                .ownership
                .list_agent_installation("claude-code", "/opt/claude")
                .unwrap()
                .is_empty(),
            "归属记录应被清除"
        );
        assert!(
            state.force_forget("claude-code", "/opt/claude").is_err(),
            "已无归属时再次强制断开应报错"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn force_strip_commit_rolls_back_primary_when_companion_read_fails() {
        let root = scratch("force-strip-companion-read-failure");
        let primary = root.join("primary.json");
        let companion = root.join("companion.json");
        std::fs::create_dir_all(&root).unwrap();
        let primary_before = br#"{"owned":"remove","keep":"primary"}"#;
        let companion_before = br#"{"owned":"remove","keep":"companion"}"#;
        std::fs::write(&primary, primary_before).unwrap();
        std::fs::write(&companion, companion_before).unwrap();
        let removals = vec![PatchOperation {
            operation: PatchKind::Remove,
            path: ConfigPath {
                segments: vec!["owned".to_string()],
            },
            value: None,
        }];
        let prepared = vec![
            prepare_force_strip_owned(
                &primary,
                DocumentFormat::Json,
                "primary fixture",
                &removals,
                None,
            )
            .unwrap()
            .unwrap(),
            prepare_force_strip_owned(
                &companion,
                DocumentFormat::Json,
                "companion fixture",
                &removals,
                None,
            )
            .unwrap()
            .unwrap(),
        ];
        let read_count = std::cell::Cell::new(0_usize);
        let reader = |target: &Path| {
            let call = read_count.get();
            read_count.set(call + 1);
            if call == 2 {
                Err("simulated companion read failure".to_string())
            } else {
                read_config_source(target)
            }
        };

        let error = match commit_force_strips(&prepared, &FsAtomicConfigWriter, &reader) {
            Ok(_) => panic!("the second file read must fail"),
            Err(error) => error,
        };

        assert!(error.message.contains("simulated companion read failure"));
        assert_eq!(std::fs::read(&primary).unwrap(), primary_before);
        assert_eq!(std::fs::read(&companion).unwrap(), companion_before);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn claude_desktop_force_forget_preserves_user_profile_metadata() {
        const TOKEN_STATION_PROFILE_ID: &str = "7f60d1f4-8d8c-4f5c-9f4c-2c2530c4f9f2";

        let state = state("claude-desktop-force-forget-metadata");
        let root = scratch("claude-desktop-force-forget-metadata-target");
        let library = root.join("Claude-3p/configLibrary");
        let primary = library.join(format!("{TOKEN_STATION_PROFILE_ID}.json"));
        let metadata = library.join("_meta.json");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(&primary, br#"{"keep":"primary"}"#).unwrap();
        std::fs::write(
            &metadata,
            format!(
                r#"{{"appliedId":"user-profile","entries":[{{"id":"user-profile","name":"User"}},{{"id":"{TOKEN_STATION_PROFILE_ID}","name":"Token Station"}}],"keep":true}}"#
            ),
        )
        .unwrap();

        let applied_id = ConfigPath {
            segments: vec!["appliedId".to_string()],
        };
        let entries = ConfigPath {
            segments: vec!["entries".to_string()],
        };
        let primary_owned_paths = connector_for("claude-desktop-3p-v1").unwrap().owned_paths();
        let primary_macs = primary_owned_paths
            .iter()
            .map(|path| (path.to_string(), "e".repeat(64)))
            .collect();
        let companion_macs = BTreeMap::from([
            (applied_id.to_string(), "f".repeat(64)),
            (entries.to_string(), "0".repeat(64)),
        ]);
        state
            .ownership
            .commit(
                crate::agent_integration::ownership::OwnershipRecord {
                    schema_version: 1,
                    revision: 0,
                    agent_id: "claude-desktop".to_string(),
                    installation_path: "/Applications/Claude.app/Contents/MacOS/Claude".to_string(),
                    target_config_path: primary.to_string_lossy().into_owned(),
                    connector_id: "claude-desktop-3p-v1".to_string(),
                    baseline_snapshot_id: "21".repeat(16),
                    last_transaction_snapshot_id: "22".repeat(16),
                    before_hash: "a".repeat(64),
                    managed_after_hash: "b".repeat(64),
                    owned_paths: primary_owned_paths,
                    owned_value_macs: primary_macs,
                    companion_files: vec![
                        crate::agent_integration::ownership::CompanionOwnership {
                            target_config_path: metadata.to_string_lossy().into_owned(),
                            document_format: Some(DocumentFormat::Json),
                            baseline_snapshot_id: "23".repeat(16),
                            last_transaction_snapshot_id: "24".repeat(16),
                            before_hash: "c".repeat(64),
                            managed_after_hash: "d".repeat(64),
                            owned_paths: vec![applied_id, entries],
                            sensitive_paths: None,
                            owned_value_macs: companion_macs,
                        },
                    ],
                    acquired_at_ms: 1,
                    updated_at_ms: 1,
                },
                None,
            )
            .unwrap();

        state
            .force_forget(
                "claude-desktop",
                "/Applications/Claude.app/Contents/MacOS/Claude",
            )
            .unwrap();

        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
        assert_eq!(restored["appliedId"], "user-profile");
        assert_eq!(
            restored["entries"],
            json!([{"id":"user-profile","name":"User"}])
        );
        assert_eq!(restored["keep"], true);
        assert!(!restored.to_string().contains(TOKEN_STATION_PROFILE_ID));
        assert!(state
            .ownership
            .list_agent_installation(
                "claude-desktop",
                "/Applications/Claude.app/Contents/MacOS/Claude"
            )
            .unwrap()
            .is_empty());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn workbuddy_force_strip_derives_the_dynamic_patch_from_the_written_revision() {
        let root = scratch("workbuddy-force-strip-single-revision");
        let target = root.join("models.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &target,
            br#"{
              "models": [
                {"id":"user-existing"},
                {"id":"user-added-after-scan"},
                {"id":"tokenstation-auto","apiKey":"managed-secret"}
              ],
              "availableModels": [
                "user-existing",
                "user-added-after-scan",
                "tokenstation-auto"
              ],
              "keep": true
            }"#,
        )
        .unwrap();
        let connector = connector_for("workbuddy-v1").unwrap();

        let prepared = prepare_connector_force_strip_owned(&target, connector)
            .unwrap()
            .expect("the existing WorkBuddy file must be projected");
        let projected = parse_source_bytes(
            Some(prepared.rendered.as_slice()),
            DocumentFormat::Json,
            connector.label(),
        )
        .unwrap();
        let semantic = semantic_json(&projected).unwrap();
        let model_ids = semantic["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            model_ids,
            vec!["user-existing", "user-added-after-scan"],
            "the force strip must preserve every user model in the exact revision it binds"
        );
        assert_eq!(
            semantic["availableModels"],
            json!(["user-existing", "user-added-after-scan"])
        );
        assert_eq!(semantic["keep"], json!(true));
        assert!(!String::from_utf8_lossy(prepared.rendered.as_slice()).contains("managed-secret"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn workbuddy_force_strip_rejects_a_user_model_added_after_projection() {
        let root = scratch("workbuddy-force-strip-revision-race");
        let target = root.join("models.json");
        std::fs::create_dir_all(&root).unwrap();
        let initial = br#"{
          "models": [
            {"id":"user-existing"},
            {"id":"tokenstation-auto","apiKey":"managed-secret"}
          ],
          "availableModels": ["user-existing", "tokenstation-auto"]
        }"#;
        std::fs::write(&target, initial).unwrap();
        let connector = connector_for("workbuddy-v1").unwrap();
        let prepared = vec![prepare_connector_force_strip_owned(&target, connector)
            .unwrap()
            .unwrap()];
        let externally_updated = br#"{
          "models": [
            {"id":"user-existing"},
            {"id":"user-added-after-projection"},
            {"id":"tokenstation-auto","apiKey":"managed-secret"}
          ],
          "availableModels": [
            "user-existing",
            "user-added-after-projection",
            "tokenstation-auto"
          ]
        }"#;
        std::fs::write(&target, externally_updated).unwrap();

        let error = match commit_force_strips(&prepared, &FsAtomicConfigWriter, &read_config_source)
        {
            Ok(_) => panic!("an external WorkBuddy model must invalidate the projection"),
            Err(error) => error,
        };

        assert_eq!(error.code, "target_changed_before_force_forget");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            externally_updated,
            "revision rejection must preserve the user's newly added model byte-for-byte"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hermes_force_forget_keeps_the_configuration_reconnectable() {
        let state = state("hermes-force-forget-reconnect");
        let root = scratch("hermes-force-forget-reconnect-target");
        let target = root.join("config.yaml");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &target,
            b"# keep root comment\nfallback_providers: [] # keep unowned field\n",
        )
        .unwrap();

        let mut installation = record(&target, false);
        installation.agent_id = "nous-hermes-agent".to_string();
        installation.executable_path = "/opt/hermes".to_string();
        installation.canonical_path = "/opt/hermes".to_string();
        installation.version_raw = Some("0.18.0".to_string());
        installation.version_normalized = Some("0.18.0".to_string());
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![installation]);
        let first_runtime = runtime("vk-hermes-first-connection");

        let connection = state
            .plan_connection(
                "nous-hermes-agent",
                "/opt/hermes",
                Some("0.18.0"),
                "main",
                &first_runtime,
            )
            .unwrap();
        state
            .apply_from_cached_scan(
                &connection.plan.operation_id,
                &connection.confirmation_token,
                "main",
                &[PlanIntent::Connect],
                Some(&first_runtime),
            )
            .unwrap();

        state
            .force_forget("nous-hermes-agent", "/opt/hermes")
            .unwrap();
        let disconnected = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(disconnected.starts_with("# keep root comment\n"));
        assert!(disconnected.contains("fallback_providers: [] # keep unowned field"));
        assert!(!disconnected.contains("vk-hermes-first-connection"));
        assert!(state
            .ownership
            .list_agent_installation("nous-hermes-agent", "/opt/hermes")
            .unwrap()
            .is_empty());

        let reconnect_runtime = runtime("vk-hermes-second-connection");
        let reconnected = state
            .plan_connection(
                "nous-hermes-agent",
                "/opt/hermes",
                Some("0.18.0"),
                "main",
                &reconnect_runtime,
            )
            .unwrap();
        state
            .apply_from_cached_scan(
                &reconnected.plan.operation_id,
                &reconnected.confirmation_token,
                "main",
                &[PlanIntent::Connect],
                Some(&reconnect_runtime),
            )
            .unwrap();
        let connected_again = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(connected_again.contains("vk-hermes-second-connection"));
        assert!(!connected_again.contains("vk-hermes-first-connection"));
        assert_eq!(
            state
                .ownership
                .list_agent_installation("nous-hermes-agent", "/opt/hermes")
                .unwrap()
                .len(),
            1
        );

        state
            .force_forget("nous-hermes-agent", "/opt/hermes")
            .unwrap();
        let cleaned = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(!cleaned.contains("vk-hermes-second-connection"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn every_non_codex_connector_completes_the_production_command_lifecycle() {
        // Keep fixture roots compact: Claude Desktop's four-file layout plus
        // atomic-write suffixes otherwise crosses classic Windows MAX_PATH.
        let root = scratch("life");
        for (index, case) in non_codex_lifecycle_cases(&root).into_iter().enumerate() {
            let state = state(&format!("life-{index}"));
            seed_lifecycle_case(&case);
            let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
            install_scan(&state, catalog, vec![lifecycle_record(&case)]);

            let first_token = format!("vk-{}-first-connection", case.label);
            let first_runtime = runtime(&first_token);
            apply_lifecycle_connection(&state, &case, &first_runtime, "lifecycle-main");
            assert_lifecycle_ownership(&state, &case, 1);
            assert_lifecycle_files(&case, &[], Some(&first_token));

            state
                .force_forget(case.agent_id, &case.installation_path)
                .unwrap_or_else(|error| {
                    panic!("{} force-forget failed: {}", case.label, error.message)
                });
            assert_lifecycle_ownership(&state, &case, 0);
            assert_lifecycle_files(
                &case,
                &[&first_token, "token-station-reconnect-check"],
                None,
            );
            assert!(
                state
                    .snapshots
                    .list_agent(case.agent_id)
                    .unwrap()
                    .iter()
                    .all(|snapshot| !snapshot.pinned),
                "{} force-forget must unpin every baseline snapshot",
                case.label
            );

            let second_token = format!("vk-{}-second-connection", case.label);
            let second_runtime = runtime(&second_token);
            apply_lifecycle_connection(&state, &case, &second_runtime, "lifecycle-main");
            assert_lifecycle_ownership(&state, &case, 1);
            assert_lifecycle_files(&case, &[&first_token], Some(&second_token));

            state
                .force_forget(case.agent_id, &case.installation_path)
                .unwrap_or_else(|error| {
                    panic!(
                        "{} final force-forget failed: {}",
                        case.label, error.message
                    )
                });
            assert_lifecycle_ownership(&state, &case, 0);
            assert_lifecycle_files(&case, &[&first_token, &second_token], None);
            clean_lifecycle_case(&state, &root);
        }
    }

    #[test]
    fn deepseek_harness_connects_from_stock_settings_without_a_credentials_file() {
        let root = scratch("dsh-stock");
        let case = LifecycleCase {
            label: "deepseek-harness",
            agent_id: "deepseek-harness",
            connector_id: "deepseek-harness-v1",
            version: "0.1.0-rc.6",
            installation_path: root.join("install/dsh").to_string_lossy().into_owned(),
            primary: LifecycleFileFixture {
                path: root.join("home/.dsh/settings.yaml"),
                baseline: b"ui-onboarding:\n  welcomeNoticeVersion: 2026-08-13.1\n",
                format: DocumentFormat::Yaml,
                marker: "welcomeNoticeVersion",
            },
            companions: vec![LifecycleFileFixture {
                path: root.join("home/.dsh/.credentials.yaml"),
                baseline: b"",
                format: DocumentFormat::Yaml,
                marker: "TOKENSTATION_API_KEY",
            }],
        };
        std::fs::create_dir_all(case.primary.path.parent().unwrap()).unwrap();
        std::fs::write(&case.primary.path, case.primary.baseline).unwrap();
        assert!(!case.companions[0].path.exists());

        let state = state("dsh-stock");
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog, vec![lifecycle_record(&case)]);
        let fixture_runtime = runtime("vk-dsh-stock-connection");

        apply_lifecycle_connection(&state, &case, &fixture_runtime, "dsh-main");
        assert!(case.companions[0].path.exists());
        assert_lifecycle_ownership(&state, &case, 1);
        let settings = std::fs::read_to_string(&case.primary.path).unwrap();
        assert!(settings.contains("welcomeNoticeVersion"));
        assert!(settings.contains("TOKENSTATION_API_KEY"));
        assert!(!settings.contains(fixture_runtime.virtual_key()));
        let credentials = std::fs::read_to_string(&case.companions[0].path).unwrap();
        assert!(credentials.contains(fixture_runtime.virtual_key()));
        let disconnect = state
            .plan_disconnect(
                case.agent_id,
                &case.installation_path,
                "dsh-disconnect-review",
            )
            .unwrap();
        let public_disconnect = serde_json::to_string(&disconnect.plan).unwrap();
        assert!(
            public_disconnect.contains(fixture_runtime.virtual_key()),
            "the local disconnect confirmation must show the companion credential"
        );
        let credential_projection = disconnect
            .plan
            .projection
            .files
            .iter()
            .find(|file| file.target_config_path.ends_with(".credentials.yaml"))
            .expect("DeepSeek credentials companion must be present in the disconnect review");
        assert!(
            credential_projection
                .forward_changes
                .iter()
                .any(|change| change.sensitive),
            "legacy and current companion credentials must fail closed as sensitive"
        );
        assert!(credential_projection
            .credential_bindings
            .iter()
            .any(|binding| {
                binding.source
                    == crate::agent_integration::types::CredentialSource::EncryptedSnapshot
            }));
        let baseline = state
            .snapshots
            .list_agent(case.agent_id)
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.target_config_path == case.primary.path.to_string_lossy())
            .expect("DeepSeek primary baseline snapshot must exist");
        let restore = state
            .plan_restore(&baseline.snapshot_id, "dsh-restore-review")
            .unwrap();
        assert!(
            serde_json::to_string(&restore.plan)
                .unwrap()
                .contains(fixture_runtime.virtual_key()),
            "the local restore confirmation must show the companion credential"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&case.companions[0].path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        clean_lifecycle_case(&state, &root);
    }

    #[test]
    fn concurrent_non_codex_connection_plans_leave_one_consistent_owner() {
        const CONTENDERS: usize = 8;
        let root = scratch("race");
        for (index, case) in non_codex_lifecycle_cases(&root).into_iter().enumerate() {
            let state = state(&format!("race-{index}"));
            seed_lifecycle_case(&case);
            let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
            install_scan(&state, catalog, vec![lifecycle_record(&case)]);
            let shared_token = format!("vk-{}-concurrent-winner", case.label);
            let shared_runtime = runtime(&shared_token);
            let plans = (0..CONTENDERS)
                .map(|_| {
                    state
                        .plan_connection(
                            case.agent_id,
                            &case.installation_path,
                            Some(case.version),
                            "concurrent-main",
                            &shared_runtime,
                        )
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let barrier = Arc::new(std::sync::Barrier::new(CONTENDERS));
            let results = std::thread::scope(|scope| {
                let handles = plans
                    .into_iter()
                    .map(|plan| {
                        let barrier = Arc::clone(&barrier);
                        let state = &state;
                        let runtime = &shared_runtime;
                        scope.spawn(move || {
                            barrier.wait();
                            state.apply_from_cached_scan(
                                &plan.plan.operation_id,
                                &plan.confirmation_token,
                                "concurrent-main",
                                &[PlanIntent::Connect],
                                Some(runtime),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap())
                    .collect::<Vec<_>>()
            });
            let success_count = results.iter().filter(|result| result.is_ok()).count();
            assert_eq!(
                success_count, 1,
                "{} must admit exactly one concurrent connection commit",
                case.label
            );
            for error in results.into_iter().filter_map(Result::err) {
                assert!(
                    matches!(
                        (error.stage, error.code.as_str()),
                        (
                            Some(TransactionStage::Ownership),
                            "ownership_already_active"
                        ) | (Some(TransactionStage::Revision), "before_hash_changed")
                            | (
                                Some(TransactionStage::Revision),
                                "companion_before_hash_changed"
                            )
                            | (
                                Some(TransactionStage::TargetWrite),
                                "target_changed_before_replace"
                            )
                            | (
                                Some(TransactionStage::OwnershipCommit),
                                "ownership_commit_failed"
                            )
                    ),
                    "{} loser must be rejected only by revision, CAS, or ownership: {} / {:?}",
                    case.label,
                    error.code,
                    error.stage
                );
                let serialized = serde_json::to_string(&error).unwrap();
                assert!(!serialized.contains(&shared_token));
            }
            assert_lifecycle_ownership(&state, &case, 1);
            assert_lifecycle_files(&case, &[], Some(&shared_token));
            assert_eq!(
                state
                    .snapshots
                    .list_agent(case.agent_id)
                    .unwrap()
                    .iter()
                    .filter(|snapshot| snapshot.pinned)
                    .count(),
                1 + case.companions.len(),
                "{} must pin only the winning connection baseline files",
                case.label
            );

            state
                .force_forget(case.agent_id, &case.installation_path)
                .unwrap();
            assert_lifecycle_ownership(&state, &case, 0);
            assert_lifecycle_files(&case, &[&shared_token], None);

            let reconnect_token = format!("vk-{}-after-contention", case.label);
            let reconnect_runtime = runtime(&reconnect_token);
            apply_lifecycle_connection(&state, &case, &reconnect_runtime, "concurrent-main");
            assert_lifecycle_ownership(&state, &case, 1);
            assert_lifecycle_files(&case, &[&shared_token], Some(&reconnect_token));
            state
                .force_forget(case.agent_id, &case.installation_path)
                .unwrap();
            clean_lifecycle_case(&state, &root);
        }
    }

    #[test]
    #[ignore = "explicit configurable filesystem/ownership/snapshot lifecycle stress"]
    fn non_codex_connectors_survive_configured_production_lifecycle_stress() {
        const DEFAULT_ROUNDS: usize = 100;
        const MAX_ROUNDS: usize = 1_000;
        let rounds = match std::env::var("TOKEN_STATION_AGENT_LIFECYCLE_ROUNDS") {
            Ok(value) => value.parse::<usize>().unwrap_or_else(|_| {
                panic!("TOKEN_STATION_AGENT_LIFECYCLE_ROUNDS must be an integer")
            }),
            Err(std::env::VarError::NotPresent) => DEFAULT_ROUNDS,
            Err(std::env::VarError::NotUnicode(_)) => {
                panic!("TOKEN_STATION_AGENT_LIFECYCLE_ROUNDS must be valid UTF-8")
            }
        };
        assert!(
            (1..=MAX_ROUNDS).contains(&rounds),
            "TOKEN_STATION_AGENT_LIFECYCLE_ROUNDS must be within 1..={MAX_ROUNDS}"
        );
        eprintln!(
            "running {rounds} lifecycle rounds across {} non-Codex connectors",
            non_codex_lifecycle_cases(Path::new("unused")).len()
        );

        let root = scratch("stress");
        for (index, case) in non_codex_lifecycle_cases(&root).into_iter().enumerate() {
            let state = state(&format!("stress-{index}"));
            seed_lifecycle_case(&case);
            let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
            install_scan(&state, catalog, vec![lifecycle_record(&case)]);

            for round in 0..rounds {
                let first_token = format!("vk-{}-stress-{round}-first", case.label);
                let first_runtime = runtime(&first_token);
                apply_lifecycle_connection(&state, &case, &first_runtime, "stress-main");
                assert_lifecycle_ownership(&state, &case, 1);
                assert_lifecycle_files(&case, &[], Some(&first_token));
                state
                    .force_forget(case.agent_id, &case.installation_path)
                    .unwrap();
                assert_lifecycle_ownership(&state, &case, 0);
                assert_lifecycle_files(&case, &[&first_token], None);

                let second_token = format!("vk-{}-stress-{round}-second", case.label);
                let second_runtime = runtime(&second_token);
                apply_lifecycle_connection(&state, &case, &second_runtime, "stress-main");
                assert_lifecycle_ownership(&state, &case, 1);
                assert_lifecycle_files(&case, &[&first_token], Some(&second_token));
                state
                    .force_forget(case.agent_id, &case.installation_path)
                    .unwrap();
                assert_lifecycle_ownership(&state, &case, 0);
                assert_lifecycle_files(&case, &[&first_token, &second_token], None);
            }

            let snapshots = state.snapshots.list_agent(case.agent_id).unwrap();
            assert!(snapshots.iter().all(|snapshot| !snapshot.pinned));
            let mut counts_by_target = BTreeMap::<&str, usize>::new();
            for snapshot in &snapshots {
                *counts_by_target
                    .entry(snapshot.target_config_path.as_str())
                    .or_default() += 1;
            }
            assert!(
                counts_by_target.values().all(|count| *count <= 5),
                "{} snapshot retention must stay bounded after stress",
                case.label
            );
            clean_lifecycle_case(&state, &root);
        }
    }

    #[test]
    fn force_forget_unknown_legacy_companion_fails_before_any_write() {
        let state = state("force-forget-unknown-companion");
        let root = scratch("force-forget-unknown-companion-target");
        let target = root.join(".gemini/.env");
        let unknown = root.join(".gemini/unknown-companion.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(
            &target,
            b"GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:8787/agents/gemini-cli\nUNOWNED=keep\n",
        )
        .unwrap();
        std::fs::write(
            &unknown,
            br#"{"security":{"auth":{"selectedType":"gemini-api-key"}},"keep":true}"#,
        )
        .unwrap();

        let primary_path = ConfigPath {
            segments: vec!["GOOGLE_GEMINI_BASE_URL".to_string()],
        };
        let companion_path = ConfigPath {
            segments: vec![
                "security".to_string(),
                "auth".to_string(),
                "selectedType".to_string(),
            ],
        };
        state
            .ownership
            .commit(
                crate::agent_integration::ownership::OwnershipRecord {
                    schema_version: 1,
                    revision: 0,
                    agent_id: "gemini-cli".to_string(),
                    installation_path: "/opt/gemini".to_string(),
                    target_config_path: target.to_string_lossy().into_owned(),
                    connector_id: "gemini-cli-v1".to_string(),
                    baseline_snapshot_id: "01".repeat(16),
                    last_transaction_snapshot_id: "02".repeat(16),
                    before_hash: "a".repeat(64),
                    managed_after_hash: "b".repeat(64),
                    owned_paths: vec![primary_path.clone()],
                    owned_value_macs: BTreeMap::from([(primary_path.to_string(), "c".repeat(64))]),
                    companion_files: vec![
                        crate::agent_integration::ownership::CompanionOwnership {
                            target_config_path: unknown.to_string_lossy().into_owned(),
                            document_format: None,
                            baseline_snapshot_id: "03".repeat(16),
                            last_transaction_snapshot_id: "04".repeat(16),
                            before_hash: "d".repeat(64),
                            managed_after_hash: "e".repeat(64),
                            owned_paths: vec![companion_path.clone()],
                            sensitive_paths: None,
                            owned_value_macs: BTreeMap::from([(
                                companion_path.to_string(),
                                "f".repeat(64),
                            )]),
                        },
                    ],
                    acquired_at_ms: 1,
                    updated_at_ms: 1,
                },
                None,
            )
            .unwrap();
        let target_before = std::fs::read(&target).unwrap();
        let unknown_before = std::fs::read(&unknown).unwrap();

        let error = state
            .force_forget("gemini-cli", "/opt/gemini")
            .expect_err("unknown legacy companion format must fail closed");

        assert!(error.message.contains("companion") && error.message.contains("格式"));
        assert_eq!(std::fs::read(&target).unwrap(), target_before);
        assert_eq!(std::fs::read(&unknown).unwrap(), unknown_before);
        assert_eq!(
            state
                .ownership
                .list_agent_installation("gemini-cli", "/opt/gemini")
                .unwrap()
                .len(),
            1,
            "ownership must remain available for a later safe recovery"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn force_forget_invalid_known_companion_fails_before_writing_primary() {
        let state = state("force-forget-invalid-companion");
        let root = scratch("force-forget-invalid-companion-target");
        let target = root.join(".gemini/.env");
        let companion = root.join(".gemini/settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(
            &target,
            b"GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:8787/agents/gemini-cli\nUNOWNED=keep\n",
        )
        .unwrap();
        std::fs::write(&companion, br#"{"security":"user-scalar","keep":true}"#).unwrap();

        let primary_path = ConfigPath {
            segments: vec!["GOOGLE_GEMINI_BASE_URL".to_string()],
        };
        let companion_path = ConfigPath {
            segments: vec![
                "security".to_string(),
                "auth".to_string(),
                "selectedType".to_string(),
            ],
        };
        state
            .ownership
            .commit(
                crate::agent_integration::ownership::OwnershipRecord {
                    schema_version: 1,
                    revision: 0,
                    agent_id: "gemini-cli".to_string(),
                    installation_path: "/opt/gemini".to_string(),
                    target_config_path: target.to_string_lossy().into_owned(),
                    connector_id: "gemini-cli-v1".to_string(),
                    baseline_snapshot_id: "11".repeat(16),
                    last_transaction_snapshot_id: "12".repeat(16),
                    before_hash: "a".repeat(64),
                    managed_after_hash: "b".repeat(64),
                    owned_paths: vec![primary_path.clone()],
                    owned_value_macs: BTreeMap::from([(primary_path.to_string(), "c".repeat(64))]),
                    companion_files: vec![
                        crate::agent_integration::ownership::CompanionOwnership {
                            target_config_path: companion.to_string_lossy().into_owned(),
                            document_format: Some(DocumentFormat::Json),
                            baseline_snapshot_id: "13".repeat(16),
                            last_transaction_snapshot_id: "14".repeat(16),
                            before_hash: "d".repeat(64),
                            managed_after_hash: "e".repeat(64),
                            owned_paths: vec![companion_path.clone()],
                            sensitive_paths: None,
                            owned_value_macs: BTreeMap::from([(
                                companion_path.to_string(),
                                "f".repeat(64),
                            )]),
                        },
                    ],
                    acquired_at_ms: 1,
                    updated_at_ms: 1,
                },
                None,
            )
            .unwrap();
        let target_before = std::fs::read(&target).unwrap();
        let companion_before = std::fs::read(&companion).unwrap();

        state
            .force_forget("gemini-cli", "/opt/gemini")
            .expect_err("an invalid known companion must fail before any write");

        assert_eq!(std::fs::read(&target).unwrap(), target_before);
        assert_eq!(std::fs::read(&companion).unwrap(), companion_before);
        assert_eq!(
            state
                .ownership
                .list_agent_installation("gemini-cli", "/opt/gemini")
                .unwrap()
                .len(),
            1
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&state.paths.snapshot_root).ok();
        std::fs::remove_dir_all(&state.paths.ownership_root).ok();
    }

    #[test]
    fn commands_preview_connection_restore_and_disconnect_share_server_owned_state() {
        let state = state("server-owned-lifecycle");
        let root = scratch("server-owned-target");
        let target = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&target, br#"{"unowned":"keep"}"#).unwrap();
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog.clone(), vec![record(&target, false)]);

        let runtime = runtime("vk-command-lifecycle");
        let connection = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                Some("2.1.211"),
                "main",
                &runtime,
            )
            .unwrap();
        let taken = state
            .take_plan(
                &connection.plan.operation_id,
                &connection.confirmation_token,
                "main",
                &[PlanIntent::Connect],
            )
            .unwrap();
        let now_ms = state.clock.now_ms();
        let engine = TransactionEngine::new(
            &state.snapshots,
            &state.ownership,
            state.keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &state.clock,
        );
        engine
            .apply_connection(
                &taken.prepared,
                &ConfirmedOperation {
                    operation_id: taken.prepared.view.operation_id.clone(),
                    confirmed_at_ms: now_ms,
                    confirmations: taken
                        .prepared
                        .view
                        .required_confirmations
                        .iter()
                        .copied()
                        .collect(),
                },
                &RuntimeAdmission {
                    compatibility_sequence: taken.prepared.view.compatibility_sequence,
                    status: CompatibilityStatus::DetectedVerified,
                },
                now_ms,
            )
            .unwrap();

        assert!(state.any_connected_to(&runtime).unwrap());
        let connected_bytes = std::fs::read(&target).unwrap();
        let tampered = String::from_utf8(connected_bytes.clone())
            .unwrap()
            .replace("127.0.0.1:8787", "127.0.0.1:9999");
        std::fs::write(&target, tampered).unwrap();
        let drift_bytes = std::fs::read(&target).unwrap();
        let drift_modified = std::fs::metadata(&target).unwrap().modified().unwrap();
        let ownership_index = state.paths.ownership_root.join("ownership-index.json");
        let snapshot_index = state.paths.snapshot_root.join("index.json");
        let ownership_index_bytes = std::fs::read(&ownership_index).unwrap();
        let snapshot_index_bytes = std::fs::read(&snapshot_index).unwrap();
        let drift = state
            .drift("claude-code", "/opt/claude")
            .expect("drift query is read-only and uses the exact scan binding");
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].status, DriftStatus::ManagedChanges);
        assert!(drift[0].changes.iter().any(|change| {
            change.path.segments == ["env", "ANTHROPIC_BASE_URL"]
                && change.scope == DriftScope::Managed
        }));
        assert_eq!(std::fs::read(&target).unwrap(), drift_bytes);
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            drift_modified
        );
        assert_eq!(
            std::fs::read(&ownership_index).unwrap(),
            ownership_index_bytes
        );
        assert_eq!(
            std::fs::read(&snapshot_index).unwrap(),
            snapshot_index_bytes
        );

        std::fs::write(&target, b"{").unwrap();
        let unparseable = state.drift("claude-code", "/opt/claude").unwrap();
        assert_eq!(unparseable[0].status, DriftStatus::Unparseable);
        assert!(unparseable[0].current_hash.is_some());
        std::fs::remove_file(&target).unwrap();
        let missing = state.drift("claude-code", "/opt/claude").unwrap();
        assert_eq!(missing[0].status, DriftStatus::Missing);
        assert!(missing[0].current_hash.is_none());
        assert_eq!(
            std::fs::read(&ownership_index).unwrap(),
            ownership_index_bytes
        );
        assert_eq!(
            std::fs::read(&snapshot_index).unwrap(),
            snapshot_index_bytes
        );
        assert!(
            !state.any_connected_to(&runtime).unwrap(),
            "ownership alone must not report a tampered Agent config as connected"
        );
        std::fs::write(&target, connected_bytes).unwrap();

        install_scan(&state, catalog, vec![record(&target, false)]);
        let snapshots = state.list_snapshots("claude-code").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].restorable);
        let restore = state
            .plan_restore(&snapshots[0].snapshot_id, "main")
            .unwrap();
        assert_eq!(restore.plan.intent, PlanIntent::Restore);
        let disconnect = state
            .plan_disconnect("claude-code", "/opt/claude", "main")
            .unwrap();
        assert_eq!(disconnect.plan.intent, PlanIntent::Disconnect);
        assert!(serde_json::to_string(&restore)
            .unwrap()
            .contains("vk-command-lifecycle"));
        std::fs::remove_dir_all(root).ok();
    }
}
#[test]
fn owned_value_plan_conflicts_keep_stable_public_reason_codes() {
    assert_eq!(
        AgentCommandError::plan(OWNED_VALUES_CHANGED.to_string()).code,
        OWNED_VALUES_CHANGED
    );
    assert_eq!(
        AgentCommandError::plan(COMPANION_OWNED_VALUES_CHANGED.to_string()).code,
        COMPANION_OWNED_VALUES_CHANGED
    );
}
