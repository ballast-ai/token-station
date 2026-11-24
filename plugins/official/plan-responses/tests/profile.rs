//! Each profile switch, both ways. The components' own suites cover the rest
//! of the codec under their real profiles.

use plan_responses::{OpaqueReasoning, Profile, ResponseMetadata, ResponsesCodec};
use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;
use token_station_protocol::{ErrorCode, ErrorEnvelope, StreamEvent};

const LENIENT: Profile = Profile {
    label: "Lenient",
    opaque_reasoning: OpaqueReasoning::Ignore,
    response_metadata: ResponseMetadata::Ignore,
};

const STRICT: Profile = Profile {
    label: "Strict",
    opaque_reasoning: OpaqueReasoning::Refuse,
    response_metadata: ResponseMetadata::Refuse,
};

fn completed(usage: Option<Value>) -> Value {
    let mut response = json!({"id":"resp_1","model":"m","status":"completed","output":[]});
    if let Some(usage) = usage {
        response["usage"] = usage;
    }
    json!({"type":"response.completed","response":response})
}

fn usage() -> Value {
    json!({"input_tokens":3,"output_tokens":2,"total_tokens":5})
}

/// The stream's events, or its first refusal.
fn read(profile: Profile, events: &[Value]) -> Result<Vec<StreamEvent>, ErrorEnvelope> {
    let body: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    let mut parser = ResponsesCodec { profile }.stream_parser();
    let mut out = parser.parse_chunk(body.as_bytes())?;
    if let Some(StreamEvent::Error { error }) = out.last() {
        return Err(error.clone());
    }
    out.extend(parser.finish()?);
    Ok(out)
}

fn reasoning_item() -> Value {
    json!({"type":"reasoning","id":"rs_1","encrypted_content":"opaque","summary":[]})
}

#[test]
fn opaque_reasoning_is_ignored_or_refused() {
    for event in [
        json!({"type":"response.output_item.added","output_index":0,"item":reasoning_item()}),
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning_item()}),
    ] {
        let stream = [event, completed(Some(usage()))];
        assert!(read(LENIENT, &stream).is_ok(), "{}", stream[0]["type"]);
        let error = read(STRICT, &stream).unwrap_err();
        assert_eq!(error.code, ErrorCode::Capability, "{}", stream[0]["type"]);
        assert_eq!(error.http_status, 400);
        assert_eq!(
            error.message,
            "Strict cannot preserve opaque Responses reasoning state"
        );
    }
}

#[test]
fn reasoning_without_opaque_state_is_read_under_both_profiles() {
    let item = json!({"type":"reasoning","id":"rs_1","summary":[]});
    let stream = [
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        completed(Some(usage())),
    ];
    assert!(read(LENIENT, &stream).is_ok());
    assert!(read(STRICT, &stream).is_ok());
}

#[test]
fn response_metadata_is_ignored_or_refused() {
    let stream = [
        json!({"type":"response.metadata","metadata":{"region":"x"}}),
        completed(Some(usage())),
    ];
    assert!(read(LENIENT, &stream).is_ok());
    let error = read(STRICT, &stream).unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderProtocolError);
    assert_eq!(
        error.message,
        "The provider returned an unsupported response event"
    );
}

#[test]
fn usage_refusals_name_the_profile() {
    for (profile, label) in [(LENIENT, "Lenient"), (STRICT, "Strict")] {
        let missing = read(profile, &[completed(None)]).unwrap_err();
        assert_eq!(
            missing.message,
            format!("{label} response carries no usage object")
        );
        let unequal = read(
            profile,
            &[completed(Some(
                json!({"input_tokens":3,"output_tokens":2,"total_tokens":6}),
            ))],
        )
        .unwrap_err();
        assert_eq!(
            unequal.message,
            format!("{label} total_tokens does not equal input_tokens plus output_tokens")
        );
    }
}
