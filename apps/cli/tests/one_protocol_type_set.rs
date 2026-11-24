//! The host and South share one protocol type set.
//!
//! South compiles against the kernel mirror's `token-station-protocol`, and
//! the root manifest patches that source to `crates/protocol`. Without the
//! patch the two byte-identical crates are distinct Rust types, and this file
//! does not compile: a host request and config cannot reach a South function,
//! and a South descriptor is not a host descriptor.

use serde_json::json;
use south_component_conformance::ProviderComponentV1;
use south_component_conformance::reference::OpenAiCompatibleReferenceV1;
use token_station_protocol::{ChatRequest, HttpRequestDescriptor, Message, ProviderConfig, Role};

#[test]
fn a_south_reference_takes_and_returns_host_protocol_types() {
    let config: ProviderConfig = serde_json::from_value(json!({
        "provider": "openai-compatible",
        "base_url": "https://api.example.com/v1",
        "auth": "provider_api_key",
        "models": [{"model": "example-model"}],
    }))
    .expect("host provider config");
    let request = ChatRequest::new("example-model", vec![Message::text(Role::User, "hi")]);

    let descriptor: HttpRequestDescriptor = OpenAiCompatibleReferenceV1
        .build_http_request(&request, &config)
        .expect("the South reference builds the host request");

    assert!(
        descriptor.url.ends_with("/chat/completions"),
        "{}",
        descriptor.url
    );
    assert_eq!(
        descriptor.body.as_ref().expect("a JSON body")["messages"][0]["content"],
        "hi"
    );
}
