use provider_copilot_plan_v2::CopilotPlan;
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ChatRequest, ProviderConfig};
fn config(paths: Value) -> ProviderConfig {
    serde_json::from_value(json!({"provider":"copilot-plan","base_url":"https://api.githubcopilot.com","auth":"local_subscription_token","models":[{"model":"real-model","tool":true,"vision":true}],"copilot_source_kind":"editor","copilot_model_endpoints":{"real-model":paths}})).unwrap()
}
fn request(role: &str) -> ChatRequest {
    serde_json::from_value(
        json!({"model":"real-model","messages":[{"role":role,"content":"Hello"}],"stream":true}),
    )
    .unwrap()
}

#[test]
fn preserves_strict_tools_on_supported_endpoints() {
    for path in ["/responses", "/chat/completions"] {
        for strict in [true, false] {
            let request: ChatRequest =
                serde_json::from_value(json!({"model":"real-model","messages":[],
                "tools":[{"name":"read","parameters":{"type":"object"}}],
                "responses_tool_strict":{"read":strict}}))
                .unwrap();
            let body = CopilotPlan
                .build_http_request(&request, &config(json!([path])))
                .unwrap()
                .body
                .unwrap();
            let function = if path == "/responses" {
                &body["tools"][0]
            } else {
                &body["tools"][0]["function"]
            };
            assert_eq!(function["strict"], strict, "{path}");
        }
    }
    let invalid: ChatRequest = serde_json::from_value(json!({"model":"real-model","messages":[],
        "tools":[{"name":"read","parameters":{"type":"object"}}],
        "responses_tool_strict":{"read":"true"}}))
    .unwrap();
    assert!(CopilotPlan
        .build_http_request(&invalid, &config(json!(["/responses"])))
        .is_err());
}

#[test]
fn messages_allows_non_strict_tools_and_rejects_required_strictness() {
    for (controls, accepted) in [
        (json!({}), true),
        (json!({"read":false}), true),
        (json!({"read":true}), false),
        (json!({"read":"false"}), false),
    ] {
        let request: ChatRequest = serde_json::from_value(json!({"model":"real-model","messages":[],"tools":[{"name":"read","parameters":{"type":"object"}}],"responses_tool_strict":controls})).unwrap();
        assert_eq!(
            CopilotPlan
                .build_http_request(&request, &config(json!(["/v1/messages"])))
                .is_ok(),
            accepted
        );
    }
}
#[test]
fn chooses_observed_endpoint_and_preserves_subscription_identity() {
    for (path, field) in [
        ("/chat/completions", "messages"),
        ("/responses", "input"),
        ("/v1/messages", "messages"),
    ] {
        let d = CopilotPlan
            .build_http_request(&request("user"), &config(json!([path])))
            .unwrap();
        assert_eq!(d.url, format!("https://api.githubcopilot.com{path}"));
        assert_eq!(d.headers.get("x-initiator"), Some("user"));
        assert_eq!(d.headers.get("copilot-integration-id"), Some("vscode-chat"));
        assert!(d.body.unwrap().get(field).is_some());
        assert_eq!(
            serde_json::to_value(d.auth).unwrap(),
            json!({"scheme":"bearer","secret":"local_subscription_token"})
        );
    }
}
#[test]
fn rejects_unknown_model_untrusted_destination_and_opaque_state() {
    let mut r = request("user");
    r.model = "auto".into();
    assert!(CopilotPlan
        .build_http_request(&r, &config(json!(["/chat/completions"])))
        .is_err());
    let mut cfg = config(json!(["/chat/completions"]));
    cfg.base_url = serde_json::from_value(json!("https://evil.test")).unwrap();
    assert!(CopilotPlan
        .build_http_request(&request("user"), &cfg)
        .is_err());
    let mut r = request("assistant");
    r.messages[0].content = serde_json::from_value(
        json!([{"type":"thinking","thinking":"private","signature":"opaque"}]),
    )
    .ok();
    assert!(CopilotPlan
        .build_http_request(&r, &config(json!(["/chat/completions"])))
        .is_err());
}
#[test]
fn tool_reply_is_agent_initiated_and_images_set_vision_header() {
    let mut r:ChatRequest=serde_json::from_value(json!({"model":"real-model","stream":true,"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.test/image.png"}}]}]})).unwrap();
    let d = CopilotPlan
        .build_http_request(&r, &config(json!(["/responses"])))
        .unwrap();
    assert_eq!(d.headers.get("copilot-vision-request"), Some("true"));
    r=serde_json::from_value(json!({"model":"real-model","stream":true,"messages":[{"role":"assistant","tool_calls":[{"id":"call1","name":"weather","arguments":"{}"}]},{"role":"tool","tool_call_id":"call1","content":"sunny"}]})).unwrap();
    let d = CopilotPlan
        .build_http_request(&r, &config(json!(["/responses"])))
        .unwrap();
    assert_eq!(d.headers.get("x-initiator"), Some("agent"));
    assert_eq!(d.body.unwrap()["input"][1]["call_id"], "call1");
}
#[test]
fn redacts_upstream_failures_and_maps_plan_allowance() {
    let p = serde_json::from_value(json!({"status":402,"headers":{},"body":"secret reflection"}))
        .unwrap();
    let e = CopilotPlan.map_provider_error(&p).unwrap();
    let s = serde_json::to_string(&e).unwrap();
    assert!(!s.contains("secret reflection"));
    assert_eq!(e.code, token_station_protocol::ErrorCode::PaymentRequired);
}
#[test]
fn streams_all_three_dialects_across_every_byte_boundary() {
    for source in [
        include_str!("../../provider-openai-compatible-v2/fixtures/provider.stream.tool-call.input.json"),
        include_str!("../../provider-chatgpt-plan-v2/fixtures/provider.stream.tool.input.json"),
        include_str!("../../provider-anthropic-v2/fixtures-anthropic/provider.stream.thinking-signature-survives.input.json"),
    ] {
        let input:Value=serde_json::from_str(source).unwrap();let chunks=input["chunks"].as_array().unwrap();
        let mut baseline=CopilotPlan.stream_parser();let mut expected=Vec::new();
        for chunk in chunks {expected.extend(baseline.parse_chunk(chunk.as_str().unwrap().as_bytes()).unwrap());}
        expected.extend(baseline.finish().unwrap());
        let mut fragmented=CopilotPlan.stream_parser();let mut actual=Vec::new();
        for chunk in chunks {for byte in chunk.as_str().unwrap().as_bytes(){actual.extend(fragmented.parse_chunk(&[*byte]).unwrap());}}
        actual.extend(fragmented.finish().unwrap());assert_eq!(actual,expected);
    }
}
#[test]
fn missing_stream_terminal_fails_instead_of_reporting_success() {
    let mut parser = CopilotPlan.stream_parser();
    parser
        .parse_chunk(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n")
        .unwrap();
    assert!(parser.finish().is_err());
    let mut parser = CopilotPlan.stream_parser();
    parser
        .parse_chunk(b"data: {\"type\":\"response.created\"")
        .unwrap();
    assert!(parser.parse_chunk(b"").is_err());
}
#[test]
fn messages_preserves_thinking_signatures_and_rejects_lossy_tool_result_content() {
    let r:ChatRequest=serde_json::from_value(json!({"model":"real-model","messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"reason","signature":"original-signature"},{"type":"text","text":"answer"}]}]})).unwrap();
    let d = CopilotPlan
        .build_http_request(&r, &config(json!(["/responses", "/v1/messages"])))
        .unwrap();
    assert!(d.url.ends_with("/v1/messages"));
    assert_eq!(
        d.body.unwrap()["messages"][0]["content"][0]["signature"],
        "original-signature"
    );
    let r:ChatRequest=serde_json::from_value(json!({"model":"real-model","messages":[{"role":"tool","tool_call_id":"call","content":[{"type":"image_url","image_url":{"url":"https://example.test/image.png"}}]}]})).unwrap();
    assert!(CopilotPlan
        .build_http_request(&r, &config(json!(["/v1/messages"])))
        .is_err());
}
#[test]
fn encrypted_responses_state_returns_capability_error() {
    let mut parser = CopilotPlan.stream_parser();
    let result=parser.parse_chunk(b"data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"opaque\"}}\n\n");
    let events = result.unwrap();
    assert!(
        matches!(&events[..],[token_station_protocol::StreamEvent::Error{error}] if error.code==token_station_protocol::ErrorCode::Capability)
    );
    let mut r = request("assistant");
    r.messages[0].extensions.insert(
        "responses_reasoning_encrypted_content".into(),
        json!("opaque"),
    );
    assert!(CopilotPlan
        .build_http_request(&r, &config(json!(["/responses"])))
        .is_err());
}
#[test]
fn chooses_a_supported_lossless_dialect_and_keeps_named_tool_choice() {
    let mut r = request("user");
    r.sampling.stop = vec!["STOP".into()];
    let d = CopilotPlan
        .build_http_request(&r, &config(json!(["/responses", "/chat/completions"])))
        .unwrap();
    assert!(d.url.ends_with("/chat/completions"));
    assert_eq!(d.body.unwrap()["stop"], json!(["STOP"]));
    let r:ChatRequest=serde_json::from_value(json!({"model":"real-model","messages":[{"role":"user","content":"weather"}],"tools":[{"name":"weather","parameters":{"type":"object"}}],"tool_choice":{"type":"tool","name":"weather"}})).unwrap();
    let d = CopilotPlan
        .build_http_request(&r, &config(json!(["/v1/messages"])))
        .unwrap();
    assert_eq!(
        d.body.unwrap()["tool_choice"],
        json!({"type":"tool","name":"weather"})
    );
}
#[test]
fn streamed_redacted_thinking_fails_without_losing_opaque_state() {
    let source=b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"opaque-state\"}}\n\n";
    for size in [1, source.len()] {
        let mut parser = CopilotPlan.stream_parser();
        let mut error = None;
        for chunk in source.chunks(size) {
            if let Err(e) = parser.parse_chunk(chunk) {
                error = Some(e);
                break;
            }
        }
        assert_eq!(
            error.unwrap().code,
            token_station_protocol::ErrorCode::Capability
        );
    }
}
#[test]
fn accepts_the_gateway_projection_of_upstream_subscription_metadata() {
    let upstream = json!({"provider":"copilot-plan","base_url":"https://api.githubcopilot.com","subscription_identity":{"copilot_source_kind":"cli","copilot_model_endpoints":{"real-model":["/responses"]}}});
    let mut projected:ProviderConfig=serde_json::from_value(json!({"provider":upstream["provider"],"base_url":upstream["base_url"],"auth":"local_subscription_token","models":[{"model":"real-model","tool":true}]})).unwrap();
    let metadata: std::collections::BTreeMap<String, Value> =
        serde_json::from_value(upstream["subscription_identity"].clone()).unwrap();
    projected.extensions.extend(metadata);
    let encoded = serde_json::to_value(&projected).unwrap();
    assert!(encoded.get("subscription_identity").is_none());
    assert_eq!(encoded["copilot_source_kind"], "cli");
    let parsed: ProviderConfig = serde_json::from_value(encoded).unwrap();
    let d = CopilotPlan
        .build_http_request(&request("user"), &parsed)
        .unwrap();
    assert_eq!(d.url, "https://api.githubcopilot.com/responses");
    assert_eq!(
        d.headers.get("copilot-integration-id"),
        Some("copilot-developer-cli")
    );
}
#[test]
fn missing_or_nested_metadata_returns_a_capability_error() {
    let mut cfg = config(json!(["/responses"]));
    cfg.extensions.clear();
    assert!(CopilotPlan
        .build_http_request(&request("user"), &cfg)
        .is_err());
    cfg.extensions.insert("subscription_identity".into(),json!({"copilot_source_kind":"editor","copilot_model_endpoints":{"real-model":["/responses"]}}));
    assert!(CopilotPlan
        .build_http_request(&request("user"), &cfg)
        .is_err());
}
