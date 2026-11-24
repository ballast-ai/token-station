//! Copilot streams read the same under every SSE line end, in all three
//! dialects, with and without a heartbeat before the first frame.

#[path = "../../test-support/sse_line_endings.rs"]
mod sse_line_endings;

use std::path::Path;

use provider_copilot_plan_v2::CopilotPlan;
use south_component_conformance::ProviderComponentV1;
use sse_line_endings::{assert_line_ends_are_irrelevant, run, stream_cases, StreamCase};

const HEARTBEAT: &[u8] = b": heartbeat\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\n";

fn responses_case() -> StreamCase {
    let frames = [
        "{\"type\":\"response.created\",\"response\":{\"id\":\"r\",\"model\":\"m\"}}",
        "{\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"hi\"}",
        "{\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"model\":\"m\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}",
    ];
    let chunks = frames
        .iter()
        .map(|data| format!("data: {data}\n\n").into_bytes())
        .collect();
    ("responses-text".to_owned(), chunks)
}

fn every_dialect() -> Vec<StreamCase> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = stream_cases(&root.join("fixtures"), "provider.stream.");
    cases.extend(stream_cases(&root.join("tests/data"), ""));
    cases.push(responses_case());
    let with_heartbeat: Vec<StreamCase> = cases
        .iter()
        .map(|(name, chunks)| {
            let mut chunks = chunks.clone();
            chunks.insert(0, HEARTBEAT.to_vec());
            (format!("{name} after a heartbeat"), chunks)
        })
        .collect();
    cases.extend(with_heartbeat);
    cases
}

#[test]
fn copilot_streams_read_the_same_under_every_line_end() {
    assert_line_ends_are_irrelevant(&CopilotPlan, &every_dialect());
}

#[test]
fn a_heartbeat_before_the_first_frame_changes_nothing() {
    let cases = every_dialect();
    let (plain, with_heartbeat) = cases.split_at(cases.len() / 2);
    for ((name, plain), (_, with_heartbeat)) in plain.iter().zip(with_heartbeat) {
        assert_eq!(
            run(&CopilotPlan, with_heartbeat),
            run(&CopilotPlan, plain),
            "{name}"
        );
    }
}

#[test]
fn a_first_frame_closed_by_a_lone_cr_blank_line_selects_the_dialect_at_once() {
    let mut parser = CopilotPlan.stream_parser();
    let events = parser
        .parse_chunk(b": heartbeat\r\rdata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\r\r")
        .unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
}

#[test]
fn a_messages_stream_that_ends_inside_a_frame_is_refused() {
    let wire = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_bl";
    let outcome = run(&CopilotPlan, &[wire.as_bytes().to_vec()]);
    assert_eq!(
        outcome["error"]["message"], "Copilot Messages ended inside a stream frame",
        "{outcome}"
    );
}

#[test]
fn a_stream_that_ends_before_its_first_frame_is_refused() {
    let outcome = run(&CopilotPlan, &[b"data: {\"choi".to_vec()]);
    assert_eq!(
        outcome["error"]["message"], "Copilot stream ended without a response",
        "{outcome}"
    );
}

#[test]
fn a_heartbeat_alone_is_an_empty_stream() {
    let mut parser = CopilotPlan.stream_parser();
    assert!(parser.parse_chunk(HEARTBEAT).unwrap().is_empty());
    assert!(parser.parse_chunk(&[]).unwrap().is_empty());
}

#[test]
fn bytes_held_before_the_first_frame_are_bounded_at_four_mib() {
    let partial = [b"data: \"".as_slice(), &vec![b'x'; 4 * 1024 * 1024]].concat();
    let error = CopilotPlan
        .stream_parser()
        .parse_chunk(&partial)
        .unwrap_err();
    assert_eq!(error.message, "Copilot stream frame exceeds its size limit");
}
