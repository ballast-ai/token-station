//! The official Anthropic Messages provider component on the v2 South ABI: a
//! wit-bindgen shell around the conformance crate's native reference
//! implementation. See the manifest and the South design records
//! (`2026-08-21-provider-runtime.md`) for the contract.

#[cfg(target_arch = "wasm32")]
mod shell {
    use std::sync::Mutex;

    wit_bindgen::generate!({
        path: "wit",
        world: "provider-adapter-v2",
    });

    use exports::token_station::adapter::provider_adapter::{
        AdapterHealth, AdapterMetadata, Guest,
    };
    use south_component_conformance::reference_anthropic::AnthropicReferenceV1;
    use south_component_conformance::{abi, ProviderComponentV1};
    use token_station::adapter::common::HealthStatus;

    /// The stream this instance is holding. Instance state on purpose: the host
    /// instantiates one component per stream.
    static STREAM: Mutex<Option<abi::StreamAbiV1>> = Mutex::new(None);

    struct Anthropic;

    impl Guest for Anthropic {
        fn metadata() -> AdapterMetadata {
            let reported = AnthropicReferenceV1.metadata();
            AdapterMetadata {
                name: reported.name,
                version: reported.version,
                api_version: reported.api_version,
            }
        }

        fn healthcheck() -> AdapterHealth {
            AdapterHealth {
                status: HealthStatus::Ready,
                detail: None,
            }
        }

        fn model_capabilities(provider_config: String) -> Result<String, String> {
            abi::model_capabilities_json(&AnthropicReferenceV1, &provider_config)
        }

        fn build_http_request(
            chat_request: String,
            provider_config: String,
        ) -> Result<String, String> {
            abi::build_http_request_json(&AnthropicReferenceV1, &chat_request, &provider_config)
        }

        fn parse_response(response_parts: String) -> Result<String, String> {
            abi::parse_response_json(&AnthropicReferenceV1, &response_parts)
        }

        fn parse_stream_chunk(chunk: Vec<u8>) -> Result<String, String> {
            let mut stream = STREAM.lock().expect("single-threaded guest");
            stream
                .get_or_insert_with(|| abi::StreamAbiV1::new(&AnthropicReferenceV1))
                .parse_chunk_json(&chunk)
        }

        fn map_provider_error(response_parts: String) -> Result<String, String> {
            abi::map_provider_error_json(&AnthropicReferenceV1, &response_parts)
        }
    }

    export!(Anthropic);
}

#[cfg(test)]
mod tests {
    /// The vendored WIT must stay byte-for-byte the South-owned source; a
    /// drift here would compile this shell against a world the runtime does
    /// not instantiate.
    #[test]
    fn the_vendored_wit_is_byte_identical_to_the_south_owned_source() {
        assert_eq!(
            include_str!("../wit/provider-adapter.wit"),
            south_provider_api::ADAPTER_WIT
        );
    }
}
