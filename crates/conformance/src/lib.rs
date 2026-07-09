#![doc = "Conformance runner primitives for adapter plugin validation."]

use token_station_plugin_api::{AdapterKind, AdapterMetadata};

#[must_use]
pub fn accepts_adapter(metadata: &AdapterMetadata) -> bool {
    matches!(metadata.kind(), AdapterKind::Agent | AdapterKind::Provider)
        && !metadata.name().is_empty()
        && !metadata.api_version().is_empty()
}

#[cfg(test)]
mod tests {
    use super::accepts_adapter;
    use token_station_plugin_api::{AdapterKind, AdapterMetadata};

    #[test]
    fn accepts_well_formed_metadata() {
        let metadata = AdapterMetadata::new(
            "provider-openai",
            AdapterKind::Provider,
            "provider-adapter-v1",
        );

        assert!(accepts_adapter(&metadata));
    }
}
