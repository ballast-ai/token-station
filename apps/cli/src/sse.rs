//! Byte-level SSE framing at the network boundary.
//!
//! Provider adapters receive complete UTF-8 frames. Socket chunk boundaries
//! are never treated as character or event boundaries.

use token_station_protocol::{ErrorCode, ErrorEnvelope};

const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct SseFrameDecoder {
    tail: Vec<u8>,
    scanned: usize,
}

impl SseFrameDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ErrorEnvelope> {
        self.tail.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut consumed = 0;
        for index in self.scanned..self.tail.len() {
            if self.tail[index] != b'\n' {
                continue;
            }
            let end = if index > consumed && self.tail[index - 1] == b'\n' {
                index - 1
            } else if index >= consumed + 3 && self.tail[index - 3..index] == *b"\r\n\r" {
                index - 3
            } else {
                continue;
            };
            let raw = &self.tail[consumed..end];
            consumed = index + 1;
            if raw.is_empty() {
                continue;
            }
            if raw.len() > MAX_SSE_FRAME_BYTES {
                return Err(frame_too_large());
            }
            let frame = std::str::from_utf8(raw).map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCode::ProviderProtocolError,
                    502,
                    "upstream SSE frame is not valid UTF-8",
                )
            })?;
            // The adapter sees one canonical complete frame while retaining
            // compatibility with its standalone incremental fixture contract.
            let mut frame = frame.replace("\r\n", "\n");
            frame.push_str("\n\n");
            frames.push(frame);
        }
        self.tail.drain(..consumed);
        self.scanned = self.tail.len();
        if self.tail.len() > MAX_SSE_FRAME_BYTES {
            return Err(frame_too_large());
        }
        Ok(frames)
    }

    pub(crate) fn finish(self) -> Result<(), ErrorEnvelope> {
        if self
            .tail
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            return Ok(());
        }
        Err(ErrorEnvelope::new(
            ErrorCode::TransportTruncated,
            502,
            "upstream SSE connection closed with an incomplete frame",
        ))
    }
}

fn frame_too_large() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCode::ProviderProtocolError,
        502,
        "upstream SSE frame exceeds the local framing limit",
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_SSE_FRAME_BYTES, SseFrameDecoder};
    use token_station_protocol::ErrorCode;

    #[test]
    fn lf_crlf_and_multiple_frames_are_canonicalized() {
        let mut decoder = SseFrameDecoder::default();
        let frames = decoder
            .push(b"data:{\"a\":1}\n\ndata: {\"b\":2}\r\n\r\ndata: [DONE]\n\n")
            .unwrap();
        assert_eq!(
            frames,
            [
                "data:{\"a\":1}\n\n",
                "data: {\"b\":2}\n\n",
                "data: [DONE]\n\n"
            ]
        );
        decoder.finish().unwrap();
    }

    #[test]
    fn split_frame_and_three_byte_character_wait_for_completion() {
        let mut decoder = SseFrameDecoder::default();
        let bytes = "data: 你\n\n".as_bytes();
        assert!(decoder.push(&bytes[..7]).unwrap().is_empty());
        assert!(decoder.push(&bytes[7..8]).unwrap().is_empty());
        assert_eq!(decoder.push(&bytes[8..]).unwrap(), ["data: 你\n\n"]);
    }

    #[test]
    fn four_byte_emoji_can_split_at_every_byte() {
        let mut decoder = SseFrameDecoder::default();
        let bytes = "data: 😀\n\n".as_bytes();
        let mut frames = Vec::new();
        for byte in bytes {
            frames.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(frames, ["data: 😀\n\n"]);
    }

    #[test]
    fn every_lf_and_crlf_delimiter_split_preserves_the_frame() {
        for separator in ["\n\n", "\r\n\r\n"] {
            for split in 0..=separator.len() {
                let mut decoder = SseFrameDecoder::default();
                let mut first = b"data: \xe4\xbd\xa0".to_vec();
                first.extend_from_slice(&separator.as_bytes()[..split]);
                let mut frames = decoder.push(&first).unwrap();
                frames.extend(decoder.push(&separator.as_bytes()[split..]).unwrap());
                assert_eq!(frames, ["data: 你\n\n"]);
                decoder.finish().unwrap();
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
            assert_eq!(frames, ["data: first\n\n", "data: second\n\n"]);
            assert_eq!(decoder.push(b"\r\n\r\n").unwrap(), ["data: tail\n\n"]);
            decoder.finish().unwrap();
        }
    }

    #[test]
    fn frame_and_pending_tail_limits_are_enforced() {
        for separator in [b"\n\n".as_slice(), b"\r\n\r\n".as_slice()] {
            let mut at_limit = vec![b'x'; MAX_SSE_FRAME_BYTES];
            at_limit.extend_from_slice(separator);
            let mut decoder = SseFrameDecoder::default();
            assert_eq!(
                decoder.push(&at_limit).unwrap()[0].len(),
                MAX_SSE_FRAME_BYTES + 2
            );
            decoder.finish().unwrap();

            let mut oversized = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
            oversized.extend_from_slice(separator);
            assert_eq!(
                SseFrameDecoder::default()
                    .push(&oversized)
                    .unwrap_err()
                    .code,
                ErrorCode::ProviderProtocolError
            );
        }
        let mut decoder = SseFrameDecoder::default();
        assert!(
            decoder
                .push(&vec![b'x'; MAX_SSE_FRAME_BYTES])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            decoder.push(b"x").unwrap_err().code,
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
            frames.extend(decoder.push(&[byte]).unwrap());
        }
        assert_eq!(frames, [format!("{text}\n\n")]);
        decoder.finish().unwrap();
    }

    #[test]
    fn large_multi_frame_chunk_keeps_the_order_and_final_tail() {
        let mut wire = "data: item\n\n".repeat(16 * 1024);
        wire.push_str("data: tail");
        let mut decoder = SseFrameDecoder::default();
        let frames = decoder.push(wire.as_bytes()).unwrap();
        assert_eq!(frames.len(), 16 * 1024);
        assert!(frames.iter().all(|frame| frame == "data: item\n\n"));
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
