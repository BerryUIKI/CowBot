use cowbot_core::agent::AgentId;
use cowbot_core::channel::Channel;
use cowbot_core::channel::ChannelId;
use cowbot_core::conversation::{
    ChannelParticipant, ContextEntry, ContextEntryKind, ContextEvent, ContextEventKind,
    ContextRole, ContextToolCall, ConversationMode, ConversationSettings,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct ParticipantRow {
    channel_id: String,
    agent_id: String,
    joined_at: String,
}

impl ParticipantRow {
    fn into_domain(self) -> CowResult<ChannelParticipant> {
        Ok(ChannelParticipant {
            channel_id: ChannelId(self.channel_id),
            agent_id: AgentId(self.agent_id),
            // M3.2 reads the stored position once migration 0013 lands.
            position: 0,
            joined_at: parse_datetime(&self.joined_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SettingsRow {
    channel_id: String,
    mode: String,
    provider_id: Option<String>,
    model: String,
    router_enabled: i64,
    auto_compact: i64,
    compact_threshold: i64,
    created_at: String,
    updated_at: String,
}

impl SettingsRow {
    fn into_domain(self) -> CowResult<ConversationSettings> {
        let mode = self
            .mode
            .parse::<ConversationMode>()
            .map_err(|_| CowError::Storage(format!("unknown conversation mode '{}'", self.mode)))?;
        Ok(ConversationSettings {
            channel_id: ChannelId(self.channel_id),
            mode,
            provider_id: self.provider_id,
            model: self.model,
            // M3.2 reads the stored column once migration 0013 lands.
            default_responder: None,
            router_enabled: self.router_enabled != 0,
            auto_compact: self.auto_compact != 0,
            compact_threshold: self.compact_threshold.clamp(1, 100) as u32,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ContextEntryRow {
    id: String,
    channel_id: String,
    agent_id: String,
    seq: i64,
    role: String,
    kind: String,
    content: String,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    excluded: i64,
    tokens: i64,
    created_at: String,
}

impl ContextEntryRow {
    fn into_domain(self) -> CowResult<ContextEntry> {
        let role = self
            .role
            .parse::<ContextRole>()
            .map_err(|_| CowError::Storage(format!("unknown context role '{}'", self.role)))?;
        let kind = self.kind.parse::<ContextEntryKind>().map_err(|_| {
            CowError::Storage(format!("unknown context entry kind '{}'", self.kind))
        })?;
        Ok(ContextEntry {
            id: self.id,
            channel_id: ChannelId(self.channel_id),
            agent_id: AgentId(self.agent_id),
            // M3.2 reads the stored column once migration 0013 lands.
            author_agent_id: None,
            seq: self.seq,
            role,
            kind,
            content: self.content,
            tool_calls: self
                .tool_calls
                .as_deref()
                .map(|value| {
                    parse_json::<Vec<ContextToolCall>>(value, "context_entries.tool_calls")
                })
                .transpose()?,
            tool_call_id: self.tool_call_id,
            excluded: self.excluded != 0,
            tokens: self.tokens.clamp(0, u32::MAX as i64) as u32,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ContextEventRow {
    id: String,
    channel_id: String,
    agent_id: String,
    kind: String,
    detail: Option<String>,
    tokens_before: i64,
    tokens_after: i64,
    created_at: String,
}

impl ContextEventRow {
    fn into_domain(self) -> CowResult<ContextEvent> {
        let kind = self.kind.parse::<ContextEventKind>().map_err(|_| {
            CowError::Storage(format!("unknown context event kind '{}'", self.kind))
        })?;
        Ok(ContextEvent {
            id: self.id,
            channel_id: ChannelId(self.channel_id),
            agent_id: AgentId(self.agent_id),
            kind,
            detail: self.detail,
            tokens_before: self.tokens_before.clamp(0, u32::MAX as i64) as u32,
            tokens_after: self.tokens_after.clamp(0, u32::MAX as i64) as u32,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

const ENTRY_COLUMNS: &str = "id, channel_id, agent_id, seq, role, kind, content, tool_calls,
     tool_call_id, excluded, tokens, created_at";

const EVENT_COLUMNS: &str = "id, channel_id, agent_id, kind, detail, tokens_before,
     tokens_after, created_at";

fn serialize_tool_calls(entry: &ContextEntry) -> CowResult<Option<String>> {
    entry
        .tool_calls
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(CowError::from)
}

impl Store {
    /// Persist a conversation channel, its settings, and its participants atomically.
    pub async fn create_conversation(
        &self,
        workspace_id: &WorkspaceId,
        channel: &Channel,
        settings: &ConversationSettings,
        participants: &[AgentId],
    ) -> CowResult<()> {
        let mut transaction = self.pool().begin().await.map_err(db)?;
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
        .execute(&mut *transaction)
        .await
        .map_err(db)?;

        sqlx::query(
            "INSERT INTO conversation_settings
                (channel_id, mode, provider_id, model, router_enabled, auto_compact,
                 compact_threshold, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&settings.channel_id.0)
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(if settings.router_enabled { 1i64 } else { 0i64 })
        .bind(if settings.auto_compact { 1i64 } else { 0i64 })
        .bind(i64::from(settings.compact_threshold))
        .bind(settings.created_at.to_rfc3339())
        .bind(settings.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;

        for agent_id in participants {
            sqlx::query(
                "INSERT INTO channel_participants (channel_id, agent_id, joined_at)
                 VALUES (?, ?, ?)",
            )
            .bind(&channel.id.0)
            .bind(&agent_id.0)
            .bind(channel.created_at.to_rfc3339())
            .execute(&mut *transaction)
            .await
            .map_err(db)?;
        }

        transaction.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn list_channel_participants(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<Vec<ChannelParticipant>> {
        let rows = sqlx::query_as::<_, ParticipantRow>(
            "SELECT channel_id, agent_id, joined_at FROM channel_participants
             WHERE channel_id = ? ORDER BY joined_at, agent_id",
        )
        .bind(&channel_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ParticipantRow::into_domain).collect()
    }

    pub async fn list_all_channel_participants(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<ChannelParticipant>> {
        let rows = sqlx::query_as::<_, ParticipantRow>(
            "SELECT p.channel_id, p.agent_id, p.joined_at
             FROM channel_participants p
             JOIN channels c ON c.id = p.channel_id
             WHERE c.workspace_id = ?
             ORDER BY p.channel_id, p.joined_at, p.agent_id",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ParticipantRow::into_domain).collect()
    }

    /// Restore a participant row exactly, used by workspace import.
    pub async fn insert_channel_participant(
        &self,
        participant: &ChannelParticipant,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO channel_participants (channel_id, agent_id, joined_at)
             VALUES (?, ?, ?)",
        )
        .bind(&participant.channel_id.0)
        .bind(&participant.agent_id.0)
        .bind(participant.joined_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_conversation_settings(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<Option<ConversationSettings>> {
        let row = sqlx::query_as::<_, SettingsRow>(
            "SELECT channel_id, mode, provider_id, model, router_enabled, auto_compact,
                    compact_threshold, created_at, updated_at
             FROM conversation_settings WHERE channel_id = ?",
        )
        .bind(&channel_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(SettingsRow::into_domain).transpose()
    }

    pub async fn list_conversation_settings_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<ConversationSettings>> {
        let rows = sqlx::query_as::<_, SettingsRow>(
            "SELECT s.channel_id, s.mode, s.provider_id, s.model, s.router_enabled,
                    s.auto_compact, s.compact_threshold, s.created_at, s.updated_at
             FROM conversation_settings s
             JOIN channels c ON c.id = s.channel_id
             WHERE c.workspace_id = ?
             ORDER BY s.channel_id",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(SettingsRow::into_domain).collect()
    }

    pub async fn update_conversation_settings(
        &self,
        settings: &ConversationSettings,
    ) -> CowResult<()> {
        sqlx::query(
            "UPDATE conversation_settings
             SET mode = ?, provider_id = ?, model = ?, router_enabled = ?,
                 auto_compact = ?, compact_threshold = ?, updated_at = ?
             WHERE channel_id = ?",
        )
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(if settings.router_enabled { 1i64 } else { 0i64 })
        .bind(if settings.auto_compact { 1i64 } else { 0i64 })
        .bind(i64::from(settings.compact_threshold))
        .bind(settings.updated_at.to_rfc3339())
        .bind(&settings.channel_id.0)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Restore settings exactly, used by workspace import.
    pub async fn insert_conversation_settings(
        &self,
        settings: &ConversationSettings,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO conversation_settings
                (channel_id, mode, provider_id, model, router_enabled, auto_compact,
                 compact_threshold, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&settings.channel_id.0)
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(if settings.router_enabled { 1i64 } else { 0i64 })
        .bind(if settings.auto_compact { 1i64 } else { 0i64 })
        .bind(i64::from(settings.compact_threshold))
        .bind(settings.created_at.to_rfc3339())
        .bind(settings.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Append one entry, assigning it the next sequence number in its branch.
    pub async fn append_context_entry(&self, entry: &ContextEntry) -> CowResult<i64> {
        let tool_calls = serialize_tool_calls(entry)?;
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO context_entries
                (id, channel_id, agent_id, seq, role, kind, content, tool_calls,
                 tool_call_id, excluded, tokens, created_at)
             SELECT ?, ?, ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?
             FROM context_entries WHERE channel_id = ? AND agent_id = ?
             RETURNING seq",
        )
        .bind(&entry.id)
        .bind(&entry.channel_id.0)
        .bind(&entry.agent_id.0)
        .bind(entry.role.as_str())
        .bind(entry.kind.as_str())
        .bind(&entry.content)
        .bind(tool_calls)
        .bind(&entry.tool_call_id)
        .bind(if entry.excluded { 1i64 } else { 0i64 })
        .bind(i64::from(entry.tokens))
        .bind(entry.created_at.to_rfc3339())
        .bind(&entry.channel_id.0)
        .bind(&entry.agent_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(seq)
    }

    /// Append entries in order, all sharing one transaction.
    pub async fn append_context_entries(&self, entries: &[ContextEntry]) -> CowResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool().begin().await.map_err(db)?;
        for entry in entries {
            let tool_calls = serialize_tool_calls(entry)?;
            sqlx::query(
                "INSERT INTO context_entries
                    (id, channel_id, agent_id, seq, role, kind, content, tool_calls,
                     tool_call_id, excluded, tokens, created_at)
                 SELECT ?, ?, ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?
                 FROM context_entries WHERE channel_id = ? AND agent_id = ?",
            )
            .bind(&entry.id)
            .bind(&entry.channel_id.0)
            .bind(&entry.agent_id.0)
            .bind(entry.role.as_str())
            .bind(entry.kind.as_str())
            .bind(&entry.content)
            .bind(tool_calls)
            .bind(&entry.tool_call_id)
            .bind(if entry.excluded { 1i64 } else { 0i64 })
            .bind(i64::from(entry.tokens))
            .bind(entry.created_at.to_rfc3339())
            .bind(&entry.channel_id.0)
            .bind(&entry.agent_id.0)
            .execute(&mut *transaction)
            .await
            .map_err(db)?;
        }
        transaction.commit().await.map_err(db)?;
        Ok(())
    }

    /// Restore an entry exactly, used by workspace import.
    pub async fn insert_context_entry(&self, entry: &ContextEntry) -> CowResult<()> {
        let tool_calls = serialize_tool_calls(entry)?;
        sqlx::query(
            "INSERT OR IGNORE INTO context_entries
                (id, channel_id, agent_id, seq, role, kind, content, tool_calls,
                 tool_call_id, excluded, tokens, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.channel_id.0)
        .bind(&entry.agent_id.0)
        .bind(entry.seq)
        .bind(entry.role.as_str())
        .bind(entry.kind.as_str())
        .bind(&entry.content)
        .bind(tool_calls)
        .bind(&entry.tool_call_id)
        .bind(if entry.excluded { 1i64 } else { 0i64 })
        .bind(i64::from(entry.tokens))
        .bind(entry.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn list_context_entries(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<Vec<ContextEntry>> {
        let rows = sqlx::query_as::<_, ContextEntryRow>(&format!(
            "SELECT {ENTRY_COLUMNS} FROM context_entries
             WHERE channel_id = ? AND agent_id = ? ORDER BY seq"
        ))
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ContextEntryRow::into_domain).collect()
    }

    pub async fn list_all_context_entries(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<ContextEntry>> {
        let rows = sqlx::query_as::<_, ContextEntryRow>(
            "SELECT e.id, e.channel_id, e.agent_id, e.seq, e.role, e.kind, e.content,
                    e.tool_calls, e.tool_call_id, e.excluded, e.tokens, e.created_at
             FROM context_entries e
             JOIN channels c ON c.id = e.channel_id
             WHERE c.workspace_id = ?
             ORDER BY e.channel_id, e.agent_id, e.seq",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ContextEntryRow::into_domain).collect()
    }

    pub async fn exclude_context_entries(&self, ids: &[String]) -> CowResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool().begin().await.map_err(db)?;
        for id in ids {
            sqlx::query("UPDATE context_entries SET excluded = 1 WHERE id = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(db)?;
        }
        transaction.commit().await.map_err(db)?;
        Ok(())
    }

    /// Apply a compaction atomically: exclude summarized entries, append the
    /// summary, and record the event.
    pub async fn compact_context(
        &self,
        summary: &ContextEntry,
        excluded_ids: &[String],
        event: &ContextEvent,
    ) -> CowResult<()> {
        let tool_calls = serialize_tool_calls(summary)?;
        let mut transaction = self.pool().begin().await.map_err(db)?;
        for id in excluded_ids {
            sqlx::query("UPDATE context_entries SET excluded = 1 WHERE id = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(db)?;
        }
        sqlx::query(
            "INSERT INTO context_entries
                (id, channel_id, agent_id, seq, role, kind, content, tool_calls,
                 tool_call_id, excluded, tokens, created_at)
             SELECT ?, ?, ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?
             FROM context_entries WHERE channel_id = ? AND agent_id = ?",
        )
        .bind(&summary.id)
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .bind(summary.role.as_str())
        .bind(summary.kind.as_str())
        .bind(&summary.content)
        .bind(tool_calls)
        .bind(&summary.tool_call_id)
        .bind(if summary.excluded { 1i64 } else { 0i64 })
        .bind(i64::from(summary.tokens))
        .bind(summary.created_at.to_rfc3339())
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .execute(&mut *transaction)
        .await
        .map_err(db)?;
        sqlx::query(
            "INSERT INTO context_events
                (id, channel_id, agent_id, kind, detail, tokens_before, tokens_after, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.channel_id.0)
        .bind(&event.agent_id.0)
        .bind(event.kind.as_str())
        .bind(&event.detail)
        .bind(i64::from(event.tokens_before))
        .bind(i64::from(event.tokens_after))
        .bind(event.created_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;
        transaction.commit().await.map_err(db)?;
        Ok(())
    }

    /// Mark every branch entry excluded. The rows stay on disk for audit.
    pub async fn exclude_all_context_entries(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let result = sqlx::query(
            "UPDATE context_entries SET excluded = 1
             WHERE channel_id = ? AND excluded = 0",
        )
        .bind(&channel_id.0)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Delete every context row in a channel, used when its history is cleared.
    pub async fn clear_context_entries(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM context_entries WHERE channel_id = ?")
            .bind(&channel_id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn record_context_event(&self, event: &ContextEvent) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO context_events
                (id, channel_id, agent_id, kind, detail, tokens_before, tokens_after, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.channel_id.0)
        .bind(&event.agent_id.0)
        .bind(event.kind.as_str())
        .bind(&event.detail)
        .bind(i64::from(event.tokens_before))
        .bind(i64::from(event.tokens_after))
        .bind(event.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Restore an event exactly, used by workspace import.
    pub async fn insert_context_event(&self, event: &ContextEvent) -> CowResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO context_events
                (id, channel_id, agent_id, kind, detail, tokens_before, tokens_after, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.channel_id.0)
        .bind(&event.agent_id.0)
        .bind(event.kind.as_str())
        .bind(&event.detail)
        .bind(i64::from(event.tokens_before))
        .bind(i64::from(event.tokens_after))
        .bind(event.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn list_context_events(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        limit: u32,
    ) -> CowResult<Vec<ContextEvent>> {
        let rows = sqlx::query_as::<_, ContextEventRow>(&format!(
            "SELECT {EVENT_COLUMNS} FROM context_events
             WHERE channel_id = ? AND agent_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ))
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ContextEventRow::into_domain).collect()
    }

    pub async fn list_all_context_events(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<ContextEvent>> {
        let rows = sqlx::query_as::<_, ContextEventRow>(
            "SELECT e.id, e.channel_id, e.agent_id, e.kind, e.detail, e.tokens_before,
                    e.tokens_after, e.created_at
             FROM context_events e
             JOIN channels c ON c.id = e.channel_id
             WHERE c.workspace_id = ?
             ORDER BY e.channel_id, e.agent_id, e.created_at",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ContextEventRow::into_domain).collect()
    }
}
