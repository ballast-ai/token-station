//! Code Assist streams read the same under every SSE line end.

#[path = "../../test-support/sse_line_endings.rs"]
mod sse_line_endings;

use std::path::Path;

use provider_codeassist_v2::CodeAssist;
use south_component_conformance::ProviderComponentV1;
use sse_line_endings::{assert_line_ends_are_irrelevant, run, stream_cases};
use token_station_protocol::ErrorCode;

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn code_assist_streams_read_the_same_under_every_line_end() {
    assert_line_ends_are_irrelevant(&CodeAssist, &stream_cases(&fixtures(), "provider.stream."));
}

#[test]
fn a_frame_closed_by_a_lone_cr_blank_line_is_read_at_once() {
    let mut parser = CodeAssist.stream_parser();
    let events = parser
        .parse_chunk(
            b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}}\r\r",
        )
        .unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
}

#[test]
fn multi_line_data_is_joined_with_line_feeds() {
    let split = "data: {\"response\":{\"candidates\":[{\"content\":\ndata: {\"parts\":[{\"text\":\"hi\"}]}}]}}\n\n";
    let events = run(&CodeAssist, &[split.as_bytes().to_vec()]);
    assert_eq!(events["events"][0]["content"], "hi", "{events}");
}

#[test]
fn a_stream_that_ends_inside_a_frame_is_refused() {
    let partial = b"data: {\"response\":{\"candidates\":[]}}\n".to_vec();
    let outcome = run(&CodeAssist, &[partial]);
    assert_eq!(
        outcome["error"]["code"], "provider_protocol_error",
        "{outcome}"
    );
}

#[test]
fn one_event_over_four_mib_is_refused() {
    let mut parser = CodeAssist.stream_parser();
    let frame = [
        b"data: \"".as_slice(),
        &vec![b'x'; 4 * 1024 * 1024],
        b"\"\n\n",
    ]
    .concat();
    let error = parser.parse_chunk(&frame).unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderProtocolError);
}
