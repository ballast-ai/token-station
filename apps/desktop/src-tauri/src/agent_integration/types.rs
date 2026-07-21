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
    EnvShebang {
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
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VersionOutputMatcher {
    SemverAnywhere,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigLocation {
    pub env_override: Option<EnvOverride>,
    pub platform_defaults: BTreeMap<Platform, Vec<String>>,
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
    DetectedInferred,
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
    VerifiedRangeMatch,
    PatchInferredFingerprintMatch,
    NoCompatibilityEntry,
    ConfigFingerprintChanged,
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
    DiscoveryOnlyNotAdmitted,
    ConnectionOwnershipActive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedAction {
    ViewDetails,
    Rescan,
    SelectInstallation,
    RunReadOnlyPreflight,
    PreviewConnect,
    ConfirmExperimentalConnect,
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
/// values and complete projected documents never enter this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigChangePlan {
    pub schema_version: u32,
    pub operation_id: String,
    pub intent: PlanIntent,
    pub agent_id: String,
    pub installation_path: String,
    pub target_config_path: String,
    pub target_existed: bool,
    pub before_hash: String,
    pub expected_after_hash: String,
    pub owned_paths: Vec<ConfigPath>,
    pub changes: Vec<RedactedChange>,
    pub human_diff: String,
    pub connector_id: String,
    pub compatibility_evidence: CompatibilityDecision,
    pub compatibility_sequence: u64,
    pub compatibility_expires_at_ms: Option<u64>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub required_confirmations: Vec<ConfirmationKind>,
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
    ExperimentalCompatibility,
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
    pub admission: AdmissionStatus,
}

impl From<&AgentDescriptor> for AgentUiMetadata {
    fn from(descriptor: &AgentDescriptor) -> Self {
        Self {
            agent_id: descriptor.agent_id.clone(),
            legacy_kind: descriptor.legacy_kind.clone(),
            display_name: descriptor.display_name.clone(),
            icon_key: descriptor.icon_key.clone(),
            admission: descriptor.admission,
        }
    }
}
