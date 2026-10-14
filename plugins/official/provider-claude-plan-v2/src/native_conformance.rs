//! Run the packaged fixture suite against the native equivalents of the guest exports.
use south_component_conformance::{
    reference_anthropic::AnthropicReferenceV1, run_provider_component_suite_v1, ComponentResultV1,
    FixturePackV1, ProviderComponentV1, StreamParserV1,
};
use south_provider_api::ComponentMetadataV1;
use token_station_protocol::{
    ChatRequest, ChatResponse, ErrorEnvelope, HttpRequestDescriptor, HttpResponseParts,
    ModelCapability, ProviderConfig,
};

struct NativeClaude;
impl ProviderComponentV1 for NativeClaude {
    fn metadata(&self) -> ComponentMetadataV1 {
        ComponentMetadataV1 {
            name: "provider-claude-plan".into(),
            version: "1.0.0".into(),
            api_version: "provider-adapter-v2".into(),
        }
    }
    fn model_capabilities(
        &self,
        config: &ProviderConfig,
    ) -> ComponentResultV1<Vec<ModelCapability>> {
        AnthropicReferenceV1.model_capabilities(config)
    }
    fn build_http_request(
        &self,
        request: &ChatRequest,
        config: &ProviderConfig,
    ) -> ComponentResultV1<HttpRequestDescriptor> {
        let result = super::build_http_request(
            &serde_json::to_string(request).unwrap(),
            &serde_json::to_string(config).unwrap(),
        )
        .map_err(|error| serde_json::from_str::<ErrorEnvelope>(&error).unwrap())?;
        Ok(serde_json::from_str(&result).unwrap())
    }
    fn parse_response(&self, parts: &HttpResponseParts) -> ComponentResultV1<ChatResponse> {
        AnthropicReferenceV1.parse_response(parts)
    }
    fn map_provider_error(&self, parts: &HttpResponseParts) -> ComponentResultV1<ErrorEnvelope> {
        AnthropicReferenceV1.map_provider_error(parts)
    }
    fn stream_parser(&self) -> Box<dyn StreamParserV1> {
        AnthropicReferenceV1.stream_parser()
    }
}

#[test]
fn packaged_fixtures_pass_every_native_provider_component_check() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let pack = FixturePackV1::load(&path).unwrap();
    let report = run_provider_component_suite_v1(&NativeClaude, &pack);
    let failures: Vec<_> = report.failures().map(ToString::to_string).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let checks: std::collections::BTreeSet<_> = report
        .outcomes()
        .iter()
        .map(|row| row.check.as_str())
        .collect();
    assert_eq!(
        checks,
        std::collections::BTreeSet::from([
            "coverage",
            "fixture_match",
            "determinism",
            "unknown_field_tolerance",
            "stream_incrementality",
            "endpoint_confinement",
            "auth_errors_are_not_retriable",
        ])
    );
    assert!(report
        .outcomes()
        .iter()
        .any(|row| row.case == "provider.error.forbidden"));
    assert!(report
        .outcomes()
        .iter()
        .any(|row| row.case == "provider.error.rejected-credential"));
}
