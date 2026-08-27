use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDescriptor {
    pub schema_version: u32,
    pub agent_id: String,
    pub legacy_kind: Option<String>,
    pub display_name: String,
    pub icon_key: String,
    pub ui_order: u16,
    pub nav_mark: String,
    pub admission: AdmissionStatus,
    pub executable_candidates: Vec<String>,
    pub known_install_locations: BTreeMap<Platform, Vec<String>>,
    pub version_probe: VersionProbe,
    pub config_locations: Vec<ConfigLocation>,
    pub protocol_binding: Option<ProtocolBinding>,
    pub local_connector_ids: Vec<String>,
    pub discovery_fingerprint_rules: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionStatus {
    Supported,
    DiscoveryOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Linux,
    Windows,
    Wsl,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionProbe {
    pub argv: Vec<String>,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub output_matcher: VersionOutputMatcher,
    #[serde(default)]
    pub retry_on_timeout: bool,
    pub runtime: Option<ProbeRuntime>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeRuntime {
    Direct,
    PassiveFile,
    EnvShebang {
        interpreter_candidates: Vec<String>,
        resolution_sources: Vec<RuntimeResolutionSource>,
        known_install_locations: BTreeMap<Platform, Vec<String>>,
    },
    NodePackage {
        interpreter_candidates: Vec<String>,
        resolution_sources: Vec<RuntimeResolutionSource>,
        known_install_locations: BTreeMap<Platform, Vec<String>>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResolutionSource {
    ObservedEntrySibling,
    KnownInstallLocations,
    Path,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VersionOutputMatcher {
    SemverAnywhere,
    SuccessOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigLocation {
    pub env_override: Option<EnvOverride>,
    pub platform_defaults: BTreeMap<Platform, Vec<String>>,
    #[serde(default)]
    pub installation_path_defaults: BTreeMap<Platform, Vec<String>>,
    pub format: ConfigFormat,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvOverride {
    pub name: String,
    pub value_kind: EnvValueKind,
    pub suffix: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvValueKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat {
    Json,
    Jsonc,
    Json5,
    Toml,
    Yaml,
    Dotenv,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolBinding {
    pub adapter_id: String,
    pub base_url_shape: BaseUrlShape,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseUrlShape {
    Origin,
    OriginV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRecord {
    pub agent_id: String,
    pub executable_path: String,
    pub canonical_path: String,
    pub binary_source: BinarySource,
    pub modified_at_ms: Option<u64>,
    pub binary_sha256: Option<String>,
    pub upgrade_command: Option<String>,
    pub version_raw: Option<String>,
    pub version_normalized: Option<String>,
    pub environment: Platform,
    pub evidence: Vec<DiscoveryEvidence>,
    pub is_path_default: bool,
    pub runnable: bool,
    pub config_candidates: Vec<String>,
    pub config_fingerprint: Option<String>,
    pub conflict_group: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub scanned_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinarySource {
    Homebrew,
    NpmGlobal,
    MicrosoftStore,
    Path,
    KnownPath,
    EnvOverride,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryEvidence {
    pub source: DiscoverySource,
    pub observed_path: String,
    pub is_path_default: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    KnownPath,
    PackageManager,
    Path,
    EnvOverride,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub reason_code: ReasonCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CompatibilityStatus {
    NotDetected,
    DetectedVerified,
    DetectedUnknown,
    DetectedBlocked,
    InstalledBroken,
    MultipleInstallations,
    Connected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    AgentNotFound,
    DefaultAdmission,
    ConnectorBindingNotUnique,
    BlockedVersionMatch,
    ReadOnlyPreflightFailed,
    VersionProbeTimeout,
    VersionProbeExitFailure,
    VersionOutputUnparseable,
    VersionOutputTruncated,
    ExecutableNotRunnable,
    MultipleCanonicalPaths,
    ConfigReadFailed,
    ConfigParseFailed,
    InvalidEnvironmentOverride,
    ConnectionOwnershipActive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedAction {
    ViewDetails,
    Rescan,
    SelectInstallation,
    PreviewConnect,
    Disconnect,
    ViewSnapshots,
    RestoreSnapshot,
    ExportDiagnostics,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityDecision {
    pub agent_id: String,
    pub installation_path: Option<String>,
    pub status: CompatibilityStatus,
    pub reason_code: ReasonCode,
    pub message: String,
    pub matched_catalog_version: Option<String>,
    pub connector_id: Option<String>,
    pub allowed_actions: std::collections::BTreeSet<AllowedAction>,
}

/// Redacted, IPC-safe view of a server-held configuration plan. Secret patch
/// values and complete projected documents never enter this type. A plan can
/// include bounded semantic previews for fields the Connector marks non-secret.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigChangePlan {
    pub schema_version: u32,
    pub operation_id: String,
    pub intent: PlanIntent,
    pub agent_id: String,
    pub installation_path: String,
    pub target_config_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_config_paths: Vec<String>,
    pub target_existed: bool,
    pub before_hash: String,
    pub expected_after_hash: String,
    pub owned_paths: Vec<ConfigPath>,
    pub changes: Vec<RedactedChange>,
    pub projection: ConnectorProjection,
    pub human_diff: String,
    pub connector_id: String,
    pub compatibility_evidence: CompatibilityDecision,
    pub compatibility_sequence: u64,
    pub compatibility_expires_at_ms: Option<u64>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub required_confirmations: Vec<ConfirmationKind>,
}

/// Public contract for one reversible Connector projection. It contains only
/// bounded non-secret previews. Exact patch values stay in the prepared plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorProjection {
    pub schema_version: u32,
    pub files: Vec<ConnectorFileProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorFileProjection {
    pub target_config_path: String,
    pub format: String,
    pub target_existed: bool,
    pub before_hash: String,
    pub expected_after_hash: String,
    pub owned_paths: Vec<ConfigPath>,
    pub forward_changes: Vec<RedactedChange>,
    pub reverse_changes: Vec<RedactedChange>,
    pub credential_bindings: Vec<CredentialBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub path: ConfigPath,
    pub source: CredentialSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    LocalVirtualKey,
    EncryptedSnapshot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanIntent {
    Connect,
    Disconnect,
    Restore,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedactedChange {
    pub operation: PatchKind,
    pub path: ConfigPath,
    pub sensitive: bool,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_preview: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPath {
    pub segments: Vec<String>,
}

/// Internal patch data. This deliberately implements neither `Serialize` nor
/// `Debug`, because values may contain a virtual key or another credential.
#[derive(Clone, PartialEq)]
pub struct PatchOperation {
    pub operation: PatchKind,
    pub path: ConfigPath,
    pub value: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchKind {
    Add,
    Replace,
    Remove,
    Test,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationKind {
    Installation,
    TargetConfig,
    ConfigurationDiff,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftStatus {
    Unmanaged,
    InSync,
    UnownedChanges,
    ManagedChanges,
    Missing,
    Unreadable,
    Unparseable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftScope {
    Managed,
    Unowned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftChangeKind {
    Added,
    Removed,
    Changed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriftChange {
    pub path: ConfigPath,
    pub scope: DriftScope,
    pub kind: DriftChangeKind,
    pub current_matches_managed: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDriftView {
    pub agent_id: String,
    pub installation_path: String,
    pub target_config_path: String,
    pub connector_id: String,
    pub status: DriftStatus,
    pub baseline_hash: String,
    pub managed_hash: String,
    pub current_hash: Option<String>,
    pub checked_at_ms: u64,
    pub changes: Vec<DriftChange>,
    pub truncated: bool,
    pub message: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecord {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub operation_id: String,
    pub agent_id: String,
    pub target_config_path: String,
    pub envelope_hash: String,
    pub before_hash: String,
    pub original_existed: bool,
    pub original_permissions: Option<u32>,
    pub original_owner: Option<String>,
    pub created_at_ms: u64,
    pub connector_id: String,
    pub app_version: String,
    pub pinned: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUiMetadata {
    pub agent_id: String,
    pub legacy_kind: Option<String>,
    pub display_name: String,
    pub icon_key: String,
    pub ui_order: u16,
    pub nav_mark: String,
    pub admission: AdmissionStatus,
    pub connector_capabilities: Vec<AgentConnectorCapabilityView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConnectorCapabilityView {
    pub connector_id: String,
    pub adapter_id: String,
    pub base_url_shape: BaseUrlShape,
    pub platforms: Vec<Platform>,
    pub config_format: String,
    pub config_path_template: String,
    pub owned_fields: Vec<String>,
    pub requires_virtual_key: bool,
    pub restart_required: bool,
}

impl From<&AgentDescriptor> for AgentUiMetadata {
    fn from(descriptor: &AgentDescriptor) -> Self {
        Self {
            agent_id: descriptor.agent_id.clone(),
            legacy_kind: descriptor.legacy_kind.clone(),
            display_name: descriptor.display_name.clone(),
            icon_key: descriptor.icon_key.clone(),
            ui_order: descriptor.ui_order,
            nav_mark: descriptor.nav_mark.clone(),
            admission: descriptor.admission,
            connector_capabilities: Vec::new(),
        }
    }
}
