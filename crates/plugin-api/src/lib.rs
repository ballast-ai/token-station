#![doc = "Stable adapter plugin API surface and manifest types."]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Agent,
    Provider,
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

#[cfg(test)]
mod tests {
    use super::{AdapterKind, AdapterMetadata};

    #[test]
    fn metadata_keeps_adapter_kind() {
        let metadata = AdapterMetadata::new("agent-openai", AdapterKind::Agent, "agent-adapter-v1");

        assert_eq!(metadata.name(), "agent-openai");
        assert_eq!(metadata.kind(), AdapterKind::Agent);
        assert_eq!(metadata.api_version(), "agent-adapter-v1");
    }
}
