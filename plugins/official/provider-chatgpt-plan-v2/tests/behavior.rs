use provider_chatgpt_plan_v2::ChatGptPlan;
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ChatRequest, ErrorCode, ProviderConfig, StreamEvent};

fn config() -> ProviderConfig {
    serde_json::from_value(json!({"provider":"chatgpt-plan", "base_url":"https://api.openai.com/v1", "auth":"chatgpt_access_token"})).unwrap()
}

fn request(value: Value) -> ChatRequest {
    serde_json::from_value(value).unwrap()
}
fn frame(v: Value) -> Vec<u8> {
    format!("data: {v}\n\n").into_bytes()
}

#[test]
fn translates_history_images_and_tool_namespaces_without_codex_identity() {
    let r = request(json!({"model":"plan-model", "stream":false,
      "sampling":{"temperature":0.2,"top_p":0.8,"max_output_tokens":4096},
      "messages":[{"role":"system","content":"You are the user's agent."},
       {"role":"user","content":[{"type":"text","text":"inspect"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]},
       {"role":"assistant","tool_calls":[{"id":"call_a","name":"read_file","arguments":"{\"path\":\"a\"}"}]},
       {"role":"tool","tool_call_id":"call_a","content":"file content"}],
      "tools":[{"name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]
    }));
    let d = ChatGptPlan.build_http_request(&r, &config()).unwrap();
    let v = serde_json::to_value(d).unwrap();
    assert_eq!(v["url"], "https://api.openai.com/v1/responses");
    assert_eq!(
        v["auth"],
        json!({"scheme":"bearer","secret":"chatgpt_access_token"})
    );
    let b = &v["body"];
    assert_eq!(b["stream"], true);
    assert_eq!(b["store"], false);
    assert!(b.get("max_output_tokens").is_none());
    assert!(b.get("temperature").is_none());
    assert_eq!(b["instructions"], "You are the user's agent.");
    assert_eq!(b["input"][0]["content"][1]["type"], "input_image");
    assert_eq!(b["input"][1]["namespace"], "tokenstation_tools");
    assert_eq!(b["input"][1]["call_id"], "call_a");
    assert_eq!(b["input"][2]["type"], "function_call_output");
    assert_eq!(b["tools"][0]["type"], "namespace");
    assert_eq!(b["tools"][0]["tools"][0]["name"], "read_file");
    assert!(!v.to_string().contains("Codex"));
}

#[test]
fn rejects_unknown_content_stop_and_wrong_credential_destinations() {
    for r in [
        json!({"model":"m","messages":[{"role":"user","content":[{"type":"audio","data":"opaque"}]}]}),
        json!({"model":"m","messages":[],"sampling":{"stop":["end"]}}),
        json!({"model":"m","messages":[],"tool_choice":{"type":"mcp","server_label":"private"}}),
    ] {
        assert_eq!(
            ChatGptPlan
                .build_http_request(&request(r), &config())
                .unwrap_err()
                .code,
            ErrorCode::Capability
        );
    }
    let mut c = config();
    c.base_url =
        token_station_protocol::ProviderEndpoint::try_new("https://evil.example/v1").unwrap();
    assert!(ChatGptPlan
        .build_http_request(&request(json!({"model":"m","messages":[]})), &c)
        .is_err());
}

#[test]
fn parallel_tool_stream_is_incremental_and_replays_in_a_second_request() {
    let frames = vec![
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","namespace":"tokenstation_tools","name":"read_file","arguments":""}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","namespace":"tokenstation_tools","name":"search","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":"{\"path\":"}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_b","output_index":1,"delta":"{\"q\":\"中\"}"}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":"\"a\"}"}),
        json!({"type":"response.function_call_arguments.done","item_id":"fc_a","output_index":0,"arguments":"{\"path\":\"a\"}"}),
        json!({"type":"response.function_call_arguments.done","item_id":"fc_b","output_index":1,"arguments":"{\"q\":\"中\"}"}),
        json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":20,"output_tokens":8,"input_tokens_details":{"cached_tokens":3},"output_tokens_details":{"reasoning_tokens":2}}}}),
    ];
    let wire: Vec<u8> = frames.into_iter().flat_map(frame).collect();
    let mut whole = ChatGptPlan.stream_parser();
    let expected = whole.parse_chunk(&wire).unwrap();
    whole.finish().unwrap();
    for size in [1, 2, 3, 7, 19] {
        let mut parser = ChatGptPlan.stream_parser();
        let actual: Vec<StreamEvent> = wire
            .chunks(size)
            .flat_map(|c| parser.parse_chunk(c).unwrap())
            .collect();
        parser.finish().unwrap();
        assert_eq!(actual, expected);
    }
    let mut args = [String::new(), String::new()];
    let mut names = [String::new(), String::new()];
    for ev in &expected {
        if let StreamEvent::ToolCallDelta {
            index,
            name,
            arguments_delta,
            ..
        } = ev
        {
            args[*index as usize].push_str(arguments_delta);
            if let Some(n) = name {
                names[*index as usize] = n.clone();
            }
        }
    }
    assert_eq!(args, ["{\"path\":\"a\"}", "{\"q\":\"中\"}"]);
    assert_eq!(names, ["read_file", "search"]);
    assert!(matches!(expected.last(), Some(StreamEvent::Done { .. })));
    let second = request(json!({"model":"m","messages":[
      {"role":"assistant","tool_calls":[{"id":"call_a","name":names[0],"arguments":args[0]},{"id":"call_b","name":names[1],"arguments":args[1]}]},
      {"role":"tool","tool_call_id":"call_a","content":"a content"},
      {"role":"tool","tool_call_id":"call_b","content":"found"}]}));
    let body = ChatGptPlan
        .build_http_request(&second, &config())
        .unwrap()
        .body
        .unwrap();
    assert_eq!(body["input"][0]["namespace"], "tokenstation_tools");
    assert_eq!(body["input"][3]["call_id"], "call_b");
}

#[test]
fn incomplete_failed_and_eof_never_report_success() {
    for event in [
        json!({"type":"response.failed","response":{"status":"failed","error":{"message":"secret"}}}),
        json!({"type":"response.incomplete","response":{"status":"incomplete"}}),
    ] {
        let mut p = ChatGptPlan.stream_parser();
        let events = p.parse_chunk(&frame(event)).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. })));
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Done { .. })));
        assert!(!serde_json::to_string(&events).unwrap().contains("secret"));
    }
    let mut p = ChatGptPlan.stream_parser();
    p.parse_chunk(&frame(
        json!({"type":"response.output_text.delta","delta":"partial"}),
    ))
    .unwrap();
    assert!(p.finish().is_err());
}

#[test]
fn forced_function_preserves_selection_and_rejects_unknown_constraints() {
    let mut value = json!({"model":"m","messages":[],"tools":[{"name":"read","parameters":{}},{"name":"write","parameters":{}}],"tool_choice":{"type":"function","function":{"name":"read"}}});
    let body = ChatGptPlan
        .build_http_request(&request(value.clone()), &config())
        .unwrap()
        .body
        .unwrap();
    assert_eq!(body["tools"][0]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["tools"][0]["name"], "read");
    assert_eq!(body["tool_choice"], "required");
    value["tool_choice"]["unsupported_constraint"] = json!(true);
    assert!(ChatGptPlan
        .build_http_request(&request(value), &config())
        .is_err());
}

#[test]
fn unsigned_summaries_do_not_block_history_but_foreign_signatures_fail_closed() {
    let mut value = json!({"model":"m","messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"summary"},{"type":"text","text":"visible"}]}]});
    let body = ChatGptPlan
        .build_http_request(&request(value.clone()), &config())
        .unwrap()
        .body
        .unwrap();
    assert_eq!(
        body["input"][0]["content"],
        json!([{"type":"output_text","text":"visible"}])
    );
    value["messages"][0]["content"][0]["signature"] = json!("foreign-signature");
    assert_eq!(
        ChatGptPlan
            .build_http_request(&request(value), &config())
            .unwrap_err()
            .code,
        ErrorCode::Capability
    );
    let redacted = request(
        json!({"model":"m","messages":[{"role":"assistant","content":[{"type":"redacted_thinking","data":"foreign-opaque-state"}]}]}),
    );
    assert_eq!(
        ChatGptPlan
            .build_http_request(&redacted, &config())
            .unwrap_err()
            .code,
        ErrorCode::Capability
    );
}

#[test]
fn buffered_clients_use_the_same_sse_terminal_and_tool_rules() {
    let wire=[
      json!({"type":"response.created","response":{"id":"resp_1","model":"m"}}),
      json!({"type":"response.output_text.delta","delta":"hello"}),
      json!({"type":"response.completed","response":{"id":"resp_1","model":"m","status":"completed","usage":{"input_tokens":4,"output_tokens":2}}})
    ].into_iter().flat_map(frame).collect::<Vec<_>>();
    let mut parts:token_station_protocol::HttpResponseParts=serde_json::from_value(json!({"status":200,"headers":{"content-type":"text/event-stream"},"body":String::from_utf8(wire).unwrap()})).unwrap();
    let response = ChatGptPlan.parse_response(&parts).unwrap();
    assert_eq!(response.id, "resp_1");
    assert_eq!(response.model, "m");
    assert_eq!(
        serde_json::to_value(response.choices[0].message.content.clone()).unwrap(),
        "hello"
    );
    assert_eq!(response.usage.input_tokens, 4);
    parts.body = String::from_utf8(frame(
        json!({"type":"response.output_text.delta","delta":"cut"}),
    ))
    .unwrap();
    assert!(ChatGptPlan.parse_response(&parts).is_err());
}

#[test]
fn malformed_completed_arguments_do_not_finish_a_tool_call() {
    let wire=[
      json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc","call_id":"call","name":"read","namespace":"tokenstation_tools"}}),
      json!({"type":"response.function_call_arguments.done","item_id":"fc","output_index":0,"arguments":"{"}),
      json!({"type":"response.completed","response":{"status":"completed"}})
    ].into_iter().flat_map(frame).collect::<Vec<_>>();
    let events = ChatGptPlan.stream_parser().parse_chunk(&wire).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Error { .. })));
    assert!(!events.iter().any(|e| matches!(e, StreamEvent::Done { .. })));
}

#[test]
fn terminal_only_tool_output_is_complete_and_unknown_namespaces_fail() {
    let mut event = json!({"type":"response.completed","response":{"status":"completed","output":[{"id":"fc1","type":"function_call","call_id":"call1","name":"read","namespace":"tokenstation_tools","arguments":"{}"}]}});
    let mut parser = ChatGptPlan.stream_parser();
    let events = parser.parse_chunk(&frame(event.clone())).unwrap();
    parser.finish().unwrap();
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    event["response"]["output"][0]["namespace"] = json!("foreign_namespace");
    let events = ChatGptPlan
        .stream_parser()
        .parse_chunk(&frame(event))
        .unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event, StreamEvent::Error { .. })));
}

#[test]
fn authorization_errors_do_not_echo_tokens_and_retry_after_is_numeric() {
    let parts=serde_json::from_value(json!({"status":429,"headers":{"Retry-After":"7"},"body":"{\"error\":{\"message\":\"at-secret\"}}"})).unwrap();
    let error = ChatGptPlan.map_provider_error(&parts).unwrap();
    assert_eq!(error.retry_after_ms, Some(7000));
    assert!(!serde_json::to_string(&error).unwrap().contains("at-secret"));
    let parts = serde_json::from_value(json!({"status":403,"body":"{}"})).unwrap();
    assert!(!ChatGptPlan
        .map_provider_error(&parts)
        .unwrap()
        .code
        .is_retriable_elsewhere());
}

#[test]
fn loopback_test_endpoints_keep_the_declared_path_and_slot() {
    for endpoint in [
        "http://127.0.0.1:9876/v1",
        "http://localhost:9876/v1",
        "http://[::1]:9876/v1",
    ] {
        let mut c = config();
        c.base_url = token_station_protocol::ProviderEndpoint::try_new(endpoint).unwrap();
        let descriptor = ChatGptPlan
            .build_http_request(&request(json!({"model":"m","messages":[]})), &c)
            .unwrap();
        assert_eq!(descriptor.url, format!("{endpoint}/responses"));
    }
    for endpoint in [
        "http://localhost:9876/private/v1",
        "https://api.openai.com.evil/v1",
    ] {
        let mut c = config();
        c.base_url = token_station_protocol::ProviderEndpoint::try_new(endpoint).unwrap();
        assert!(ChatGptPlan
            .build_http_request(&request(json!({"model":"m","messages":[]})), &c)
            .is_err());
    }
}

#[test]
fn crlf_and_utf8_byte_splits_match_and_empty_stream_does_not_complete() {
    assert!(ChatGptPlan.stream_parser().finish().is_err());
    let wire="data: {\"type\":\"response.output_text.delta\",\"delta\":\"中\"}\r\n\r\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\r\n\r\n";
    let mut parser = ChatGptPlan.stream_parser();
    let mut events = Vec::new();
    for chunk in wire.as_bytes().chunks(1) {
        events.extend(parser.parse_chunk(chunk).unwrap());
    }
    parser.finish().unwrap();
    assert!(matches!(&events[0],StreamEvent::Delta{content,..} if content=="中"));
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
}

#[test]
fn tool_argument_events_reject_conflicting_or_unknown_identifiers() {
    let starts = [
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","namespace":"tokenstation_tools"}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"write","namespace":"tokenstation_tools"}}),
    ].into_iter().flat_map(frame).collect::<Vec<_>>();
    for (item_id, output_index) in [("fc_a", 1), ("unknown", 0), ("fc_a", 9)] {
        for kind in [
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
        ] {
            let mut parser = ChatGptPlan.stream_parser();
            parser.parse_chunk(&starts).unwrap();
            let event = json!({"type":kind,"item_id":item_id,"output_index":output_index,"delta":"{}","arguments":"{}"});
            let events = parser.parse_chunk(&frame(event)).unwrap();
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::Error { .. })),
                "{kind}: {item_id}/{output_index} must fail"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::ToolCallDelta { .. })),
                "An invalid identity must not emit tool arguments"
            );
            assert!(parser.finish().is_err());
        }
    }
}

#[test]
fn exact_reasoning_effort_rejection_has_a_fixed_client_recovery_message() {
    use token_station_protocol::HttpResponseParts;
    const SAFE: &str = "Unsupported value for reasoning_effort. The selected model does not support this reasoning effort.";
    for message in [
        Value::Null,
        json!("REFLECTED_USER_TEXT secret_access_token model-private arbitrary-allowed-values"),
    ] {
        let parts=HttpResponseParts {status:400,headers:Default::default(),extensions:Default::default(),body:json!({"error":{"code":"unsupported_value","param":"reasoning.effort","message":message},"model":"private-model","token":"secret_access_token"}).to_string()};
        let error = ChatGptPlan.map_provider_error(&parts).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.http_status, 400);
        assert_eq!(error.message, SAFE);
        let serialized = serde_json::to_string(&error).unwrap();
        for secret in [
            "REFLECTED_USER_TEXT",
            "secret_access_token",
            "private-model",
            "arbitrary-allowed-values",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert_eq!(
            ChatGptPlan.parse_response(&parts).unwrap_err().message,
            SAFE
        );
    }
}

#[test]
fn near_miss_reasoning_errors_keep_the_generic_redacted_mapping() {
    use token_station_protocol::HttpResponseParts;
    for (status, code, param) in [
        (422, "unsupported_value", "reasoning.effort"),
        (401, "unsupported_value", "reasoning.effort"),
        (400, "invalid_value", "reasoning.effort"),
        (400, "unsupported_value", "reasoning_effort"),
        (400, "unsupported_value", "reasoning.effort.extra"),
        (400, "unsupported_value", "reasoning.effort "),
        (400, "Unsupported_Value", "reasoning.effort"),
        (400, "unsupported_value", "text.format"),
    ] {
        let parts=HttpResponseParts {status,headers:Default::default(),extensions:Default::default(),body:json!({"error":{"code":code,"param":param,"message":"unsupported reasoning_effort secret-reflection"}}).to_string()};
        let error = ChatGptPlan.map_provider_error(&parts).unwrap();
        assert!(!error.message.contains("reasoning_effort"));
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("secret-reflection"));
        if status == 400 {
            assert_eq!(
                error.message,
                "The ChatGPT plan endpoint rejected the request."
            );
        }
    }
}

#[test]
fn sse_reasoning_rejection_is_exact_redacted_and_terminal() {
    use token_station_protocol::HttpResponseParts;
    const SAFE: &str = "Unsupported value for reasoning_effort. The selected model does not support this reasoning effort.";
    for (kind, code, param, expected) in [
        (
            "error",
            "unsupported_value",
            "reasoning.effort",
            ErrorCode::InvalidRequest,
        ),
        (
            "error",
            "unsupported_value",
            "reasoning_effort",
            ErrorCode::ProviderProtocolError,
        ),
        (
            "error",
            "unsupported_value",
            "text.format",
            ErrorCode::ProviderProtocolError,
        ),
        (
            "error",
            "invalid_value",
            "reasoning.effort",
            ErrorCode::ProviderProtocolError,
        ),
        (
            "response.failed",
            "unsupported_value",
            "reasoning.effort",
            ErrorCode::ProviderProtocolError,
        ),
    ] {
        let event = json!({"type":kind,"error":{"code":code,"param":param,"message":"secret-reflected-model-token"}});
        let bytes = frame(event);
        for size in [1, 7, bytes.len()] {
            let mut parser = ChatGptPlan.stream_parser();
            let events: Vec<_> = bytes
                .chunks(size)
                .flat_map(|chunk| parser.parse_chunk(chunk).unwrap())
                .collect();
            let [StreamEvent::Error { error }] = events.as_slice() else {
                panic!("Expected terminal error: {events:?}")
            };
            assert_eq!(error.code, expected);
            assert_eq!(
                error.http_status,
                if expected == ErrorCode::InvalidRequest {
                    400
                } else {
                    502
                }
            );
            assert!(!error.message.contains("secret-reflected"));
            assert_eq!(error.message == SAFE, expected == ErrorCode::InvalidRequest);
            assert_eq!(parser.finish().unwrap_err().code, expected);
        }
        let parts = HttpResponseParts {
            status: 200,
            headers: Default::default(),
            extensions: Default::default(),
            body: String::from_utf8(bytes).unwrap(),
        };
        assert_eq!(
            ChatGptPlan.parse_response(&parts).unwrap_err().code,
            expected
        );
    }
}
