use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::registry::AgentRegistry;
use super::safe_fs::atomic_replace;
use super::types::{
    AdmissionStatus, AgentDescriptor, AllowedAction, CompatibilityDecision, CompatibilityStatus,
    DiscoveryRecord, ReasonCode,
};

pub const BUILTIN_COMPATIBILITY_JSON: &str =
    include_str!("../../agent-registry/builtin-compatibility.json");

const CATALOG_SCHEMA_VERSION: u32 = 1;
const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_FILE: &str = "agent-compatibility-cache.json";
const MAX_CATALOG_BYTES: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_RULES_PER_ENTRY: usize = 64;
const MAX_REMOTE_LIFETIME_MS: u64 = 366 * 24 * 60 * 60 * 1_000;
const CLOCK_SKEW_MS: u64 = 5 * 60 * 1_000;
const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);
static CACHE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CACHE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Production builds leave both values empty until release engineering embeds
/// a reviewed HTTPS endpoint and Ed25519 public key. Empty means no request.
pub const PRODUCTION_CATALOG_URL: &str = "";
pub const PRODUCTION_CATALOG_PUBLIC_KEY: &str = "";

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
    pub verified: Vec<CompatibilityRule>,
    pub inferred: Vec<CompatibilityRule>,
    pub blocked: Vec<BlockedRule>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityRule {
    pub version_requirement: String,
    pub baseline_version: Option<String>,
    pub connector_id: Option<String>,
    pub config_fingerprint: Option<String>,
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
        validate_catalog(&catalog, registry, false, catalog.issued_at_ms)?;
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
    Remote,
}

pub struct CatalogSelection {
    pub catalog: CompatibilityCatalog,
    pub source: CatalogSource,
    pub warning: Option<String>,
}

pub struct SignedCatalogResponse {
    pub body: Vec<u8>,
    pub signature: String,
}

pub trait CatalogTransport {
    fn fetch(&self, endpoint: &str) -> Result<SignedCatalogResponse, String>;
}

pub struct UreqCatalogTransport;

impl CatalogTransport for UreqCatalogTransport {
    fn fetch(&self, endpoint: &str) -> Result<SignedCatalogResponse, String> {
        let http = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(REMOTE_TIMEOUT))
                .max_redirects(0)
                .http_status_as_error(false)
                .build(),
        );
        let response = http
            .get(endpoint)
            .header("accept", "application/json")
            .header("accept-encoding", "identity")
            .header("user-agent", "token-station-desktop/compatibility-catalog")
            .call()
            .map_err(|_| "兼容目录请求失败".to_string())?;
        if response.status().as_u16() != 200 {
            return Err(format!("兼容目录返回 HTTP {}", response.status().as_u16()));
        }
        if response
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
        {
            return Err("兼容目录不允许内容编码转换".to_string());
        }
        let mut signatures = response
            .headers()
            .get_all("x-token-station-compatibility-signature")
            .iter();
        let signature = signatures
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|value| is_lower_hex(value, 128))
            .ok_or_else(|| "兼容目录缺少有效签名头".to_string())?
            .to_string();
        if signatures.next().is_some() {
            return Err("兼容目录签名头必须恰好出现一次".to_string());
        }
        let mut body = Vec::new();
        response
            .into_body()
            .into_with_config()
            .limit(MAX_CATALOG_BYTES)
            .reader()
            .read_to_end(&mut body)
            .map_err(|_| "读取兼容目录失败".to_string())?;
        Ok(SignedCatalogResponse { body, signature })
    }
}

pub struct RemoteCatalogConfig<'a> {
    endpoint: &'a str,
    public_key_hex: &'a str,
    #[cfg(test)]
    allow_test_endpoint: bool,
}

#[must_use]
pub fn production_remote_config() -> RemoteCatalogConfig<'static> {
    RemoteCatalogConfig {
        endpoint: PRODUCTION_CATALOG_URL,
        public_key_hex: PRODUCTION_CATALOG_PUBLIC_KEY,
        #[cfg(test)]
        allow_test_endpoint: false,
    }
}

pub fn select_catalog<T: CatalogTransport>(
    registry: &AgentRegistry,
    cache_dir: &Path,
    config: &RemoteCatalogConfig<'_>,
    transport: &T,
    now_ms: u64,
) -> Result<CatalogSelection, String> {
    let builtin = CompatibilityCatalog::builtin(registry)?;
    if config.endpoint.is_empty() && config.public_key_hex.is_empty() {
        return Ok(CatalogSelection {
            catalog: builtin,
            source: CatalogSource::Builtin,
            warning: None,
        });
    }
    if config.endpoint.is_empty() || config.public_key_hex.is_empty() {
        return Ok(fallback(
            builtin,
            "兼容目录端点或公钥未完整配置，已使用内置目录",
        ));
    }
    validate_https_endpoint(config.endpoint)?;
    #[cfg(test)]
    if !config.allow_test_endpoint && config.endpoint != PRODUCTION_CATALOG_URL {
        return Ok(fallback(
            builtin,
            "兼容目录端点不是编译期固定生产地址，已使用内置目录",
        ));
    }
    #[cfg(not(test))]
    if config.endpoint != PRODUCTION_CATALOG_URL {
        return Ok(fallback(
            builtin,
            "兼容目录端点不是编译期固定生产地址，已使用内置目录",
        ));
    }
    if !is_lower_hex(config.public_key_hex, 64) {
        return Ok(fallback(builtin, "兼容目录公钥编码无效，已使用内置目录"));
    }
    let public_key = match token_station_release::parse_public_key(config.public_key_hex) {
        Ok(key) => key,
        Err(_) => {
            return Ok(fallback(builtin, "兼容目录公钥无效，已使用内置目录"));
        }
    };
    let cache = read_cache(cache_dir, registry, config.endpoint, config.public_key_hex);
    let accepted_sequence = cache.catalog().map_or(builtin.sequence, |catalog| {
        catalog.sequence.max(builtin.sequence)
    });
    let response = match transport.fetch(config.endpoint) {
        Ok(response) => response,
        Err(_) => {
            return Ok(restricted_fallback(
                builtin,
                &cache,
                "兼容目录网络不可用，已使用更保守的内置目录",
            ));
        }
    };
    if response.body.len() as u64 > MAX_CATALOG_BYTES {
        return Ok(restricted_fallback(
            builtin,
            &cache,
            "兼容目录响应过大，已使用内置目录",
        ));
    }
    if !is_lower_hex(&response.signature, 128) {
        return Ok(restricted_fallback(
            builtin,
            &cache,
            "兼容目录签名编码无效，已使用内置目录",
        ));
    }
    if token_station_release::verify_bytes(&public_key, &response.body, &response.signature)
        .is_err()
    {
        return Ok(restricted_fallback(
            builtin,
            &cache,
            "兼容目录签名验证失败，已使用内置目录",
        ));
    }
    let remote = match parse_catalog(&response.body)
        .and_then(|catalog| validate_catalog(&catalog, registry, true, now_ms).map(|()| catalog))
    {
        Ok(catalog) => catalog,
        Err(_) => {
            return Ok(restricted_fallback(
                builtin,
                &cache,
                "兼容目录格式、时效或引用无效，已使用内置目录",
            ));
        }
    };
    if remote.sequence < accepted_sequence {
        return Ok(restricted_fallback(
            builtin,
            &cache,
            "兼容目录 sequence 回退，已拒绝并使用内置目录",
        ));
    }
    if remote.sequence == accepted_sequence {
        if cache
            .raw_body()
            .is_some_and(|accepted| accepted == response.body)
        {
            return Ok(CatalogSelection {
                catalog: remote,
                source: CatalogSource::Remote,
                warning: None,
            });
        }
        return Ok(restricted_fallback(
            builtin,
            &cache,
            "兼容目录 sequence 未前进且内容不同，已拒绝目录冲突",
        ));
    }

    let cache_warning = write_cache(cache_dir, registry, config, &response, remote.sequence).err();
    Ok(CatalogSelection {
        catalog: remote,
        source: CatalogSource::Remote,
        warning: cache_warning,
    })
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
            "安装入口存在，但版本探测未成功".to_string(),
            None,
            &[],
        );
    }
    let Some(version) = discovery
        .version_normalized
        .as_deref()
        .and_then(|version| Version::parse(version).ok())
    else {
        return decision(
            CompatibilityStatus::DetectedUnknown,
            ReasonCode::VersionOutputUnparseable,
            "已检测到 Agent，但版本无法归一化".to_string(),
            None,
            &[],
        );
    };
    if descriptor.admission == AdmissionStatus::DiscoveryOnly {
        return decision(
            CompatibilityStatus::DetectedUnknown,
            ReasonCode::DiscoveryOnlyNotAdmitted,
            "该 Agent 当前仅支持只读发现，尚未准入配置接管".to_string(),
            None,
            &[],
        );
    }
    let experimental_connector_id = match descriptor.local_connector_ids.as_slice() {
        [connector_id] => Some(connector_id.clone()),
        _ => None,
    };
    // Default-allow (aligns with cc-switch): the catalog can DENY (blocklist) or
    // fast-path (verified badge), but its ABSENCE never blocks. The safety is the
    // pipeline — read-only preflight + snapshot + rollback + blocklist — not an
    // allowlist. So a runnable install we know how to write (a unique connector)
    // whose preflight passes is connectable whether or not it is catalogued.
    let entry = catalog.entry(&descriptor.agent_id);
    if let Some(entry) = entry {
        if let Some(blocked) = entry
            .blocked
            .iter()
            .find(|rule| blocked_requirement_matches(&rule.version_requirement, &version))
        {
            return decision(
                CompatibilityStatus::DetectedBlocked,
                ReasonCode::BlockedVersionMatch,
                blocked.reason.clone(),
                None,
                &[],
            );
        }
        if let Some(verified) = entry
            .verified
            .iter()
            .find(|rule| requirement_matches(&rule.version_requirement, &version))
        {
            return decision(
                CompatibilityStatus::DetectedVerified,
                ReasonCode::VerifiedRangeMatch,
                "版本命中已验证兼容范围".to_string(),
                verified.connector_id.clone(),
                &[
                    AllowedAction::RunReadOnlyPreflight,
                    AllowedAction::PreviewConnect,
                ],
            );
        }
    }
    // The real safety gate, catalogued or not: if the config cannot be read or
    // parsed, we do not write to it.
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
    if let Some(inferred) = entry.and_then(|entry| {
        entry
            .inferred
            .iter()
            .find(|rule| inferred_rule_matches(rule, &version))
    }) {
        if inferred.config_fingerprint == discovery.config_fingerprint {
            return decision(
                CompatibilityStatus::DetectedInferred,
                ReasonCode::PatchInferredFingerprintMatch,
                "补丁版本与配置结构指纹匹配，需先通过只读预检".to_string(),
                inferred.connector_id.clone(),
                &[AllowedAction::RunReadOnlyPreflight],
            );
        }
    }
    // Default-allow fallthrough: not blocked, preflight passed. Connectable when
    // we know a single way to write this agent's config; otherwise we honestly
    // cannot pick a writer.
    decision(
        CompatibilityStatus::DetectedUnknown,
        ReasonCode::NoCompatibilityEntry,
        if experimental_connector_id.is_some() {
            "默认放行：版本未在验证目录，但已通过只读预检；接入前会创建快照，失败可回滚"
        } else {
            "无法确定该 Agent 的配置写法（Connector 绑定不唯一）"
        }
        .to_string(),
        experimental_connector_id.clone(),
        if experimental_connector_id.is_some() {
            &[AllowedAction::RunReadOnlyPreflight]
        } else {
            &[]
        },
    )
}

fn parse_catalog(bytes: &[u8]) -> Result<CompatibilityCatalog, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("兼容目录 JSON 无效：{error}"))
}

fn validate_catalog(
    catalog: &CompatibilityCatalog,
    registry: &AgentRegistry,
    remote: bool,
    now_ms: u64,
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
    if remote {
        let expires = catalog
            .expires_at_ms
            .ok_or_else(|| "远程兼容目录必须过期".to_string())?;
        if expires <= now_ms
            || catalog.issued_at_ms > now_ms.saturating_add(CLOCK_SKEW_MS)
            || expires <= catalog.issued_at_ms
            || expires.saturating_sub(catalog.issued_at_ms) > MAX_REMOTE_LIFETIME_MS
        {
            return Err("远程兼容目录已过期或时效窗口无效".to_string());
        }
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
        let descriptor = descriptors
            .get(entry.agent_id.as_str())
            .ok_or_else(|| "兼容目录引用未知 Agent".to_string())?;
        if entry.verified.len() + entry.inferred.len() + entry.blocked.len() > MAX_RULES_PER_ENTRY {
            return Err("兼容目录规则过多".to_string());
        }
        for rule in &entry.verified {
            validate_rule(rule, descriptor, false)?;
        }
        for rule in &entry.inferred {
            validate_rule(rule, descriptor, true)?;
        }
        for rule in &entry.blocked {
            validate_requirement(&rule.version_requirement)?;
            validate_safe_text(&rule.reason)?;
        }
        let mut unique_rules = BTreeSet::new();
        for (kind, requirement) in entry
            .verified
            .iter()
            .map(|rule| ("verified", rule.version_requirement.as_str()))
            .chain(
                entry
                    .inferred
                    .iter()
                    .map(|rule| ("inferred", rule.version_requirement.as_str())),
            )
            .chain(
                entry
                    .blocked
                    .iter()
                    .map(|rule| ("blocked", rule.version_requirement.as_str())),
            )
        {
            if !unique_rules.insert((kind, requirement)) {
                return Err("兼容目录包含重复规则".to_string());
            }
        }
    }
    Ok(())
}

fn validate_rule(
    rule: &CompatibilityRule,
    descriptor: &AgentDescriptor,
    inferred: bool,
) -> Result<(), String> {
    validate_requirement(&rule.version_requirement)?;
    match descriptor.admission {
        AdmissionStatus::Supported => {
            let connector = rule
                .connector_id
                .as_deref()
                .ok_or_else(|| "正式准入规则缺少 Connector".to_string())?;
            if !descriptor
                .local_connector_ids
                .iter()
                .any(|known| known == connector)
            {
                return Err("兼容目录引用未知 Connector".to_string());
            }
        }
        AdmissionStatus::DiscoveryOnly => {
            return Err("只读发现 Agent 不能被 verified/inferred 规则准入".to_string());
        }
    }
    if inferred {
        let baseline = rule
            .baseline_version
            .as_deref()
            .and_then(|version| Version::parse(version).ok())
            .filter(|version| version.pre.is_empty())
            .ok_or_else(|| "推定规则缺少稳定 SemVer baseline_version".to_string())?;
        if !requirement_matches(&rule.version_requirement, &baseline) {
            return Err("推定规则必须包含 baseline_version".to_string());
        }
        let fingerprint = rule
            .config_fingerprint
            .as_deref()
            .ok_or_else(|| "推定规则缺少配置指纹".to_string())?;
        if fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err("推定规则配置指纹不是小写 SHA-256".to_string());
        }
    } else if rule.config_fingerprint.is_some() || rule.baseline_version.is_some() {
        return Err("已验证规则不得携带配置指纹或推定基线".to_string());
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

fn requirement_matches(requirement: &str, version: &Version) -> bool {
    VersionReq::parse(requirement).is_ok_and(|requirement| requirement.matches(version))
}

/// Blocking is intentionally more conservative than normal SemVer admission:
/// a prerelease inherits every block applied to its `major.minor.patch` core.
fn blocked_requirement_matches(requirement: &str, version: &Version) -> bool {
    let core_version = Version::new(version.major, version.minor, version.patch);
    requirement_matches(requirement, &core_version)
}

fn inferred_rule_matches(rule: &CompatibilityRule, version: &Version) -> bool {
    let Some(baseline) = rule
        .baseline_version
        .as_deref()
        .and_then(|value| Version::parse(value).ok())
    else {
        return false;
    };
    version.pre.is_empty()
        && version.major == baseline.major
        && version.minor == baseline.minor
        && requirement_matches(&rule.version_requirement, version)
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

fn validate_https_endpoint(endpoint: &str) -> Result<(), String> {
    let rest = endpoint
        .strip_prefix("https://")
        .ok_or_else(|| "兼容目录端点必须使用 HTTPS".to_string())?;
    let authority = rest.split('/').next().unwrap_or_default();
    if endpoint.len() > 512
        || authority.is_empty()
        || authority.contains(['@', '?', '#'])
        || endpoint.contains(['?', '#'])
    {
        return Err("兼容目录端点不是固定 HTTPS URL".to_string());
    }
    Ok(())
}

fn fallback(catalog: CompatibilityCatalog, warning: &str) -> CatalogSelection {
    CatalogSelection {
        catalog,
        source: CatalogSource::Builtin,
        warning: Some(warning.to_string()),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedCatalogCache {
    schema_version: u32,
    source_url: String,
    public_key_fingerprint: String,
    signature: String,
    catalog_utf8: String,
}

enum CacheState {
    Missing,
    Valid {
        catalog: CompatibilityCatalog,
        raw_body: Vec<u8>,
    },
    Invalid,
}

impl CacheState {
    fn catalog(&self) -> Option<&CompatibilityCatalog> {
        match self {
            Self::Valid { catalog, .. } => Some(catalog),
            Self::Missing | Self::Invalid => None,
        }
    }

    fn raw_body(&self) -> Option<&[u8]> {
        match self {
            Self::Valid { raw_body, .. } => Some(raw_body),
            Self::Missing | Self::Invalid => None,
        }
    }
}

fn cache_path(cache_dir: &Path) -> std::path::PathBuf {
    cache_dir.join(CACHE_FILE)
}

fn read_cache(
    cache_dir: &Path,
    registry: &AgentRegistry,
    source_url: &str,
    public_key_hex: &str,
) -> CacheState {
    let path = cache_path(cache_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return CacheState::Missing,
        Err(_) => return CacheState::Invalid,
    };
    if bytes.len() as u64 > MAX_CATALOG_BYTES * 2 {
        return CacheState::Invalid;
    }
    let Ok(cache) = serde_json::from_slice::<SignedCatalogCache>(&bytes) else {
        return CacheState::Invalid;
    };
    if cache.schema_version != CACHE_SCHEMA_VERSION
        || cache.source_url != source_url
        || cache.public_key_fingerprint != public_key_fingerprint(public_key_hex)
    {
        return CacheState::Invalid;
    }
    let Ok(public_key) = token_station_release::parse_public_key(public_key_hex) else {
        return CacheState::Invalid;
    };
    if token_station_release::verify_bytes(
        &public_key,
        cache.catalog_utf8.as_bytes(),
        &cache.signature,
    )
    .is_err()
    {
        return CacheState::Invalid;
    }
    let Ok(catalog) = parse_catalog(cache.catalog_utf8.as_bytes()) else {
        return CacheState::Invalid;
    };
    if validate_catalog(&catalog, registry, false, catalog.issued_at_ms).is_err() {
        return CacheState::Invalid;
    }
    CacheState::Valid {
        catalog,
        raw_body: cache.catalog_utf8.into_bytes(),
    }
}

fn write_cache(
    cache_dir: &Path,
    registry: &AgentRegistry,
    config: &RemoteCatalogConfig<'_>,
    response: &SignedCatalogResponse,
    remote_sequence: u64,
) -> Result<(), String> {
    let _guard = CACHE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "兼容目录缓存写锁已损坏".to_string())?;
    if read_cache(cache_dir, registry, config.endpoint, config.public_key_hex)
        .catalog()
        .is_some_and(|catalog| catalog.sequence >= remote_sequence)
    {
        return Err("兼容目录缓存已有相同或更高 sequence，未覆盖".to_string());
    }
    let catalog_utf8 = std::str::from_utf8(&response.body)
        .map_err(|_| "兼容目录不是 UTF-8，未缓存".to_string())?;
    let cache = SignedCatalogCache {
        schema_version: CACHE_SCHEMA_VERSION,
        source_url: config.endpoint.to_string(),
        public_key_fingerprint: public_key_fingerprint(config.public_key_hex),
        signature: response.signature.clone(),
        catalog_utf8: catalog_utf8.to_string(),
    };
    let mut rendered =
        serde_json::to_vec(&cache).map_err(|_| "序列化兼容目录缓存失败".to_string())?;
    rendered.push(b'\n');
    std::fs::create_dir_all(cache_dir).map_err(|_| "创建兼容目录缓存目录失败".to_string())?;
    let sequence = CACHE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = cache_dir.join(format!(
        ".{CACHE_FILE}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "创建兼容目录临时缓存失败".to_string())?;
    if file
        .write_all(&rendered)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        drop(file);
        std::fs::remove_file(&temporary).ok();
        return Err("持久化兼容目录临时缓存失败".to_string());
    }
    drop(file);
    if atomic_replace(&temporary, &cache_path(cache_dir)).is_err() {
        std::fs::remove_file(&temporary).ok();
        return Err("原子替换兼容目录缓存失败".to_string());
    }
    #[cfg(unix)]
    if let Ok(directory) = std::fs::File::open(cache_dir) {
        if directory.sync_all().is_err() {
            return Err("兼容目录缓存已替换，但目录持久化未确认".to_string());
        }
    }
    Ok(())
}

fn restricted_fallback(
    mut builtin: CompatibilityCatalog,
    cache: &CacheState,
    warning: &str,
) -> CatalogSelection {
    match cache {
        CacheState::Missing => {}
        CacheState::Invalid => {
            for entry in &mut builtin.entries {
                entry.verified.clear();
                entry.inferred.clear();
            }
            builtin.catalog_version = format!("{}-cache-invalid", builtin.catalog_version);
        }
        CacheState::Valid { catalog, .. } => {
            for cached_entry in &catalog.entries {
                let target = if let Some(entry) = builtin
                    .entries
                    .iter_mut()
                    .find(|entry| entry.agent_id == cached_entry.agent_id)
                {
                    entry
                } else {
                    builtin.entries.push(AgentCompatibilityEntry {
                        agent_id: cached_entry.agent_id.clone(),
                        verified: Vec::new(),
                        inferred: Vec::new(),
                        blocked: Vec::new(),
                    });
                    builtin.entries.last_mut().expect("just pushed")
                };
                for blocked in &cached_entry.blocked {
                    if !target.blocked.contains(blocked) {
                        target.blocked.push(blocked.clone());
                    }
                }
            }
            builtin
                .entries
                .sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
            builtin.sequence = builtin.sequence.max(catalog.sequence);
            builtin.catalog_version =
                format!("{}-offline-{}", builtin.catalog_version, catalog.sequence);
        }
    }
    fallback(builtin, warning)
}

fn public_key_fingerprint(public_key_hex: &str) -> String {
    format!("{:x}", Sha256::digest(public_key_hex.as_bytes()))
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::agent_integration::types::{
        Diagnostic, DiscoveryEvidence, DiscoverySource, Platform,
    };

    struct FakeTransport {
        calls: AtomicUsize,
        response: MutexResponse,
    }

    enum MutexResponse {
        Failure,
        Success { body: Vec<u8>, signature: String },
    }

    impl CatalogTransport for FakeTransport {
        fn fetch(&self, _endpoint: &str) -> Result<SignedCatalogResponse, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.response {
                MutexResponse::Failure => Err("offline".to_string()),
                MutexResponse::Success { body, signature } => Ok(SignedCatalogResponse {
                    body: body.clone(),
                    signature: signature.clone(),
                }),
            }
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "token-station-compatibility-{name}-{}-{}",
            std::process::id(),
            nonce
        ))
    }

    fn discovery(agent_id: &str, version: &str, fingerprint: Option<&str>) -> DiscoveryRecord {
        DiscoveryRecord {
            agent_id: agent_id.to_string(),
            executable_path: format!("/tmp/{agent_id}"),
            canonical_path: format!("/tmp/{agent_id}"),
            version_raw: Some(version.to_string()),
            version_normalized: Some(version.to_string()),
            environment: Platform::Macos,
            evidence: vec![DiscoveryEvidence {
                source: DiscoverySource::Path,
                observed_path: format!("/tmp/{agent_id}"),
                is_path_default: true,
            }],
            is_path_default: true,
            runnable: true,
            config_candidates: Vec::new(),
            config_fingerprint: fingerprint.map(str::to_string),
            conflict_group: None,
            diagnostics: Vec::<Diagnostic>::new(),
            scanned_at_ms: 0,
        }
    }

    fn signed_response(
        catalog: &CompatibilityCatalog,
        sign: impl FnOnce(&[u8]) -> String,
    ) -> MutexResponse {
        let body = serde_json::to_vec(catalog).unwrap();
        let signature = sign(&body);
        MutexResponse::Success { body, signature }
    }

    fn test_remote_config(public_key_hex: &str) -> RemoteCatalogConfig<'_> {
        RemoteCatalogConfig {
            endpoint: "https://catalog.example.invalid/agent-compatibility.json",
            public_key_hex,
            allow_test_endpoint: true,
        }
    }

    #[test]
    fn compatibility_builtin_verified_ranges_cover_the_patch_line_and_others_are_experimental() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();

        // The reported case (P1): a Claude Code whose patch differs from the one
        // the catalog was cut against must still be one-click verified, not
        // downgraded to experimental. The verified range is the 2.1 patch line.
        let claude = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();
        let local = evaluate_discovery(&catalog, claude, &discovery("claude-code", "2.1.173", None));
        assert_eq!(local.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(local.connector_id.as_deref(), Some("claude-code-v1"));
        assert!(local.allowed_actions.contains(&AllowedAction::PreviewConnect));
        let next_minor =
            evaluate_discovery(&catalog, claude, &discovery("claude-code", "2.2.0", None));
        assert_eq!(next_minor.status, CompatibilityStatus::DetectedUnknown);

        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "codex")
            .unwrap();

        let verified = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("codex", "0.145.0-alpha.18", None),
        );
        let newer = evaluate_discovery(&catalog, descriptor, &discovery("codex", "0.146.0", None));
        let older = evaluate_discovery(&catalog, descriptor, &discovery("codex", "0.144.0", None));

        assert_eq!(verified.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(verified.connector_id.as_deref(), Some("codex-v1"));
        assert!(verified
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));
        for unknown in [&newer, &older] {
            assert_eq!(unknown.status, CompatibilityStatus::DetectedUnknown);
            assert_eq!(unknown.connector_id.as_deref(), Some("codex-v1"));
            assert!(unknown
                .allowed_actions
                .contains(&AllowedAction::RunReadOnlyPreflight));
            assert!(!unknown
                .allowed_actions
                .contains(&AllowedAction::PreviewConnect));
        }

        assert!(!newer
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));

        let openclaw = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "openclaw")
            .unwrap();
        let verified = evaluate_discovery(
            &catalog,
            openclaw,
            &discovery("openclaw", "2026.6.11", None),
        );
        let future =
            evaluate_discovery(&catalog, openclaw, &discovery("openclaw", "2026.7.1", None));
        assert_eq!(verified.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(verified.connector_id.as_deref(), Some("openclaw-v1"));
        assert_eq!(future.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(future.connector_id.as_deref(), Some("openclaw-v1"));
        assert!(future
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));
        assert!(!future
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));

        let hermes = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "nous-hermes-agent")
            .unwrap();
        let verified = evaluate_discovery(
            &catalog,
            hermes,
            &discovery("nous-hermes-agent", "0.18.0", None),
        );
        // A patch within the verified line is verified too (this is the widening).
        let patch = evaluate_discovery(
            &catalog,
            hermes,
            &discovery("nous-hermes-agent", "0.18.2", None),
        );
        let future = evaluate_discovery(
            &catalog,
            hermes,
            &discovery("nous-hermes-agent", "0.19.0", None),
        );
        assert_eq!(verified.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(verified.connector_id.as_deref(), Some("hermes-v1"));
        assert_eq!(patch.status, CompatibilityStatus::DetectedVerified);
        assert_eq!(future.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(future.connector_id.as_deref(), Some("hermes-v1"));
        assert!(future
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));
        assert!(!future
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));
    }

    #[test]
    fn compatibility_default_allow_still_needs_parseable_version_unique_connector_and_preflight() {
        let registry = AgentRegistry::builtin().unwrap();
        let catalog = CompatibilityCatalog::builtin(&registry).unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "codex")
            .unwrap();

        let mut unparseable = discovery("codex", "not-semver", None);
        unparseable.version_normalized = None;
        let unparseable = evaluate_discovery(&catalog, descriptor, &unparseable);
        assert_eq!(
            unparseable.reason_code,
            ReasonCode::VersionOutputUnparseable
        );
        assert_eq!(unparseable.connector_id, None);
        assert!(!unparseable
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));

        // Default-allow: an agent with NO catalog entry still connects when it
        // has a unique connector and preflight passes — absence never blocks.
        let mut missing_entry = catalog.clone();
        missing_entry
            .entries
            .retain(|entry| entry.agent_id != "codex");
        let missing_entry = evaluate_discovery(
            &missing_entry,
            descriptor,
            &discovery("codex", "0.146.0", None),
        );
        assert_eq!(missing_entry.reason_code, ReasonCode::NoCompatibilityEntry);
        assert_eq!(missing_entry.connector_id.as_deref(), Some("codex-v1"));
        assert!(missing_entry
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));

        let mut failed_preflight = discovery("codex", "0.146.0", None);
        failed_preflight.diagnostics.push(Diagnostic {
            reason_code: ReasonCode::ConfigParseFailed,
            message: "invalid config".to_string(),
        });
        let failed_preflight = evaluate_discovery(&catalog, descriptor, &failed_preflight);
        assert_eq!(failed_preflight.reason_code, ReasonCode::ConfigParseFailed);
        assert_eq!(failed_preflight.connector_id, None);
        assert!(!failed_preflight
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));

        let mut ambiguous = descriptor.clone();
        ambiguous.local_connector_ids.push("codex-v2".to_string());
        let ambiguous =
            evaluate_discovery(&catalog, &ambiguous, &discovery("codex", "0.146.0", None));
        assert_eq!(ambiguous.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(ambiguous.connector_id, None);
        assert!(!ambiguous
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));
    }

    #[test]
    fn compatibility_blocked_wins_and_inferred_requires_the_exact_fingerprint() {
        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "opencode")
            .unwrap();
        let fingerprint = "a".repeat(64);
        let catalog = CompatibilityCatalog {
            schema_version: 1,
            catalog_version: "fixture-2".to_string(),
            sequence: 2,
            issued_at_ms: 10,
            expires_at_ms: None,
            minimum_app_version: "0.1.0".to_string(),
            entries: vec![AgentCompatibilityEntry {
                agent_id: "opencode".to_string(),
                verified: vec![],
                inferred: vec![CompatibilityRule {
                    version_requirement: ">=1.18.3, <1.19.0".to_string(),
                    baseline_version: Some("1.18.3".to_string()),
                    connector_id: Some("opencode-v1".to_string()),
                    config_fingerprint: Some(fingerprint.clone()),
                }],
                blocked: vec![BlockedRule {
                    version_requirement: "=1.18.9".to_string(),
                    reason: "该版本存在已知配置破坏".to_string(),
                }],
            }],
        };
        validate_catalog(&catalog, &registry, false, 10).unwrap();

        let inferred = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.18.4", Some(&fingerprint)),
        );
        let changed = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.18.4", Some(&"b".repeat(64))),
        );
        let blocked = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.18.9", Some(&fingerprint)),
        );
        let blocked_prerelease = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.18.9-rc.1", Some(&fingerprint)),
        );
        let next_minor = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.19.0", Some(&fingerprint)),
        );
        let prerelease = evaluate_discovery(
            &catalog,
            descriptor,
            &discovery("opencode", "1.18.5-alpha.1", Some(&fingerprint)),
        );

        assert_eq!(inferred.status, CompatibilityStatus::DetectedInferred);
        assert_eq!(
            inferred.allowed_actions,
            BTreeSet::from([
                AllowedAction::ViewDetails,
                AllowedAction::Rescan,
                AllowedAction::RunReadOnlyPreflight,
                AllowedAction::ExportDiagnostics,
            ])
        );
        // Default-allow: a changed fingerprint no longer blocks — it falls
        // through to the connectable default path (preflight passed), rather
        // than the old ConfigFingerprintChanged refusal. The fingerprint match
        // above still earns the DetectedInferred badge; the mismatch is simply
        // "not catalogued", still connectable via the unique connector.
        assert_eq!(changed.reason_code, ReasonCode::NoCompatibilityEntry);
        assert_eq!(changed.connector_id.as_deref(), Some("opencode-v1"));
        assert!(changed
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));
        assert_eq!(blocked.status, CompatibilityStatus::DetectedBlocked);
        assert_eq!(
            blocked_prerelease.status,
            CompatibilityStatus::DetectedBlocked
        );
        assert_eq!(
            blocked.allowed_actions,
            BTreeSet::from([
                AllowedAction::ViewDetails,
                AllowedAction::Rescan,
                AllowedAction::ExportDiagnostics,
            ])
        );
        assert_eq!(next_minor.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(next_minor.connector_id.as_deref(), Some("opencode-v1"));
        assert!(next_minor
            .allowed_actions
            .contains(&AllowedAction::RunReadOnlyPreflight));
        assert_eq!(prerelease.status, CompatibilityStatus::DetectedUnknown);
        assert_eq!(prerelease.connector_id.as_deref(), Some("opencode-v1"));
    }

    #[test]
    fn compatibility_catalog_rejects_unknown_schema_fields_agents_and_connectors() {
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
        assert!(validate_catalog(&catalog, &registry, false, 0).is_err());

        let mut connector: serde_json::Value =
            serde_json::from_str(BUILTIN_COMPATIBILITY_JSON).unwrap();
        connector["entries"][0]["verified"][0]["connector_id"] = json!("router-core-v1");
        let catalog = parse_catalog(&serde_json::to_vec(&connector).unwrap()).unwrap();
        assert!(validate_catalog(&catalog, &registry, false, 0)
            .unwrap_err()
            .contains("Connector"));

        let mut agent: serde_json::Value =
            serde_json::from_str(BUILTIN_COMPATIBILITY_JSON).unwrap();
        // Keep the catalog sorted so this assertion reaches the unknown-Agent
        // validation instead of failing earlier on ordering.
        agent["entries"][2]["agent_id"] = json!("hermes-unknown");
        let catalog = parse_catalog(&serde_json::to_vec(&agent).unwrap()).unwrap();
        assert!(validate_catalog(&catalog, &registry, false, 0)
            .unwrap_err()
            .contains("未知 Agent"));

        let mut command: serde_json::Value =
            serde_json::from_str(BUILTIN_COMPATIBILITY_JSON).unwrap();
        command["entries"][0]["verified"][0]["install_command"] = json!("curl | sh");
        assert!(parse_catalog(&serde_json::to_vec(&command).unwrap())
            .unwrap_err()
            .contains("install_command"));

        let mut discovery_only = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "nous-hermes-agent")
            .unwrap()
            .clone();
        discovery_only.admission = AdmissionStatus::DiscoveryOnly;
        discovery_only.protocol_binding = None;
        discovery_only.local_connector_ids.clear();
        let rule = CompatibilityRule {
            version_requirement: "=0.1.0".to_string(),
            baseline_version: None,
            connector_id: Some("opencode-v1".to_string()),
            config_fingerprint: None,
        };
        assert!(validate_rule(&rule, &discovery_only, false)
            .unwrap_err()
            .contains("只读发现"));
    }

    #[test]
    fn compatibility_empty_production_config_never_calls_the_network() {
        let registry = AgentRegistry::builtin().unwrap();
        let root = scratch("empty-config");
        let transport = FakeTransport {
            calls: AtomicUsize::new(0),
            response: MutexResponse::Failure,
        };

        let selected = select_catalog(
            &registry,
            &root,
            &production_remote_config(),
            &transport,
            100,
        )
        .unwrap();

        assert_eq!(selected.source, CatalogSource::Builtin);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert!(!root.exists());
    }

    #[test]
    fn compatibility_signature_tamper_expiry_and_network_failure_fall_back_without_cache_write() {
        let registry = AgentRegistry::builtin().unwrap();
        let key = token_station_release::keygen().unwrap();
        let public = token_station_release::hex(key.verifying_key().as_bytes());
        let config = test_remote_config(&public);
        let base = CompatibilityCatalog::builtin(&registry).unwrap();

        let cases = [
            FakeTransport {
                calls: AtomicUsize::new(0),
                response: MutexResponse::Failure,
            },
            FakeTransport {
                calls: AtomicUsize::new(0),
                response: MutexResponse::Success {
                    body: serde_json::to_vec(&CompatibilityCatalog {
                        sequence: 2,
                        expires_at_ms: Some(2_000),
                        ..base.clone()
                    })
                    .unwrap(),
                    signature: "00".repeat(64),
                },
            },
            FakeTransport {
                calls: AtomicUsize::new(0),
                response: signed_response(
                    &CompatibilityCatalog {
                        sequence: 2,
                        issued_at_ms: 100,
                        expires_at_ms: Some(200),
                        ..base.clone()
                    },
                    |body| token_station_release::sign_bytes(&key, body),
                ),
            },
        ];
        for (index, transport) in cases.into_iter().enumerate() {
            let root = scratch(&format!("fallback-{index}"));
            let selected = select_catalog(&registry, &root, &config, &transport, 1_000).unwrap();
            assert_eq!(selected.source, CatalogSource::Builtin);
            assert!(selected.warning.is_some());
            assert!(!root.exists());
        }
    }

    #[test]
    fn compatibility_valid_signed_catalog_is_cached_idempotently_and_rollback_is_rejected() {
        let registry = AgentRegistry::builtin().unwrap();
        let root = scratch("sequence");
        let key = token_station_release::keygen().unwrap();
        let public = token_station_release::hex(key.verifying_key().as_bytes());
        let config = test_remote_config(&public);
        let mut remote = CompatibilityCatalog::builtin(&registry).unwrap();
        remote.sequence = 2;
        remote.issued_at_ms = 1_000;
        remote.expires_at_ms = Some(2_000);
        let first = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&remote, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };

        let selected = select_catalog(&registry, &root, &config, &first, 1_500).unwrap();
        assert_eq!(selected.source, CatalogSource::Remote);
        assert_eq!(selected.catalog.sequence, 2);
        assert!(cache_path(&root).exists());
        let cached_once = std::fs::read(cache_path(&root)).unwrap();

        let identical = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&remote, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        let selected = select_catalog(&registry, &root, &config, &identical, 1_500).unwrap();
        assert_eq!(selected.source, CatalogSource::Remote);
        assert_eq!(std::fs::read(cache_path(&root)).unwrap(), cached_once);

        let mut equivocation = remote.clone();
        equivocation.catalog_version = "remote-same-sequence-different".to_string();
        let equivocation = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&equivocation, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        let selected = select_catalog(&registry, &root, &config, &equivocation, 1_500).unwrap();
        assert_eq!(selected.source, CatalogSource::Builtin);
        assert!(selected
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("内容不同")));
        assert_eq!(std::fs::read(cache_path(&root)).unwrap(), cached_once);

        let mut older = remote.clone();
        older.sequence = 1;
        let rollback = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&older, |body| token_station_release::sign_bytes(&key, body)),
        };
        let selected = select_catalog(&registry, &root, &config, &rollback, 1_500).unwrap();
        assert_eq!(selected.source, CatalogSource::Builtin);
        assert!(
            selected
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("回退")),
            "unexpected rollback warning: {:?}",
            selected.warning
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(cache_path(&root))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compatibility_offline_cache_preserves_blocks_but_revokes_remote_allows() {
        let registry = AgentRegistry::builtin().unwrap();
        let root = scratch("offline-restriction");
        let key = token_station_release::keygen().unwrap();
        let public = token_station_release::hex(key.verifying_key().as_bytes());
        let config = test_remote_config(&public);
        let mut remote = CompatibilityCatalog::builtin(&registry).unwrap();
        remote.sequence = 2;
        remote.issued_at_ms = 1_000;
        remote.expires_at_ms = Some(2_000);
        let claude = remote
            .entries
            .iter_mut()
            .find(|entry| entry.agent_id == "claude-code")
            .unwrap();
        claude.blocked.push(BlockedRule {
            version_requirement: "=2.1.211".to_string(),
            reason: "远端已知安全问题".to_string(),
        });
        let opencode = remote
            .entries
            .iter_mut()
            .find(|entry| entry.agent_id == "opencode")
            .unwrap();
        // A version outside the builtin's own verified range, so revoking this
        // remote allow offline is observable (the builtin does not re-verify it).
        opencode.verified.push(CompatibilityRule {
            version_requirement: "=1.19.5".to_string(),
            baseline_version: None,
            connector_id: Some("opencode-v1".to_string()),
            config_fingerprint: None,
        });
        let online = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&remote, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        assert_eq!(
            select_catalog(&registry, &root, &config, &online, 1_500)
                .unwrap()
                .source,
            CatalogSource::Remote
        );

        let offline = FakeTransport {
            calls: AtomicUsize::new(0),
            response: MutexResponse::Failure,
        };
        let selected = select_catalog(&registry, &root, &config, &offline, 3_000).unwrap();
        let claude_descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();
        let opencode_descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "opencode")
            .unwrap();
        assert_eq!(
            evaluate_discovery(
                &selected.catalog,
                claude_descriptor,
                &discovery("claude-code", "2.1.211", None),
            )
            .status,
            CompatibilityStatus::DetectedBlocked
        );
        assert_eq!(
            evaluate_discovery(
                &selected.catalog,
                opencode_descriptor,
                &discovery("opencode", "1.19.5", None),
            )
            .status,
            CompatibilityStatus::DetectedUnknown
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compatibility_corrupt_cache_fails_closed_for_all_write_admission() {
        let registry = AgentRegistry::builtin().unwrap();
        let root = scratch("corrupt-cache");
        let key = token_station_release::keygen().unwrap();
        let public = token_station_release::hex(key.verifying_key().as_bytes());
        let config = test_remote_config(&public);
        let mut remote = CompatibilityCatalog::builtin(&registry).unwrap();
        remote.sequence = 2;
        remote.issued_at_ms = 1_000;
        remote.expires_at_ms = Some(2_000);
        let online = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&remote, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        select_catalog(&registry, &root, &config, &online, 1_500).unwrap();
        std::fs::write(cache_path(&root), b"corrupt").unwrap();

        let offline = FakeTransport {
            calls: AtomicUsize::new(0),
            response: MutexResponse::Failure,
        };
        let selected = select_catalog(&registry, &root, &config, &offline, 1_600).unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();
        let decision = evaluate_discovery(
            &selected.catalog,
            descriptor,
            &discovery("claude-code", "2.1.211", None),
        );
        assert_eq!(decision.status, CompatibilityStatus::DetectedUnknown);
        assert!(!decision
            .allowed_actions
            .contains(&AllowedAction::PreviewConnect));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compatibility_release_drill_blocks_then_withdraws_at_a_higher_sequence() {
        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();
        let root = scratch("release-drill");
        let key = token_station_release::keygen().unwrap();
        let public = token_station_release::hex(key.verifying_key().as_bytes());
        let config = test_remote_config(&public);
        let mut blocked = CompatibilityCatalog::builtin(&registry).unwrap();
        blocked.sequence = 2;
        blocked.catalog_version = "release-drill-block-2".to_string();
        blocked.issued_at_ms = 1_000;
        blocked.expires_at_ms = Some(10_000);
        blocked
            .entries
            .iter_mut()
            .find(|entry| entry.agent_id == "claude-code")
            .unwrap()
            .blocked
            .push(BlockedRule {
                version_requirement: "=2.1.211".to_string(),
                reason: "发布演练紧急阻断".to_string(),
            });
        let block_response = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&blocked, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        let selected = select_catalog(&registry, &root, &config, &block_response, 2_000).unwrap();
        assert_eq!(selected.source, CatalogSource::Remote);
        assert_eq!(
            evaluate_discovery(
                &selected.catalog,
                descriptor,
                &discovery("claude-code", "2.1.211", None),
            )
            .status,
            CompatibilityStatus::DetectedBlocked
        );

        let mut withdrawn = CompatibilityCatalog::builtin(&registry).unwrap();
        withdrawn.sequence = 3;
        withdrawn.catalog_version = "release-drill-withdraw-3".to_string();
        withdrawn.issued_at_ms = 2_000;
        withdrawn.expires_at_ms = Some(10_000);
        let withdraw_response = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&withdrawn, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        let selected =
            select_catalog(&registry, &root, &config, &withdraw_response, 3_000).unwrap();
        assert_eq!(selected.catalog.sequence, 3);
        assert_eq!(
            evaluate_discovery(
                &selected.catalog,
                descriptor,
                &discovery("claude-code", "2.1.211", None),
            )
            .status,
            CompatibilityStatus::DetectedVerified
        );

        let rollback = FakeTransport {
            calls: AtomicUsize::new(0),
            response: signed_response(&blocked, |body| {
                token_station_release::sign_bytes(&key, body)
            }),
        };
        let selected = select_catalog(&registry, &root, &config, &rollback, 3_500).unwrap();
        assert_eq!(selected.source, CatalogSource::Builtin);
        assert!(selected.catalog.sequence >= 3);
        assert_eq!(
            evaluate_discovery(
                &selected.catalog,
                descriptor,
                &discovery("claude-code", "2.1.211", None),
            )
            .status,
            CompatibilityStatus::DetectedVerified
        );
        std::fs::remove_dir_all(root).ok();
    }
}
