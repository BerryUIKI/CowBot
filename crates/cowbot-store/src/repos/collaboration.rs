use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: String,
    name: String,
    topic: String,
    created_at: String,
    agent_id: Option<String>,
}

impl ChannelRow {
    fn into_domain(self) -> CowResult<Channel> {
        Ok(Channel {
            id: ChannelId(self.id),
            name: self.name,
            topic: self.topic,
            created_at: parse_datetime(&self.created_at)?,
            agent_id: self.agent_id.map(AgentId),
        })
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: String,
    channel_id: String,
    sender: String,
    content: String,
    thread_id: Option<String>,
    created_at: String,
}

impl MessageRow {
    fn into_domain(self) -> CowResult<ChannelMessage> {
        Ok(ChannelMessage {
            id: self.id,
            channel_id: ChannelId(self.channel_id),
            sender: parse_json::<Sender>(&self.sender, "messages.sender")?,
            content: self.content,
            thread_id: self.thread_id,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

impl Store {
    pub async fn create_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel: &Channel,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO channels (id, workspace_id, name, topic, created_at, agent_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&channel.id.0)
        .bind(&workspace_id.0)
        .bind(&channel.name)
        .bind(&channel.topic)
        .bind(channel.created_at.to_rfc3339())
        .bind(channel.agent_id.as_ref().map(|agent| agent.0.clone()))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_channel(&self, id: &ChannelId) -> CowResult<Option<Channel>> {
        let row = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, topic, created_at, agent_id FROM channels WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ChannelRow::into_domain).transpose()
    }

    pub async fn get_channel_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> CowResult<Option<Channel>> {
        let row = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, topic, created_at, agent_id FROM channels
             WHERE workspace_id = ? AND agent_id = ?",
        )
        .bind(&workspace_id.0)
        .bind(&agent_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ChannelRow::into_domain).transpose()
    }

    pub async fn list_channels(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Channel>> {
        let rows = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, topic, created_at, agent_id FROM channels
             WHERE workspace_id = ? ORDER BY name",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ChannelRow::into_domain).collect()
    }

    pub async fn append_message(&self, message: &ChannelMessage) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO messages (id, channel_id, sender, content, thread_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&message.id)
        .bind(&message.channel_id.0)
        .bind(serde_json::to_string(&message.sender).map_err(cowbot_core::error::CowError::from)?)
        .bind(&message.content)
        .bind(&message.thread_id)
        .bind(message.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_message(&self, id: &str) -> CowResult<Option<ChannelMessage>> {
        let row = sqlx::query_as::<_, MessageRow>(
            "SELECT id, channel_id, sender, content, thread_id, created_at
             FROM messages WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(MessageRow::into_domain).transpose()
    }

    pub async fn list_messages(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ChannelMessage>> {
        let rows = sqlx::query_as::<_, MessageRow>(
            "SELECT id, channel_id, sender, content, thread_id, created_at FROM (
                 SELECT id, channel_id, sender, content, thread_id, created_at
                 FROM messages WHERE channel_id = ?
                 ORDER BY created_at DESC LIMIT ?
             ) ORDER BY created_at ASC",
        )
        .bind(&channel_id.0)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(MessageRow::into_domain).collect()
    }
}
