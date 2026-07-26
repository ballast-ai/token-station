//! Proves the `v1` Canonical IR can express a complete OpenAI tool-calling
//! exchange on both sides of the host: inbound from an agent, outbound to a
//! provider. This is the A0 exit criterion from the adapter architecture.
//!
//! Every fixture is asserted to survive `parse -> serialize` unchanged. That is
//! stronger than a semantic round trip: it pins the wire format, so a field that
//! silently stops serializing fails here rather than in a plugin written against
//! a stale shape. Conformance requires the same determinism of adapters.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use token_station_protocol::{
    AgentRequestEnvelope, Auth, ChatRequest, ChatResponse, ErrorCode, ErrorEnvelope, FinishReason,
    HttpMethod, HttpRequestDescriptor, HttpResponseParts, ModelCapability, ProviderConfig, Role,
    StreamEvent,
};

/// Parses `fixture`, re-serializes it, and asserts nothing moved.
fn assert_exact_round_trip<T>(fixture: &str) -> T
where
    T: DeserializeOwned + Serialize,
{
    let expected: Value = serde_json::from_str(fixture).expect("fixture is valid JSON");
    let parsed: T = serde_json::from_value(expected.clone()).expect("fixture matches the IR");
    let actual = serde_json::to_value(&parsed).expect("IR is serializable");

    assert_eq!(actual, expected, "fixture did not round-trip byte-for-byte");
    parsed
}

const ENVELOPE: &str = include_str!("fixtures/inbound.envelope.json");
const CHAT_REQUEST: &str = include_str!("fixtures/inbound.chat.request.json");
const PROVIDER_REQUEST: &str = include_str!("fixtures/outbound.provider.request.json");
const PROVIDER_RESPONSE: &str = include_str!("fixtures/outbound.provider.response.json");
const CHAT_RESPONSE: &str = include_str!("fixtures/outbound.chat.response.tool-call.json");
const STREAM: &str = include_str!("fixtures/outbound.stream.tool-call.json");
const RATE_LIMIT: &str = include_str!("fixtures/error.rate-limit.json");
const CAPABILITY: &str = include_str!("fixtures/model.capability.json");
const PROVIDER_CONFIG: &str = include_str!("fixtures/provider.config.json");

#[test]
fn inbound_envelope_expresses_an_openai_request_without_its_credential() {
    let envelope: AgentRequestEnvelope = assert_exact_round_trip(ENVELOPE);

    assert_eq!(envelope.protocol, "openai-chat-completions");
    assert_eq!(envelope.agent_tool.as_deref(), Some("claude-code"));

    // The adapter can see that the caller authenticated, and can read the hint
    // header, but cannot read the credential.
    assert!(envelope.headers.contains("authorization"));
    assert_eq!(envelope.headers.value("authorization"), None);
    assert_eq!(envelope.headers.value("x-agent-step"), Some("planning"));

    assert_eq!(envelope.hints.len(), 1);
    assert_eq!(envelope.hints[0].value, "planning");
}

#[test]
fn inbound_chat_request_expresses_tools_sampling_and_unmodelled_fields() {
    let request: ChatRequest = assert_exact_round_trip(CHAT_REQUEST);

    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, Role::System);
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name, "get_weather");
    assert_eq!(request.sampling.temperature, Some(0.2));
    assert!(request.stream);

    // `seed` is not modelled by v1 and must survive anyway.
    assert_eq!(request.extensions["seed"], serde_json::json!(42));
}

#[test]
fn outbound_provider_request_names_a_credential_and_carries_none() {
    let descriptor: HttpRequestDescriptor = assert_exact_round_trip(PROVIDER_REQUEST);

    assert_eq!(descriptor.method, HttpMethod::Post);
    assert_eq!(
        descriptor.auth,
        Some(Auth::bearer(token_station_protocol::SecretRef::new(
            "provider_api_key"
        ))),
        "the adapter says which credential and how to present it, never what it is"
    );
    assert_eq!(
        descriptor.headers.get("content-type"),
        Some("application/json")
    );

    // The host has not injected anything yet, so no header may look like auth.
    for (name, _) in descriptor.headers.iter() {
        assert_ne!(name.to_ascii_lowercase(), "authorization");
    }
}

#[test]
fn provider_config_binds_an_upstream_without_naming_its_key() {
    let config: ProviderConfig = assert_exact_round_trip(PROVIDER_CONFIG);

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(
        config
            .auth
            .as_ref()
            .map(token_station_protocol::SecretRef::as_str),
        Some("provider_api_key"),
        "the host names the credential slot; the adapter chooses how to present it"
    );
    assert_eq!(config.models[0].context_window, 400_000);

    // `organization` is not modelled by v1 and must survive anyway.
    assert_eq!(
        config.extensions["organization"],
        serde_json::json!("org-local")
    );
}

/// The two halves of the boundary, exercised together on the fixtures the
/// official OpenAI adapter is held to.
///
/// A provider adapter picks the URL *and* names the credential. Separately each
/// is harmless; together they would send the operator's key anywhere the plugin
/// liked. `authorize` is the only thing standing between those two facts, so it
/// is asserted against the same fixture pair a real request flows through.
#[test]
fn a_descriptor_addressed_off_its_upstream_is_refused_the_credential() {
    let config: ProviderConfig = serde_json::from_str(PROVIDER_CONFIG).expect("valid config");
    let descriptor: HttpRequestDescriptor =
        serde_json::from_str(PROVIDER_REQUEST).expect("valid descriptor");

    assert_eq!(config.authorize(&descriptor), Ok(()));

    let mut exfiltrating = descriptor;
    exfiltrating.url = "https://attacker.example/collect".to_owned();

    assert!(
        config.authorize(&exfiltrating).is_err(),
        "a plugin must not be able to choose where the host sends a credential"
    );
}

#[test]
fn outbound_provider_response_carries_a_raw_streaming_body() {
    let parts: HttpResponseParts = assert_exact_round_trip(PROVIDER_RESPONSE);

    assert_eq!(parts.status, 200);
    assert_eq!(
        parts.headers.get("content-type").map(String::as_str),
        Some("text/event-stream")
    );
    assert!(parts.body.starts_with("data: "));
}

#[test]
fn outbound_chat_response_expresses_a_tool_call_and_cache_aware_usage() {
    let response: ChatResponse = assert_exact_round_trip(CHAT_RESPONSE);

    let choice = &response.choices[0];
    assert_eq!(choice.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(choice.message.role, Role::Assistant);
    assert!(
        choice.message.content.is_none(),
        "a pure tool call has no text content"
    );

    let call = &choice.message.tool_calls[0];
    assert_eq!(call.name, "get_weather");
    assert_eq!(call.arguments, r#"{"city": "Beijing"}"#);

    assert_eq!(response.usage.cache_read_tokens, 32);
    assert_eq!(response.usage.total(), 65);
}

#[test]
fn outbound_stream_expresses_a_fragmented_tool_call() {
    let events: Vec<StreamEvent> = assert_exact_round_trip(STREAM);

    assert!(matches!(events[0], StreamEvent::Delta { .. }));

    // The call identity arrives once, then only argument fragments.
    let StreamEvent::ToolCallDelta { ref id, .. } = events[1] else {
        panic!("expected the opening tool_call_delta");
    };
    assert_eq!(id.as_deref(), Some("call_1"));

    let StreamEvent::ToolCallDelta {
        ref id,
        ref arguments_delta,
        ..
    } = events[2]
    else {
        panic!("expected a continuation tool_call_delta");
    };
    assert_eq!(id.as_deref(), None);
    assert_eq!(arguments_delta, ": \"Beijing\"}");

    // Reassembling the fragments yields the same arguments the non-streaming
    // response carries, which is what lets the router bill either shape alike.
    let reassembled: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCallDelta {
                arguments_delta, ..
            } => Some(arguments_delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reassembled, r#"{"city": "Beijing"}"#);

    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            finish_reason: Some(FinishReason::ToolCalls),
            stop_sequence: None,
        })
    ));
}

#[test]
fn rate_limit_error_is_retriable_on_another_upstream() {
    let error: ErrorEnvelope = assert_exact_round_trip(RATE_LIMIT);

    assert_eq!(error.code, ErrorCode::RateLimit);
    assert_eq!(error.http_status, 429);
    assert_eq!(error.retry_after_ms, Some(2000));
    assert!(error.code.is_retriable_elsewhere());
}

#[test]
fn model_capability_expresses_what_the_router_filters_on() {
    let capability: ModelCapability = assert_exact_round_trip(CAPABILITY);

    assert!(capability.tool_state().is_supported());
    assert_eq!(capability.context_window, 400_000);
    assert!(capability.supported_parameters.contains("temperature"));
}
