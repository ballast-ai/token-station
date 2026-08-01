use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Which side of the host an adapter sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterKind {
    #[serde(rename = "agent-adapter")]
    Agent,
    #[serde(rename = "provider-adapter")]
    Provider,
}

impl AdapterKind {
    /// The `world` in `wit/adapter.wit` this kind must be built against.
    #[must_use]
    pub const fn expected_api_version(self) -> &'static str {
        match self {
            Self::Agent => "agent-adapter-v1",
            Self::Provider => "provider-adapter-v1",
        }
    }

    /// The conformance suite that gates installation of this kind.
    #[must_use]
    pub const fn expected_conformance_suite(self) -> &'static str {
        match self {
            Self::Agent => "agent-protocol-v1",
            Self::Provider => "provider-protocol-v1",
        }
    }
}

/// Which part of the `v1` ABI surface an adapter implements.
///
/// Not a taxonomy of what models can do — that is `protocol::ModelCapability`,
/// which is open and carries an `extensions` bag. This is the narrower claim
/// "the host may call these functions on me, and I honour these IR fields":
/// `chat`, `stream` and `agent_hint` name functions in the WIT world; `tool_call`
/// and `json_schema` name `ChatRequest` fields the adapter promises to translate.
///
/// So the set is closed by construction, not by policy. A third party cannot
/// implement a capability `v1` does not define, because `v1` defines no function
/// or field to carry it. The set grows when the world does, in `-v2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Stream,
    ToolCall,
    JsonSchema,
    /// Implements `extract-agent-hint`. Meaningless for a provider adapter,
    /// which never sees an inbound request.
    AgentHint,
}

/// Identity an adapter declares, and re-reports at run time via `metadata()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterMetadata {
    pub name: String,
    pub version: String,
    pub kind: AdapterKind,
    pub api_version: String,
}

impl AdapterMetadata {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        kind: AdapterKind,
        api_version: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            kind,
            api_version: api_version.into(),
        }
    }
}

/// What the sandbox must grant.
///
/// `network` and `filesystem` exist so a manifest can *ask*, and be refused with
/// a named reason. Silently ignoring the request would let a plugin author
/// believe they had been granted something.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterPermissions {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub filesystem: bool,
    /// Names of credentials, never credentials. Each must look like a reference
    /// name, which is what stops a key being pasted here.
    #[serde(default)]
    pub secrets: Vec<String>,
}

impl AdapterPermissions {
    #[must_use]
    pub fn new<I, S>(network: bool, filesystem: bool, secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            network,
            filesystem,
            secrets: secrets.into_iter().map(Into::into).collect(),
        }
    }
}

/// Where the adapter's conformance fixtures live, and which suite gates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceSpec {
    pub required_suite: String,
    /// Directory inside the plugin package, e.g. `fixtures/`.
    pub fixtures: String,
}

/// A plugin package's `manifest.json`.
///
/// Untrusted input from a third party, so it parses permissively and
/// [`AdapterManifest::validate`] rejects with an enumerable reason. That is the
/// opposite discipline to `token-station-protocol`, where illegal states are made
/// unrepresentable — deliberately: a registry has to record *why* it turned a
/// package away, and "deserialization failed" is not a reason it can act on.
///
/// Unknown fields are refused, and `permissions` is mandatory. Both matter more
/// than they look. With `serde(flatten)` or a defaulted `permissions`, a manifest
/// misspelling the key would deserialize into an all-false default and validate
/// clean, and the registry would then record a plugin that asked for the network
/// as one that did not. The sandbox would still deny it at run time, but every
/// later decision keyed on that record — canary, rollback, audit — would be
/// reasoning about a package that does not exist. Refusing unknown fields also
/// forces a new manifest field through a version bump, matching the ABI rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterManifest {
    pub name: String,
    pub version: String,
    pub kind: AdapterKind,
    pub api_version: String,
    /// Agent adapters only, e.g. `openai-chat-completions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_protocols: Vec<String>,
    /// Agent adapters only, e.g. `cursor`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tools: Vec<String>,
    /// Provider adapters only, e.g. `openai-compatible`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    pub capabilities: BTreeSet<Capability>,
    pub permissions: AdapterPermissions,
    pub conformance: ConformanceSpec,
}

impl AdapterManifest {
    /// The identity a loaded adapter must report back from `metadata()`.
    #[must_use]
    pub fn metadata(&self) -> AdapterMetadata {
        AdapterMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            kind: self.kind,
            api_version: self.api_version.clone(),
        }
    }

    /// Checks everything the host runtime requires before a package may install.
    ///
    /// # Errors
    ///
    /// Returns the first [`ManifestError`] found. Order is deliberate: identity
    /// first, then the sandbox, then role coherence, so a package missing a name
    /// is not reported as a role violation.
    pub fn validate(&self) -> Result<(), ManifestError> {
        self.validate_identity()?;
        self.validate_sandbox()?;
        self.validate_role()?;
        self.validate_conformance()
    }

    fn validate_identity(&self) -> Result<(), ManifestError> {
        if self.name.is_empty() {
            return Err(ManifestError::MissingName);
        }
        validate_plugin_name(&self.name)?;
        if !is_semver_triple(&self.version) {
            return Err(ManifestError::InvalidVersion(self.version.clone()));
        }
        if self.api_version != self.kind.expected_api_version() {
            return Err(ManifestError::ApiVersionDoesNotMatchKind);
        }
        Ok(())
    }

    fn validate_sandbox(&self) -> Result<(), ManifestError> {
        if self.permissions.network {
            return Err(ManifestError::NetworkPermissionDenied);
        }
        if self.permissions.filesystem {
            return Err(ManifestError::FilesystemPermissionDenied);
        }
        if self.kind == AdapterKind::Agent && !self.permissions.secrets.is_empty() {
            return Err(ManifestError::AgentAdapterCannotRequestSecrets);
        }
        for secret in &self.permissions.secrets {
            if !is_secret_ref_name(secret) {
                return Err(ManifestError::SecretIsNotAReferenceName(secret.clone()));
            }
        }
        Ok(())
    }

    fn validate_role(&self) -> Result<(), ManifestError> {
        if !self.capabilities.contains(&Capability::Chat) {
            return Err(ManifestError::ChatCapabilityRequired);
        }

        match self.kind {
            AdapterKind::Agent => {
                if self.agent_protocols.is_empty() {
                    return Err(ManifestError::AgentAdapterMustDeclareProtocol);
                }
                if !self.providers.is_empty() {
                    return Err(ManifestError::AgentAdapterCannotDeclareProviders);
                }
            }
            AdapterKind::Provider => {
                if self.providers.is_empty() {
                    return Err(ManifestError::ProviderAdapterMustDeclareProvider);
                }
                if !self.agent_protocols.is_empty() || !self.agent_tools.is_empty() {
                    return Err(ManifestError::ProviderAdapterCannotDeclareAgentBinding);
                }
                if self.capabilities.contains(&Capability::AgentHint) {
                    return Err(ManifestError::ProviderAdapterCannotExtractHints);
                }
            }
        }
        Ok(())
    }

    fn validate_conformance(&self) -> Result<(), ManifestError> {
        if self.conformance.required_suite != self.kind.expected_conformance_suite() {
            return Err(ManifestError::ConformanceSuiteDoesNotMatchKind);
        }
        if self.conformance.fixtures.is_empty() {
            return Err(ManifestError::MissingFixtures);
        }
        validate_package_relative_path(&self.conformance.fixtures)?;
        Ok(())
    }
}

/// Validates the one-component package identity used below the plugin root.
///
/// # Errors
///
/// Returns [`ManifestError::InvalidName`] unless `value` is lowercase ASCII
/// kebab-case, starts with an alphanumeric byte, ends with an alphanumeric byte,
/// and is at most 64 bytes.
pub fn validate_plugin_name(value: &str) -> Result<(), ManifestError> {
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidName(value.to_owned()))
    }
}

/// Validates a normalized package-relative path without consulting the file
/// system. Runtime walkers still reject symlinks and special files.
///
/// # Errors
///
/// Returns [`ManifestError::InvalidFixturesPath`] for absolute, parent-relative,
/// platform-ambiguous, empty-component, control-character, or over-deep paths.
pub fn validate_package_relative_path(value: &str) -> Result<(), ManifestError> {
    let normalized = value.strip_suffix('/').unwrap_or(value);
    let components: Vec<_> = normalized.split('/').collect();
    let invalid = value.is_empty()
        || value.len() > 256
        || value.starts_with('/')
        || value.ends_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || components.len() > 16
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."));
    if invalid {
        Err(ManifestError::InvalidFixturesPath(value.to_owned()))
    } else {
        Ok(())
    }
}

/// A reference name is lowercase alphanumeric with underscores, starting with a
/// letter.
///
/// Deliberately narrow. A real credential — `sk-live-abc`, a base64 blob, a JWT
/// — cannot satisfy it, so pasting one into `permissions.secrets` fails at
/// install rather than leaking into a registry.
fn is_secret_ref_name(value: &str) -> bool {
    let mut chars = value.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    starts_with_letter && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_semver_triple(value: &str) -> bool {
    let mut parts = value.split('.');
    let triple = [parts.next(), parts.next(), parts.next()];
    parts.next().is_none()
        && triple.iter().all(|part| {
            part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Why the host refused a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    MissingName,
    InvalidName(String),
    InvalidVersion(String),
    ApiVersionDoesNotMatchKind,
    NetworkPermissionDenied,
    FilesystemPermissionDenied,
    AgentAdapterCannotRequestSecrets,
    SecretIsNotAReferenceName(String),
    ChatCapabilityRequired,
    AgentAdapterMustDeclareProtocol,
    AgentAdapterCannotDeclareProviders,
    ProviderAdapterMustDeclareProvider,
    ProviderAdapterCannotDeclareAgentBinding,
    ProviderAdapterCannotExtractHints,
    ConformanceSuiteDoesNotMatchKind,
    MissingFixtures,
    InvalidFixturesPath(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingName => f.write_str("manifest declares no name"),
            Self::InvalidName(name) => write!(
                f,
                "plugin name `{name}` must be one lowercase kebab-case component of at most 64 bytes"
            ),
            Self::InvalidVersion(version) => {
                write!(f, "version `{version}` is not a `major.minor.patch` triple")
            }
            Self::ApiVersionDoesNotMatchKind => {
                f.write_str("api_version does not match the adapter kind")
            }
            Self::NetworkPermissionDenied => {
                f.write_str("adapters have no network; the host makes every request")
            }
            Self::FilesystemPermissionDenied => f.write_str("adapters have no file system"),
            Self::AgentAdapterCannotRequestSecrets => {
                f.write_str("an agent adapter never sees a credential, so it may declare none")
            }
            Self::SecretIsNotAReferenceName(secret) => write!(
                f,
                "`{secret}` is not a credential reference name; declare a name, never a credential"
            ),
            Self::ChatCapabilityRequired => f.write_str("every adapter must support `chat`"),
            Self::AgentAdapterMustDeclareProtocol => {
                f.write_str("an agent adapter must declare at least one agent protocol")
            }
            Self::AgentAdapterCannotDeclareProviders => {
                f.write_str("an agent adapter has no southbound binding")
            }
            Self::ProviderAdapterMustDeclareProvider => {
                f.write_str("a provider adapter must declare at least one provider")
            }
            Self::ProviderAdapterCannotDeclareAgentBinding => {
                f.write_str("a provider adapter has no northbound binding")
            }
            Self::ProviderAdapterCannotExtractHints => f.write_str(
                "a provider adapter never sees an inbound request, so it cannot extract hints",
            ),
            Self::ConformanceSuiteDoesNotMatchKind => {
                f.write_str("conformance.required_suite does not match the adapter kind")
            }
            Self::MissingFixtures => f.write_str("conformance.fixtures is empty"),
            Self::InvalidFixturesPath(path) => write!(
                f,
                "conformance.fixtures `{path}` must be a normalized relative package path"
            ),
        }
    }
}

impl Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::{
        AdapterKind, AdapterManifest, AdapterPermissions, Capability, ConformanceSpec,
        ManifestError, is_secret_ref_name, is_semver_triple,
    };
    use std::collections::BTreeSet;

    fn provider_manifest() -> AdapterManifest {
        AdapterManifest {
            name: "adapter-openai-provider".to_owned(),
            version: "1.2.0".to_owned(),
            kind: AdapterKind::Provider,
            api_version: "provider-adapter-v1".to_owned(),
            agent_protocols: Vec::new(),
            agent_tools: Vec::new(),
            providers: vec!["openai-compatible".to_owned()],
            capabilities: BTreeSet::from([Capability::Chat, Capability::Stream]),
            permissions: AdapterPermissions::new(false, false, ["provider_api_key"]),
            conformance: ConformanceSpec {
                required_suite: "provider-protocol-v1".to_owned(),
                fixtures: "fixtures/".to_owned(),
            },
        }
    }

    fn agent_manifest() -> AdapterManifest {
        AdapterManifest {
            name: "adapter-openai-client".to_owned(),
            version: "1.0.0".to_owned(),
            kind: AdapterKind::Agent,
            api_version: "agent-adapter-v1".to_owned(),
            agent_protocols: vec!["openai-chat-completions".to_owned()],
            agent_tools: vec!["cursor".to_owned()],
            providers: Vec::new(),
            capabilities: BTreeSet::from([Capability::Chat, Capability::AgentHint]),
            permissions: AdapterPermissions::default(),
            conformance: ConformanceSpec {
                required_suite: "agent-protocol-v1".to_owned(),
                fixtures: "fixtures/".to_owned(),
            },
        }
    }

    #[test]
    fn accepts_the_official_shapes() {
        assert_eq!(provider_manifest().validate(), Ok(()));
        assert_eq!(agent_manifest().validate(), Ok(()));
    }

    #[test]
    fn rejects_a_credential_pasted_into_the_secrets_list() {
        let mut manifest = provider_manifest();
        manifest.permissions.secrets = vec!["sk-live-abc123".to_owned()];

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::SecretIsNotAReferenceName(
                "sk-live-abc123".to_owned()
            ))
        );
    }

    #[test]
    fn rejects_network_and_filesystem_requests() {
        let mut networked = provider_manifest();
        networked.permissions.network = true;
        let mut on_disk = provider_manifest();
        on_disk.permissions.filesystem = true;

        assert_eq!(
            networked.validate(),
            Err(ManifestError::NetworkPermissionDenied)
        );
        assert_eq!(
            on_disk.validate(),
            Err(ManifestError::FilesystemPermissionDenied)
        );
    }

    #[test]
    fn rejects_an_agent_adapter_that_asks_for_a_credential() {
        let mut manifest = agent_manifest();
        manifest.permissions.secrets = vec!["provider_api_key".to_owned()];

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::AgentAdapterCannotRequestSecrets)
        );
    }

    #[test]
    fn rejects_a_provider_adapter_that_claims_to_extract_hints() {
        let mut manifest = provider_manifest();
        manifest.capabilities.insert(Capability::AgentHint);

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::ProviderAdapterCannotExtractHints)
        );
    }

    #[test]
    fn rejects_a_role_that_declares_the_other_role_binding() {
        let mut agent = agent_manifest();
        agent.providers = vec!["openai-compatible".to_owned()];

        let mut provider = provider_manifest();
        provider.agent_protocols = vec!["openai-chat-completions".to_owned()];

        assert_eq!(
            agent.validate(),
            Err(ManifestError::AgentAdapterCannotDeclareProviders)
        );
        assert_eq!(
            provider.validate(),
            Err(ManifestError::ProviderAdapterCannotDeclareAgentBinding)
        );
    }

    #[test]
    fn rejects_an_api_version_from_the_other_world() {
        let mut manifest = provider_manifest();
        manifest.api_version = "agent-adapter-v1".to_owned();

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::ApiVersionDoesNotMatchKind)
        );
    }

    #[test]
    fn rejects_a_conformance_suite_from_the_other_kind() {
        let mut manifest = provider_manifest();
        manifest.conformance.required_suite = "agent-protocol-v1".to_owned();

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::ConformanceSuiteDoesNotMatchKind)
        );
    }

    #[test]
    fn identity_is_checked_before_role_coherence() {
        let mut manifest = provider_manifest();
        manifest.name = String::new();
        manifest.providers.clear();

        assert_eq!(manifest.validate(), Err(ManifestError::MissingName));
    }

    #[test]
    fn plugin_identity_rejects_every_non_component_name() {
        let overlong = "a".repeat(65);
        for unsafe_name in [
            "..",
            "../outside",
            "nested/plugin",
            r"nested\plugin",
            "/absolute",
            r"C:\absolute",
            "Uppercase",
            "-leading",
            "trailing-",
            "contains space",
            "line\nbreak",
            overlong.as_str(),
        ] {
            let mut manifest = provider_manifest();
            manifest.name = unsafe_name.to_owned();
            assert!(
                manifest.validate().is_err(),
                "plugin name must be one bounded lowercase filesystem component: {unsafe_name:?}"
            );
        }
    }

    #[test]
    fn conformance_fixtures_reject_paths_that_can_escape_or_change_meaning() {
        for unsafe_path in [
            "..",
            "../fixtures",
            "fixtures/../outside",
            "/absolute",
            r"C:\absolute",
            r"fixtures\windows",
            "fixtures//nested",
            "./fixtures",
        ] {
            let mut manifest = provider_manifest();
            manifest.conformance.fixtures = unsafe_path.to_owned();
            assert!(
                manifest.validate().is_err(),
                "fixtures must be a normalized relative path below the package: {unsafe_path:?}"
            );
        }
    }

    #[test]
    fn secret_reference_names_exclude_anything_shaped_like_a_credential() {
        assert!(is_secret_ref_name("provider_api_key"));
        assert!(is_secret_ref_name("k2"));

        assert!(!is_secret_ref_name(""));
        assert!(!is_secret_ref_name("sk-live-abc"));
        assert!(!is_secret_ref_name("Bearer_abc"));
        assert!(!is_secret_ref_name("2fast"));
        assert!(!is_secret_ref_name("eyJhbGciOiJIUzI1NiJ9.abc"));
    }

    #[test]
    fn version_must_be_a_numeric_triple() {
        assert!(is_semver_triple("1.2.0"));
        assert!(is_semver_triple("0.0.1"));

        assert!(!is_semver_triple("1.2"));
        assert!(!is_semver_triple("1.2.3.4"));
        assert!(!is_semver_triple("v1.2.3"));
        assert!(!is_semver_triple("1.2.x"));
        assert!(!is_semver_triple(""));
    }
}
