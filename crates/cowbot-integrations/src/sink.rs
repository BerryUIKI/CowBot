//! The sink an adapter hands an accepted message to (ADR-0056).

use async_trait::async_trait;
use cowbot_core::error::CowResult;
use cowbot_core::integration::IntegrationRecord;
use cowbot_service::ChannelService;
use cowbot_store::Store;

use crate::adapter::MessageSink;
use crate::inbound;
use crate::model::{InboundMessage, Ingested};

/// Stores an inbound message and starts a turn when it addresses an agent.
///
/// The turn is started *after* the message is stored and never awaited by the
/// adapter, because Slack and Feishu give three seconds to acknowledge and a
/// model turn takes far longer. That ordering is the whole reason the sink
/// exists separately from the adapter: an adapter acknowledges, the sink runs.
pub struct ChannelSink {
    store: Store,
    channels: ChannelService,
    integration: IntegrationRecord,
    /// Started with the posted message when it addresses an agent.
    ///
    /// `None` in a deployment with no executor — a server without a model
    /// provider. The message is still recorded, so the operator sees the
    /// question rather than losing it.
    turns: Option<Box<dyn TurnStart>>,
}

/// Starts a turn for a message the sink stored.
pub trait TurnStart: Send + Sync {
    fn start(&self, channel_id: cowbot_core::channel::ChannelId, message_id: String);
}

impl ChannelSink {
    pub fn new(store: Store, channels: ChannelService, integration: IntegrationRecord) -> Self {
        Self {
            store,
            channels,
            integration,
            turns: None,
        }
    }

    /// Have the sink start turns through `turns`.
    pub fn with_turns(mut self, turns: Box<dyn TurnStart>) -> Self {
        self.turns = Some(turns);
        self
    }
}

impl std::fmt::Debug for ChannelSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChannelSink")
            .field("integration", &self.integration.id)
            .field("starts_turns", &self.turns.is_some())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl MessageSink for ChannelSink {
    async fn accept(&self, message: InboundMessage) -> CowResult<Ingested> {
        let outcome =
            inbound::ingest(&self.store, &self.channels, &self.integration, &message).await?;
        if let (
            Ingested::Posted {
                starts_turn: true,
                channel_id,
                message_id,
                ..
            },
            Some(turns),
        ) = (&outcome, &self.turns)
        {
            turns.start(channel_id.clone(), message_id.clone());
        }
        Ok(outcome)
    }
}

/// A turn starter that records what it was asked to run.
///
/// Public because the crate's tests and any deployment's acceptance test want to
/// assert that a message starts a turn without running a model.
#[derive(Default, Clone)]
pub struct RecordingTurns {
    started: std::sync::Arc<std::sync::Mutex<Vec<(cowbot_core::channel::ChannelId, String)>>>,
}

impl RecordingTurns {
    pub fn new() -> Self {
        Self::default()
    }

    /// What this sink was asked to run, oldest first.
    pub fn started(&self) -> Vec<(cowbot_core::channel::ChannelId, String)> {
        self.started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl TurnStart for RecordingTurns {
    fn start(&self, channel_id: cowbot_core::channel::ChannelId, message_id: String) {
        self.started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((channel_id, message_id));
    }
}
