//! Feishu's long connection (ADR-0061).
//!
//! The shape is Mattermost's socket's with Feishu's protocol inside: dial out,
//! answer what arrives, notice a dead path, reconnect. What is Feishu's own:
//!
//! * the address is fetched per dial from `/callback/ws/endpoint`, which also
//!   gives the `service_id` every frame must carry;
//! * frames are protobuf ([`crate::frame`]), and a data frame arrives either
//!   whole or fragmented by `message_id` in `seq` order;
//! * the handler has three seconds to answer on the same socket;
//! * a `ping` must go out on an interval and a pong must come back, or a NAT that
//!   silently dropped the path looks exactly like an idle connection.

use std::collections::HashMap;
use std::time::Duration;

use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::api::Api;
use crate::frame::Frame;

/// First wait before reconnecting after a drop.
const RECONNECT_INITIAL: Duration = Duration::from_secs(2);

/// Longest wait between reconnect attempts.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// How long to wait for a pong before treating the path as dead.
///
/// Twice the ping interval plus a margin, which is what the SDKs use: a single
/// missed pong is ordinary, two in a row are a dead connection.
const PONG_GRACE: Duration = Duration::from_secs(5);

/// Run the connection until cancelled, reconnecting after a drop.
pub async fn run<F, Fut>(api: Api, cancel: CancelSignal, mut on_event: F) -> CowResult<()>
where
    F: FnMut(Frame) -> Fut,
    Fut: std::future::Future<Output = CowResult<Option<String>>>,
{
    let mut delay = RECONNECT_INITIAL;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match pump(&api, &cancel, &mut on_event).await {
            Ok(Outcome::Cancelled) => return Ok(()),
            Ok(Outcome::Closed(reason)) => {
                info!(%reason, "the Feishu long connection closed; reconnecting");
                delay = RECONNECT_INITIAL;
            }
            Err(error) => {
                warn!(%error, "the Feishu long connection dropped");
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = cancel.cancelled() => return Ok(()),
                }
                delay = (delay * 2).min(RECONNECT_MAX);
            }
        }
    }
}

enum Outcome {
    Cancelled,
    Closed(String),
}

/// Fragments of a message that has not finished arriving.
#[derive(Default)]
struct Partial {
    pieces: HashMap<u64, Vec<u8>>,
    service: i32,
    log_id: u64,
    seq_id: u64,
    method: i32,
}

async fn pump<F, Fut>(api: &Api, cancel: &CancelSignal, on_event: &mut F) -> CowResult<Outcome>
where
    F: FnMut(Frame) -> Fut,
    Fut: std::future::Future<Output = CowResult<Option<String>>>,
{
    let connection = api.open_connection().await.map_err(|error| {
        if error.is_auth() {
            CowError::Config(format!(
                "Feishu refused the app credentials ({}); check the integration's configuration",
                error.message
            ))
        } else {
            CowError::from(error)
        }
    })?;
    let service = connection.service_id();
    let ping_every = Duration::from_secs(connection.client_config.ping_interval.max(10));

    let (mut socket, _) = tokio_tungstenite::connect_async(&connection.url)
        .await
        .map_err(|error| CowError::Llm(format!("feishu: could not connect: {error}")))?;
    info!(service, "the Feishu long connection is open");

    let mut ping = tokio::time::interval(ping_every);
    // The first tick fires at once; skipping it keeps the connection from
    // pinging before Feishu has finished setting up.
    ping.tick().await;
    let mut deadline = tokio::time::Instant::now() + ping_every + ping_every + PONG_GRACE;
    let mut partial: HashMap<String, Partial> = HashMap::new();

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let _ = socket.close(None).await;
                return Ok(Outcome::Cancelled);
            }
            _ = ping.tick() => {
                let frame = Frame::ping(service);
                if let Err(error) = socket.send(Message::Binary(frame.encode().into())).await {
                    return Ok(Outcome::Closed(format!("could not ping: {error}")));
                }
                if tokio::time::Instant::now() > deadline {
                    // A ping that went unanswered for two intervals means the
                    // path is dead even though the socket is still open.
                    return Ok(Outcome::Closed("no pong arrived".into()));
                }
            }
            frame = socket.next() => {
                let Some(frame) = frame else {
                    return Ok(Outcome::Closed("the socket closed".into()));
                };
                let frame = frame
                    .map_err(|error| CowError::Llm(format!("feishu: socket error: {error}")))?;
                let bytes = match frame {
                    Message::Binary(bytes) => bytes,
                    // Feishu sends binary frames; a text frame is not part of the
                    // protocol and ignoring it is safer than guessing.
                    _ => continue,
                };
                let Some(frame) = Frame::decode(&bytes) else {
                    debug!("ignoring an unparsable Feishu frame");
                    continue;
                };

                match frame.kind() {
                    Some("pong") => {
                        // Proof the path is alive; push the deadline out.
                        deadline = tokio::time::Instant::now() + ping_every + ping_every + PONG_GRACE;
                        continue;
                    }
                    Some("ping") => {
                        let pong = Frame {
                            seq_id: frame.seq_id,
                            method: crate::frame::METHOD_CONTROL,
                            headers: vec![(
                                crate::frame::HEADER_TYPE.to_string(),
                                "pong".to_string(),
                            )],
                            service,
                            ..Frame::default()
                        };
                        let _ = socket.send(Message::Binary(pong.encode().into())).await;
                        continue;
                    }
                    _ => {}
                }
                if frame.method != crate::frame::METHOD_DATA {
                    continue;
                }

                // A fragmented payload is keyed by its message id, which the
                // `message_id` header carries.
                let Some(message_id) = frame.header("message_id").map(str::to_string) else {
                    let _ = answer(&mut socket, &frame, None).await;
                    continue;
                };
                let sum: u64 = frame
                    .header("sum")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                let seq: u64 = frame
                    .header("seq")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                if sum <= 1 {
                    let reply = on_event(frame.clone()).await.unwrap_or(None);
                    let _ = answer(&mut socket, &frame, reply.as_deref()).await;
                    continue;
                }

                let entry = partial.entry(message_id.clone()).or_insert_with(|| Partial {
                    service,
                    log_id: frame.log_id,
                    seq_id: frame.seq_id,
                    method: frame.method,
                    ..Partial::default()
                });
                entry.pieces.insert(seq, frame.payload.clone());
                if entry.pieces.len() as u64 != sum {
                    continue;
                }
                let Some(entry) = partial.remove(&message_id) else {
                    continue;
                };
                let mut ordered = entry.pieces.into_iter().collect::<Vec<_>>();
                ordered.sort_by_key(|(seq, _)| *seq);
                let joined: Vec<u8> = ordered.into_iter().flat_map(|(_, piece)| piece).collect();
                let whole = Frame {
                    seq_id: entry.seq_id,
                    log_id: entry.log_id,
                    service: entry.service,
                    method: entry.method,
                    headers: frame.headers.clone(),
                    payload_encoding: frame.payload_encoding.clone(),
                    payload_type: frame.payload_type.clone(),
                    payload: joined,
                    log_id_new: String::new(),
                };
                let reply = on_event(whole.clone()).await.unwrap_or(None);
                let _ = answer(&mut socket, &whole, reply.as_deref()).await;
            }
        }
    }
}

/// Write the answer to one data frame.
///
/// A handler that fails still answers, with code 500: an unanswered frame shows
/// the person an interaction error and tells Feishu nothing.
async fn answer<S>(socket: &mut S, frame: &Frame, data: Option<&str>) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::fmt::Debug,
{
    // 200 either way: the code says whether Feishu should redeliver, and a
    // handler that failed must not be retried — the message it could not store
    // is already logged, and a redelivery would store it twice at best.
    let response = Frame::response(frame, 200, data);
    socket
        .send(Message::Binary(response.encode().into()))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_fragmented_payload_is_reassembled_in_seq_order() {
        // Feishu splits a large payload and the pieces may arrive out of order;
        // joining them as they arrive would corrupt the JSON.
        let mut pieces: Vec<(u64, Vec<u8>)> = vec![(1, b"world".to_vec()), (0, b"hello ".to_vec())];
        pieces.sort_by_key(|(seq, _)| *seq);
        let joined: Vec<u8> = pieces.into_iter().flat_map(|(_, piece)| piece).collect();
        assert_eq!(String::from_utf8_lossy(&joined), "hello world");
    }
}
