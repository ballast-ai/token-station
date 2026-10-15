use provider_codeassist_v2::CodeAssist;
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ChatRequest, HttpResponseParts, ProviderConfig, StreamEvent};
fn config(kind: &str) -> ProviderConfig {
    serde_json::from_value(json!({"provider":kind,"base_url":if kind=="gemini-cli-plan"{"https://cloudcode-pa.googleapis.com"}else{"https://daily-cloudcode-pa.googleapis.com"},"auth":"local_subscription_token","google_project":"project-fixture","google_client_version":"0.61.0"})).unwrap()
}
fn req(v: Value) -> ChatRequest {
    serde_json::from_value(v).unwrap()
}
fn response(v: Value) -> HttpResponseParts {
    HttpResponseParts {
        status: 200,
        headers: Default::default(),
        body: v.to_string(),
        extensions: Default::default(),
    }
}
#[test]
fn subscription_request_has_envelope_and_bearer() {
    for kind in ["gemini-cli-plan", "antigravity-plan"] {
        let c = config(kind);
        let d=CodeAssist.build_http_request(&req(json!({"model":"gemini-fixture","messages":[{"role":"user","content":"hello"}],"stream":true})),&c).unwrap();
        c.authorize(&d).unwrap();
        let v = serde_json::to_value(d).unwrap();
        assert!(v["url"]
            .as_str()
            .unwrap()
            .ends_with("/v1internal:streamGenerateContent?alt=sse"));
        assert_eq!(
            v["auth"],
            json!({"scheme":"bearer","secret":"local_subscription_token"})
        );
        assert_eq!(v["body"]["project"], "project-fixture");
        assert_eq!(
            v["body"]["request"]["contents"][0]["parts"][0]["text"],
            "hello"
        );
    }
}
#[test]
fn rejects_untrusted_endpoint_and_opaque_history() {
    let mut c = config("gemini-cli-plan");
    c.base_url = serde_json::from_value(json!("https://evil.invalid")).unwrap();
    assert!(CodeAssist
        .build_http_request(&req(json!({"model":"m","messages":[]})), &c)
        .is_err());
    let r = req(
        json!({"model":"m","messages":[{"role":"assistant","content":[{"type":"redacted_thinking","data":"foreign"}]}]}),
    );
    assert!(CodeAssist
        .build_http_request(&r, &config("antigravity-plan"))
        .is_err());
}
#[test]
fn response_tool_signature_survives_history() {
    let r=CodeAssist.parse_response(&response(json!({"response":{"modelVersion":"m","candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"signed-fixture"}]},"finishReason":"STOP"}]}}))).unwrap();
    let call = &r.choices[0].message.tool_calls[0];
    let d=CodeAssist.build_http_request(&req(json!({"model":"m","messages":[{"role":"assistant","tool_calls":[call]},{"role":"tool","tool_call_id":call.id,"content":"result"}]})),&config("gemini-cli-plan")).unwrap();
    assert_eq!(
        serde_json::to_value(d).unwrap()["body"]["request"]["contents"][0]["parts"][0]
            ["thoughtSignature"],
        "signed-fixture"
    );
}
#[test]
fn sse_envelope_handles_split_frames_and_usage() {
    let data = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"response":{"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}}),
        json!({"response":{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"cachedContentTokenCount":3,"candidatesTokenCount":4,"thoughtsTokenCount":2}}})
    );
    let mut parser = CodeAssist.stream_parser();
    let mut events = Vec::new();
    for c in data.as_bytes().chunks(7) {
        events.extend(parser.parse_chunk(c).unwrap());
    }
    events.extend(parser.parse_chunk(&[]).unwrap());
    assert!(events
        .iter()
        .any(|e| matches!(e,StreamEvent::Delta{content,..} if content=="hello")));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Done { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|e|matches!(e,StreamEvent::Usage{usage} if usage.input_tokens==9 && usage.output_tokens==6 && usage.cache_read_tokens==3 && usage.reasoning_tokens==2)));
}
#[test]
fn stream_failure_is_not_success() {
    let mut p = CodeAssist.stream_parser();
    assert!(p.parse_chunk(b"data: {bad}\n\n").is_err());
    let mut p = CodeAssist.stream_parser();
    let events = p
        .parse_chunk(
            format!(
                "data: {}\n\n",
                json!({"error":{"code":429,"message":"fixture failure"}})
            )
            .as_bytes(),
        )
        .unwrap();
    assert!(matches!(&events[0], StreamEvent::Error { .. }));
    assert!(p.parse_chunk(&[]).unwrap().is_empty());
}
#[test]
fn json_schema_and_supported_reasoning_are_preserved() {
    let request = req(
        json!({"model":"gemini-3-fixture","messages":[{"role":"user","content":"fixture"}],"reasoning_effort":"low","response_format":{"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object","properties":{"answer":{"type":"string"}}}}}}),
    );
    let d = CodeAssist
        .build_http_request(&request, &config("gemini-cli-plan"))
        .unwrap();
    let v = serde_json::to_value(d).unwrap();
    let generation = &v["body"]["request"]["generationConfig"];
    assert_eq!(generation["responseMimeType"], "application/json");
    assert_eq!(generation["responseJsonSchema"]["type"], "object");
    assert_eq!(generation["thinkingConfig"]["thinkingLevel"], "low");
    let unsupported =
        req(json!({"model":"gemini-3-fixture","messages":[],"parallel_tool_calls":false}));
    assert!(CodeAssist
        .build_http_request(&unsupported, &config("gemini-cli-plan"))
        .is_err());
}
#[test]
fn antigravity_session_identity_is_stable_across_turns() {
    let a = req(json!({"model":"m","messages":[{"role":"user","content":"first"}]}));
    let b = req(
        json!({"model":"m","messages":[{"role":"user","content":"first"},{"role":"assistant","content":"answer"},{"role":"user","content":"next"}]}),
    );
    let ca = config("antigravity-plan");
    let a = serde_json::to_value(CodeAssist.build_http_request(&a, &ca).unwrap()).unwrap();
    let b = serde_json::to_value(CodeAssist.build_http_request(&b, &ca).unwrap()).unwrap();
    assert_eq!(
        a["body"]["request"]["sessionId"],
        b["body"]["request"]["sessionId"]
    );
}
#[test]
fn truncated_stream_and_unknown_content_fail_explicitly() {
    let mut p = CodeAssist.stream_parser();
    p.parse_chunk(b"data: {").unwrap();
    assert!(p.parse_chunk(&[]).is_err());
    assert!(CodeAssist.parse_response(&response(json!({"response":{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"fixture"}}]},"finishReason":"STOP"}]}}))).is_err());
}
#[test]
fn signed_visible_json_keeps_text_and_optional_metadata_without_fake_thinking() {
    let out = CodeAssist.parse_response(&response(json!({"response":{"modelVersion":"gemini-2.5-flash","candidates":[{"content":{"parts":[{"text":"{\"ok\":true}","thoughtSignature":"optional-text-fixture"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}}))).unwrap();
    assert_eq!(
        serde_json::to_value(&out.choices[0].message.content).unwrap(),
        json!("{\"ok\":true}")
    );
    let metadata = &out.choices[0].message.extensions["codeassist_optional_text_signatures"];
    assert_eq!(metadata["model"], "gemini-2.5-flash");
    assert_eq!(metadata["parts"][0]["part_index"], 0);
    assert_eq!(
        metadata["parts"][0]["thoughtSignature"],
        "optional-text-fixture"
    );
    assert_eq!(out.choices[0].message.tool_calls.len(), 0);
}
#[test]
fn signed_visible_stream_and_empty_signature_trailer_are_normal_text() {
    let mut parser = CodeAssist.stream_parser();
    let wire = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"response":{"modelVersion":"gemini-2.5-flash","candidates":[{"content":{"parts":[{"text":"visible answer","thoughtSignature":"optional-inline-fixture"}]}}]}}),
        json!({"response":{"candidates":[{"content":{"parts":[{"text":"","thoughtSignature":"optional-trailer-fixture"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}})
    );
    let mut events = Vec::new();
    for chunk in wire.as_bytes().chunks(13) {
        events.extend(parser.parse_chunk(chunk).unwrap());
    }
    events.extend(parser.parse_chunk(&[]).unwrap());
    let texts = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Delta { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(texts, "visible answer");
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ThinkingDelta { .. }
            | StreamEvent::ThinkingSignatureDelta { .. }
            | StreamEvent::Error { .. }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, StreamEvent::Done { .. }))
            .count(),
        1
    );
}

#[test]
fn reasoning_alias_tool_signatures_replay_in_buffered_and_streamed_rounds() {
    let value = json!({"response":{"modelVersion":"gemini-3.8-flash-n","candidates":[{"content":{"parts":[{"functionCall":{"id":"wire-call","name":"check","args":{}},"thoughtSignature":"exact-signature"}]},"finishReason":"STOP"}]}});
    let buffered = CodeAssist
        .parse_response(&response(value.clone()))
        .unwrap()
        .choices[0]
        .message
        .tool_calls[0]
        .clone();
    let mut parser = CodeAssist.stream_parser();
    let events = parser
        .parse_chunk(format!("data: {value}\n\n").as_bytes())
        .unwrap();
    let streamed_id = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolCallDelta { id: Some(id), .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap();
    for id in [buffered.id.clone(), streamed_id] {
        let mut call = buffered.clone();
        call.id = id;
        let request = |model: &str| {
            req(
                json!({"model":model,"messages":[{"role":"assistant","tool_calls":[call]},{"role":"tool","tool_call_id":call.id,"content":"{\"ok\":true}"}]}),
            )
        };
        let descriptor = CodeAssist
            .build_http_request(
                &request("gemini-3.8-flash-medium"),
                &config("antigravity-plan"),
            )
            .unwrap();
        let body = descriptor.body.unwrap();
        assert_eq!(body["model"], "gemini-3.8-flash-medium");
        assert_eq!(
            body["request"]["contents"][0]["parts"][0]["thoughtSignature"],
            "exact-signature"
        );
        assert_eq!(
            body["request"]["contents"][0]["parts"][0]["functionCall"]["id"],
            "wire-call"
        );
        for other in [
            "gemini-3.5-flash-medium",
            "gemini-3.8-flash-lite",
            "gemini-3.8-flash-image",
            "gemini-3.8-pro",
            "claude-sonnet-4-6",
            "gemini-3.8-flash-unrecognized",
        ] {
            assert!(
                CodeAssist
                    .build_http_request(&request(other), &config("antigravity-plan"))
                    .is_err(),
                "reject history for {other}"
            );
        }
    }
}
