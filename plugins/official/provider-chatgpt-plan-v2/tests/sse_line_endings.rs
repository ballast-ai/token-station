//! ChatGPT plan streams read the same under every SSE line end.

#[path = "../../test-support/sse_line_endings.rs"]
mod sse_line_endings;

use std::path::Path;

use provider_chatgpt_plan_v2::ChatGptPlan;
use south_component_conformance::ProviderComponentV1;
use sse_line_endings::{assert_line_ends_are_irrelevant, run, stream_cases};
use token_station_protocol::StreamEvent;

const CREATED: &str = "{\"type\":\"response.created\",\"response\":{\"id\":\"r\",\"model\":\"m\"}}";
const DELTA: &str = "{\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"hi\"}";

#[test]
fn chatgpt_plan_streams_read_the_same_under_every_line_end() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    assert_line_ends_are_irrelevant(&ChatGptPlan, &stream_cases(&fixtures, "provider.stream."));
}

#[test]
fn a_frame_closed_by_a_lone_cr_blank_line_is_read_at_once() {
    let mut parser = ChatGptPlan.stream_parser();
    parser
        .parse_chunk(format!("data: {CREATED}\r\r").as_bytes())
        .unwrap();
    let events = parser
        .parse_chunk(format!("data: {DELTA}\r\r").as_bytes())
        .unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
}

#[test]
fn multi_line_data_is_joined_with_line_feeds() {
    let (head, tail) = DELTA.split_at(DELTA.find("\"delta\":").unwrap());
    let wire = format!("data: {CREATED}\n\ndata: {head}\ndata: {tail}\n\n");
    let events = ChatGptPlan
        .stream_parser()
        .parse_chunk(wire.as_bytes())
        .unwrap();
    assert!(
        matches!(&events[..], [StreamEvent::Delta { content, .. }] if content == "hi"),
        "{events:?}"
    );
}

#[test]
fn a_stream_that_ends_inside_a_frame_after_completion_is_refused() {
    let completed = "{\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"model\":\"m\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}";
    let wire = format!("data: {CREATED}\n\ndata: {completed}\n\ndata: {{\"partial");
    let outcome = run(&ChatGptPlan, &[wire.into_bytes()]);
    assert_eq!(
        outcome["error"]["code"], "provider_protocol_error",
        "{outcome}"
    );
    assert_eq!(
        outcome["error"]["message"], "The stream ended inside an SSE frame",
        "{outcome}"
    );
}
