use provider_chatgpt_plan_v2::ChatGptPlan;
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ErrorCode, ErrorEnvelope, HttpResponseParts, StreamEvent};

fn frame(event: Value) -> Vec<u8> {
    format!("data: {event}\n\n").into_bytes()
}

fn metadata() -> Value {
    json!({"type":"response.metadata","sequence_number":5,"response_id":"resp_test",
        "metadata":{"private":"PRIVATE_METADATA","status":"completed",
            "error":{"message":"PRIVATE_METADATA"},"output":[{"type":"function_call"}],
            "openai_chatgpt_moderation_metadata":{"prompt":{"is_blocked":false}}}})
}

fn buffered(wire: &[u8]) -> HttpResponseParts {
    HttpResponseParts {
        status: 200,
        headers: Default::default(),
        extensions: Default::default(),
        body: String::from_utf8(wire.to_vec()).unwrap(),
    }
}

fn stream(wire: &[u8], size: usize) -> Result<Vec<StreamEvent>, ErrorEnvelope> {
    let mut parser = ChatGptPlan.stream_parser();
    let mut events = Vec::new();
    for chunk in wire.chunks(size) {
        events.extend(parser.parse_chunk(chunk)?);
    }
    events.extend(parser.finish()?);
    Ok(events)
}

#[test]
fn metadata_preserves_real_text_tool_usage_and_completion_events() {
    for input in [
        include_str!("../fixtures/provider.stream.text.input.json"),
        include_str!("../fixtures/provider.stream.tool.input.json"),
    ] {
        let input: Value = serde_json::from_str(input).unwrap();
        let chunks = input["chunks"].as_array().unwrap();
        let baseline = chunks
            .iter()
            .flat_map(|chunk| chunk.as_str().unwrap().bytes())
            .collect::<Vec<_>>();
        let expected = stream(&baseline, baseline.len()).unwrap();
        assert!(expected
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. })));
        let expected_response = ChatGptPlan.parse_response(&buffered(&baseline)).unwrap();
        let mut wire = frame(metadata());
        for chunk in chunks {
            wire.extend_from_slice(chunk.as_str().unwrap().as_bytes());
            wire.extend(frame(metadata()));
        }
        for size in [1, 7, wire.len()] {
            let events = stream(&wire, size).unwrap();
            assert_eq!(events, expected);
            assert!(!serde_json::to_string(&events)
                .unwrap()
                .contains("PRIVATE_METADATA"));
        }
        assert_eq!(
            ChatGptPlan.parse_response(&buffered(&wire)).unwrap(),
            expected_response
        );
    }
}

#[test]
fn metadata_accepts_opaque_payloads_and_repeated_frames_without_creating_output() {
    let mut wire = Vec::new();
    for value in [
        Value::Null,
        json!(42),
        json!("PRIVATE_METADATA"),
        json!([]),
        json!({}),
    ] {
        wire.extend(frame(json!({"type":"response.metadata","metadata":value})));
    }
    for _ in 0..1000 {
        wire.extend(frame(metadata()));
    }
    let mut parser = ChatGptPlan.stream_parser();
    assert!(parser.parse_chunk(&wire).unwrap().is_empty());
    assert_eq!(
        parser.finish().unwrap_err().code,
        ErrorCode::TransportTruncated
    );
}

#[test]
fn metadata_cannot_replace_completion_or_finish_a_partial_response() {
    for prefix in [
        Vec::new(),
        frame(json!({"type":"response.output_text.delta","delta":"partial"})),
    ] {
        let mut wire = prefix;
        wire.extend(frame(metadata()));
        for size in [1, wire.len()] {
            assert_eq!(
                stream(&wire, size).unwrap_err().code,
                ErrorCode::TransportTruncated
            );
        }
        assert_eq!(
            ChatGptPlan
                .parse_response(&buffered(&wire))
                .unwrap_err()
                .code,
            ErrorCode::TransportTruncated
        );
    }
}

#[test]
fn metadata_does_not_hide_errors_refusals_or_truncation() {
    for failure in [
        json!({"type":"error","error":{"message":"PRIVATE_METADATA"}}),
        json!({"type":"response.failed","response":{"status":"failed"}}),
        json!({"type":"response.incomplete","response":{"status":"incomplete"}}),
        json!({"type":"response.refusal.delta","delta":"PRIVATE_METADATA"}),
        json!({"type":"response.completed","response":{"status":"failed"}}),
    ] {
        let baseline = frame(failure);
        let expected = ChatGptPlan
            .parse_response(&buffered(&baseline))
            .unwrap_err();
        for before in [false, true] {
            let mut wire = Vec::new();
            if before {
                wire.extend(frame(metadata()));
            }
            wire.extend(&baseline);
            wire.extend(frame(metadata()));
            wire.extend(frame(
                json!({"type":"response.completed","response":{"status":"completed"}}),
            ));
            for size in [1, 7, wire.len()] {
                let error = stream(&wire, size).unwrap_err();
                assert_eq!(error, expected);
                assert!(!serde_json::to_string(&error)
                    .unwrap()
                    .contains("PRIVATE_METADATA"));
            }
            assert_eq!(
                ChatGptPlan.parse_response(&buffered(&wire)).unwrap_err(),
                expected
            );
        }
    }
}

#[test]
fn metadata_compatibility_does_not_accept_unknown_event_names() {
    for kind in [
        "response.metadata.delta",
        "response.metadata ",
        "Response.metadata",
        "response.unknown",
    ] {
        let mut wire = frame(metadata());
        wire.extend(frame(json!({"type":kind,"metadata":{}})));
        wire.extend(frame(
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ));
        for size in [1, wire.len()] {
            assert_eq!(
                stream(&wire, size).unwrap_err().code,
                ErrorCode::ProviderProtocolError
            );
        }
        assert_eq!(
            ChatGptPlan
                .parse_response(&buffered(&wire))
                .unwrap_err()
                .code,
            ErrorCode::ProviderProtocolError
        );
    }
}

#[test]
fn metadata_keeps_frame_validation_and_size_limits() {
    for suffix in [
        b"data: {\"type\":\"response.metadata\",\"metadata\":\n\n".to_vec(),
        b"data: {\"type\":\"response.metadata\",\"metadata\":\"\xff\"}\n\n".to_vec(),
        vec![b'x'; 4 * 1024 * 1024 + 1],
    ] {
        let mut parser = ChatGptPlan.stream_parser();
        assert!(parser.parse_chunk(&frame(metadata())).unwrap().is_empty());
        let error = match parser.parse_chunk(&suffix) {
            Err(error) => error,
            Ok(events) => {
                assert!(events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::Error { .. })));
                parser.finish().unwrap_err()
            }
        };
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
    }
}
