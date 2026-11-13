use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::ChannelKind;
use cowbot_core::error::{CowError, CowResult};
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
    pinned: i64,
    kind: String,
}

impl ChannelRow {
    fn into_domain(self) -> CowResult<Channel> {
        let kind = self
            .kind
            .parse::<ChannelKind>()
            .map_err(|_| CowError::Storage(format!("unknown channel kind '{}'", self.kind)))?;
        Ok(Channel {
            id: ChannelId(self.id),
            name: self.name,
            topic: self.topic,
            created_at: parse_datetime(&self.created_at)?,
            agent_id: self.agent_id.map(AgentId),
            pinned: self.pinned != 0,
            kind,
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
            "INSERT INTO channels (id, workspace_id, name, topic, created_at, agent_id, pinned, kind)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&channel.id.0)
        .bind(&workspace_id.0)
        .bind(&channel.name)
        .bind(&channel.topic)
        .bind(channel.created_at.to_rfc3339())
        .bind(channel.agent_id.as_ref().map(|agent| agent.0.clone()))
        .bind(if channel.pinned { 1i64 } else { 0i64 })
        .bind(channel.kind.as_str())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_channel(&self, id: &ChannelId) -> CowResult<Option<Channel>> {
        let row = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, topic, created_at, agent_id, pinned, kind
             FROM channels WHERE id = ?",
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
            "SELECT id, name, topic, created_at, agent_id, pinned, kind FROM channels
             WHERE workspace_id = ? AND agent_id = ?",
        )
        .bind(&workspace_id.0)
        .bind(&agent_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ChannelRow::into_domain).transpose()
    }

    /// Whether a channel belongs to the given workspace.
    pub async fn channel_in_workspace(
        &self,
        id: &ChannelId,
        workspace_id: &WorkspaceId,
    ) -> CowResult<bool> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM channels WHERE id = ? AND workspace_id = ?")
                .bind(&id.0)
                .bind(&workspace_id.0)
                .fetch_one(self.pool())
                .await
                .map_err(db)?;
        Ok(count > 0)
    }

    /// The workspace that owns a channel.
    pub async fn channel_workspace(&self, id: &ChannelId) -> CowResult<Option<WorkspaceId>> {
        let id: Option<String> =
            sqlx::query_scalar("SELECT workspace_id FROM channels WHERE id = ?")
                .bind(&id.0)
                .fetch_optional(self.pool())
                .await
                .map_err(db)?;
        Ok(id.map(WorkspaceId))
    }

    pub async fn list_channels(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Channel>> {
        let rows = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, topic, created_at, agent_id, pinned, kind FROM channels
             WHERE workspace_id = ? ORDER BY name",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ChannelRow::into_domain).collect()
    }

    /// Whether another channel in the workspace already uses `name`.
    ///
    /// Names compare case-insensitively; `exclude` lets a rename keep its own
    /// name.
    pub async fn channel_name_taken(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
        exclude: Option<&ChannelId>,
    ) -> CowResult<bool> {
        let exclude = exclude.map(|channel| channel.0.clone()).unwrap_or_default();
        let found: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM channels
                 WHERE workspace_id = ? AND name = ? COLLATE NOCASE AND id <> ?
             )",
        )
        .bind(&workspace_id.0)
        .bind(name)
        .bind(exclude)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(found != 0)
    }

    pub async fn rename_channel(&self, id: &ChannelId, name: &str, topic: &str) -> CowResult<()> {
        sqlx::query("UPDATE channels SET name = ?, topic = ? WHERE id = ?")
            .bind(name)
            .bind(topic)
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn set_channel_pinned(&self, id: &ChannelId, pinned: bool) -> CowResult<()> {
        sqlx::query("UPDATE channels SET pinned = ? WHERE id = ?")
            .bind(if pinned { 1i64 } else { 0i64 })
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn delete_channel(&self, id: &ChannelId) -> CowResult<()> {
        sqlx::query("DELETE FROM channels WHERE id = ?")
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Delete every message in a channel, keeping the channel itself.
    pub async fn clear_channel_messages(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM messages WHERE channel_id = ?")
            .bind(&channel_id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
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
