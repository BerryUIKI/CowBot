use cowbot_core::agent::AgentId;
use cowbot_core::channel::Channel;
use cowbot_core::channel::ChannelId;
use cowbot_core::conversation::{
    ChannelParticipant, CompactionWrite, ContextEntry, ContextEntryKind, ContextEvent,
    ContextEventKind, ContextRole, ContextToolCall, ConversationMode, ConversationSettings,
    ConversationTurn, RouteSource, TurnId,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::TaskId;

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct ParticipantRow {
    channel_id: String,
    agent_id: String,
    position: i64,
    joined_at: String,
}

impl ParticipantRow {
    fn into_domain(self) -> CowResult<ChannelParticipant> {
        Ok(ChannelParticipant {
            channel_id: ChannelId(self.channel_id),
            agent_id: AgentId(self.agent_id),
            position: self.position.clamp(0, u32::MAX as i64) as u32,
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
    default_responder: Option<String>,
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
            default_responder: self.default_responder.map(AgentId),
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
    author_agent_id: Option<String>,
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
            author_agent_id: self.author_agent_id.map(AgentId),
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

const ENTRY_COLUMNS: &str = "id, channel_id, agent_id, author_agent_id, seq, role, kind, content,
     tool_calls, tool_call_id, excluded, tokens, created_at";

const EVENT_COLUMNS: &str = "id, channel_id, agent_id, kind, detail, tokens_before,
     tokens_after, created_at";

const TURN_COLUMNS: &str = "id, channel_id, message_id, source, selected_agents,
     unknown_mentions, reason, router_call_id, task_ids, mirrored_at, created_at";

fn serialize_tool_calls(entry: &ContextEntry) -> CowResult<Option<String>> {
    entry
        .tool_calls
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(CowError::from)
}

fn serialize_agent_ids(agents: &[AgentId]) -> CowResult<String> {
    let ids: Vec<&str> = agents.iter().map(|agent| agent.0.as_str()).collect();
    serde_json::to_string(&ids).map_err(CowError::from)
}

fn serialize_task_ids(tasks: &[TaskId]) -> CowResult<String> {
    let ids: Vec<&str> = tasks.iter().map(|task| task.0.as_str()).collect();
    serde_json::to_string(&ids).map_err(CowError::from)
}

fn parse_agent_ids(value: &str, field: &str) -> CowResult<Vec<AgentId>> {
    Ok(parse_json::<Vec<String>>(value, field)?
        .into_iter()
        .map(AgentId)
        .collect())
}

fn parse_task_ids(value: &str, field: &str) -> CowResult<Vec<TaskId>> {
    Ok(parse_json::<Vec<String>>(value, field)?
        .into_iter()
        .map(TaskId)
        .collect())
}

#[derive(sqlx::FromRow)]
struct ConversationTurnRow {
    id: String,
    channel_id: String,
    message_id: String,
    source: String,
    selected_agents: String,
    unknown_mentions: String,
    reason: Option<String>,
    router_call_id: Option<String>,
    task_ids: String,
    mirrored_at: Option<String>,
    created_at: String,
}

impl ConversationTurnRow {
    fn into_domain(self) -> CowResult<ConversationTurn> {
        let source = self
            .source
            .parse::<RouteSource>()
            .map_err(|_| CowError::Storage(format!("unknown route source '{}'", self.source)))?;
        Ok(ConversationTurn {
            id: TurnId(self.id),
            channel_id: ChannelId(self.channel_id),
            message_id: self.message_id,
            source,
            selected_agents: parse_agent_ids(
                &self.selected_agents,
                "conversation_turns.selected_agents",
            )?,
            unknown_mentions: parse_json(
                &self.unknown_mentions,
                "conversation_turns.unknown_mentions",
            )?,
            reason: self.reason,
            router_call_id: self.router_call_id,
            task_ids: parse_task_ids(&self.task_ids, "conversation_turns.task_ids")?,
            mirrored_at: self
                .mirrored_at
                .as_deref()
                .map(parse_datetime)
                .transpose()?,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
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
                (channel_id, mode, provider_id, model, default_responder, router_enabled,
                 auto_compact, compact_threshold, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&settings.channel_id.0)
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(settings.default_responder.as_ref().map(|agent| &agent.0))
        .bind(if settings.router_enabled { 1i64 } else { 0i64 })
        .bind(if settings.auto_compact { 1i64 } else { 0i64 })
        .bind(i64::from(settings.compact_threshold))
        .bind(settings.created_at.to_rfc3339())
        .bind(settings.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;

        for (position, agent_id) in participants.iter().enumerate() {
            sqlx::query(
                "INSERT INTO channel_participants (channel_id, agent_id, position, joined_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&channel.id.0)
            .bind(&agent_id.0)
            .bind(position as i64)
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
            "SELECT channel_id, agent_id, position, joined_at FROM channel_participants
             WHERE channel_id = ? ORDER BY position, joined_at, agent_id",
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
            "SELECT p.channel_id, p.agent_id, p.position, p.joined_at
             FROM channel_participants p
             JOIN channels c ON c.id = p.channel_id
             WHERE c.workspace_id = ?
             ORDER BY p.channel_id, p.position, p.joined_at, p.agent_id",
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
            "INSERT OR IGNORE INTO channel_participants (channel_id, agent_id, position, joined_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&participant.channel_id.0)
        .bind(&participant.agent_id.0)
        .bind(i64::from(participant.position))
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
            "SELECT channel_id, mode, provider_id, model, default_responder, router_enabled,
                    auto_compact, compact_threshold, created_at, updated_at
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
            "SELECT s.channel_id, s.mode, s.provider_id, s.model, s.default_responder,
                    s.router_enabled, s.auto_compact, s.compact_threshold, s.created_at,
                    s.updated_at
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
             SET mode = ?, provider_id = ?, model = ?, default_responder = ?,
                 router_enabled = ?, auto_compact = ?, compact_threshold = ?, updated_at = ?
             WHERE channel_id = ?",
        )
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(settings.default_responder.as_ref().map(|agent| &agent.0))
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
                (channel_id, mode, provider_id, model, default_responder, router_enabled,
                 auto_compact, compact_threshold, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&settings.channel_id.0)
        .bind(settings.mode.as_str())
        .bind(&settings.provider_id)
        .bind(&settings.model)
        .bind(settings.default_responder.as_ref().map(|agent| &agent.0))
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
    ///
    /// The returned sequence is not stable across a later compaction, which
    /// renumbers the kept tail; treat it as ordering, not identity.
    pub async fn append_context_entry(&self, entry: &ContextEntry) -> CowResult<i64> {
        let tool_calls = serialize_tool_calls(entry)?;
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO context_entries
                (id, channel_id, agent_id, author_agent_id, seq, role, kind, content,
                 tool_calls, tool_call_id, excluded, tokens, created_at)
             SELECT ?, ?, ?, ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?
             FROM context_entries WHERE channel_id = ? AND agent_id = ?
             RETURNING seq",
        )
        .bind(&entry.id)
        .bind(&entry.channel_id.0)
        .bind(&entry.agent_id.0)
        .bind(entry.author_agent_id.as_ref().map(|agent| &agent.0))
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
                    (id, channel_id, agent_id, author_agent_id, seq, role, kind, content,
                     tool_calls, tool_call_id, excluded, tokens, created_at)
                 SELECT ?, ?, ?, ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?
                 FROM context_entries WHERE channel_id = ? AND agent_id = ?",
            )
            .bind(&entry.id)
            .bind(&entry.channel_id.0)
            .bind(&entry.agent_id.0)
            .bind(entry.author_agent_id.as_ref().map(|agent| &agent.0))
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
                (id, channel_id, agent_id, author_agent_id, seq, role, kind, content,
                 tool_calls, tool_call_id, excluded, tokens, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.channel_id.0)
        .bind(&entry.agent_id.0)
        .bind(entry.author_agent_id.as_ref().map(|agent| &agent.0))
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
            "SELECT e.id, e.channel_id, e.agent_id, e.author_agent_id, e.seq, e.role, e.kind,
                    e.content, e.tool_calls, e.tool_call_id, e.excluded, e.tokens, e.created_at
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

    /// Exclude every active entry in one branch.
    ///
    /// Returns the affected row count and the pre-clear active token total.
    /// Updating by branch rather than by a previously read id list keeps a
    /// compaction that commits concurrently from leaving its new summary
    /// active after a clear reports success.
    pub async fn exclude_active_context_entries(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<(u64, u32)> {
        let mut transaction = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db)?;
        let tokens_before: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(tokens), 0) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
        )
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        let result = sqlx::query(
            "UPDATE context_entries SET excluded = 1
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
        )
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .execute(&mut *transaction)
        .await
        .map_err(db)?;
        transaction.commit().await.map_err(db)?;
        Ok((
            result.rows_affected(),
            u32::try_from(tokens_before).unwrap_or(u32::MAX),
        ))
    }

    /// Apply a compaction after checking that the branch still matches the
    /// summarized prefix.
    ///
    /// `cutoff_seq` is the sequence of the first entry that must stay active.
    /// The summarized prefix is excluded, the tail is renumbered one position
    /// later to free the cutoff, and the summary is inserted at `cutoff_seq`.
    /// The unique `(channel_id, agent_id, seq)` index makes a plain
    /// `seq = seq + 1` update fail mid-statement, so the tail is moved through
    /// negative values first.
    pub async fn compact_context(
        &self,
        summary: &ContextEntry,
        excluded_ids: &[String],
        cutoff_seq: i64,
        event: &mut ContextEvent,
    ) -> CowResult<CompactionWrite> {
        if excluded_ids.is_empty() {
            return Err(CowError::Config(
                "compaction requires at least one summarized entry".to_string(),
            ));
        }
        if cutoff_seq < 1 {
            return Err(CowError::Config(
                "compaction cutoff must be a positive sequence number".to_string(),
            ));
        }

        let tool_calls = serialize_tool_calls(summary)?;
        let mut transaction = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db)?;

        // Imported packages can carry arbitrary sequence numbers. The
        // two-phase renumber below uses negative values as a disjoint staging
        // range, so a non-positive sequence would make it ambiguous.
        let non_positive: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND seq <= 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        if non_positive > 0 {
            transaction.rollback().await.map_err(db)?;
            return Err(CowError::Config(
                "cannot compact a branch with non-positive context sequence numbers".to_string(),
            ));
        }

        // A clear can remove every row while the summary is being generated.
        // Report that directly instead of the more general stale message.
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        if active_count == 0 {
            transaction.rollback().await.map_err(db)?;
            return Ok(CompactionWrite::Empty);
        }

        // The service selected `excluded_ids` as every active entry before
        // the cutoff. If that is no longer true, another compaction or a
        // clear changed the branch while the model was running.
        let active_prefix: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0 AND seq < ?",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .bind(cutoff_seq)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        if active_prefix != excluded_ids.len() as i64 {
            transaction.rollback().await.map_err(db)?;
            return Ok(CompactionWrite::Stale);
        }

        let tokens_before: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(tokens), 0) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;

        let mut excluded = 0_u64;
        for id in excluded_ids {
            let result = sqlx::query(
                "UPDATE context_entries SET excluded = 1
                 WHERE id = ? AND channel_id = ? AND agent_id = ? AND excluded = 0",
            )
            .bind(id)
            .bind(&summary.channel_id.0)
            .bind(&summary.agent_id.0)
            .execute(&mut *transaction)
            .await
            .map_err(db)?;
            excluded += result.rows_affected();
        }
        if excluded != excluded_ids.len() as u64 {
            transaction.rollback().await.map_err(db)?;
            return Ok(CompactionWrite::Stale);
        }

        let cutoff_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND seq = ? AND excluded = 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .bind(cutoff_seq)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        if cutoff_exists == 0 {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM context_entries
                 WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
            )
            .bind(&summary.channel_id.0)
            .bind(&summary.agent_id.0)
            .fetch_one(&mut *transaction)
            .await
            .map_err(db)?;
            transaction.rollback().await.map_err(db)?;
            return Ok(if active == 0 {
                CompactionWrite::Empty
            } else {
                CompactionWrite::Stale
            });
        }

        // Free the cutoff and shift the tail by one. Negation is injective
        // and disjoint from the positive sequence space, so neither update
        // can collide with the unique index.
        let shifted = sqlx::query(
            "UPDATE context_entries SET seq = -seq
             WHERE channel_id = ? AND agent_id = ? AND seq >= ?",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .bind(cutoff_seq)
        .execute(&mut *transaction)
        .await
        .map_err(db)?
        .rows_affected();

        let restored = sqlx::query(
            "UPDATE context_entries SET seq = -seq + 1
             WHERE channel_id = ? AND agent_id = ? AND seq < 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .execute(&mut *transaction)
        .await
        .map_err(db)?
        .rows_affected();

        if shifted != restored {
            transaction.rollback().await.map_err(db)?;
            return Err(CowError::Storage(
                "context sequence renumber did not preserve the branch".to_string(),
            ));
        }

        sqlx::query(
            "INSERT INTO context_entries
                (id, channel_id, agent_id, author_agent_id, seq, role, kind, content,
                 tool_calls, tool_call_id, excluded, tokens, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&summary.id)
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .bind(summary.author_agent_id.as_ref().map(|agent| &agent.0))
        .bind(cutoff_seq)
        .bind(summary.role.as_str())
        .bind(summary.kind.as_str())
        .bind(&summary.content)
        .bind(tool_calls)
        .bind(&summary.tool_call_id)
        .bind(if summary.excluded { 1i64 } else { 0i64 })
        .bind(i64::from(summary.tokens))
        .bind(summary.created_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;

        let tokens_after: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(tokens), 0) FROM context_entries
             WHERE channel_id = ? AND agent_id = ? AND excluded = 0",
        )
        .bind(&summary.channel_id.0)
        .bind(&summary.agent_id.0)
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;

        // Report the branch as it was actually written, not as the service
        // read it before the model call. Entries appended during the call are
        // part of both totals and stay after the summary.
        event.tokens_before = u32::try_from(tokens_before).unwrap_or(u32::MAX);
        event.tokens_after = u32::try_from(tokens_after).unwrap_or(u32::MAX);

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
        Ok(CompactionWrite::Applied {
            summary_seq: cutoff_seq,
            tokens_before: event.tokens_before,
            tokens_after: event.tokens_after,
        })
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

    /// Persist a group routing decision.
    ///
    /// A second insert for the same message is a no-op so a retried turn cannot
    /// duplicate the audit row.
    pub async fn insert_conversation_turn(&self, turn: &ConversationTurn) -> CowResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO conversation_turns
                (id, channel_id, message_id, source, selected_agents, unknown_mentions,
                 reason, router_call_id, task_ids, mirrored_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&turn.id.0)
        .bind(&turn.channel_id.0)
        .bind(&turn.message_id)
        .bind(turn.source.as_str())
        .bind(serialize_agent_ids(&turn.selected_agents)?)
        .bind(serde_json::to_string(&turn.unknown_mentions).map_err(CowError::from)?)
        .bind(&turn.reason)
        .bind(&turn.router_call_id)
        .bind(serialize_task_ids(&turn.task_ids)?)
        .bind(turn.mirrored_at.map(|at| at.to_rfc3339()))
        .bind(turn.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Update the decision and settlement parts of a persisted turn.
    pub async fn update_conversation_turn(&self, turn: &ConversationTurn) -> CowResult<()> {
        sqlx::query(
            "UPDATE conversation_turns
             SET selected_agents = ?, unknown_mentions = ?, reason = ?, router_call_id = ?,
                 task_ids = ?, mirrored_at = ?
             WHERE id = ?",
        )
        .bind(serialize_agent_ids(&turn.selected_agents)?)
        .bind(serde_json::to_string(&turn.unknown_mentions).map_err(CowError::from)?)
        .bind(&turn.reason)
        .bind(&turn.router_call_id)
        .bind(serialize_task_ids(&turn.task_ids)?)
        .bind(turn.mirrored_at.map(|at| at.to_rfc3339()))
        .bind(&turn.id.0)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Read the routing decision of one user message, if it was recorded.
    pub async fn get_conversation_turn(
        &self,
        channel_id: &ChannelId,
        message_id: &str,
    ) -> CowResult<Option<ConversationTurn>> {
        let row = sqlx::query_as::<_, ConversationTurnRow>(&format!(
            "SELECT {TURN_COLUMNS} FROM conversation_turns
             WHERE channel_id = ? AND message_id = ?"
        ))
        .bind(&channel_id.0)
        .bind(message_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ConversationTurnRow::into_domain).transpose()
    }

    /// List a channel's routing decisions, most recent first.
    pub async fn list_conversation_turns(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ConversationTurn>> {
        let rows = sqlx::query_as::<_, ConversationTurnRow>(&format!(
            "SELECT {TURN_COLUMNS} FROM conversation_turns
             WHERE channel_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"
        ))
        .bind(&channel_id.0)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(ConversationTurnRow::into_domain)
            .collect()
    }
}
