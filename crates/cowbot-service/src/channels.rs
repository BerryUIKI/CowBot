use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_store::Store;

/// Longest accepted channel name, in characters.
const MAX_CHANNEL_NAME: usize = 80;

pub(crate) fn normalize_channel_name(name: &str) -> CowResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CowError::Config("channel name cannot be empty".into()));
    }
    if trimmed.chars().count() > MAX_CHANNEL_NAME {
        return Err(CowError::Config(format!(
            "channel name must be {MAX_CHANNEL_NAME} characters or fewer"
        )));
    }
    Ok(trimmed.to_string())
}

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
        let name = normalize_channel_name(&name.into())?;
        if self
            .store
            .channel_name_taken(workspace_id, &name, None)
            .await?
        {
            return Err(CowError::Config(format!(
                "a channel named '{name}' already exists"
            )));
        }

        let channel = Channel::new(name, topic.into().trim().to_string());
        self.store.create_channel(workspace_id, &channel).await?;
        self.bus
            .publish(Event::new(EventPayload::ChannelCreated(channel.clone())));
        Ok(channel)
    }

    pub async fn list_channels(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Channel>> {
        self.store.list_channels(workspace_id).await
    }

    /// Rename a channel and replace its topic.
    pub async fn rename_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
        name: impl Into<String>,
        topic: impl Into<String>,
    ) -> CowResult<Channel> {
        let Some(mut channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "channel '{channel_id}' was not found"
            )));
        };
        let name = normalize_channel_name(&name.into())?;
        if !channel.name.eq_ignore_ascii_case(&name)
            && self
                .store
                .channel_name_taken(workspace_id, &name, Some(channel_id))
                .await?
        {
            return Err(CowError::Config(format!(
                "a channel named '{name}' already exists"
            )));
        }

        let topic = topic.into().trim().to_string();
        self.store.rename_channel(channel_id, &name, &topic).await?;
        channel.name = name;
        channel.topic = topic;
        self.bus
            .publish(Event::new(EventPayload::ChannelUpdated(channel.clone())));
        Ok(channel)
    }

    pub async fn set_channel_pinned(
        &self,
        channel_id: &ChannelId,
        pinned: bool,
    ) -> CowResult<Channel> {
        let Some(mut channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "channel '{channel_id}' was not found"
            )));
        };
        self.store.set_channel_pinned(channel_id, pinned).await?;
        channel.pinned = pinned;
        self.bus
            .publish(Event::new(EventPayload::ChannelUpdated(channel.clone())));
        Ok(channel)
    }

    /// Delete a group channel or user-created conversation. Direct agent
    /// conversations are kept; callers must clear their message history instead.
    pub async fn delete_channel(&self, channel_id: &ChannelId) -> CowResult<Channel> {
        let Some(channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "channel '{channel_id}' was not found"
            )));
        };
        if channel.is_direct() {
            return Err(CowError::Config(
                "agent conversations cannot be deleted; clear the history instead".into(),
            ));
        }

        self.store.delete_channel(channel_id).await?;
        // Prompt snapshots are evidence about calls made in this channel, so
        // they do not outlive it. Usage rows stay: they are not channel-scoped
        // by a foreign key, and deleting a channel is not a usage reset.
        if let Err(error) = self
            .store
            .delete_prompt_snapshots_for_channel(channel_id)
            .await
        {
            tracing::warn!(
                error = %error,
                channel_id = %channel_id.0,
                "Failed to delete the prompt snapshots of a deleted channel"
            );
        }
        self.bus.publish(Event::new(EventPayload::ChannelDeleted {
            channel_id: channel_id.clone(),
        }));
        Ok(channel)
    }

    /// Delete every message in a channel, keeping the channel.
    ///
    /// A user-created conversation also loses its model context; leaving it
    /// behind would make the model remember messages the user can no longer see.
    pub async fn clear_channel_messages(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let Some(channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "channel '{channel_id}' was not found"
            )));
        };
        if channel.is_conversation() {
            self.store.clear_context_entries(channel_id).await?;
        }
        let cleared = self.store.clear_channel_messages(channel_id).await?;
        // Subscribers (a second desktop, a server stream) need to know the
        // history is gone; a silent clear leaves them showing messages that no
        // longer exist.
        self.bus.publish(Event::new(EventPayload::ChannelCleared {
            channel_id: channel_id.clone(),
        }));
        Ok(cleared)
    }

    /// Return the agent's direct conversation, creating it on first use.
    ///
    /// Every agent gets exactly one direct channel, so the desktop can show a
    /// contact list that never spawns duplicates. An existing conversation is
    /// reconciled with the agent's current name and role.
    pub async fn ensure_agent_channel(
        &self,
        workspace_id: &WorkspaceId,
        agent: &AgentProfile,
    ) -> CowResult<Channel> {
        if let Some(mut channel) = self
            .store
            .get_channel_for_agent(workspace_id, &agent.id)
            .await?
        {
            if channel.name == agent.name && channel.topic == agent.role {
                return Ok(channel);
            }
            channel.name = agent.name.clone();
            channel.topic = agent.role.clone();
            self.store
                .rename_channel(&channel.id, &channel.name, &channel.topic)
                .await?;
            self.bus
                .publish(Event::new(EventPayload::ChannelUpdated(channel.clone())));
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

    pub(crate) async fn post_system_notice(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<ChannelMessage> {
        self.post_message(channel_id, Sender::System, content.to_string())
            .await
    }
}
