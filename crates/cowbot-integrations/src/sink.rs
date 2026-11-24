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
    /// Where attachment bytes are written, when the deployment has a root.
    attachments: Option<std::path::PathBuf>,
    /// The adapter that can fetch them, set once it exists.
    ///
    /// Held rather than passed in because the sink is the only place that has
    /// both the pipeline and the root, and the adapter is the only thing that
    /// knows how to download from its platform.
    adapter: Option<std::sync::Arc<dyn crate::adapter::Integration>>,
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
            attachments: None,
            adapter: None,
        }
    }

    /// Give the sink the adapter that can fetch an attachment's bytes.
    pub fn with_adapter(
        mut self,
        adapter: std::sync::Arc<dyn crate::adapter::Integration>,
    ) -> Self {
        self.adapter = Some(adapter);
        self
    }

    /// Write attachments below `root`, the directory the deployment's tools are
    /// rooted at (ADR-0060).
    ///
    /// The pipeline uses the same root the file tools do, so the path it names in
    /// the message text is one a tool can open without being told an absolute
    /// server path.
    pub fn with_attachments(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.attachments = Some(root.into());
        self
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
        // The store and the adapter are both needed to save a file, and the sink
        // is the only place that has the root. `files` borrows the adapter, so it
        // has to outlive the call to `ingest`; it is empty when there is nothing
        // to save, which is the ordinary case.
        let store = self
            .attachments
            .as_ref()
            .map(crate::attachments::AttachmentStore::new);
        let files = store
            .as_ref()
            .zip(self.adapter.as_ref())
            .map(|(store, adapter)| crate::attachments::Inbound {
                store,
                adapter: adapter.as_ref(),
                integration: &self.integration.id,
            });
        let outcome = inbound::ingest(
            &self.store,
            &self.channels,
            &self.integration,
            &message,
            files.as_ref(),
        )
        .await?;
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
