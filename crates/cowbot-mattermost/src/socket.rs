//! The Mattermost WebSocket: events in, reconnects handled (ADR-0057).
//!
//! Mattermost's socket is unusual in one way that shapes this module: it
//! authenticates *after* connecting, by sending the token as the first frame.
//! So a connect is not "done" when the handshake completes — it is done when the
//! server answers the auth challenge with a status event. A module that treated
//! the TCP connection as readiness would start reading events that never arrive
//! and look, to an operator, exactly like a bot that is online but deaf.

use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::config::MattermostConfig;

/// First wait before reconnecting after a drop.
const RECONNECT_INITIAL: Duration = Duration::from_secs(2);

/// Longest wait between reconnect attempts.
///
/// Capped so a server that comes back after an outage is noticed within a
/// minute rather than after the backoff has grown to hours.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// The first frame, which carries the token.
#[derive(Debug, serde::Serialize)]
struct Authentication<'a> {
    seq: u32,
    action: &'static str,
    data: AuthData<'a>,
}

#[derive(Debug, serde::Serialize)]
struct AuthData<'a> {
    token: &'a str,
}

/// One event frame from the server.
///
/// Mattermost sends a great many event kinds (typing, presence, channel
/// viewed). Only `posted` becomes a CowBot message, and only a status event
/// matters during the handshake; everything else is dropped here rather than
/// carried through the pipeline.
#[derive(Debug, Deserialize)]
pub struct SocketEvent {
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub broadcast: Option<Broadcast>,
}

#[derive(Debug, Deserialize)]
pub struct Broadcast {
    pub channel_id: Option<String>,
}

/// A `posted` event's payload.
///
/// Mattermost nests the post as a JSON *string* inside the event, because the
/// field predates a typed envelope. Parsing that string is the only way to read
/// it.
#[derive(Debug, Deserialize)]
pub struct PostedData {
    /// The serialized post.
    pub post: String,
    /// The id of the user who wrote it, also available inside `post`.
    #[serde(default)]
    pub sender_name: Option<String>,
}

/// A Mattermost post.
#[derive(Debug, Clone, Deserialize)]
pub struct Post {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub message: String,
    /// The post this one replies to, empty when it starts a thread.
    #[serde(default)]
    pub root_id: String,
    /// Whether this is a system message (join, leave, header change).
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub create_at: i64,
    /// Files attached to this post (ADR-0060).
    ///
    /// Ids only: the post event carries the upload's identity, and the bytes come
    /// from a second, authenticated call. Empty for a post with no attachment.
    #[serde(default)]
    pub file_ids: Vec<String>,
}

impl Post {
    /// Whether the server generated this rather than a person.
    ///
    /// A system post has no meaningful author and answering one would produce a
    /// reply to "user joined the channel".
    pub fn is_system(&self) -> bool {
        !self.kind.is_empty()
    }

    /// The thread this post belongs to, if any.
    pub fn thread_id(&self) -> Option<String> {
        (!self.root_id.is_empty()).then(|| self.root_id.clone())
    }
}

/// Read one `posted` event into a post, if it is one.
pub fn posted(event: &SocketEvent) -> Option<Post> {
    if event.event != "posted" {
        return None;
    }
    let data: PostedData = serde_json::from_value(event.data.clone()).ok()?;
    serde_json::from_str(&data.post).ok()
}

/// Run the socket until cancelled, reconnecting after a drop.
///
/// `on_post` is called for every post that arrives. Returning an error from it
/// is logged and the loop continues: one message the pipeline refused must not
/// take the connection down with it.
pub async fn run<F, Fut>(
    config: MattermostConfig,
    cancel: CancelSignal,
    mut on_post: F,
) -> CowResult<()>
where
    F: FnMut(Post) -> Fut,
    Fut: std::future::Future<Output = CowResult<()>>,
{
    let url = config.socket_url();
    let mut delay = RECONNECT_INITIAL;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        match connect_and_pump(&config, &url, &cancel, &mut on_post).await {
            Ok(()) => {
                // A clean return means the cancel fired.
                return Ok(());
            }
            Err(error) => {
                warn!(%error, url = %url, "the Mattermost socket dropped");
            }
        }

        // A cancelled run must not sit out a backoff before noticing.
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancel.cancelled() => return Ok(()),
        }
        delay = (delay * 2).min(RECONNECT_MAX);
    }
}

/// Connect, authenticate, and pump frames until the socket closes or is
/// cancelled.
async fn connect_and_pump<F, Fut>(
    config: &MattermostConfig,
    url: &str,
    cancel: &CancelSignal,
    on_post: &mut F,
) -> CowResult<()>
where
    F: FnMut(Post) -> Fut,
    Fut: std::future::Future<Output = CowResult<()>>,
{
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|error| CowError::Llm(format!("mattermost: could not connect: {error}")))?;

    // The token goes in the first frame. There is no header to carry it: the
    // WebSocket handshake cannot set an Authorization header from a browser,
    // and Mattermost chose an in-band challenge rather than a query parameter.
    let auth = serde_json::to_string(&Authentication {
        seq: 1,
        action: "authentication_challenge",
        data: AuthData {
            token: &config.bot_token,
        },
    })
    .map_err(CowError::Serialization)?;
    socket
        .send(Message::Text(auth.into()))
        .await
        .map_err(|error| CowError::Llm(format!("mattermost: could not authenticate: {error}")))?;

    info!(url = %url, "the Mattermost socket is connected");

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                // A close frame is polite and lets the server drop its session
                // record at once rather than at a timeout.
                let _ = socket.close(None).await;
                return Ok(());
            }
            frame = socket.next() => {
                let Some(frame) = frame else {
                    return Err(CowError::Llm("mattermost: the socket closed".into()));
                };
                let frame = frame
                    .map_err(|error| CowError::Llm(format!("mattermost: socket error: {error}")))?;
                let Message::Text(text) = frame else {
                    // Ping, pong, and binary frames carry nothing for the
                    // pipeline; tungstenite answers pings itself.
                    continue;
                };
                let Ok(event) = serde_json::from_str::<SocketEvent>(&text) else {
                    debug!("ignoring an unparsable Mattermost frame");
                    continue;
                };
                match event.event.as_str() {
                    "status" => {
                        // The auth challenge's answer. A failure here is worth
                        // reporting as an error rather than retrying blindly:
                        // a revoked token will never start working.
                        let status = event.data.get("status").and_then(|s| s.as_str());
                        if status == Some("FAIL") {
                            return Err(CowError::Llm(
                                "mattermost rejected the bot token; check the integration's \
                                 configuration"
                                    .into(),
                            ));
                        }
                        debug!("the Mattermost socket authenticated");
                    }
                    "posted" => {
                        if let Some(post) = posted(&event) {
                            if let Err(error) = on_post(post).await {
                                warn!(%error, "the pipeline refused a Mattermost post");
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, data: serde_json::Value) -> SocketEvent {
        SocketEvent {
            event: kind.into(),
            data,
            broadcast: None,
        }
    }

    fn post_json(fields: serde_json::Value) -> String {
        serde_json::json!({ "post": fields.to_string() }).to_string()
    }

    #[test]
    fn a_posted_event_yields_its_post() {
        let payload = post_json(serde_json::json!({
            "id": "p1",
            "channel_id": "c1",
            "user_id": "u1",
            "message": "hello",
            "root_id": "",
            "type": "",
            "create_at": 1_700_000_000_000i64,
        }));
        let event = event("posted", serde_json::from_str(&payload).unwrap());
        let post = posted(&event).expect("a posted event parses");
        assert_eq!(post.id, "p1");
        assert_eq!(post.message, "hello");
        assert!(!post.is_system());
        assert_eq!(post.thread_id(), None);
    }

    #[test]
    fn a_reply_carries_its_thread() {
        let payload = post_json(serde_json::json!({
            "id": "p2", "channel_id": "c1", "user_id": "u1",
            "message": "reply", "root_id": "p1",
        }));
        let post = posted(&event("posted", serde_json::from_str(&payload).unwrap())).unwrap();
        assert_eq!(post.thread_id().as_deref(), Some("p1"));
    }

    #[test]
    fn a_system_post_is_recognised() {
        // "user joined the channel" has no author worth answering.
        let payload = post_json(serde_json::json!({
            "id": "p3", "channel_id": "c1", "user_id": "",
            "message": "alice joined", "type": "system_join_channel",
        }));
        let post = posted(&event("posted", serde_json::from_str(&payload).unwrap())).unwrap();
        assert!(post.is_system());
    }

    #[test]
    fn another_event_kind_is_not_a_post() {
        assert!(posted(&event("typing", serde_json::json!({}))).is_none());
        assert!(posted(&event("status", serde_json::json!({ "status": "OK" }))).is_none());
    }

    #[test]
    fn an_unparsable_post_string_is_ignored_rather_than_panicking() {
        let mut bad = event("posted", serde_json::json!({ "post": "{not json" }));
        assert!(posted(&bad).is_none());
        bad.data = serde_json::json!({});
        assert!(posted(&bad).is_none());
    }

    #[test]
    fn the_authentication_frame_names_the_challenge_and_carries_the_token() {
        let frame = serde_json::to_value(&Authentication {
            seq: 1,
            action: "authentication_challenge",
            data: AuthData { token: "abc" },
        })
        .unwrap();
        assert_eq!(frame["action"], "authentication_challenge");
        assert_eq!(frame["data"]["token"], "abc");
        assert_eq!(frame["seq"], 1);
    }
}
