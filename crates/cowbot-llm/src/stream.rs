//! Streaming primitives shared by the provider implementations.
//!
//! A provider streams by feeding server-sent events through a decoder and
//! applying each payload to a protocol-specific accumulator. Abort is observed
//! through [`CancelSignal`], a clonable flag with a wakeup, so a stop request
//! drops the in-flight HTTP response instead of waiting for the next chunk
//! (ADR-0039).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cowbot_core::error::{CowError, CowResult};
use tokio::sync::Notify;

/// A clonable, one-way cancellation flag.
///
/// Cancelling is idempotent and wakes every waiter, so a stopped turn aborts
/// the upstream request and any wait around it.
#[derive(Clone, Default)]
pub struct CancelSignal {
    state: Arc<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop the work this signal guards. Repeated calls are no-ops.
    pub fn cancel(&self) {
        if !self.state.cancelled.swap(true, Ordering::SeqCst) {
            self.state.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Resolve as soon as the signal is cancelled.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            // Register before the check so a cancellation between the two can
            // not be missed.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Fail with [`CowError::Cancelled`] when the signal is already tripped.
    pub fn ensure_live(&self) -> CowResult<()> {
        if self.is_cancelled() {
            Err(CowError::Cancelled)
        } else {
            Ok(())
        }
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

        loop {
            let Some((end, separator)) = frame_boundary(&self.buffer) else {
                break;
            };
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
    use std::time::Duration;

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

    #[tokio::test]
    async fn cancel_wakes_waiters_and_is_idempotent() {
        let signal = CancelSignal::new();
        assert!(!signal.is_cancelled());
        let waiter = signal.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
            true
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        signal.cancel();
        signal.cancel();
        assert!(signal.is_cancelled());
        assert!(task.await.unwrap());
        assert!(signal.ensure_live().is_err());
    }

    #[tokio::test]
    async fn cancel_after_waiting_still_resolves() {
        let signal = CancelSignal::new();
        signal.cancel();
        tokio::time::timeout(Duration::from_secs(1), signal.cancelled())
            .await
            .expect("an already cancelled signal resolves immediately");
    }
}
