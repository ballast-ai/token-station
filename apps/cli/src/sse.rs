//! Byte-level SSE framing at the network boundary.
//!
//! The grammar is South's `SseDecoderV1`, the decoder the server cuts its
//! upstream frames with. LF, CR and CRLF each end a line and may be mixed in
//! one stream. Every chunk also goes to the decoder, which refuses a line that
//! is not UTF-8 and a line or event over South's bounds, and which decides at
//! the end of input whether the rest holds an event.
//!
//! A frame ends at every blank line, not only at the blank lines that dispatch
//! an event: the component owns the interpretation of the whole stream, so a
//! frame without a `data:` line is handed on at its blank line too. The
//! component gets the upstream's raw frame bytes. Socket chunk boundaries are
//! never treated as character or event boundaries.
//!
//! Each frame also carries the event its blank line dispatched, if any. A host
//! reader that needs the event content, such as accounting or a search stream,
//! reads that event and never splits the raw bytes itself.

use std::collections::VecDeque;

use south_host_grammars::{MAX_SSE_EVENT_BYTES, SseDecoderV1, SseErrorV1, SseEventV1};
use token_station_protocol::{ErrorCode, ErrorEnvelope};

/// The bound on one frame's raw bytes, and on the bytes not yet framed.
///
/// The decoder bounds a line and an event, but it drops comment lines, unknown
/// fields and blank lines that dispatch nothing, while the host keeps the raw
/// bytes for the component. Without this bound, a stream that never sends a
/// blank line would grow the buffer without end. The server and South use the
/// same number.
const MAX_SSE_FRAME_BYTES: usize = MAX_SSE_EVENT_BYTES;

/// One frame: the upstream's raw bytes up to and including a blank line, and
/// the event that blank line dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseFrame {
    pub(crate) raw: String,
    /// `None` when the frame has no `data` field: a comment, or only `event`
    /// or `id` lines.
    pub(crate) event: Option<SseEventV1>,
}

#[derive(Default)]
pub(crate) struct SseFrameDecoder {
    decoder: SseDecoderV1,
    /// The events the decoder dispatched that no frame took yet, each with the
    /// stream offset of the end of the blank line that dispatched it.
    events: VecDeque<(u64, SseEventV1)>,
    /// The stream offset of `pending[0]`.
    offset: u64,
    /// Raw bytes received after the end of the last frame.
    pending: Vec<u8>,
    /// Where the next line in `pending` starts. Every line before it was read
    /// and was not blank.
    line_start: usize,
    /// Where the search for the next line end resumes, so a long line that
    /// arrives in many chunks is scanned once.
    searched: usize,
}

impl SseFrameDecoder {
    /// Feeds one chunk of upstream bytes and returns the frames it completes.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseFrame>, ErrorEnvelope> {
        self.decoder.push(bytes);
        while let Some(event) = self.decoder.next_event().map_err(refusal)? {
            // Right after an event, the decoder's position is the end of the
            // blank line that dispatched it: the end of a frame.
            self.events.push_back((self.decoder.position(), event));
        }
        self.pending.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(end) = self.next_frame_end() {
            if end > MAX_SSE_FRAME_BYTES {
                return Err(frame_too_large());
            }
            let raw: Vec<u8> = self.pending.drain(..end).collect();
            self.line_start = 0;
            self.searched = 0;
            self.offset += end as u64;
            // Both cut lines with the same rules, so a frame takes the event
            // its own blank line dispatched, and no other.
            let event = if self
                .events
                .front()
                .is_some_and(|(at, _)| *at == self.offset)
            {
                self.events.pop_front().map(|(_, event)| event)
            } else {
                None
            };
            debug_assert!(self.events.front().is_none_or(|(at, _)| *at > self.offset));
            // A frame of line ends only carries nothing. It is not handed on.
            if raw.iter().all(|byte| matches!(byte, b'\r' | b'\n')) {
                continue;
            }
            let raw = String::from_utf8(raw).map_err(|_| not_utf8())?;
            frames.push(SseFrame { raw, event });
        }
        if self.pending.len() > MAX_SSE_FRAME_BYTES {
            return Err(frame_too_large());
        }
        Ok(frames)
    }

    /// Reads `pending` line by line from `line_start`, with the SSE line ends,
    /// and returns the end of the first blank line: the end of a frame.
    fn next_frame_end(&mut self) -> Option<usize> {
        loop {
            let from = self.searched.max(self.line_start);
            let Some(offset) = self.pending[from..]
                .iter()
                .position(|byte| matches!(byte, b'\r' | b'\n'))
            else {
                self.searched = self.pending.len();
                return None;
            };
            let at = from + offset;
            let terminator = match (self.pending[at], self.pending.get(at + 1)) {
                // Maybe the first half of a CRLF: wait for the next byte, or
                // for `finish`.
                (b'\r', None) => {
                    self.searched = at;
                    return None;
                }
                (b'\r', Some(b'\n')) => 2,
                _ => 1,
            };
            let blank = at == self.line_start;
            self.line_start = at + terminator;
            self.searched = self.line_start;
            if blank {
                return Some(self.line_start);
            }
        }
    }

    /// Ends the input. Returns the last frame when a blank line made of a
    /// final lone CR closes it, since only the end of input shows that the CR
    /// is not the first half of a CRLF.
    ///
    /// An event or a field line that no blank line closes is a truncated
    /// stream. The component never sees it. A rest of whitespace or comments
    /// ends the stream cleanly.
    pub(crate) fn finish(self) -> Result<Option<SseFrame>, ErrorEnvelope> {
        let rest = self.pending;
        match self.decoder.finish().map_err(refusal)? {
            Some(event) if ends_with_blank_line(&rest) => {
                let raw = String::from_utf8(rest).map_err(|_| not_utf8())?;
                Ok(Some(SseFrame {
                    raw,
                    event: Some(event),
                }))
            }
            Some(_) => Err(truncated()),
            None if has_field_line(&rest) => Err(truncated()),
            None => Ok(None),
        }
    }
}

/// Whether the rest ends with a blank line. `push` cuts at every blank line it
/// can read, so the only blank line left is a final lone CR whose line before
/// it already ended.
fn ends_with_blank_line(rest: &[u8]) -> bool {
    matches!(rest, [.., b'\n' | b'\r', b'\r'])
}

/// Whether the rest holds a field line: a line that is not blank after
/// trimming and does not start with `:`. Called only when the decoder found no
/// event in the rest, to tell comments and whitespace from a cut-off frame.
fn has_field_line(rest: &[u8]) -> bool {
    rest.split(|byte| matches!(byte, b'\r' | b'\n'))
        .map(<[u8]>::trim_ascii)
        .any(|line| !line.is_empty() && !line.starts_with(b":"))
}

fn refusal(error: SseErrorV1) -> ErrorEnvelope {
    match error {
        SseErrorV1::NotUtf8 => not_utf8(),
        SseErrorV1::LineTooLong
        | SseErrorV1::EventTooLarge
        | SseErrorV1::TooManyEvents
        | SseErrorV1::UndrainedEvent => frame_too_large(),
    }
}

fn not_utf8() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCode::ProviderProtocolError,
        502,
        "upstream SSE frame is not valid UTF-8",
    )
}

fn frame_too_large() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCode::ProviderProtocolError,
        502,
        "upstream SSE frame exceeds the local framing limit",
    )
}

fn truncated() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCode::TransportTruncated,
        502,
        "upstream SSE connection closed with an incomplete frame",
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_SSE_FRAME_BYTES, SseFrame, SseFrameDecoder};
    use south_host_grammars::{SseEventV1, decode_sse_v1};
    use token_station_protocol::{ErrorCode, ErrorEnvelope};

    /// The frames of `chunks`, then the end of input.
    fn frames(chunks: &[&[u8]]) -> (Vec<SseFrame>, Result<Option<SseFrame>, ErrorEnvelope>) {
        let mut decoder = SseFrameDecoder::default();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(decoder.push(chunk).expect("valid SSE"));
        }
        (out, decoder.finish())
    }

    fn raw(frames: impl IntoIterator<Item = SseFrame>) -> Vec<String> {
        frames.into_iter().map(|frame| frame.raw).collect()
    }

    #[test]
    fn lf_crlf_and_multiple_frames_are_handed_on_as_raw_bytes() {
        let mut decoder = SseFrameDecoder::default();
        let frames = raw(decoder
            .push(b"data:{\"a\":1}\n\ndata: {\"b\":2}\r\n\r\ndata: [DONE]\n\n")
            .unwrap());
        assert_eq!(
            frames,
            [
                "data:{\"a\":1}\n\n",
                "data: {\"b\":2}\r\n\r\n",
                "data: [DONE]\n\n"
            ]
        );
        assert_eq!(decoder.finish().unwrap(), None);
    }

    #[test]
    fn split_frame_and_three_byte_character_wait_for_completion() {
        let mut decoder = SseFrameDecoder::default();
        let bytes = "data: 你\n\n".as_bytes();
        assert!(decoder.push(&bytes[..7]).unwrap().is_empty());
        assert!(decoder.push(&bytes[7..8]).unwrap().is_empty());
        assert_eq!(raw(decoder.push(&bytes[8..]).unwrap()), ["data: 你\n\n"]);
    }

    #[test]
    fn four_byte_emoji_can_split_at_every_byte() {
        let mut decoder = SseFrameDecoder::default();
        let bytes = "data: 😀\n\n".as_bytes();
        let mut frames = Vec::new();
        for byte in bytes {
            frames.extend(raw(decoder.push(&[*byte]).unwrap()));
        }
        assert_eq!(frames, ["data: 😀\n\n"]);
    }

    #[test]
    fn every_delimiter_split_preserves_the_frame() {
        for separator in ["\n\n", "\r\n\r\n", "\r\r", "\n\r\n", "\r\n\n"] {
            for split in 0..=separator.len() {
                let mut decoder = SseFrameDecoder::default();
                let mut first = b"data: \xe4\xbd\xa0".to_vec();
                first.extend_from_slice(&separator.as_bytes()[..split]);
                let mut frames = decoder.push(&first).unwrap();
                frames.extend(decoder.push(&separator.as_bytes()[split..]).unwrap());
                let last = decoder.finish().unwrap();
                frames.extend(last);
                assert_eq!(
                    frames,
                    [SseFrame {
                        raw: format!("data: 你{separator}"),
                        event: Some(SseEventV1::new("message", "你")),
                    }],
                    "{separator:?}"
                );
            }
        }
    }

    #[test]
    fn multiple_frames_and_partial_tail_survive_every_chunk_cut() {
        let wire = b"\n\ndata: first\r\n\r\ndata: second\n\ndata: tail";
        for split in 0..=wire.len() {
            let mut decoder = SseFrameDecoder::default();
            let mut frames = decoder.push(&wire[..split]).unwrap();
            frames.extend(decoder.push(&wire[split..]).unwrap());
            assert_eq!(raw(frames), ["data: first\r\n\r\n", "data: second\n\n"]);
            assert_eq!(
                raw(decoder.push(b"\r\n\r\n").unwrap()),
                ["data: tail\r\n\r\n"]
            );
            assert_eq!(decoder.finish().unwrap(), None);
        }
    }

    /// Every spelling of the line ends, fed whole, byte by byte and at every
    /// cut, gives frames that decode one by one to the stream's own events.
    #[test]
    fn line_ends_and_chunking_do_not_change_the_frames() {
        let lines = [
            "event: message_start",
            "data: {\"a\":1}",
            "",
            ": keep-alive",
            "",
            "data: {\"b\":",
            "data: 2}",
            "",
            "id: 7",
            "data: [DONE]",
            "",
        ];
        for ends in [
            &["\n"][..],
            &["\r"],
            &["\r\n"],
            &["\n", "\r", "\r\n"],
            // Never a CR line end right before an LF-ended blank line: the two
            // would read as one CRLF.
            &["\n", "\r\n", "\r"],
        ] {
            let mut body = String::new();
            for (index, line) in lines.iter().enumerate() {
                body.push_str(line);
                body.push_str(ends[index % ends.len()]);
            }
            let expected = decode_sse_v1(body.as_bytes()).expect("valid SSE");
            assert_eq!(expected.len(), 3);
            let bytes = body.as_bytes();
            let mut feeds: Vec<Vec<&[u8]>> = vec![vec![bytes]];
            feeds.push(bytes.chunks(1).collect());
            for cut in 1..bytes.len() {
                feeds.push(vec![&bytes[..cut], &bytes[cut..]]);
            }
            for feed in feeds {
                let (mut cut, end) = frames(&feed);
                cut.extend(end.expect("a clean end"));
                let raws: Vec<&str> = cut.iter().map(|frame| frame.raw.as_str()).collect();
                assert_eq!(raws.concat(), body, "{ends:?}");
                // Each frame carries exactly the event its own bytes decode to.
                for frame in &cut {
                    let own = decode_sse_v1(frame.raw.as_bytes()).expect("one frame");
                    assert_eq!(frame.event.iter().cloned().collect::<Vec<_>>(), own);
                }
                let events: Vec<SseEventV1> =
                    cut.iter().filter_map(|frame| frame.event.clone()).collect();
                assert_eq!(events, expected, "{ends:?}");
                // The comment frame is handed on too: the component owns it.
                assert_eq!(cut.len(), 4, "{ends:?}");
            }
        }
    }

    #[test]
    fn a_leading_byte_order_mark_stays_with_the_first_frame() {
        let (cut, end) = frames(&[b"\xEF\xBB\xBFdata: 1\r\rdata: 2\r\r"]);
        assert_eq!(cut[0].event, Some(SseEventV1::new("message", "1")));
        assert_eq!(raw(cut), ["\u{feff}data: 1\r\r"]);
        // The final CR could start a CRLF, so only the end of input closes it.
        let end = end.unwrap().expect("the last frame");
        assert_eq!(end.raw, "data: 2\r\r");
        assert_eq!(end.event, Some(SseEventV1::new("message", "2")));
    }

    #[test]
    fn a_last_event_without_a_blank_line_is_truncated() {
        for tail in [&b"data: 2"[..], b"data: 2\n", b"data: 2\r", b"data: 2\r\n"] {
            let (cut, end) = frames(&[b"data: 1\n\n", tail]);
            assert_eq!(raw(cut), ["data: 1\n\n"]);
            assert_eq!(
                end.unwrap_err().code,
                ErrorCode::TransportTruncated,
                "{tail:?}"
            );
        }
    }

    #[test]
    fn a_blank_line_ended_by_a_final_cr_completes_the_last_frame() {
        for tail in ["data: 2\n\r", "data: 2\r\r", "data: 2\r\n\r"] {
            let (_, end) = frames(&[b"data: 1\n\n", tail.as_bytes()]);
            assert_eq!(
                end.unwrap().map(|frame| frame.raw).as_deref(),
                Some(tail),
                "{tail:?}"
            );
        }
    }

    #[test]
    fn trailing_whitespace_and_comments_end_cleanly() {
        for tail in [&b"\n"[..], b" ", b"\r\n ", b": keep-alive\n\n", b": bye"] {
            let (_, end) = frames(&[b"data: 1\n\n", tail]);
            assert_eq!(end.unwrap(), None, "{tail:?}");
        }
    }

    #[test]
    fn an_event_cut_off_before_its_data_is_truncated() {
        let (_, end) = frames(&[b"data: 1\n\nevent: message_delta\n"]);
        assert_eq!(end.unwrap_err().code, ErrorCode::TransportTruncated);
    }

    #[test]
    fn frame_and_pending_tail_limits_are_enforced() {
        for separator in [b"\n\n".as_slice(), b"\r\n\r\n".as_slice(), b"\r\r"] {
            let prefix = b"data: ";
            let filler = MAX_SSE_FRAME_BYTES - prefix.len() - separator.len();
            let mut at_limit = prefix.to_vec();
            at_limit.resize(prefix.len() + filler, b'x');
            at_limit.extend_from_slice(separator);
            let mut decoder = SseFrameDecoder::default();
            let mut frames = decoder.push(&at_limit).unwrap();
            frames.extend(decoder.finish().unwrap());
            assert_eq!(frames[0].raw.len(), MAX_SSE_FRAME_BYTES);

            let mut oversized = b"data: x".to_vec();
            oversized.extend_from_slice(&at_limit[prefix.len()..]);
            let error = SseFrameDecoder::default().push(&oversized).unwrap_err();
            assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        }
        // Comment lines dispatch nothing, so only the frame bound stops them.
        let mut decoder = SseFrameDecoder::default();
        let line = format!(": {}\n", "x".repeat(1022));
        let mut refused = None;
        for _ in 0..=(MAX_SSE_FRAME_BYTES / line.len()) {
            if let Err(error) = decoder.push(line.as_bytes()) {
                refused = Some(error);
                break;
            }
        }
        assert_eq!(
            refused.expect("unframed bytes over the bound").code,
            ErrorCode::ProviderProtocolError
        );
    }

    #[test]
    fn large_single_byte_schedule_preserves_all_bytes() {
        let text = format!("data: {}", "x".repeat(128 * 1024));
        let wire = format!("{text}\r\n\r\n");
        let mut decoder = SseFrameDecoder::default();
        let mut frames = Vec::new();
        for byte in wire.bytes() {
            frames.extend(raw(decoder.push(&[byte]).unwrap()));
        }
        assert_eq!(frames, [wire]);
        assert_eq!(decoder.finish().unwrap(), None);
    }

    #[test]
    fn large_multi_frame_chunk_keeps_the_order_and_final_tail() {
        let mut wire = "data: item\n\n".repeat(16 * 1024);
        wire.push_str("data: tail");
        let mut decoder = SseFrameDecoder::default();
        let frames = decoder.push(wire.as_bytes()).unwrap();
        assert_eq!(frames.len(), 16 * 1024);
        assert!(frames.iter().all(|frame| frame.raw == "data: item\n\n"));
        assert_eq!(
            decoder.finish().unwrap_err().code,
            ErrorCode::TransportTruncated
        );
    }

    #[test]
    fn invalid_utf8_is_a_provider_protocol_error() {
        let mut decoder = SseFrameDecoder::default();
        let error = decoder.push(&[b'd', b'a', b't', b'a', b':', 0xff, b'\n', b'\n']);
        assert_eq!(error.unwrap_err().code, ErrorCode::ProviderProtocolError);
    }

    #[test]
    fn nonempty_eof_tail_is_transport_truncated() {
        let mut decoder = SseFrameDecoder::default();
        decoder.push(b"data: {\"half\":").unwrap();
        assert_eq!(
            decoder.finish().unwrap_err().code,
            ErrorCode::TransportTruncated
        );
    }
}
