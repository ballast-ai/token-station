use std::path::{Component, Path};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::config_codec::{
    apply_patch, parse_rendered, parse_source_bytes, project_owned_paths, render_document,
    DocumentFormat,
};
use super::connectors::{validate_patch_ownership, ConnectInput, Connector};
use super::ownership::{ownership_matches, OwnershipRecord};
use super::types::{
    AllowedAction, CompatibilityDecision, CompatibilityStatus, ConfigChangePlan, ConfigPath,
    ConfirmationKind, DiscoveryRecord, PatchKind, PatchOperation, PlanIntent, RedactedChange,
    SnapshotRecord,
};

const PLAN_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PLAN_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// Exact source state captured during preview. Bytes are zeroized on drop and
/// the type deliberately has no `Debug` or serialization implementation.
#[derive(Clone)]
pub struct ConfigSource {
    pub existed: bool,
    pub exact_bytes: Zeroizing<Vec<u8>>,
    pub original_permissions: Option<u32>,
    pub original_owner: Option<String>,
}

impl ConfigSource {
    #[must_use]
    pub fn missing() -> Self {
        Self {
            existed: false,
            exact_bytes: Zeroizing::new(Vec::new()),
            original_permissions: None,
            original_owner: None,
        }
    }

    #[must_use]
    pub fn existing(bytes: Vec<u8>, permissions: Option<u32>, owner: Option<String>) -> Self {
        Self {
            existed: true,
            exact_bytes: Zeroizing::new(bytes),
            original_permissions: permissions,
            original_owner: owner,
        }
    }
}

/// Complete server-held plan. The only serializable projection is `view`;
/// patch values stay in memory and can contain credentials.
pub struct PreparedChangePlan {
    pub view: ConfigChangePlan,
    pub(crate) projected_bytes: Zeroizing<Vec<u8>>,
    pub(crate) format: DocumentFormat,
    pub(crate) label: &'static str,
    pub(crate) ownership: Option<PlanOwnershipBinding>,
}

pub(crate) struct PlanOwnershipBinding {
    pub record: OwnershipRecord,
    pub disposition: OwnershipDisposition,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum OwnershipDisposition {
    Remove,
    Update,
}

pub fn read_config_source(path: &Path) -> Result<ConfigSource, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err("目标配置是符号链接，必须重新选择真实配置文件".to_string());
            }
            if !metadata.is_file() {
                return Err("目标配置存在但不是普通文件".to_string());
            }
            if metadata.len() > MAX_CONFIG_BYTES {
                return Err("目标配置超过安全读取上限".to_string());
            }
            let bytes = std::fs::read(path).map_err(|_| "读取目标配置失败".to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                Ok(ConfigSource::existing(
                    bytes,
                    Some(metadata.permissions().mode() & 0o7777),
                    Some(format!("{}:{}", metadata.uid(), metadata.gid())),
                ))
            }
            #[cfg(not(unix))]
            {
                Ok(ConfigSource::existing(bytes, None, None))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigSource::missing()),
        Err(_) => Err("读取目标配置元数据失败".to_string()),
    }
}

pub fn generate_operation_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "生成安全 operation ID 失败".to_string())?;
    Ok(lower_hex(&bytes))
}

#[allow(clippy::too_many_arguments)]
pub fn build_connection_plan(
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    target_path: &Path,
    source: &ConfigSource,
    input: &ConnectInput<'_>,
    compatibility_sequence: u64,
    compatibility_expires_at_ms: Option<u64>,
    now_ms: u64,
    operation_id: String,
) -> Result<PreparedChangePlan, String> {
    validate_binding(
        connector,
        discovery,
        compatibility,
        target_path,
        compatibility_sequence,
        compatibility_expires_at_ms,
        now_ms,
        &operation_id,
    )?;
    connector.validate_preconditions(input)?;

    let mut document = parse_source_bytes(
        source.existed.then_some(source.exact_bytes.as_slice()),
        connector.format(),
        connector.label(),
    )?;
    connector.validate_source(&document)?;
    let operations = connector.connect_patch(input)?;
    let owned_paths = connector.owned_paths();
    validate_owned_paths(&owned_paths)?;
    validate_patch_ownership(&operations, &owned_paths)?;
    apply_patch(&mut document, &operations)?;
    connector.validate_projected(&document, input)?;
    let rendered = render_document(&document, connector.label())?;
    let reparsed = parse_rendered(&rendered, connector.format(), connector.label())?;
    connector.validate_projected(&reparsed, input)?;

    let before_hash = file_revision_hash(target_path, source)?;
    let projected_bytes = rendered.into_bytes();
    let projected = ConfigSource::existing(
        projected_bytes.clone(),
        source.original_permissions.or(Some(0o600)),
        source.original_owner.clone(),
    );
    let expected_after_hash = file_revision_hash(target_path, &projected)?;
    let changes = redact_changes(&operations, &connector.sensitive_paths());
    let human_diff = changes
        .iter()
        .map(|change| {
            format!(
                "{} {}: {}",
                patch_symbol(change.operation),
                change.path,
                change.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let required_confirmations = vec![
        ConfirmationKind::Installation,
        ConfirmationKind::TargetConfig,
        ConfirmationKind::ConfigurationDiff,
    ];
    let expires_at_ms = now_ms
        .saturating_add(DEFAULT_PLAN_TTL_MS)
        .min(compatibility_expires_at_ms.unwrap_or(u64::MAX));

    Ok(PreparedChangePlan {
        view: ConfigChangePlan {
            schema_version: PLAN_SCHEMA_VERSION,
            operation_id,
            intent: PlanIntent::Connect,
            agent_id: connector.agent_id().to_string(),
            installation_path: discovery.canonical_path.clone(),
            target_config_path: strict_path_text(target_path)?.to_string(),
            target_existed: source.existed,
            before_hash,
            expected_after_hash,
            owned_paths,
            changes,
            human_diff,
            connector_id: connector.connector_id().to_string(),
            compatibility_evidence: compatibility.clone(),
            compatibility_sequence,
            compatibility_expires_at_ms,
            created_at_ms: now_ms,
            expires_at_ms,
            required_confirmations,
        },
        projected_bytes: Zeroizing::new(projected_bytes),
        format: connector.format(),
        label: connector.label(),
        ownership: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_disconnect_plan(
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    target_path: &Path,
    current: &ConfigSource,
    ownership: &OwnershipRecord,
    baseline_record: &SnapshotRecord,
    baseline: &ConfigSource,
    master_key: &Zeroizing<[u8; 32]>,
    compatibility_sequence: u64,
    compatibility_expires_at_ms: Option<u64>,
    now_ms: u64,
    operation_id: String,
) -> Result<PreparedChangePlan, String> {
    if baseline_record.snapshot_id != ownership.baseline_snapshot_id {
        return Err("断开基线快照与 ownership 不一致".to_string());
    }
    build_owned_projection_plan(
        PlanIntent::Disconnect,
        OwnershipDisposition::Remove,
        connector,
        discovery,
        compatibility,
        target_path,
        current,
        ownership,
        baseline_record,
        baseline,
        master_key,
        compatibility_sequence,
        compatibility_expires_at_ms,
        now_ms,
        operation_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_snapshot_restore_plan(
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    target_path: &Path,
    current: &ConfigSource,
    ownership: &OwnershipRecord,
    restore_record: &SnapshotRecord,
    restore_source: &ConfigSource,
    master_key: &Zeroizing<[u8; 32]>,
    compatibility_sequence: u64,
    compatibility_expires_at_ms: Option<u64>,
    now_ms: u64,
    operation_id: String,
) -> Result<PreparedChangePlan, String> {
    build_owned_projection_plan(
        PlanIntent::Restore,
        OwnershipDisposition::Update,
        connector,
        discovery,
        compatibility,
        target_path,
        current,
        ownership,
        restore_record,
        restore_source,
        master_key,
        compatibility_sequence,
        compatibility_expires_at_ms,
        now_ms,
        operation_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_owned_projection_plan(
    intent: PlanIntent,
    disposition: OwnershipDisposition,
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    target_path: &Path,
    current: &ConfigSource,
    ownership: &OwnershipRecord,
    source_record: &SnapshotRecord,
    source_snapshot: &ConfigSource,
    master_key: &Zeroizing<[u8; 32]>,
    compatibility_sequence: u64,
    compatibility_expires_at_ms: Option<u64>,
    now_ms: u64,
    operation_id: String,
) -> Result<PreparedChangePlan, String> {
    validate_management_binding(
        connector,
        discovery,
        compatibility,
        ownership,
        source_record,
        target_path,
        compatibility_sequence,
        &operation_id,
    )?;
    if source_record.original_existed != source_snapshot.existed
        || source_record.original_permissions != source_snapshot.original_permissions
        || source_record.original_owner != source_snapshot.original_owner
    {
        return Err("快照元数据与解密载荷不一致".to_string());
    }
    let mut current_document = parse_source_bytes(
        current.existed.then_some(current.exact_bytes.as_slice()),
        connector.format(),
        connector.label(),
    )?;
    if !ownership_matches(ownership, &current_document, master_key)? {
        return Err("当前 owned paths 已与 ownership 记录冲突，必须重新预览".to_string());
    }
    let source_document = parse_source_bytes(
        source_snapshot
            .existed
            .then_some(source_snapshot.exact_bytes.as_slice()),
        connector.format(),
        connector.label(),
    )?;
    project_owned_paths(
        &mut current_document,
        &source_document,
        &ownership.owned_paths,
    )?;
    let rendered = render_document(&current_document, connector.label())?;
    parse_rendered(&rendered, connector.format(), connector.label())?;
    let before_hash = file_revision_hash(target_path, current)?;
    let projected_bytes = rendered.into_bytes();
    let projected = ConfigSource::existing(
        projected_bytes.clone(),
        current.original_permissions.or(Some(0o600)),
        current.original_owner.clone(),
    );
    let expected_after_hash = file_revision_hash(target_path, &projected)?;
    let sensitive_paths = connector.sensitive_paths();
    let changes: Vec<_> = ownership
        .owned_paths
        .iter()
        .map(|path| {
            let sensitive = sensitive_paths.iter().any(|sensitive| {
                is_path_prefix(path, sensitive) || is_path_prefix(sensitive, path)
            });
            RedactedChange {
                operation: PatchKind::Replace,
                path: path.clone(),
                sensitive,
                summary: if sensitive {
                    "<恢复受管敏感值，内容已隐藏>"
                } else {
                    "<恢复受管值>"
                }
                .to_string(),
            }
        })
        .collect();
    let human_diff = changes
        .iter()
        .map(|change| format!("~ {}: {}", change.path, change.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let expires_at_ms = now_ms.saturating_add(DEFAULT_PLAN_TTL_MS);
    Ok(PreparedChangePlan {
        view: ConfigChangePlan {
            schema_version: PLAN_SCHEMA_VERSION,
            operation_id,
            intent,
            agent_id: ownership.agent_id.clone(),
            installation_path: ownership.installation_path.clone(),
            target_config_path: ownership.target_config_path.clone(),
            target_existed: current.existed,
            before_hash,
            expected_after_hash,
            owned_paths: ownership.owned_paths.clone(),
            changes,
            human_diff,
            connector_id: ownership.connector_id.clone(),
            compatibility_evidence: compatibility.clone(),
            compatibility_sequence,
            compatibility_expires_at_ms,
            created_at_ms: now_ms,
            expires_at_ms,
            required_confirmations: vec![
                ConfirmationKind::Installation,
                ConfirmationKind::TargetConfig,
                ConfirmationKind::ConfigurationDiff,
            ],
        },
        projected_bytes: Zeroizing::new(projected_bytes),
        format: connector.format(),
        label: connector.label(),
        ownership: Some(PlanOwnershipBinding {
            record: ownership.clone(),
            disposition,
        }),
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_management_binding(
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    ownership: &OwnershipRecord,
    source_record: &SnapshotRecord,
    target_path: &Path,
    compatibility_sequence: u64,
    operation_id: &str,
) -> Result<(), String> {
    validate_target_path(target_path)?;
    if operation_id.len() != 32
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || compatibility_sequence == 0
    {
        return Err("管理计划 operation 或 sequence 无效".to_string());
    }
    let target = strict_path_text(target_path)?;
    if connector.agent_id() != ownership.agent_id
        || connector.connector_id() != ownership.connector_id
        || discovery.agent_id != ownership.agent_id
        || discovery.canonical_path != ownership.installation_path
        || compatibility.agent_id != ownership.agent_id
        || compatibility.installation_path.as_deref() != Some(discovery.canonical_path.as_str())
        || target != ownership.target_config_path
        || source_record.agent_id != ownership.agent_id
        || source_record.target_config_path != ownership.target_config_path
        || source_record.connector_id != ownership.connector_id
    {
        return Err("Agent 管理计划与 installation/ownership/snapshot 绑定不一致".to_string());
    }
    if discovery.conflict_group.is_some() {
        return Err("检测到多个安装实例，必须先选择精确实例".to_string());
    }
    if !discovery.config_candidates.is_empty()
        && !discovery
            .config_candidates
            .iter()
            .any(|candidate| candidate == target)
    {
        return Err("目标配置不属于已发现实例".to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_binding(
    connector: &dyn Connector,
    discovery: &DiscoveryRecord,
    compatibility: &CompatibilityDecision,
    target_path: &Path,
    compatibility_sequence: u64,
    compatibility_expires_at_ms: Option<u64>,
    now_ms: u64,
    operation_id: &str,
) -> Result<(), String> {
    if operation_id.len() != 32
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("operation ID 无效".to_string());
    }
    if compatibility_sequence == 0 {
        return Err("兼容目录 sequence 无效".to_string());
    }
    if compatibility_expires_at_ms.is_some_and(|expiry| expiry <= now_ms) {
        return Err("兼容目录已在计划生成前过期".to_string());
    }
    if discovery.agent_id != connector.agent_id()
        || compatibility.agent_id != connector.agent_id()
        || compatibility.installation_path.as_deref() != Some(discovery.canonical_path.as_str())
        || compatibility.connector_id.as_deref() != Some(connector.connector_id())
    {
        return Err("Agent、安装实例、兼容证据与 Connector 绑定不一致".to_string());
    }
    if discovery.conflict_group.is_some() {
        return Err("检测到多个安装实例，必须先选择精确实例".to_string());
    }
    let allowed = match compatibility.status {
        CompatibilityStatus::DetectedVerified => compatibility
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect),
        _ => false,
    };
    if !allowed {
        return Err("当前兼容状态不允许生成可执行配置计划".to_string());
    }
    validate_target_path(target_path)?;
    let target = strict_path_text(target_path)?;
    if !discovery.config_candidates.is_empty()
        && !discovery
            .config_candidates
            .iter()
            .any(|candidate| candidate == target)
    {
        return Err("目标配置不属于已发现实例".to_string());
    }
    Ok(())
}

fn validate_target_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("目标配置必须是无遍历分量的绝对路径".to_string());
    }
    strict_path_text(path)?;
    Ok(())
}

fn validate_owned_paths(paths: &[ConfigPath]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("Connector 未声明 owned paths".to_string());
    }
    for (index, path) in paths.iter().enumerate() {
        if path.segments.is_empty() {
            return Err("Connector owned path 不能为空".to_string());
        }
        for other in paths.iter().skip(index + 1) {
            if is_path_prefix(path, other) || is_path_prefix(other, path) {
                return Err("Connector owned paths 重复或祖先范围重叠".to_string());
            }
        }
    }
    Ok(())
}

fn redact_changes(
    operations: &[PatchOperation],
    sensitive_paths: &[ConfigPath],
) -> Vec<RedactedChange> {
    operations
        .iter()
        .map(|operation| {
            let sensitive = sensitive_paths.iter().any(|sensitive| {
                is_path_prefix(&operation.path, sensitive)
                    || is_path_prefix(sensitive, &operation.path)
            });
            let summary = match (operation.operation, sensitive) {
                (PatchKind::Add | PatchKind::Replace, true) => "<敏感值已隐藏>".to_string(),
                (PatchKind::Add | PatchKind::Replace, false) => "<设置受管值>".to_string(),
                (PatchKind::Remove, _) => "<移除受管值>".to_string(),
                (PatchKind::Test, _) => "<核对受管值>".to_string(),
            };
            RedactedChange {
                operation: operation.operation,
                path: operation.path.clone(),
                sensitive,
                summary,
            }
        })
        .collect()
}

fn is_path_prefix(prefix: &ConfigPath, path: &ConfigPath) -> bool {
    prefix.segments.len() <= path.segments.len()
        && prefix.segments == path.segments[..prefix.segments.len()]
}

fn patch_symbol(kind: PatchKind) -> &'static str {
    match kind {
        PatchKind::Add => "+",
        PatchKind::Replace => "~",
        PatchKind::Remove => "-",
        PatchKind::Test => "?",
    }
}

fn strict_path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "目标配置路径不是有效 UTF-8".to_string())
}

pub fn file_revision_hash(path: &Path, source: &ConfigSource) -> Result<String, String> {
    let path = strict_path_text(path)?;
    let mut hash = Sha256::new();
    hash.update(b"token-station-config-revision-v1\0");
    encode_hash_field(&mut hash, path.as_bytes());
    hash.update([u8::from(source.existed)]);
    hash.update(
        source
            .original_permissions
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    encode_hash_field(
        &mut hash,
        source.original_owner.as_deref().unwrap_or("").as_bytes(),
    );
    encode_hash_field(&mut hash, source.exact_bytes.as_slice());
    Ok(format!("{:x}", hash.finalize()))
}

fn encode_hash_field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::agent_integration::connectors::ClaudeCodeConnector;
    use crate::agent_integration::types::{
        Diagnostic, DiscoveryEvidence, DiscoverySource, Platform, ReasonCode,
    };

    fn discovery(target: &Path) -> DiscoveryRecord {
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
            conflict_group: None,
            diagnostics: Vec::<Diagnostic>::new(),
            scanned_at_ms: 1,
        }
    }

    fn verified() -> CompatibilityDecision {
        CompatibilityDecision {
            agent_id: "claude-code".to_string(),
            installation_path: Some("/opt/claude".to_string()),
            status: CompatibilityStatus::DetectedVerified,
            reason_code: ReasonCode::DefaultAdmission,
            message: "admitted".to_string(),
            matched_catalog_version: Some("fixture".to_string()),
            connector_id: Some("claude-code-v1".to_string()),
            allowed_actions: BTreeSet::from([
                AllowedAction::ViewDetails,
                AllowedAction::PreviewConnect,
            ]),
        }
    }

    #[test]
    fn plan_is_redacted_and_binds_instance_revision_catalog_and_expiry() {
        let target = Path::new("/tmp/token-station-plan/settings.json");
        let source_marker = "source-configuration-marker";
        let secret = "vk-sensitive-plan-secret";
        let source = ConfigSource::existing(
            format!("{{\"unowned\":\"{source_marker}\"}}").into_bytes(),
            Some(0o640),
            Some("501:20".to_string()),
        );
        let prepared = build_connection_plan(
            &ClaudeCodeConnector,
            &discovery(target),
            &verified(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some(secret),
                adapter_ready: true,
            },
            7,
            Some(20_000),
            10_000,
            "01".repeat(16),
        )
        .unwrap();

        let serialized = serde_json::to_string(&prepared.view).unwrap();
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains(source_marker));
        assert!(!serialized.contains("patch_operations"));
        assert_eq!(prepared.view.compatibility_sequence, 7);
        assert_eq!(prepared.view.expires_at_ms, 20_000);
        assert_eq!(prepared.view.installation_path, "/opt/claude");
        assert!(prepared.view.changes.iter().any(|change| change.sensitive));
        assert_ne!(prepared.view.before_hash, prepared.view.expected_after_hash);
    }

    #[test]
    fn plan_missing_and_empty_files_have_distinct_revisions() {
        let target = Path::new("/tmp/token-station-plan/config.json");
        let missing = file_revision_hash(target, &ConfigSource::missing()).unwrap();
        let empty = file_revision_hash(
            target,
            &ConfigSource::existing(Vec::new(), Some(0o600), None),
        )
        .unwrap();
        assert_ne!(missing, empty);
    }

    #[test]
    fn plan_only_connectable_status_is_accepted_and_unsafe_states_are_rejected() {
        let target = Path::new("/tmp/token-station-plan/settings.json");
        let source = ConfigSource::missing();
        let mut unknown = verified();
        unknown.status = CompatibilityStatus::DetectedUnknown;
        unknown.reason_code = ReasonCode::ConnectorBindingNotUnique;
        unknown.allowed_actions.clear();
        let error = build_connection_plan(
            &ClaudeCodeConnector,
            &discovery(target),
            &unknown,
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some("hidden"),
                adapter_ready: true,
            },
            1,
            None,
            10,
            "03".repeat(16),
        )
        .err()
        .expect("a non-connectable status is rejected");
        assert!(error.contains("不允许"), "{error}");

        for status in [
            CompatibilityStatus::DetectedBlocked,
            CompatibilityStatus::InstalledBroken,
        ] {
            let mut decision = verified();
            decision.status = status;
            let error = build_connection_plan(
                &ClaudeCodeConnector,
                &discovery(target),
                &decision,
                target,
                &source,
                &ConnectInput {
                    base_url: "http://127.0.0.1:8787",
                    token: Some("hidden"),
                    adapter_ready: true,
                },
                1,
                None,
                10,
                "04".repeat(16),
            )
            .err()
            .expect("unsafe state is rejected");
            assert!(error.contains("不允许"), "{error}");
        }

        let mut multiple = discovery(target);
        multiple.conflict_group = Some("conflict".to_string());
        assert!(build_connection_plan(
            &ClaudeCodeConnector,
            &multiple,
            &verified(),
            target,
            &source,
            &ConnectInput {
                base_url: "http://127.0.0.1:8787",
                token: Some("hidden"),
                adapter_ready: true,
            },
            1,
            None,
            10,
            "05".repeat(16),
        )
        .is_err());
    }
}
