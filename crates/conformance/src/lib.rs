#![doc = "Conformance runner primitives for adapter plugin validation."]

use token_station_plugin_api::{AdapterKind, AdapterManifest, AdapterMetadata};

#[must_use]
pub fn accepts_adapter(metadata: &AdapterMetadata) -> bool {
    matches!(metadata.kind(), AdapterKind::Agent | AdapterKind::Provider)
        && !metadata.name().is_empty()
        && !metadata.api_version().is_empty()
}

#[must_use]
pub fn accepts_manifest(manifest: &AdapterManifest) -> bool {
    accepts_adapter(manifest.metadata()) && manifest.validate().is_ok()
}

#[cfg(test)]
mod tests {
    use super::{accepts_adapter, accepts_manifest};
    use token_station_plugin_api::{
        AdapterKind, AdapterManifest, AdapterMetadata, AdapterPermissions,
    };

    #[test]
    fn accepts_well_formed_metadata() {
        let metadata = AdapterMetadata::new(
            "provider-openai",
            AdapterKind::Provider,
            "provider-adapter-v1",
        );

        assert!(accepts_adapter(&metadata));
    }

    #[test]
    fn rejects_manifest_that_fails_permission_validation() {
        let manifest = AdapterManifest::new(
            AdapterMetadata::new("agent-openai", AdapterKind::Agent, "agent-adapter-v1"),
            AdapterPermissions::new(false, false, ["provider_api_key"]),
        );

        assert!(!accepts_manifest(&manifest));
    }

    #[test]
    fn accepts_manifest_that_passes_permission_validation() {
        let manifest = AdapterManifest::new(
            AdapterMetadata::new(
                "provider-openai",
                AdapterKind::Provider,
                "provider-adapter-v1",
            ),
            AdapterPermissions::new(false, false, ["provider_api_key"]),
        );

        assert!(accepts_manifest(&manifest));
    }
}
