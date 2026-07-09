#![doc = "WASM adapter runtime boundary for local client and server."]

use token_station_plugin_api::AdapterMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedAdapter {
    metadata: AdapterMetadata,
}

impl LoadedAdapter {
    #[must_use]
    pub fn new(metadata: AdapterMetadata) -> Self {
        Self { metadata }
    }

    #[must_use]
    pub fn metadata(&self) -> &AdapterMetadata {
        &self.metadata
    }
}
