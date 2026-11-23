//! The inbound surface for a webhook transport (ADR-0062).
//!
//! Every adapter before this one dialled out, which is why CowBot needed no
//! public URL. A webhook inverts that: the platform POSTs to a URL the operator
//! registered, so the route below is the first thing in this server that accepts
//! a request from an unknown caller with no CowBot session.
//!
//! Three rules shape it, and all three are about failing closed:
//!
//! * **The URL names the integration.** `…/integrations/{id}/webhook` means the
//!   secret checked against the body is that integration's, so verification
//!   never has to work out *whose* body it is holding before it can check it.
//! * **The adapter verifies, the route does not.** A signature, a secret header,
//!   and a decryption are platform-specific; the route hands over the raw bytes
//!   and trusts nothing until [`Integration::webhook`] returns.
//! * **A revoked integration is a 404, not a 401.** A retired bot must not be
//!   driveable, and an attacker who guesses an id should not learn whether it
//!   ever existed.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use cowbot_core::error::CowResult;
use cowbot_core::integration::{IntegrationId, IntegrationRecord};
use cowbot_integrations::adapter::{InboundRequest, WebhookReply};
use cowbot_integrations::sink::{ChannelSink, TurnStart};
use cowbot_service::ChannelService;
use cowbot_store::Store;
use tracing::{debug, warn};

use crate::error::ApiError;
use crate::router::AppState;

/// Everything the route needs to turn a request into an accepted message.
///
/// Built once at startup because it holds what does not vary per request: the
/// store, the adapter registry, the turn starter, and the directory attachments
/// land in. `None` on a deployment with no model provider — the same rule
/// startup uses, because a webhook that stores a question nobody answers is
/// worse than one that is not registered.
#[derive(Clone)]
pub struct WebhookReceiver {
    store: Store,
    bus: cowbot_bus::EventBus,
    /// Started with a stored message that addresses an agent.
    turns: Arc<dyn TurnStart>,
    /// The directory the deployment's file tools are rooted at.
    attachments: Option<std::path::PathBuf>,
}

impl WebhookReceiver {
    pub fn new(
        store: Store,
        bus: cowbot_bus::EventBus,
        turns: Arc<dyn TurnStart>,
        attachments: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            store,
            bus,
            turns,
            attachments,
        }
    }

    /// The adapter for one integration, with its configuration applied.
    ///
    /// Returns `None` for an unknown or revoked integration, and for a kind this
    /// build has no adapter for. All three read the same way to a caller: there
    /// is nothing here to serve.
    async fn adapter_for(
        &self,
        integration_id: &IntegrationId,
    ) -> CowResult<Option<(IntegrationRecord, Arc<dyn cowbot_integrations::Integration>)>> {
        let Some(record) = self.store.get_integration(integration_id).await? else {
            return Ok(None);
        };
        if !record.is_active() {
            // A revoked bot keeps its row so an operator can read what it was
            // bound to; it must not keep accepting traffic.
            return Ok(None);
        }
        let Some(config) = self.store.integration_config(&record.id).await? else {
            return Ok(None);
        };
        // A configuration this build refuses is a row nothing can run, which is
        // the same answer as a kind with no adapter. It is not a failure of this
        // request: the platform's retry would fail identically, so reporting it
        // as a 5xx would only produce a retry loop against a broken row.
        let adapter = match crate::adapters::registry().build(record.kind, &config) {
            Ok(Some(adapter)) => adapter,
            Ok(None) => return Ok(None),
            Err(error) => {
                warn!(
                    integration = %record.id,
                    kind = record.kind.as_str(),
                    %error,
                    "a webhook integration has a configuration this build refuses"
                );
                return Ok(None);
            }
        };
        if !adapter.accepts_webhook() {
            // A row configured for a socket transport has no inbound surface. It
            // is not an error an attacker can trigger usefully, and it is not a
            // success either.
            return Ok(None);
        }
        Ok(Some((record, adapter)))
    }

    /// Handle one delivery.
    pub async fn receive(
        &self,
        integration_id: &IntegrationId,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<WebhookReply, ApiError> {
        let Some((record, adapter)) = self.adapter_for(integration_id).await? else {
            // The same answer for "no such integration", "revoked", and "this
            // build cannot run it": a caller learns nothing it did not already
            // know, and an operator has the startup log for the last one.
            return Err(ApiError::not_found("no integration accepts webhooks here"));
        };

        // The sink is per request because it carries the integration's identity,
        // and a body has to be attributed to the workspace that owns it. It is
        // built the same way startup builds one, so a webhook message and a
        // socket message take one path.
        let sink = Arc::new(
            ChannelSink::new(
                self.store.clone(),
                ChannelService::new(self.store.clone(), self.bus.clone()),
                record.clone(),
            )
            .with_turns(Box::new(SharedTurns(self.turns.clone())))
            .with_adapter(adapter.clone()),
        );
        let ctx = cowbot_integrations::IntegrationContext {
            store: self.store.clone(),
            bus: self.bus.clone(),
            integration: record.clone(),
            sink,
            attachments_dir: self.attachments.clone(),
        };

        match adapter
            .webhook(&ctx, InboundRequest { headers, body })
            .await
        {
            Ok(reply) => {
                debug!(
                    integration = %record.id,
                    kind = record.kind.as_str(),
                    ?reply,
                    "handled a webhook delivery"
                );
                Ok(reply)
            }
            Err(error) => {
                // A refusal is the platform's own verification failing, which is
                // worth a log line and a 401 — not a 500, which would tell the
                // caller to retry.
                warn!(
                    integration = %record.id,
                    kind = record.kind.as_str(),
                    %error,
                    "a webhook delivery was refused"
                );
                Err(ApiError::unauthorized(error.to_string()))
            }
        }
    }
}

impl std::fmt::Debug for WebhookReceiver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebhookReceiver")
            .field("attachments", &self.attachments)
            .finish_non_exhaustive()
    }
}

/// The turn starter, shared with startup rather than duplicated.
struct SharedTurns(Arc<dyn TurnStart>);

impl TurnStart for SharedTurns {
    fn start(&self, channel_id: cowbot_core::channel::ChannelId, message_id: String) {
        self.0.start(channel_id, message_id);
    }
}

/// The route.
///
/// No bearer token: a platform has no CowBot session. Authentication is the
/// adapter's own verification of the body, which is why the body is read as raw
/// bytes rather than extracted as JSON.
pub async fn receive(
    State(state): State<AppState>,
    Path(integration_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let Some(receiver) = state.webhooks.clone() else {
        // A deployment with no turn executor starts no integration either, so
        // this is the same "nothing is running" answer the socket path gives.
        return Err(ApiError::not_found("no integration accepts webhooks here"));
    };

    let flattened: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect();

    let reply = receiver
        .receive(&IntegrationId::new(integration_id), &flattened, &body)
        .await?;

    Ok(match reply {
        WebhookReply::Ack => StatusCode::OK.into_response(),
        WebhookReply::Challenge(body) => {
            // The platform expects its own challenge echoed; the adapter already
            // shaped it, so this is passed through as text.
            (StatusCode::OK, [("content-type", "application/json")], body).into_response()
        }
        WebhookReply::Interaction(body) => match body {
            // Slack ignores the body of an interaction response; Feishu reads a
            // toast from it. Both accept an empty 200.
            Some(body) => {
                (StatusCode::OK, [("content-type", "application/json")], body).into_response()
            }
            None => StatusCode::OK.into_response(),
        },
    })
}

/// A JSON body, for the challenge a Feishu handshake answers with.
pub fn challenge_body(challenge: &str) -> String {
    Json(serde_json::json!({ "challenge": challenge })).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unknown_integration_reads_as_missing() {
        // Not a 401: an attacker who guesses an id learns nothing, and an
        // operator who mistyped the URL sees the same thing a real deployment
        // with no such row would.
        let store = cowbot_store::Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let receiver = WebhookReceiver::new(
            store,
            cowbot_bus::EventBus::default_bus(),
            Arc::new(cowbot_integrations::sink::RecordingTurns::new()),
            None,
        );
        let error = receiver
            .receive(&IntegrationId::new("nope"), &[], b"{}")
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_revoked_integration_does_not_accept_webhooks() {
        use cowbot_core::identity::Workspace;
        use cowbot_core::integration::{IntegrationKind, NewIntegration};

        let store = cowbot_store::Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();
        let new = NewIntegration::new(&workspace.id, IntegrationKind::Telegram, "Bot", "{}");
        store.create_integration(&new).await.unwrap();
        store.revoke_integration(&new.id).await.unwrap();

        let receiver = WebhookReceiver::new(
            store,
            cowbot_bus::EventBus::default_bus(),
            Arc::new(cowbot_integrations::sink::RecordingTurns::new()),
            None,
        );
        // Its row still exists, and it still refuses.
        let error = receiver.receive(&new.id, &[], b"{}").await.unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_challenge_is_echoed_in_the_shape_the_platforms_expect() {
        let body = challenge_body("abc123");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["challenge"], "abc123");
    }
}
