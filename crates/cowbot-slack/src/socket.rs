//! Slack's Socket Mode connection (ADR-0059).
//!
//! Two of Slack's rules shape this module:
//!
//! * the WebSocket address comes from `apps.connections.open`, is single-use and
//!   expires after thirty seconds — so it is fetched immediately before every
//!   dial, never cached;
//! * every envelope is acknowledged individually by echoing its `envelope_id`
//!   within three seconds. An unacknowledged envelope is redelivered, and enough
//!   of them make Slack drop the connection with a `warning` disconnect.
//!
//! That second rule is why the ack is sent *before* the handler runs. The handler
//! runs the pipeline, which can take as long as it likes; the ack cannot wait for
//! it. This is the opposite of a request/response transport and the reason the
//! shared pipeline returns an outcome rather than answering inline.

use std::time::Duration;

use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::api::Api;

/// First wait before reconnecting after a drop.
const RECONNECT_INITIAL: Duration = Duration::from_secs(2);

/// Longest wait between reconnect attempts, so a restored network is noticed
/// within a minute rather than after the backoff has grown to hours.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// One frame Slack sends.
///
/// Only the two envelopes the adapter handles are modelled; `hello`, `disconnect`,
/// and `interactive` are matched by `kind` and read as raw JSON where needed.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    /// `hello`, `events_api`, `interactive`, `disconnect`, `slash_commands`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Present on everything that must be acknowledged.
    #[serde(default)]
    pub envelope_id: Option<String>,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    /// Why Slack is closing the socket, on a `disconnect` frame.
    #[serde(default)]
    pub reason: Option<String>,
}

impl Envelope {
    /// Whether this frame must be acknowledged.
    ///
    /// `hello` and `disconnect` may not be: echoing an envelope id Slack did not
    /// issue is an error on the socket.
    pub fn needs_ack(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "events_api" | "interactive" | "slash_commands"
        ) && self.envelope_id.is_some()
    }

    /// Whether Slack is asking to close, and why.
    ///
    /// Every reason is answered the same way — dial again — but `warning` means
    /// the previous connection was too slow to acknowledge and is worth logging
    /// differently from a routine refresh.
    pub fn disconnect_reason(&self) -> Option<&str> {
        (self.kind == "disconnect").then(|| self.reason.as_deref().unwrap_or("unknown"))
    }
}

/// Run the socket until cancelled, reconnecting after a drop.
///
/// `on_envelope` is called for every envelope that needs handling. Its result is
/// logged and the loop continues: one message the pipeline refused must not take
/// the connection down with it.
pub async fn run<F, Fut>(api: Api, cancel: CancelSignal, mut on_envelope: F) -> CowResult<()>
where
    F: FnMut(Envelope) -> Fut,
    Fut: std::future::Future<Output = CowResult<()>>,
{
    let mut delay = RECONNECT_INITIAL;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        match pump(&api, &cancel, &mut on_envelope).await {
            Ok(Outcome::Cancelled) => return Ok(()),
            Ok(Outcome::Closed(reason)) => {
                if reason == "warning" {
                    // Slack's own diagnosis of a consumer that fell behind its
                    // three-second ack. Worth saying plainly: the fix is the
                    // acknowledgment ordering, not the network.
                    warn!("Slack closed the socket because it was not acknowledged fast enough");
                } else {
                    info!(reason = %reason, "Slack closed the socket; reconnecting");
                }
                // A graceful close is Slack's planned refresh and is retried at
                // once; a drop backs off.
                delay = RECONNECT_INITIAL;
            }
            Err(error) => {
                // A rejected token is reported by `pump` as a configuration error
                // and never reaches here, so everything left is worth a retry.
                warn!(%error, "the Slack socket dropped");
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

/// Open one connection and pump it until it closes or is cancelled.
async fn pump<F, Fut>(api: &Api, cancel: &CancelSignal, on_envelope: &mut F) -> CowResult<Outcome>
where
    F: FnMut(Envelope) -> Fut,
    Fut: std::future::Future<Output = CowResult<()>>,
{
    // Fetched here and not cached: the address is single-use and expires in
    // thirty seconds.
    // A rejected app token is terminal: it never starts working, and retrying
    // hides which token is wrong behind a reconnect loop.
    let url = api.open_connection().await.map_err(|error| {
        if error.is_auth() {
            CowError::Config(format!(
                "Slack rejected the app token ({}); the app-level token needs the                  connections:write scope. Check the integration's configuration",
                error.error
            ))
        } else {
            CowError::from(error)
        }
    })?;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|error| CowError::Llm(format!("slack: could not connect: {error}")))?;
    info!("the Slack socket is connected");

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let _ = socket.close(None).await;
                return Ok(Outcome::Cancelled);
            }
            frame = socket.next() => {
                let Some(frame) = frame else {
                    return Ok(Outcome::Closed("the socket closed".into()));
                };
                let frame = frame
                    .map_err(|error| CowError::Llm(format!("slack: socket error: {error}")))?;
                let Message::Text(text) = frame else {
                    // Ping and pong are answered by the library itself.
                    continue;
                };
                let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
                    debug!("ignoring an unparsable Slack frame");
                    continue;
                };
                if let Some(reason) = envelope.disconnect_reason() {
                    return Ok(Outcome::Closed(reason.to_string()));
                }
                if envelope.kind == "hello" {
                    debug!("the Slack socket said hello");
                    continue;
                }
                if !envelope.needs_ack() {
                    continue;
                }
                // The ack goes first and on its own: Slack gives three seconds
                // and the pipeline can take minutes, so anything else would drop
                // messages under load.
                if let Some(id) = &envelope.envelope_id {
                    let ack = serde_json::json!({ "envelope_id": id }).to_string();
                    if let Err(error) = socket.send(Message::Text(ack.into())).await {
                        return Err(CowError::Llm(format!(
                            "slack: could not acknowledge an envelope: {error}"
                        )));
                    }
                }
                if let Err(error) = on_envelope(envelope.clone()).await {
                    warn!(%error, "the pipeline refused a Slack envelope");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(json: serde_json::Value) -> Envelope {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn an_event_envelope_and_an_interaction_must_both_be_acknowledged() {
        // A missed ack is a redelivery, which the dedupe hides — until Slack
        // drops the connection with `warning`, which reads as a network fault.
        assert!(envelope(serde_json::json!({
            "type": "events_api", "envelope_id": "e1", "payload": {}
        }))
        .needs_ack());
        assert!(envelope(serde_json::json!({
            "type": "interactive", "envelope_id": "e2", "payload": {}
        }))
        .needs_ack());
        // Hello carries no id, and echoing one Slack did not issue is an error.
        assert!(!envelope(serde_json::json!({ "type": "hello" })).needs_ack());
        assert!(!envelope(serde_json::json!({
            "type": "disconnect", "reason": "refresh_requested"
        }))
        .needs_ack());
    }

    #[test]
    fn a_disconnect_carries_its_reason() {
        let warning = envelope(serde_json::json!({ "type": "disconnect", "reason": "warning" }));
        assert_eq!(warning.disconnect_reason(), Some("warning"));

        let refresh = envelope(serde_json::json!({
            "type": "disconnect", "reason": "refresh_requested"
        }));
        assert_eq!(refresh.disconnect_reason(), Some("refresh_requested"));

        // A disconnect without a reason still means reconnect.
        let bare = envelope(serde_json::json!({ "type": "disconnect" }));
        assert_eq!(bare.disconnect_reason(), Some("unknown"));

        assert!(envelope(serde_json::json!({ "type": "hello" }))
            .disconnect_reason()
            .is_none());
    }
}
