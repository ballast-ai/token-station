//! Line-end checks shared by the plan components' `sse_line_endings` tests.
//!
//! A host may hand a component SSE bytes with LF, CR or CRLF line ends, mixed
//! in one stream. Each recorded stream is re-spelled with every line-end
//! style and fed whole, byte by byte, and frame by frame. The events, or the
//! refusal, must equal those of the recording. This follows South's
//! `sse_line_endings_v1` for its own components.
//!
//! Each test file includes this file with `#[path]`, because the components
//! are separate workspaces.

use std::path::Path;

use serde_json::{json, Value};
use south_component_conformance::ProviderComponentV1;

/// One recorded stream: its name and its chunks.
pub type StreamCase = (String, Vec<Vec<u8>>);

/// Every `*.input.json` stream recording in `dir` whose name starts with
/// `prefix`. A recording lists its text chunks under `chunks`.
pub fn stream_cases(dir: &Path, prefix: &str) -> Vec<StreamCase> {
    let mut cases: Vec<StreamCase> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(prefix) && name.ends_with(".input.json"))
        .map(|name| {
            let input: Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join(&name)).unwrap()).unwrap();
            let chunks = input["chunks"]
                .as_array()
                .expect("a stream recording lists its chunks")
                .iter()
                .map(|chunk| chunk.as_str().expect("text chunks").as_bytes().to_vec())
                .collect();
            (name, chunks)
        })
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "{} has stream recordings", dir.display());
    cases
}

/// The events of one stream, then its end, or its refusal, as JSON.
pub fn run(component: &dyn ProviderComponentV1, chunks: &[Vec<u8>]) -> Value {
    let mut parser = component.stream_parser();
    let mut events = Vec::new();
    for chunk in chunks
        .iter()
        .map(Vec::as_slice)
        .chain(std::iter::once(&[][..]))
    {
        match parser.parse_chunk(chunk) {
            Ok(more) => events.extend(more),
            Err(error) => return json!({"events": events, "error": error}),
        }
    }
    json!({"events": events})
}

/// `body` with each line end spelled by `ends` in turn.
pub fn respell(body: &[u8], ends: &[&str]) -> Vec<u8> {
    let body = String::from_utf8(body.to_vec()).unwrap();
    let normalised = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalised.len() * 2);
    let mut index = 0;
    for piece in normalised.split_inclusive('\n') {
        if let Some(line) = piece.strip_suffix('\n') {
            out.push_str(line);
            out.push_str(ends[index % ends.len()]);
            index += 1;
        } else {
            out.push_str(piece);
        }
    }
    out.into_bytes()
}

/// The frames a host cuts from `body`: each blank line closes one, under LF,
/// CR or CRLF.
pub fn host_frames(body: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let (mut start, mut line_start, mut at) = (0, 0, 0);
    while at < body.len() {
        let end = match body[at] {
            b'\r' if body.get(at + 1) == Some(&b'\n') => at + 2,
            b'\r' | b'\n' => at + 1,
            _ => {
                at += 1;
                continue;
            }
        };
        if at == line_start {
            frames.push(body[start..end].to_vec());
            start = end;
        }
        line_start = end;
        at = end;
    }
    if start < body.len() {
        frames.push(body[start..].to_vec());
    }
    frames
}

/// Every case reads the same under every line-end style and every feed, and
/// with a leading byte-order mark.
pub fn assert_line_ends_are_irrelevant(component: &dyn ProviderComponentV1, cases: &[StreamCase]) {
    for (case, chunks) in cases {
        let expected = run(component, chunks);
        let body = chunks.concat();
        let spellings: [(&str, &[&str]); 4] = [
            ("lf", &["\n"]),
            ("cr", &["\r"]),
            ("crlf", &["\r\n"]),
            ("mixed", &["\n", "\r", "\r\n"]),
        ];
        for (spelling, ends) in spellings {
            let respelled = respell(&body, ends);
            let bytes: Vec<Vec<u8>> = respelled.iter().map(|byte| vec![*byte]).collect();
            for (feed, fed) in [
                ("whole", vec![respelled.clone()]),
                ("byte by byte", bytes),
                ("frame by frame", host_frames(&respelled)),
            ] {
                let got = run(component, &fed);
                // A chunk that ends in a refusal returns no events, so a body
                // fed whole loses the events before it. Only the refusal is
                // comparable then.
                if feed == "whole" && expected.get("error").is_some() {
                    assert_eq!(
                        got["error"], expected["error"],
                        "{case}: {spelling} fed {feed}"
                    );
                    continue;
                }
                assert_eq!(got, expected, "{case}: {spelling} line ends fed {feed}");
            }
        }
        let with_bom = [b"\xEF\xBB\xBF".as_slice(), &body].concat();
        let got = run(component, &[with_bom]);
        if expected.get("error").is_some() {
            assert_eq!(got["error"], expected["error"], "{case}: byte-order mark");
        } else {
            assert_eq!(got, expected, "{case}: byte-order mark");
        }
    }
}
