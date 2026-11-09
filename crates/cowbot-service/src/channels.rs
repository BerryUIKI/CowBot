use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::error::CowResult;
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_store::Store;

/// Application service for channels and messages.
///
/// Write operations persist first and then publish the corresponding event on
/// the bus, keeping storage and real-time subscribers in sync.
#[derive(Clone)]
pub struct ChannelService {
    store: Store,
    bus: EventBus,
}

impl ChannelService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self { store, bus }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub async fn create_channel(
        &self,
        workspace_id: &WorkspaceId,
        name: impl Into<String>,
        topic: impl Into<String>,
    ) -> CowResult<Channel> {
        let channel = Channel::new(name, topic);
        self.store.create_channel(workspace_id, &channel).await?;
        self.bus
            .publish(Event::new(EventPayload::ChannelCreated(channel.clone())));
        Ok(channel)
    }

    pub async fn list_channels(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Channel>> {
        self.store.list_channels(workspace_id).await
    }

    /// Return the agent's direct conversation, creating it on first use.
    ///
    /// Every agent gets exactly one direct channel, so the desktop can show a
    /// contact list that never spawns duplicates.
    pub async fn ensure_agent_channel(
        &self,
        workspace_id: &WorkspaceId,
        agent: &AgentProfile,
    ) -> CowResult<Channel> {
        if let Some(channel) = self
            .store
            .get_channel_for_agent(workspace_id, &agent.id)
            .await?
        {
            return Ok(channel);
        }

        let channel = Channel::direct(agent, agent.role.clone());
        self.store.create_channel(workspace_id, &channel).await?;
        self.bus
            .publish(Event::new(EventPayload::ChannelCreated(channel.clone())));
        Ok(channel)
    }

    /// Create the direct conversation of every agent that lacks one.
    pub async fn ensure_agent_channels(
        &self,
        workspace_id: &WorkspaceId,
        agents: &[AgentProfile],
    ) -> CowResult<()> {
        for agent in agents {
            self.ensure_agent_channel(workspace_id, agent).await?;
        }
        Ok(())
    }

    pub async fn post_message(
        &self,
        channel_id: &ChannelId,
        sender: Sender,
        content: impl Into<String>,
    ) -> CowResult<ChannelMessage> {
        self.post_thread_message(channel_id, sender, content, None)
            .await
    }

    pub async fn post_thread_message(
        &self,
        channel_id: &ChannelId,
        sender: Sender,
        content: impl Into<String>,
        thread_id: Option<String>,
    ) -> CowResult<ChannelMessage> {
        let mut message = ChannelMessage::new(channel_id.clone(), sender, content);
        if let Some(thread_id) = thread_id {
            message = message.with_thread(thread_id);
        }

        self.store.append_message(&message).await?;
        self.bus
            .publish(Event::new(EventPayload::MessagePosted(message.clone())));
        Ok(message)
    }

    pub async fn list_messages(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ChannelMessage>> {
        self.store.list_messages(channel_id, limit).await
    }
}
