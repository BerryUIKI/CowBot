//! Running one integration: its inbound loop beside its outbound mirror
//! (ADR-0056).
//!
//! The two directions are separate tasks on purpose. An adapter's inbound loop
//! is a platform-specific socket or poll that fails in platform-specific ways,
//! while the mirror is one bus subscription doing the same thing for every
//! platform; running them together would make a reconnect delay the mirror and a
//! slow `deliver` stall the next inbound message.

use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::integration::IntegrationRecord;
use cowbot_store::Store;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::adapter::{Integration, IntegrationContext, MessageSink, Registry};
use crate::outbound;

/// One running integration: its adapter and the task driving it.
pub struct Running {
    pub integration: IntegrationRecord,
    kind: cowbot_core::integration::IntegrationKind,
    cancel: CancelSignal,
}

impl Running {
    pub fn kind(&self) -> cowbot_core::integration::IntegrationKind {
        self.kind
    }

    /// Stop this integration. Idempotent; the tasks exit at their own pace.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// Start every active integration in a workspace, and the mirror for each.
///
/// Returns what started and what could not, rather than failing the whole set:
/// one misconfigured bot must not stop the other three. A kind this build has no
/// adapter for is reported the same way, because the row can exist before the
/// build that runs it.
pub async fn start_workspace(
    store: &Store,
    bus: &EventBus,
    registry: &Registry,
    sink: Arc<dyn MessageSink>,
    workspace_id: &cowbot_core::identity::WorkspaceId,
) -> CowResult<Started> {
    let mut started = Started::default();
    for record in store.list_active_integrations(workspace_id).await? {
        // Configuration is read only for an adapter that will run, so a revoked
        // or unsupported row never decrypts anything.
        let config = match store.integration_config(&record.id).await? {
            Some(config) => config,
            None => continue,
        };
        let Some(adapter) = registry.build(record.kind, &config)? else {
            debug!(
                integration = %record.id,
                kind = record.kind.as_str(),
                "No adapter in this build for the integration kind"
            );
            started.unsupported.push(record.id.clone());
            continue;
        };
        started.running.push(start(
            store.clone(),
            bus.clone(),
            record,
            adapter,
            sink.clone(),
        ));
    }
    Ok(started)
}

/// What [`start_workspace`] managed to do.
#[derive(Default, Debug)]
pub struct Started {
    pub running: Vec<Running>,
    /// Integrations whose kind has no adapter in this build.
    pub unsupported: Vec<cowbot_core::integration::IntegrationId>,
}

impl Started {
    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.unsupported.is_empty()
    }

    /// Stop every integration this call started.
    pub fn stop_all(&self) {
        for running in &self.running {
            running.stop();
        }
    }
}

/// Start one integration: its adapter and its mirror.
pub fn start(
    store: Store,
    bus: EventBus,
    record: IntegrationRecord,
    adapter: Arc<dyn Integration>,
    sink: Arc<dyn MessageSink>,
) -> Running {
    let cancel = CancelSignal::new();
    let context = IntegrationContext {
        store: store.clone(),
        integration: record.clone(),
        sink,
    };

    let inbound_adapter = adapter.clone();
    let inbound_cancel = cancel.clone();
    let inbound_record = record.clone();
    tokio::spawn(async move {
        let kind = inbound_adapter.kind();
        match inbound_adapter.run(context, inbound_cancel).await {
            Ok(()) => {
                info!(
                    integration = %inbound_record.id,
                    kind = kind.as_str(),
                    "An IM integration stopped"
                );
            }
            Err(error) => {
                // Reported as a warning and not retried here: a platform's own
                // reconnect logic is the adapter's, and a supervisor loop around
                // it would hide which one is failing.
                warn!(
                    integration = %inbound_record.id,
                    kind = kind.as_str(),
                    error = %error,
                    "An IM integration stopped with an error"
                );
            }
        }
    });

    let mirror = Mirror {
        store,
        events: bus.subscribe(),
        integration: record.clone(),
        adapter: adapter.clone(),
    };
    let mirror_cancel = cancel.clone();
    tokio::spawn(async move {
        mirror.run(mirror_cancel).await;
    });

    Running {
        kind: record.kind,
        integration: record,
        cancel,
    }
}

/// The outbound half: bus events in, platform posts out.
struct Mirror {
    store: Store,
    /// Taken synchronously by [`start`], before the task runs.
    ///
    /// Subscribing inside the task would leave a window between `start`
    /// returning and the subscription existing, and an agent reply in that
    /// window would never reach the IM.
    events: tokio::sync::broadcast::Receiver<cowbot_core::event::Event>,
    integration: IntegrationRecord,
    adapter: Arc<dyn Integration>,
}

impl Mirror {
    async fn run(mut self, cancel: CancelSignal) {
        loop {
            tokio::select! {
                // Woken the instant the integration is stopped, rather than at
                // the next event, so a quiet channel does not delay shutdown.
                () = cancel.cancelled() => break,
                received = self.events.recv() => match received {
                    Ok(event) => {
                        if let Err(error) = self.mirror_one(&event.payload).await {
                            warn!(
                                integration = %self.integration.id,
                                error = %error,
                                "Failed to mirror a channel message to the IM"
                            );
                        }
                    }
                    // Falling behind is ordinary on a busy bus. The message is
                    // still in the channel, so only the IM copy is missed; a
                    // reply the IM did not see is not worth stopping for.
                    Err(RecvError::Lagged(skipped)) => {
                        warn!(
                            integration = %self.integration.id,
                            skipped,
                            "The IM mirror fell behind the event bus"
                        );
                    }
                    Err(RecvError::Closed) => break,
                },
            }
        }
    }

    async fn mirror_one(&self, payload: &cowbot_core::event::EventPayload) -> CowResult<()> {
        let Some(message) = outbound::mirrorable(payload) else {
            return Ok(());
        };
        let posts = outbound::mirror_posts(&self.store, &self.integration, message).await?;
        for post in posts {
            // One failed chunk does not stop the rest: the answer is already in
            // CowBot, and a half-delivered long reply is better than none of it.
            if let Err(error) = self.adapter.deliver(&post.target, &post.content).await {
                warn!(
                    integration = %self.integration.id,
                    channel = %message.channel_id.0,
                    part = post.content.part.index,
                    total = post.content.part.total,
                    error = %error,
                    "Failed to deliver one chunk of a mirrored reply"
                );
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for Mirror {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mirror")
            .field("integration", &self.integration.id)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Running {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Running")
            .field("integration", &self.integration.id)
            .field("kind", &self.kind().as_str())
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

/// A sink that accepts nothing.
///
/// Used by a build with no executor — a server deployed without a model
/// provider — so an adapter can still be started, verified, and tested without
/// claiming a turn will run.
pub struct DiscardingSink;

#[async_trait::async_trait]
impl MessageSink for DiscardingSink {
    async fn accept(
        &self,
        _message: crate::model::InboundMessage,
    ) -> CowResult<crate::model::Ingested> {
        Err(CowError::Config(
            "this deployment has no turn executor configured".into(),
        ))
    }
}
