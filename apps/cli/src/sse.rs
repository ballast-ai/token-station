//! Byte-level SSE framing at the network boundary.
//!
//! Provider adapters receive complete UTF-8 frames. Socket chunk boundaries
//! are never treated as character or event boundaries.

use token_station_protocol::{ErrorCode, ErrorEnvelope};

const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct SseFrameDecoder {
    tail: Vec<u8>,
}

impl SseFrameDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ErrorEnvelope> {
        self.tail.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some((end, separator)) = frame_boundary(&self.tail) {
            let raw = self.tail[..end].to_vec();
            self.tail.drain(..end + separator);
            if raw.is_empty() {
                continue;
            }
            if raw.len() > MAX_SSE_FRAME_BYTES {
                return Err(frame_too_large());
            }
            let frame = String::from_utf8(raw).map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCode::ProviderProtocolError,
                    502,
                    "upstream SSE frame is not valid UTF-8",
                )
            })?;
            // The adapter sees one canonical complete frame while retaining
            // compatibility with its standalone incremental fixture contract.
            frames.push(format!("{}\n\n", frame.replace("\r\n", "\n")));
        }
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

fn frame_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (_, Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::SseFrameDecoder;
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
