#![doc = "Stable adapter plugin API surface and manifest types."]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Agent,
    Provider,
}

impl AdapterKind {
    #[must_use]
    pub const fn expected_api_version(self) -> &'static str {
        match self {
            Self::Agent => "agent-adapter-v1",
            Self::Provider => "provider-adapter-v1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterMetadata {
    name: String,
    kind: AdapterKind,
    api_version: String,
}

impl AdapterMetadata {
    #[must_use]
    pub fn new(name: impl Into<String>, kind: AdapterKind, api_version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            api_version: api_version.into(),
        }
    }

    #[must_use]
    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    #[must_use]
    pub fn kind(&self) -> AdapterKind {
        self.kind
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdapterPermissions {
    network: bool,
    filesystem: bool,
    secrets: Vec<String>,
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

    #[must_use]
    pub const fn network(&self) -> bool {
        self.network
    }

    #[must_use]
    pub const fn filesystem(&self) -> bool {
        self.filesystem
    }

    #[must_use]
    pub fn secrets(&self) -> &[String] {
        &self.secrets
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterManifest {
    metadata: AdapterMetadata,
    permissions: AdapterPermissions,
}

impl AdapterManifest {
    #[must_use]
    pub fn new(metadata: AdapterMetadata, permissions: AdapterPermissions) -> Self {
        Self {
            metadata,
            permissions,
        }
    }

    #[must_use]
    pub const fn metadata(&self) -> &AdapterMetadata {
        &self.metadata
    }

    #[must_use]
    pub const fn permissions(&self) -> &AdapterPermissions {
        &self.permissions
    }

    /// Validates that this manifest can be accepted by the host runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] when the manifest is missing required identity
    /// fields, declares an API version that does not match its adapter kind, or
    /// requests permissions that the default sandbox denies.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.metadata.name().is_empty() {
            return Err(ManifestError::MissingName);
        }

        if self.metadata.api_version() != self.metadata.kind().expected_api_version() {
            return Err(ManifestError::ApiVersionDoesNotMatchKind);
        }

        if self.permissions.network() {
            return Err(ManifestError::NetworkPermissionDenied);
        }

        if self.permissions.filesystem() {
            return Err(ManifestError::FilesystemPermissionDenied);
        }

        if self.metadata.kind() == AdapterKind::Agent && !self.permissions.secrets().is_empty() {
            return Err(ManifestError::AgentAdapterCannotRequestSecrets);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    MissingName,
    ApiVersionDoesNotMatchKind,
    NetworkPermissionDenied,
    FilesystemPermissionDenied,
    AgentAdapterCannotRequestSecrets,
}

#[cfg(test)]
mod tests {
    use super::{AdapterKind, AdapterManifest, AdapterMetadata, AdapterPermissions, ManifestError};

    #[test]
    fn metadata_keeps_adapter_kind() {
        let metadata = AdapterMetadata::new("agent-openai", AdapterKind::Agent, "agent-adapter-v1");

        assert_eq!(metadata.name(), "agent-openai");
        assert_eq!(metadata.kind(), AdapterKind::Agent);
        assert_eq!(metadata.api_version(), "agent-adapter-v1");
    }

    #[test]
    fn accepts_provider_manifest_with_secret_refs() {
        let manifest = AdapterManifest::new(
            AdapterMetadata::new(
                "provider-openai-compatible",
                AdapterKind::Provider,
                "provider-adapter-v1",
            ),
            AdapterPermissions::new(false, false, ["provider_api_key"]),
        );

        assert_eq!(manifest.validate(), Ok(()));
    }

    #[test]
    fn rejects_agent_manifest_that_requests_secrets() {
        let manifest = AdapterManifest::new(
            AdapterMetadata::new("agent-openai", AdapterKind::Agent, "agent-adapter-v1"),
            AdapterPermissions::new(false, false, ["provider_api_key"]),
        );

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::AgentAdapterCannotRequestSecrets)
        );
    }

    #[test]
    fn rejects_network_and_filesystem_permissions() {
        let network_manifest = AdapterManifest::new(
            AdapterMetadata::new(
                "provider-openai-compatible",
                AdapterKind::Provider,
                "provider-adapter-v1",
            ),
            AdapterPermissions::new(true, false, ["provider_api_key"]),
        );
        let filesystem_manifest = AdapterManifest::new(
            AdapterMetadata::new(
                "provider-openai-compatible",
                AdapterKind::Provider,
                "provider-adapter-v1",
            ),
            AdapterPermissions::new(false, true, ["provider_api_key"]),
        );

        assert_eq!(
            network_manifest.validate(),
            Err(ManifestError::NetworkPermissionDenied)
        );
        assert_eq!(
            filesystem_manifest.validate(),
            Err(ManifestError::FilesystemPermissionDenied)
        );
    }

    #[test]
    fn rejects_api_version_that_does_not_match_adapter_kind() {
        let manifest = AdapterManifest::new(
            AdapterMetadata::new("agent-openai", AdapterKind::Agent, "provider-adapter-v1"),
            AdapterPermissions::default(),
        );

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::ApiVersionDoesNotMatchKind)
        );
    }
}
