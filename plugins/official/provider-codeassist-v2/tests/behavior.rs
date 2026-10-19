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
fn non_object_tool_arguments_fail_before_any_lossy_translation() {
    let valid = json!({"response":{"modelVersion":"m","candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{}}}]},"finishReason":"STOP"}]}});
    let call = CodeAssist
        .parse_response(&response(valid.clone()))
        .unwrap()
        .choices[0]
        .message
        .tool_calls[0]
        .clone();
    for arguments in [json!([]), json!(1), Value::Null] {
        let mut invalid_call = call.clone();
        invalid_call.arguments = arguments.to_string();
        let request = req(json!({"model":"m","messages":[
            {"role":"assistant","tool_calls":[invalid_call]},
            {"role":"tool","tool_call_id":invalid_call.id,"content":"result"}]}));
        assert!(CodeAssist
            .build_http_request(&request, &config("gemini-cli-plan"))
            .is_err());
        let mut invalid_response = valid.clone();
        invalid_response["response"]["candidates"][0]["content"]["parts"][0]["functionCall"]
            ["args"] = arguments;
        assert!(CodeAssist
            .parse_response(&response(invalid_response.clone()))
            .is_err());
        let mut parser = CodeAssist.stream_parser();
        assert!(parser
            .parse_chunk(format!("data: {invalid_response}\n\n").as_bytes())
            .is_err());
    }
}

#[test]
fn explicit_non_strict_tools_remain_usable() {
    for controls in [json!({}), json!({"read":false})] {
        let request = req(
            json!({"model":"m","messages":[],"tools":[{"name":"read","parameters":{"type":"object"}}],"responses_tool_strict":controls}),
        );
        assert!(CodeAssist
            .build_http_request(&request, &config("gemini-cli-plan"))
            .is_ok());
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
fn rejects_non_text_system_and_tool_parts_before_reference_flattening() {
    let out = CodeAssist
        .parse_response(&response(json!({"response":{"modelVersion":"m","candidates":[{"content":{"parts":[{"functionCall":{"id":"wire-call","name":"read","args":{}},"thoughtSignature":"signature"}]},"finishReason":"STOP"}]}})))
        .unwrap();
    let call = &out.choices[0].message.tool_calls[0];
    for role in ["system", "tool"] {
        for part in [
            json!({"type":"image_url","image_url":{"url":"data:image/png;base64,Zml4dHVyZQ=="}}),
            json!({"type":"future_block","data":"fixture"}),
            json!({"type":"thinking","thinking":"fixture"}),
        ] {
            let message = if role == "tool" {
                json!({"role":role,"tool_call_id":call.id,"content":[{"type":"text","text":"prefix"},part]})
            } else {
                json!({"role":role,"content":[{"type":"text","text":"prefix"},part]})
            };
            let messages = if role == "tool" {
                json!([{"role":"assistant","tool_calls":[call]},message])
            } else {
                json!([message,{"role":"user","content":"fixture"}])
            };
            let request = req(json!({"model":"m","messages":messages}));
            let error = CodeAssist
                .build_http_request(&request, &config("gemini-cli-plan"))
                .expect_err("Unsupported content must fail before reference flattening");
            assert_eq!(error.code, token_station_protocol::ErrorCode::Capability);
        }
    }
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
fn strict_function_tool_constraints_fail_explicitly() {
    let request = req(
        json!({"model":"m","messages":[{"role":"user","content":"fixture"}],"tools":[{"name":"read","parameters":{"type":"object"}}],"responses_tool_strict":{"read":true}}),
    );
    let error = CodeAssist
        .build_http_request(&request, &config("gemini-cli-plan"))
        .unwrap_err();
    assert_eq!(error.code, token_station_protocol::ErrorCode::Capability);
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

#[test]
fn repeated_unsigned_tool_calls_have_unique_ids_and_preserve_absent_wire_ids() {
    let wire = json!({"response":{"modelVersion":"gemini-2.5-flash","candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{}}}]},"finishReason":"STOP"}]}});
    for streamed in [false, true] {
        let mut calls = Vec::new();
        for _ in 0..2 {
            let call = if streamed {
                let mut parser = CodeAssist.stream_parser();
                let events = parser
                    .parse_chunk(format!("data: {wire}\n\n").as_bytes())
                    .unwrap();
                let id = events
                    .iter()
                    .find_map(|event| match event {
                        StreamEvent::ToolCallDelta { id: Some(id), .. } => Some(id.clone()),
                        _ => None,
                    })
                    .unwrap();
                json!({"id":id,"name":"read","arguments":"{}"})
            } else {
                serde_json::to_value(
                    &CodeAssist
                        .parse_response(&response(wire.clone()))
                        .unwrap()
                        .choices[0]
                        .message
                        .tool_calls[0],
                )
                .unwrap()
            };
            calls.push(call);
        }
        assert_ne!(calls[0]["id"], calls[1]["id"]);
        let request = req(json!({"model":"gemini-2.5-flash","messages":[
            {"role":"assistant","tool_calls":[calls[0]]},
            {"role":"tool","tool_call_id":calls[0]["id"],"content":"first"},
            {"role":"assistant","tool_calls":[calls[1]]},
            {"role":"tool","tool_call_id":calls[1]["id"],"content":"second"}
        ]}));
        let body = CodeAssist
            .build_http_request(&request, &config("gemini-cli-plan"))
            .unwrap()
            .body
            .unwrap();
        for index in [0, 2] {
            assert!(
                body["request"]["contents"][index]["parts"][0]["functionCall"]
                    .get("id")
                    .is_none()
            );
            assert!(
                body["request"]["contents"][index + 1]["parts"][0]["functionResponse"]
                    .get("id")
                    .is_none()
            );
        }
    }
}

fn assert_thinking_replay(content: Value, original: &[Value]) {
    let mut request = req(json!({"model":"gemini-3.8-flash-medium","messages":[
        {"role":"assistant","content":content},
        {"role":"user","content":"continue"}
    ]}));
    let body = CodeAssist
        .build_http_request(&request, &config("antigravity-plan"))
        .unwrap()
        .body
        .unwrap();
    let replayed = body["request"]["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(&replayed[..original.len()], original);
    request.model = "gemini-3.8-pro".into();
    assert!(CodeAssist
        .build_http_request(&request, &config("antigravity-plan"))
        .is_err());
    request.model = "gemini-3.8-flash-medium".into();
    let mut altered = content;
    altered[0]["thinking"] = json!("modified history");
    request.messages[0].content = serde_json::from_value(altered).unwrap();
    assert!(CodeAssist
        .build_http_request(&request, &config("antigravity-plan"))
        .is_err());
}

#[test]
fn buffered_signed_thinking_replays_exact_parts_and_rejects_modified_history() {
    let thoughts = vec![
        json!({"text":"first α","thought":true,"thoughtSignature":"signature-a"}),
        json!({"text":"second β","thought":true,"thoughtSignature":"signature-b"}),
        json!({"thought":true,"thoughtSignature":"signature-trailer"}),
    ];
    let mut parts = thoughts.clone();
    parts.push(json!({"text":"visible answer"}));
    let out = CodeAssist
        .parse_response(&response(json!({"response":{"modelVersion":"gemini-3.8-flash-n","candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}})))
        .unwrap();
    let content = serde_json::to_value(&out.choices[0].message.content).unwrap();
    assert_eq!(content[0]["thinking"], "first αsecond β");
    assert_thinking_replay(content, &thoughts);
}

#[test]
fn streamed_signed_thinking_replays_all_parts_across_byte_fragments() {
    let thoughts = vec![
        json!({"text":"first α","thought":true,"thoughtSignature":"signature-a"}),
        json!({"text":"second β","thought":true,"thoughtSignature":"signature-b"}),
        json!({"thought":true,"thoughtSignature":"signature-trailer"}),
    ];
    let mut wire = String::new();
    for part in &thoughts {
        wire.push_str(&format!("data: {}\n\n", json!({"response":{"modelVersion":"gemini-3.8-flash-n","candidates":[{"content":{"parts":[part]}}]}})));
    }
    wire.push_str(&format!("data: {}\n\n", json!({"response":{"candidates":[{"content":{"parts":[{"text":"visible answer"}]},"finishReason":"STOP"}]}})));
    for chunk_size in [1, 7, wire.len()] {
        let mut parser = CodeAssist.stream_parser();
        let mut events = Vec::new();
        for chunk in wire.as_bytes().chunks(chunk_size) {
            events.extend(parser.parse_chunk(chunk).unwrap());
        }
        events.extend(parser.finish().unwrap());
        let mut thinking = String::new();
        let mut signature = String::new();
        let mut signatures = 0;
        for event in events {
            match event {
                StreamEvent::ThinkingDelta { thinking_delta, .. } => {
                    thinking.push_str(&thinking_delta);
                }
                StreamEvent::ThinkingSignatureDelta {
                    signature_delta, ..
                } => {
                    signature.push_str(&signature_delta);
                    signatures += 1;
                }
                _ => {}
            }
        }
        assert_eq!(signatures, 1);
        assert_thinking_replay(
            json!([{"type":"thinking","thinking":thinking,"signature":signature}]),
            &thoughts,
        );
    }
}

#[test]
fn legacy_tool_ids_replay_and_malformed_carriers_fail_closed() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let id = format!(
        "ca1_{}",
        URL_SAFE_NO_PAD.encode(json!(["legacy-wire-id", "legacy-signature", "m"]).to_string())
    );
    let request = req(json!({"model":"m","messages":[
        {"role":"assistant","tool_calls":[{"id":id,"name":"read","arguments":"{}"}]},
        {"role":"tool","tool_call_id":id,"content":"fixture"}
    ]}));
    let body = CodeAssist
        .build_http_request(&request, &config("gemini-cli-plan"))
        .unwrap()
        .body
        .unwrap();
    assert_eq!(
        body["request"]["contents"][0]["parts"][0]["functionCall"]["id"],
        "legacy-wire-id"
    );
    assert_eq!(
        body["request"]["contents"][0]["parts"][0]["thoughtSignature"],
        "legacy-signature"
    );
    for signature in [
        "foreign-signature".to_owned(),
        "ca_th1_invalid-base64".to_owned(),
        format!(
            "ca_th1_{}",
            URL_SAFE_NO_PAD.encode(
                json!(["m", [{"thought":true,"text":"fixture","unexpected":"state"}]]).to_string()
            )
        ),
        format!("ca_th1_{}", "A".repeat(6 * 1024 * 1024)),
    ] {
        let request = req(
            json!({"model":"m","messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"fixture","signature":signature}]}]}),
        );
        assert!(CodeAssist
            .build_http_request(&request, &config("gemini-cli-plan"))
            .is_err());
    }
}
