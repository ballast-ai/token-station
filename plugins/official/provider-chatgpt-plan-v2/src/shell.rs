use std::sync::Mutex;

wit_bindgen::generate!({
    path: "wit",
    world: "provider-adapter-v2",
});

use crate::ChatGptPlan;
use exports::token_station::adapter::provider_adapter::{AdapterHealth, AdapterMetadata, Guest};
use south_component_conformance::{abi, ProviderComponentV1};
use token_station::adapter::common::HealthStatus;

/// The stream this instance is holding. Instance state on purpose: the host
/// instantiates one component per stream.
static STREAM: Mutex<Option<abi::StreamAbiV1>> = Mutex::new(None);

struct ChatGptPlanGuest;

impl Guest for ChatGptPlanGuest {
    fn metadata() -> AdapterMetadata {
        let reported = ChatGptPlan.metadata();
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
        abi::model_capabilities_json(&ChatGptPlan, &provider_config)
    }

    fn build_http_request(chat_request: String, provider_config: String) -> Result<String, String> {
        abi::build_http_request_json(&ChatGptPlan, &chat_request, &provider_config)
    }

    fn parse_response(response_parts: String) -> Result<String, String> {
        abi::parse_response_json(&ChatGptPlan, &response_parts)
    }

    fn parse_stream_chunk(chunk: Vec<u8>) -> Result<String, String> {
        let mut stream = STREAM.lock().expect("single-threaded guest");
        stream
            .get_or_insert_with(|| abi::StreamAbiV1::new(&ChatGptPlan))
            .parse_chunk_json(&chunk)
    }

    fn map_provider_error(response_parts: String) -> Result<String, String> {
        abi::map_provider_error_json(&ChatGptPlan, &response_parts)
    }
}

export!(ChatGptPlanGuest);
