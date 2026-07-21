//! Structured Tauri IPC for Agent discovery and configuration transactions.
//!
//! Client input is deliberately limited to opaque IDs, a path that must match
//! the latest server-side scan exactly, and a short-lived confirmation token.
//! Target paths, patches, configuration bytes and executable commands are
//! never accepted from the renderer.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use ring::hmac;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{State, WebviewWindow};
use zeroize::Zeroizing;

use super::compatibility::{
    evaluate_discovery, production_remote_config, select_catalog, CatalogSource,
    CompatibilityCatalog, UreqCatalogTransport,
};
use super::connectors::{
    ClaudeCodeConnector, CodexConnector, ConnectInput, Connector, HermesConnector,
    OpenClawConnector, OpenCodeConnector,
};
use super::discovery::DiscoveryScanner;
use super::ownership::{FileOwnershipStore, OwnershipStore};
use super::plan::{
    build_connection_plan, build_disconnect_plan, build_snapshot_restore_plan,
    generate_operation_id, read_config_source, ConfigSource, PreparedChangePlan,
};
use super::registry::AgentRegistry;
use super::snapshot::{FileSnapshotStore, MasterKeyStore, OsKeychainMasterKeyStore, SnapshotStore};
use super::transaction::{
    Clock, ConfirmedOperation, FsAtomicConfigWriter, ParseOnlyVerifier, RecoveryStatus,
    RuntimeAdmission, SystemClock, TransactionEngine, TransactionFailure, TransactionOutcome,
    TransactionStage,
};
use super::types::{
    AgentUiMetadata, AllowedAction, CompatibilityDecision, CompatibilityStatus, ConfigChangePlan,
    DiscoveryRecord, PlanIntent, ReasonCode, SnapshotRecord,
};
use crate::{
    anthropic_inbound_ready, openai_inbound_ready, responses_inbound_ready, AgentIntegrationPaths,
    AppStateManaged,
};

const MAX_PENDING_PLANS: usize = 64;
const MAX_SESSION_LABEL_BYTES: usize = 128;
const CONFIRMATION_TOKEN_BYTES: usize = 32;

static CLAUDE_CONNECTOR: ClaudeCodeConnector = ClaudeCodeConnector;
static CODEX_CONNECTOR: CodexConnector = CodexConnector;
static OPENCODE_CONNECTOR: OpenCodeConnector = OpenCodeConnector;
static OPENCLAW_CONNECTOR: OpenClawConnector = OpenClawConnector;
static HERMES_CONNECTOR: HermesConnector = HermesConnector;

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInstallationView {
    pub discovery: DiscoveryRecord,
    pub compatibility: CompatibilityDecision,
    pub connected: bool,
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
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandError {
    pub code: String,
    pub message: String,
    pub stage: Option<TransactionStage>,
    pub recovery: Option<RecoveryStatus>,
    pub recovery_reason_code: Option<String>,
}

impl AgentCommandError {
    fn boundary(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            stage: None,
            recovery: None,
            recovery_reason_code: None,
        }
    }

    fn internal(message: String) -> Self {
        Self::boundary("agent_operation_rejected", message)
    }
}

impl From<TransactionFailure> for AgentCommandError {
    fn from(value: TransactionFailure) -> Self {
        Self {
            code: value.reason_code.clone(),
            message: "Agent 配置事务未完成，请重新扫描并预览".to_string(),
            stage: Some(value.stage),
            recovery: Some(value.recovery),
            recovery_reason_code: value.recovery_reason_code,
        }
    }
}

struct ScanSnapshot {
    catalog: CompatibilityCatalog,
    source: CatalogSource,
    warning: Option<String>,
    records: Vec<DiscoveryRecord>,
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
        let mut selected = matches.into_iter().next().expect("length checked");
        // Every row in a multi-install scan carries the conflict group. Exact
        // lookup above is the only trusted transition to a selected instance.
        selected.conflict_group = None;
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
    claude_origin: String,
    codex_base: String,
    opencode_base: String,
    openclaw_base: String,
    hermes_base: String,
    virtual_key: Zeroizing<String>,
    anthropic_ready: bool,
    responses_ready: bool,
    openai_ready: bool,
}

impl AgentProxyRuntime {
    fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"token-station-agent-proxy-binding-v1\0");
        hash_field(&mut hash, self.claude_origin.as_bytes());
        hash_field(&mut hash, self.codex_base.as_bytes());
        hash_field(&mut hash, self.opencode_base.as_bytes());
        hash_field(&mut hash, self.openclaw_base.as_bytes());
        hash_field(&mut hash, self.hermes_base.as_bytes());
        hash_field(&mut hash, self.virtual_key.as_bytes());
        hash.update([
            u8::from(self.anthropic_ready),
            u8::from(self.responses_ready),
            u8::from(self.openai_ready),
        ]);
        hash.finalize().into()
    }

    fn input_for<'a>(&'a self, connector_id: &str) -> Result<ConnectInput<'a>, AgentCommandError> {
        match connector_id {
            "claude-code-v1" => Ok(ConnectInput {
                base_url: &self.claude_origin,
                token: Some(self.virtual_key.as_str()),
                adapter_ready: self.anthropic_ready,
            }),
            "codex-v1" => Ok(ConnectInput {
                base_url: &self.codex_base,
                token: None,
                adapter_ready: self.responses_ready,
            }),
            "opencode-v1" => Ok(ConnectInput {
                base_url: &self.opencode_base,
                token: Some(self.virtual_key.as_str()),
                adapter_ready: self.openai_ready,
            }),
            "openclaw-v1" => Ok(ConnectInput {
                base_url: &self.openclaw_base,
                token: Some(self.virtual_key.as_str()),
                adapter_ready: self.openai_ready,
            }),
            "hermes-v1" => Ok(ConnectInput {
                base_url: &self.hermes_base,
                token: Some(self.virtual_key.as_str()),
                adapter_ready: self.openai_ready,
            }),
            _ => Err(AgentCommandError::boundary(
                "unsupported_connector",
                "兼容目录引用了未实现的 Connector",
            )),
        }
    }
}

pub struct AgentCommandState {
    registry: AgentRegistry,
    paths: AgentIntegrationPaths,
    keys: Arc<dyn MasterKeyStore>,
    snapshots: FileSnapshotStore<Arc<dyn MasterKeyStore>>,
    ownership: FileOwnershipStore,
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

impl AgentCommandState {
    pub fn new(paths: AgentIntegrationPaths) -> Result<Self, String> {
        Self::new_with_master_key(paths, Arc::new(OsKeychainMasterKeyStore))
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
        let ownership = FileOwnershipStore::new(paths.ownership_root.clone());
        Ok(Self {
            registry,
            paths,
            keys,
            snapshots,
            ownership,
            token_key: hmac::Key::new(hmac::HMAC_SHA256, &process_key),
            session: Mutex::new(CommandSession::default()),
            scan_in_progress: AtomicBool::new(false),
            clock: SystemClock,
        })
    }

    fn registry_metadata(&self) -> Vec<AgentUiMetadata> {
        self.registry.ui_metadata()
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

    fn scan(&self) -> Result<Vec<AgentView>, AgentCommandError> {
        let _scan_guard = self.begin_scan()?;
        let snapshot = self.perform_scan()?;
        let views = self.views(&snapshot)?;
        self.session
            .lock()
            .map_err(|_| AgentCommandError::boundary("state_poisoned", "Agent 会话状态不可用"))?
            .scan = Some(snapshot);
        Ok(views)
    }

    fn perform_scan(&self) -> Result<ScanSnapshot, AgentCommandError> {
        let now_ms = self.clock.now_ms();
        let selection = select_catalog(
            &self.registry,
            &self.paths.compatibility_cache_dir,
            &production_remote_config(),
            &UreqCatalogTransport,
            now_ms,
        )
        .map_err(AgentCommandError::internal)?;
        let records = DiscoveryScanner::from_process(&self.registry).scan_registry(&self.registry);
        Ok(ScanSnapshot {
            catalog: selection.catalog,
            source: selection.source,
            warning: selection.warning,
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

    fn views(&self, snapshot: &ScanSnapshot) -> Result<Vec<AgentView>, AgentCommandError> {
        self.registry
            .descriptors()
            .iter()
            .map(|descriptor| {
                let installations: Result<Vec<_>, AgentCommandError> = snapshot
                    .records
                    .iter()
                    .filter(|record| record.agent_id == descriptor.agent_id)
                    .map(|record| {
                        let connected = !self
                            .ownership
                            .list_agent_installation(&record.agent_id, &record.canonical_path)
                            .map_err(AgentCommandError::internal)?
                            .is_empty();
                        Ok(AgentInstallationView {
                            discovery: record.clone(),
                            compatibility: evaluate_discovery(
                                &snapshot.catalog,
                                descriptor,
                                record,
                            ),
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
                    metadata: AgentUiMetadata::from(descriptor),
                    installations,
                    status,
                    catalog_sequence: snapshot.catalog.sequence,
                    catalog_expires_at_ms: snapshot.catalog.expires_at_ms,
                    catalog_source: match snapshot.source {
                        CatalogSource::Builtin => "builtin",
                        CatalogSource::Remote => "remote",
                    },
                    catalog_warning: snapshot.warning.clone(),
                })
            })
            .collect()
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
        session_label: &str,
        runtime: &AgentProxyRuntime,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        validate_session_label(session_label)?;
        let (record, mut decision, sequence, catalog_expiry) =
            self.selected(agent_id, installation_path)?;
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
        if decision.status == CompatibilityStatus::DetectedInferred {
            // The scanner already proved the exact structural fingerprint. The
            // connector's source/projected validation below is the second,
            // read-only preflight gate before experimental confirmation.
            decision
                .allowed_actions
                .insert(AllowedAction::ConfirmExperimentalConnect);
        }
        let connector_id = decision
            .connector_id
            .as_deref()
            .ok_or_else(|| AgentCommandError::boundary("not_admitted", "当前版本不能接入"))?;
        let connector = connector_for(connector_id)?;
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
        let prepared = build_disconnect_plan(
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
        .map_err(AgentCommandError::internal)?;
        self.issue_plan(prepared, &record, session_label, None)
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

    fn plan_restore(
        &self,
        snapshot_id: &str,
        session_label: &str,
    ) -> Result<ConfigPlanView, AgentCommandError> {
        validate_session_label(session_label)?;
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
        let prepared = build_snapshot_restore_plan(
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
        .map_err(AgentCommandError::internal)?;
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
        let confirmation = ConfirmedOperation {
            operation_id: operation_id.to_string(),
            confirmed_at_ms: self.clock.now_ms(),
            confirmations: taken
                .prepared
                .view
                .required_confirmations
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
        };
        let admission = RuntimeAdmission {
            compatibility_sequence: sequence,
            status: decision.status,
        };
        let engine = TransactionEngine::new(
            &self.snapshots,
            &self.ownership,
            self.keys.as_ref(),
            &FsAtomicConfigWriter,
            &ParseOnlyVerifier,
            &self.clock,
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

fn connector_for(connector_id: &str) -> Result<&'static dyn Connector, AgentCommandError> {
    match connector_id {
        "claude-code-v1" => Ok(&CLAUDE_CONNECTOR),
        "codex-v1" => Ok(&CODEX_CONNECTOR),
        "opencode-v1" => Ok(&OPENCODE_CONNECTOR),
        "openclaw-v1" => Ok(&OPENCLAW_CONNECTOR),
        "hermes-v1" => Ok(&HERMES_CONNECTOR),
        _ => Err(AgentCommandError::boundary(
            "unsupported_connector",
            "Connector 不在本机构建的准入列表中",
        )),
    }
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
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
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

fn runtime_from_app(state: &AppStateManaged) -> Result<AgentProxyRuntime, AgentCommandError> {
    let inner = state
        .0
        .lock()
        .map_err(|_| AgentCommandError::boundary("app_state_poisoned", "应用状态不可用"))?;
    inner
        .ensure_editable()
        .map_err(AgentCommandError::internal)?;
    let serve = inner.serve_view();
    if !serve.running {
        return Err(AgentCommandError::boundary(
            "proxy_not_running",
            "请先启动代理再生成或应用连接计划",
        ));
    }
    let origin = format!("http://{}", serve.listen)
        .trim_end_matches('/')
        .to_string();
    Ok(AgentProxyRuntime {
        claude_origin: format!("{origin}/agents/claude-code"),
        codex_base: format!("{origin}/agents/codex/v1"),
        opencode_base: format!("{origin}/agents/opencode/v1"),
        openclaw_base: format!("{origin}/agents/openclaw/v1"),
        hermes_base: format!("{origin}/agents/nous-hermes-agent/v1"),
        virtual_key: Zeroizing::new(
            serve
                .virtual_key
                .unwrap_or_else(|| "token-station-no-auth".to_string()),
        ),
        anthropic_ready: anthropic_inbound_ready(&inner.draft["plugins"]),
        responses_ready: responses_inbound_ready(&inner.draft["plugins"]),
        openai_ready: openai_inbound_ready(&inner.draft["plugins"]),
    })
}

#[tauri::command]
pub(crate) fn list_agent_registry(state: State<'_, AgentCommandState>) -> Vec<AgentUiMetadata> {
    state.registry_metadata()
}

#[tauri::command(async)]
pub(crate) fn scan_agents(
    state: State<'_, AgentCommandState>,
) -> Result<Vec<AgentView>, AgentCommandError> {
    state.scan()
}

#[tauri::command(async)]
pub(crate) fn plan_agent_connection(
    state: State<'_, AgentCommandState>,
    app_state: State<'_, AppStateManaged>,
    window: WebviewWindow,
    agent_id: String,
    installation_path: String,
) -> Result<ConfigPlanView, AgentCommandError> {
    let runtime = runtime_from_app(&app_state)?;
    state.refresh_scan()?;
    state.plan_connection(&agent_id, &installation_path, window.label(), &runtime)
}

#[tauri::command(async)]
pub(crate) fn apply_agent_plan(
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
    state.apply(
        &operation_id,
        &confirmation_token,
        window.label(),
        &[PlanIntent::Connect, PlanIntent::Disconnect],
        runtime.as_ref(),
    )
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

#[tauri::command]
pub(crate) fn list_agent_snapshots(
    state: State<'_, AgentCommandState>,
    agent_id: String,
) -> Result<Vec<SnapshotView>, AgentCommandError> {
    state.list_snapshots(&agent_id)
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

    use super::*;
    use crate::agent_integration::connectors::ConnectInput;
    use crate::agent_integration::plan::build_connection_plan;
    use crate::agent_integration::types::{
        Diagnostic, DiscoveryEvidence, DiscoverySource, Platform,
    };

    struct MemoryMasterKey;

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
                compatibility_cache_dir: root.join("cache"),
                snapshot_root: root.join("snapshots"),
                ownership_root: root.join("ownership"),
            },
            Arc::new(MemoryMasterKey),
        )
        .unwrap()
    }

    fn record(target: &Path, conflict: bool) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: "claude-code".to_string(),
            executable_path: "/opt/claude".to_string(),
            canonical_path: "/opt/claude".to_string(),
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
            reason_code: ReasonCode::VerifiedRangeMatch,
            message: "verified".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("claude-code-v1".to_string()),
            allowed_actions: BTreeSet::from([AllowedAction::PreviewConnect]),
        }
    }

    fn prepared(target: &Path, secret: &str, now_ms: u64) -> PreparedChangePlan {
        let source = read_config_source(target).unwrap();
        build_connection_plan(
            &CLAUDE_CONNECTOR,
            &record(target, false),
            &decision(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some(secret),
                adapter_ready: true,
            },
            1,
            None,
            now_ms,
            generate_operation_id().unwrap(),
        )
        .unwrap()
    }

    fn runtime(token: &str) -> AgentProxyRuntime {
        AgentProxyRuntime {
            claude_origin: "http://127.0.0.1:8787/agents/claude-code".to_string(),
            codex_base: "http://127.0.0.1:8787/agents/codex/v1".to_string(),
            opencode_base: "http://127.0.0.1:8787/agents/opencode/v1".to_string(),
            openclaw_base: "http://127.0.0.1:8787/agents/openclaw/v1".to_string(),
            hermes_base: "http://127.0.0.1:8787/agents/nous-hermes-agent/v1".to_string(),
            virtual_key: Zeroizing::new(token.to_string()),
            anthropic_ready: true,
            responses_ready: true,
            openai_ready: true,
        }
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
    fn commands_plan_token_is_session_bound_one_shot_and_secret_free() {
        let state = state("token");
        let target = scratch("target").join("settings.json");
        let now_ms = state.clock.now_ms();
        let prepared = prepared(&target, "vk-command-secret", now_ms);
        let view = state
            .issue_plan(prepared, &record(&target, false), "main", Some([7_u8; 32]))
            .unwrap();
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(!encoded.contains("vk-command-secret"));
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
        assert_eq!(registry_metadata.len(), 5);
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
        let views = state.views(&empty).unwrap();
        assert_eq!(views.len(), 5);
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
    fn commands_plan_is_memory_only_and_blocked_or_unknown_versions_cannot_bypass_it() {
        let state = state("plan-boundary");
        let target = scratch("plan-boundary-target").join("missing/settings.json");
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        install_scan(&state, catalog.clone(), vec![record(&target, false)]);
        let plan = state
            .plan_connection(
                "claude-code",
                "/opt/claude",
                "main",
                &runtime("vk-plan-memory-only"),
            )
            .unwrap();
        assert_eq!(plan.plan.target_config_path, target.to_str().unwrap());
        assert!(!target.exists());
        assert!(!state.paths.snapshot_root.exists());
        assert!(!state.paths.ownership_root.exists());
        assert!(!serde_json::to_string(&plan)
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
            .plan_connection("claude-code", "/opt/claude", "main", &runtime("vk-blocked"))
            .err()
            .expect("blocked version cannot produce a plan");
        assert_eq!(blocked.code, "not_admitted");

        let mut unknown_record = record(&target, false);
        unknown_record.version_raw = Some("99.0.0".to_string());
        unknown_record.version_normalized = Some("99.0.0".to_string());
        install_scan(&state, catalog, vec![unknown_record]);
        let unknown = state
            .plan_connection("claude-code", "/opt/claude", "main", &runtime("vk-unknown"))
            .err()
            .expect("unknown version cannot produce a plan");
        assert_eq!(unknown.code, "not_admitted");
        assert!(!target.exists());
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
            ("codex-v1", "http://127.0.0.1:8787/agents/codex/v1"),
            ("opencode-v1", "http://127.0.0.1:8787/agents/opencode/v1"),
            ("openclaw-v1", "http://127.0.0.1:8787/agents/openclaw/v1"),
            (
                "hermes-v1",
                "http://127.0.0.1:8787/agents/nous-hermes-agent/v1",
            ),
        ] {
            let connector = connector_for(connector_id).unwrap();
            let input = runtime_view.input_for(connector_id).unwrap();
            assert_eq!(input.base_url, expected_base);
            assert_eq!(connector.connector_id(), connector_id);
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
            let state = AppStateManaged(Mutex::new(crate::AppInner {
                config_path: root.join("token-station.json"),
                draft: crate::template(&root),
                load_error,
                server: crate::ServerLifecycle::stopped(),
            }));

            let error = runtime_from_app(&state)
                .err()
                .expect("runtime must fail closed");

            assert_eq!(error.code, expected_code);
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn commands_scan_views_and_plan_rejections_cover_stale_and_preflight_states() {
        let state = state("scan-views");
        let target = scratch("scan-view-target").join("settings.json");
        let catalog = CompatibilityCatalog::builtin(&state.registry).unwrap();
        let snapshot = ScanSnapshot {
            catalog: catalog.clone(),
            source: CatalogSource::Remote,
            warning: Some("catalog warning".to_string()),
            records: vec![record(&target, false)],
        };
        let views = state.views(&snapshot).unwrap();
        let claude = views
            .iter()
            .find(|view| view.metadata.agent_id == "claude-code")
            .unwrap();
        assert_eq!(claude.catalog_source, "remote");
        assert_eq!(claude.catalog_warning.as_deref(), Some("catalog warning"));
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
            .plan_connection("claude-code", "/opt/claude", "main", &runtime("vk"))
            .err()
            .expect("preflight diagnostic blocks planning");
        assert_eq!(error.code, "read_only_preflight_failed");

        let scanned = state.scan().unwrap();
        assert_eq!(scanned.len(), 5);
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
            .plan_connection("claude-code", "/opt/claude", "main", &runtime)
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
                    compatibility_sequence: 1,
                    status: CompatibilityStatus::DetectedVerified,
                },
                now_ms,
            )
            .unwrap();

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
        assert!(!serde_json::to_string(&restore)
            .unwrap()
            .contains("vk-command-lifecycle"));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(state.paths.compatibility_cache_dir.parent().unwrap()).ok();
    }
}
