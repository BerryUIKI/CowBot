//! Streaming primitives shared by the provider implementations.
//!
//! A provider streams by feeding server-sent events through a decoder and
//! applying each payload to a protocol-specific accumulator. Abort is observed
//! through [`CancelSignal`], a clonable flag with a wakeup, so a stop request
//! drops the in-flight HTTP response instead of waiting for the next chunk
//! (ADR-0041).

pub use cowbot_core::cancel::CancelSignal;

use cowbot_core::error::{CowError, CowResult};

/// Fail with [`CowError::Cancelled`](cowbot_core::error::CowError::Cancelled)
/// when the signal is already tripped.
///
/// Providers call this before starting an upstream request; the streaming loop
/// races [`CancelSignal::cancelled`] instead of polling.
pub fn ensure_live(cancel: &CancelSignal) -> CowResult<()> {
    if cancel.is_cancelled() {
        Err(CowError::Cancelled)
    } else {
        Ok(())
    }
}

/// A single server-sent event frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental decoder for the `text/event-stream` framing used by every
/// streaming protocol CowBot supports.
///
/// Chunks split at arbitrary byte boundaries, so the decoder buffers partial
/// lines and joins consecutive `data:` lines of one frame with newlines.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes and return the frames that completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();

        while let Some((end, separator)) = frame_boundary(&self.buffer) {
            let block = self.buffer[..end].to_vec();
            self.buffer.drain(..end + separator);
            let text = String::from_utf8_lossy(&block);
            if let Some(frame) = parse_block(&text) {
                frames.push(frame);
            }
        }

        frames
    }
}

/// Locate the first blank-line frame separator, tolerating CRLF.
fn frame_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < buffer.len() {
        if buffer[index] == b'\n' {
            if buffer.get(index + 1) == Some(&b'\n') {
                return Some((index, 2));
            }
            if buffer.get(index + 1) == Some(&b'\r') && buffer.get(index + 2) == Some(&b'\n') {
                return Some((index, 3));
            }
        }
        index += 1;
    }
    None
}

fn parse_block(block: &str) -> Option<SseFrame> {
    let mut event = None;
    let mut data = Vec::new();
    for line in block.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value).to_string());
        }
    }
    if event.is_none() && data.is_empty() {
        return None;
    }
    Some(SseFrame {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_on_a_blank_line() {
        let mut decoder = SseDecoder::new();
        let frames = decoder.push(b"event: delta\ndata: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(
            frames,
            vec![
                SseFrame {
                    event: Some("delta".to_string()),
                    data: "{\"a\":1}".to_string()
                },
                SseFrame {
                    event: None,
                    data: "[DONE]".to_string()
                },
            ]
        );
    }

    #[test]
    fn partial_chunks_are_buffered() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"te").is_empty());
        assert!(decoder.push(b"xt\":\"hi\"}").is_empty());
        let frames = decoder.push(b"\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"text\":\"hi\"}");
    }

    #[test]
    fn crlf_framing_is_accepted() {
        let mut decoder = SseDecoder::new();
        let frames = decoder.push(b"event: ping\r\ndata: {}\r\n\r\n");
        assert_eq!(frames[0].event.as_deref(), Some("ping"));
        assert_eq!(frames[0].data, "{}");
    }

    #[test]
    fn multiline_data_is_joined() {
        let mut decoder = SseDecoder::new();
        let frames = decoder.push(b"data: first\ndata: second\n\n");
        assert_eq!(frames[0].data, "first\nsecond");
    }
}
