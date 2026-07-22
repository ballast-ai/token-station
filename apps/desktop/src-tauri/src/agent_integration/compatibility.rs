use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::registry::AgentRegistry;
use super::types::{
    AgentDescriptor, AllowedAction, CompatibilityDecision, CompatibilityStatus, DiscoveryRecord,
    ReasonCode,
};

pub const BUILTIN_COMPATIBILITY_JSON: &str =
    include_str!("../../agent-registry/builtin-compatibility.json");

const CATALOG_SCHEMA_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 64;
const MAX_RULES_PER_ENTRY: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityCatalog {
    pub schema_version: u32,
    pub catalog_version: String,
    pub sequence: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub minimum_app_version: String,
    pub entries: Vec<AgentCompatibilityEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCompatibilityEntry {
    pub agent_id: String,
    pub blocked: Vec<BlockedRule>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockedRule {
    pub version_requirement: String,
    pub reason: String,
}

impl CompatibilityCatalog {
    pub fn builtin(registry: &AgentRegistry) -> Result<Self, String> {
        let catalog = parse_catalog(BUILTIN_COMPATIBILITY_JSON.as_bytes())?;
        validate_catalog(&catalog, registry)?;
        Ok(catalog)
    }

    #[must_use]
    pub fn entry(&self, agent_id: &str) -> Option<&AgentCompatibilityEntry> {
        self.entries.iter().find(|entry| entry.agent_id == agent_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogSource {
    Builtin,
}

pub fn evaluate_discovery(
    catalog: &CompatibilityCatalog,
    descriptor: &AgentDescriptor,
    discovery: &DiscoveryRecord,
) -> CompatibilityDecision {
    let base_actions = BTreeSet::from([
        AllowedAction::ViewDetails,
        AllowedAction::Rescan,
        AllowedAction::ExportDiagnostics,
    ]);
    let decision =
        |status, reason_code, message: String, connector_id, extra_actions: &[AllowedAction]| {
            let mut allowed_actions = base_actions.clone();
            allowed_actions.extend(extra_actions.iter().copied());
            CompatibilityDecision {
                agent_id: descriptor.agent_id.clone(),
                installation_path: Some(discovery.canonical_path.clone()),
                status,
                reason_code,
                message,
                matched_catalog_version: Some(catalog.catalog_version.clone()),
                connector_id,
                allowed_actions,
            }
        };

    if discovery.conflict_group.is_some() {
        return decision(
            CompatibilityStatus::MultipleInstallations,
            ReasonCode::MultipleCanonicalPaths,
            "检测到多个安装实例，请先选择精确路径".to_string(),
            None,
            &[AllowedAction::SelectInstallation],
        );
    }
    if !discovery.runnable {
        return decision(
            CompatibilityStatus::InstalledBroken,
            discovery
                .diagnostics
                .first()
                .map_or(ReasonCode::ExecutableNotRunnable, |value| value.reason_code),
            "安装入口存在，但版本探测进程未成功运行".to_string(),
            None,
            &[],
        );
    }

    let connector_id = match descriptor.local_connector_ids.as_slice() {
        [connector_id] => connector_id.clone(),
        _ => {
            return decision(
                CompatibilityStatus::DetectedUnknown,
                ReasonCode::ConnectorBindingNotUnique,
                "无法唯一确定该 Agent 的配置 Connector".to_string(),
                None,
                &[],
            );
        }
    };

    if let Some(version) = discovery
        .version_normalized
        .as_deref()
        .and_then(|version| Version::parse(version).ok())
    {
        if let Some(blocked) = catalog.entry(&descriptor.agent_id).and_then(|entry| {
            entry
                .blocked
                .iter()
                .find(|rule| blocked_requirement_matches(&rule.version_requirement, &version))
        }) {
            return decision(
                CompatibilityStatus::DetectedBlocked,
                ReasonCode::BlockedVersionMatch,
                blocked.reason.clone(),
                None,
                &[],
            );
        }
    }

    if let Some(diagnostic) = discovery.diagnostics.iter().find(|diagnostic| {
        matches!(
            diagnostic.reason_code,
            ReasonCode::ReadOnlyPreflightFailed
                | ReasonCode::ConfigReadFailed
                | ReasonCode::ConfigParseFailed
                | ReasonCode::InvalidEnvironmentOverride
        )
    }) {
        return decision(
            CompatibilityStatus::DetectedUnknown,
            diagnostic.reason_code,
            "只读配置预检未通过，当前安装不能接入".to_string(),
            None,
            &[],
        );
    }

    decision(
        CompatibilityStatus::DetectedVerified,
        ReasonCode::DefaultAdmission,
        "已通过只读预检，可以安全接入".to_string(),
        Some(connector_id),
        &[AllowedAction::PreviewConnect],
    )
}

fn parse_catalog(bytes: &[u8]) -> Result<CompatibilityCatalog, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("兼容目录 JSON 无效：{error}"))
}

fn validate_catalog(
    catalog: &CompatibilityCatalog,
    registry: &AgentRegistry,
) -> Result<(), String> {
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        return Err("不支持的兼容目录 schema_version".to_string());
    }
    validate_catalog_version(&catalog.catalog_version)?;
    let minimum_app = Version::parse(&catalog.minimum_app_version)
        .map_err(|_| "兼容目录 minimum_app_version 无效".to_string())?;
    let current_app = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "当前 App 版本不是 SemVer".to_string())?;
    if minimum_app > current_app {
        return Err("兼容目录要求更高版本的 App".to_string());
    }
    if catalog.sequence == 0 || catalog.entries.is_empty() || catalog.entries.len() > MAX_ENTRIES {
        return Err("兼容目录 sequence 或 entries 无效".to_string());
    }

    let descriptors: BTreeMap<_, _> = registry
        .descriptors()
        .iter()
        .map(|descriptor| (descriptor.agent_id.as_str(), descriptor))
        .collect();
    if !catalog
        .entries
        .windows(2)
        .all(|pair| pair[0].agent_id < pair[1].agent_id)
    {
        return Err("兼容目录 entries 必须按 agent_id 严格排序".to_string());
    }
    let mut agent_ids = BTreeSet::new();
    for entry in &catalog.entries {
        if !agent_ids.insert(entry.agent_id.as_str()) {
            return Err("兼容目录包含重复 agent_id".to_string());
        }
        if !descriptors.contains_key(entry.agent_id.as_str()) {
            return Err("兼容目录引用未知 Agent".to_string());
        }
        if entry.blocked.len() > MAX_RULES_PER_ENTRY {
            return Err("兼容目录规则过多".to_string());
        }
        let mut unique_requirements = BTreeSet::new();
        for rule in &entry.blocked {
            validate_requirement(&rule.version_requirement)?;
            validate_safe_text(&rule.reason)?;
            if !unique_requirements.insert(rule.version_requirement.as_str()) {
                return Err("兼容目录包含重复规则".to_string());
            }
        }
    }
    Ok(())
}

fn validate_requirement(requirement: &str) -> Result<(), String> {
    if requirement.is_empty() || requirement.len() > 128 || VersionReq::parse(requirement).is_err()
    {
        return Err("兼容目录版本范围无效".to_string());
    }
    Ok(())
}

fn validate_safe_text(text: &str) -> Result<(), String> {
    if text.is_empty()
        || text.len() > 256
        || text.trim() != text
        || text.chars().any(char::is_control)
        || text.contains("://")
    {
        return Err("兼容目录阻断原因无效".to_string());
    }
    Ok(())
}

/// Blocking is intentionally more conservative than normal SemVer admission:
/// a prerelease inherits every block applied to its `major.minor.patch` core.
fn blocked_requirement_matches(requirement: &str, version: &Version) -> bool {
    let core_version = Version::new(version.major, version.minor, version.patch);
    VersionReq::parse(requirement).is_ok_and(|requirement| requirement.matches(&core_version))
}

fn validate_catalog_version(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || value.trim() != value
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("兼容目录 catalog_version 无效".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent_integration::types::{
        Diagnostic, DiscoveryEvidence, DiscoverySource, Platform,
    };

    fn discovery(agent_id: &str, version: Option<&str>) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: agent_id.to_string(),
            executable_path: format!("/tmp/{agent_id}"),
            canonical_path: format!("/tmp/{agent_id}"),
            version_raw: version.map(str::to_string),
            version_normalized: version.map(str::to_string),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: format!("/tmp/{agent_id}"),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: Vec::new(),
            config_fingerprint: None,
            conflict_group: None,
            diagnostics: Vec::new(),
            scanned_at_ms: 0,
        }
    }

    fn descriptor<'a>(registry: &'a AgentRegistry, agent_id: &str) -> &'a AgentDescriptor {
        registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == agent_id)
            .unwrap()
    }

    #[test]
    fn builtin_catalog_is_a_sorted_blocklist() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();

        assert_eq!(catalog.entries.len(), registry.descriptors().len());
        assert!(catalog.entries.iter().all(|entry| entry.blocked.is_empty()));
    }

    #[test]
    fn any_unblocked_version_with_unique_connector_and_preflight_is_connectable() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();
        let codex = descriptor(&registry, "codex");

        for version in [Some("0.145.0-alpha.18"), Some("99.0.0"), None] {
            let decision = evaluate_discovery(&catalog, codex, &discovery("codex", version));
            assert_eq!(decision.status, CompatibilityStatus::DetectedVerified);
            assert_eq!(decision.reason_code, ReasonCode::DefaultAdmission);
            assert_eq!(decision.connector_id.as_deref(), Some("codex-v1"));
            assert!(decision
                .allowed_actions
                .contains(&AllowedAction::PreviewConnect));
        }

        let mut non_semver = discovery("codex", None);
        non_semver.version_raw = Some("Codex nightly".to_string());
        non_semver.diagnostics.push(Diagnostic {
            reason_code: ReasonCode::VersionOutputUnparseable,
            message: "版本命令成功，但输出中没有可识别的 SemVer".to_string(),
        });
        let decision = evaluate_discovery(&catalog, codex, &non_semver);
        assert_eq!(decision.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(decision.connector_id.as_deref(), Some("codex-v1"));
    }

    #[test]
    fn blocklist_matches_release_and_prerelease_but_not_unparseable_versions() {
        let registry = AgentRegistry::builtin().unwrap();
        let opencode = descriptor(&registry, "opencode");
        let mut catalog = CompatibilityCatalog::builtin(&registry).unwrap();
        catalog
            .entries
            .iter_mut()
            .find(|entry| entry.agent_id == "opencode")
            .unwrap()
            .blocked
            .push(BlockedRule {
                version_requirement: "=1.18.9".to_string(),
                reason: "该版本存在已知配置破坏".to_string(),
            });
        validate_catalog(&catalog, &registry).unwrap();

        for version in ["1.18.9", "1.18.9-rc.1"] {
            let decision =
                evaluate_discovery(&catalog, opencode, &discovery("opencode", Some(version)));
            assert_eq!(decision.status, CompatibilityStatus::DetectedBlocked);
            assert_eq!(decision.reason_code, ReasonCode::BlockedVersionMatch);
            assert_eq!(decision.connector_id, None);
        }

        let decision = evaluate_discovery(&catalog, opencode, &discovery("opencode", None));
        assert_eq!(decision.status, CompatibilityStatus::DetectedVerified);
    }

    #[test]
    fn preflight_connector_installation_and_runtime_failures_still_block() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();
        let codex = descriptor(&registry, "codex");

        for reason_code in [
            ReasonCode::ReadOnlyPreflightFailed,
            ReasonCode::ConfigReadFailed,
            ReasonCode::ConfigParseFailed,
            ReasonCode::InvalidEnvironmentOverride,
        ] {
            let mut record = discovery("codex", None);
            record.diagnostics.push(Diagnostic {
                reason_code,
                message: "preflight failed".to_string(),
            });
            let decision = evaluate_discovery(&catalog, codex, &record);
            assert_eq!(decision.status, CompatibilityStatus::DetectedUnknown);
            assert_eq!(decision.reason_code, reason_code);
            assert_eq!(decision.connector_id, None);
        }

        let mut ambiguous = codex.clone();
        ambiguous.local_connector_ids.push("codex-v2".to_string());
        let decision = evaluate_discovery(&catalog, &ambiguous, &discovery("codex", None));
        assert_eq!(decision.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(decision.reason_code, ReasonCode::ConnectorBindingNotUnique);

        let mut multiple = discovery("codex", None);
        multiple.conflict_group = Some("codex".to_string());
        let decision = evaluate_discovery(&catalog, codex, &multiple);
        assert_eq!(decision.status, CompatibilityStatus::MultipleInstallations);

        let mut broken = discovery("codex", None);
        broken.runnable = false;
        broken.diagnostics.push(Diagnostic {
            reason_code: ReasonCode::ExecutableNotRunnable,
            message: "not runnable".to_string(),
        });
        let decision = evaluate_discovery(&catalog, codex, &broken);
        assert_eq!(decision.status, CompatibilityStatus::InstalledBroken);
    }

    #[test]
    fn catalog_rejects_unknown_fields_agents_invalid_and_duplicate_blocks() {
        let registry = AgentRegistry::builtin().unwrap();
        let forbidden =
            include_bytes!("../../tests/fixtures/compatibility/forbidden-router-field.json");
        assert!(parse_catalog(forbidden)
            .unwrap_err()
            .contains("router_config"));

        let mut value: serde_json::Value =
            serde_json::from_str(BUILTIN_COMPATIBILITY_JSON).unwrap();
        value["schema_version"] = json!(2);
        let catalog = parse_catalog(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(validate_catalog(&catalog, &registry).is_err());

        let mut unknown = CompatibilityCatalog::builtin(&registry).unwrap();
        unknown.entries[2].agent_id = "hermes-unknown".to_string();
        assert!(validate_catalog(&unknown, &registry)
            .unwrap_err()
            .contains("未知 Agent"));

        let mut duplicate = CompatibilityCatalog::builtin(&registry).unwrap();
        duplicate.entries[0].blocked = vec![
            BlockedRule {
                version_requirement: "=1.0.0".to_string(),
                reason: "first".to_string(),
            },
            BlockedRule {
                version_requirement: "=1.0.0".to_string(),
                reason: "second".to_string(),
            },
        ];
        assert!(validate_catalog(&duplicate, &registry)
            .unwrap_err()
            .contains("重复规则"));
    }
}
