use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{
    clamp_memory_budget, MemoryEntry, MemoryKind, MemorySettings, MemorySource, MemoryStatus, Note,
};

use crate::error::{db, parse_datetime};
use crate::store::Store;

const NOTE_COLUMNS: &str = "id, workspace_id, channel_id, title, content, created_at, updated_at";

const ENTRY_COLUMNS: &str = "id, workspace_id, agent_id, channel_id, kind, content, source,
     source_ref, status, pinned, tokens, recall_count, last_recalled_at, created_at, updated_at";

const ENTRY_COLUMNS_M: &str = "m.id, m.workspace_id, m.agent_id, m.channel_id, m.kind, m.content,
     m.source, m.source_ref, m.status, m.pinned, m.tokens, m.recall_count, m.last_recalled_at,
     m.created_at, m.updated_at";

#[derive(sqlx::FromRow)]
struct NoteRow {
    id: String,
    workspace_id: String,
    channel_id: Option<String>,
    title: String,
    content: String,
    created_at: String,
    updated_at: String,
}

impl NoteRow {
    fn into_domain(self) -> CowResult<Note> {
        Ok(Note {
            id: self.id,
            workspace_id: WorkspaceId(self.workspace_id),
            channel_id: self.channel_id.map(ChannelId),
            title: self.title,
            content: self.content,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct MemoryEntryRow {
    id: String,
    workspace_id: String,
    agent_id: Option<String>,
    channel_id: Option<String>,
    kind: String,
    content: String,
    source: String,
    source_ref: Option<String>,
    status: String,
    pinned: i64,
    tokens: i64,
    recall_count: i64,
    last_recalled_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl MemoryEntryRow {
    fn into_domain(self) -> CowResult<MemoryEntry> {
        let kind = self
            .kind
            .parse::<MemoryKind>()
            .map_err(|_| CowError::Storage(format!("unknown memory kind '{}'", self.kind)))?;
        let source = self
            .source
            .parse::<MemorySource>()
            .map_err(|_| CowError::Storage(format!("unknown memory source '{}'", self.source)))?;
        let status = self
            .status
            .parse::<MemoryStatus>()
            .map_err(|_| CowError::Storage(format!("unknown memory status '{}'", self.status)))?;
        Ok(MemoryEntry {
            id: self.id,
            workspace_id: WorkspaceId(self.workspace_id),
            agent_id: self.agent_id.map(AgentId),
            channel_id: self.channel_id.map(ChannelId),
            kind,
            content: self.content,
            source,
            source_ref: self.source_ref,
            status,
            pinned: self.pinned != 0,
            tokens: self.tokens.clamp(0, u32::MAX as i64) as u32,
            recall_count: self.recall_count.clamp(0, u32::MAX as i64) as u32,
            last_recalled_at: self
                .last_recalled_at
                .as_deref()
                .map(parse_datetime)
                .transpose()?,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct MemoryHitRow {
    #[sqlx(flatten)]
    entry: MemoryEntryRow,
    score: f64,
}

#[derive(sqlx::FromRow)]
struct MemorySettingsRow {
    workspace_id: String,
    extraction_enabled: i64,
    recall_enabled: i64,
    notes_budget_tokens: i64,
    recall_budget_tokens: i64,
    updated_at: String,
}

impl MemorySettingsRow {
    fn into_domain(self) -> CowResult<MemorySettings> {
        Ok(MemorySettings {
            workspace_id: WorkspaceId(self.workspace_id),
            extraction_enabled: self.extraction_enabled != 0,
            recall_enabled: self.recall_enabled != 0,
            notes_budget_tokens: clamp_memory_budget(
                self.notes_budget_tokens.clamp(0, u32::MAX as i64) as u32,
            ),
            recall_budget_tokens: clamp_memory_budget(
                self.recall_budget_tokens.clamp(0, u32::MAX as i64) as u32,
            ),
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

impl Store {
    pub async fn get_note(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
    ) -> CowResult<Option<Note>> {
        let sql = match channel_id {
            Some(_) => format!(
                "SELECT {NOTE_COLUMNS} FROM notes
                 WHERE workspace_id = ? AND channel_id = ?"
            ),
            None => format!(
                "SELECT {NOTE_COLUMNS} FROM notes
                 WHERE workspace_id = ? AND channel_id IS NULL"
            ),
        };
        let mut query = sqlx::query_as::<_, NoteRow>(&sql).bind(&workspace_id.0);
        if let Some(channel_id) = channel_id {
            query = query.bind(&channel_id.0);
        }
        let row = query.fetch_optional(self.pool()).await.map_err(db)?;
        row.map(NoteRow::into_domain).transpose()
    }

    /// Insert or update the note at its workspace or conversation scope.
    ///
    /// The stored row keeps its original id and creation time when it already
    /// exists; the returned note is the persisted record.
    pub async fn save_note(&self, note: &Note) -> CowResult<Note> {
        let existing = self
            .get_note(&note.workspace_id, note.channel_id.as_ref())
            .await?;
        match existing {
            Some(mut existing) => {
                sqlx::query("UPDATE notes SET title = ?, content = ?, updated_at = ? WHERE id = ?")
                    .bind(&note.title)
                    .bind(&note.content)
                    .bind(note.updated_at.to_rfc3339())
                    .bind(&existing.id)
                    .execute(self.pool())
                    .await
                    .map_err(db)?;
                existing.title = note.title.clone();
                existing.content = note.content.clone();
                existing.updated_at = note.updated_at;
                Ok(existing)
            }
            None => {
                sqlx::query(
                    "INSERT INTO notes
                        (id, workspace_id, channel_id, title, content, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&note.id)
                .bind(&note.workspace_id.0)
                .bind(note.channel_id.as_ref().map(|channel| &channel.0))
                .bind(&note.title)
                .bind(&note.content)
                .bind(note.created_at.to_rfc3339())
                .bind(note.updated_at.to_rfc3339())
                .execute(self.pool())
                .await
                .map_err(db)?;
                Ok(note.clone())
            }
        }
    }

    pub async fn delete_note(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
    ) -> CowResult<u64> {
        let result = match channel_id {
            Some(channel_id) => {
                sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND channel_id = ?")
                    .bind(&workspace_id.0)
                    .bind(&channel_id.0)
            }
            None => sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND channel_id IS NULL")
                .bind(&workspace_id.0),
        }
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn list_all_notes(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Note>> {
        let rows = sqlx::query_as::<_, NoteRow>(&format!(
            "SELECT {NOTE_COLUMNS} FROM notes WHERE workspace_id = ?
             ORDER BY channel_id, id"
        ))
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(NoteRow::into_domain).collect()
    }

    /// Persist an entry; the insert trigger indexes its content.
    pub async fn insert_memory_entry(&self, entry: &MemoryEntry) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO memory_entries
                (id, workspace_id, agent_id, channel_id, kind, content, source, source_ref,
                 status, pinned, tokens, recall_count, last_recalled_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.workspace_id.0)
        .bind(entry.agent_id.as_ref().map(|agent| &agent.0))
        .bind(entry.channel_id.as_ref().map(|channel| &channel.0))
        .bind(entry.kind.as_str())
        .bind(&entry.content)
        .bind(entry.source.as_str())
        .bind(&entry.source_ref)
        .bind(entry.status.as_str())
        .bind(if entry.pinned { 1i64 } else { 0i64 })
        .bind(i64::from(entry.tokens))
        .bind(i64::from(entry.recall_count))
        .bind(entry.last_recalled_at.map(|at| at.to_rfc3339()))
        .bind(entry.created_at.to_rfc3339())
        .bind(entry.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Update every mutable column; the update trigger reindexes content.
    pub async fn update_memory_entry(&self, entry: &MemoryEntry) -> CowResult<()> {
        sqlx::query(
            "UPDATE memory_entries
             SET agent_id = ?, channel_id = ?, kind = ?, content = ?, source = ?, source_ref = ?,
                 status = ?, pinned = ?, tokens = ?, recall_count = ?, last_recalled_at = ?,
                 updated_at = ?
             WHERE id = ?",
        )
        .bind(entry.agent_id.as_ref().map(|agent| &agent.0))
        .bind(entry.channel_id.as_ref().map(|channel| &channel.0))
        .bind(entry.kind.as_str())
        .bind(&entry.content)
        .bind(entry.source.as_str())
        .bind(&entry.source_ref)
        .bind(entry.status.as_str())
        .bind(if entry.pinned { 1i64 } else { 0i64 })
        .bind(i64::from(entry.tokens))
        .bind(i64::from(entry.recall_count))
        .bind(entry.last_recalled_at.map(|at| at.to_rfc3339()))
        .bind(entry.updated_at.to_rfc3339())
        .bind(&entry.id)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Delete an entry; the delete trigger removes its index row.
    pub async fn delete_memory_entry(&self, id: &str) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM memory_entries WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn get_memory_entry(&self, id: &str) -> CowResult<Option<MemoryEntry>> {
        let row = sqlx::query_as::<_, MemoryEntryRow>(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(MemoryEntryRow::into_domain).transpose()
    }

    /// List entries with optional exact scope and status filters.
    pub async fn list_memory_entries(
        &self,
        workspace_id: &WorkspaceId,
        status: Option<MemoryStatus>,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
    ) -> CowResult<Vec<MemoryEntry>> {
        let mut sql = format!("SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE workspace_id = ?");
        if status.is_some() {
            sql.push_str(" AND status = ?");
        }
        if agent_id.is_some() {
            sql.push_str(" AND agent_id = ?");
        }
        if channel_id.is_some() {
            sql.push_str(" AND channel_id = ?");
        }
        sql.push_str(" ORDER BY updated_at DESC, id");

        let mut query = sqlx::query_as::<_, MemoryEntryRow>(&sql).bind(&workspace_id.0);
        if let Some(status) = status {
            query = query.bind(status.as_str());
        }
        if let Some(agent_id) = agent_id {
            query = query.bind(&agent_id.0);
        }
        if let Some(channel_id) = channel_id {
            query = query.bind(&channel_id.0);
        }
        let rows = query.fetch_all(self.pool()).await.map_err(db)?;
        rows.into_iter().map(MemoryEntryRow::into_domain).collect()
    }

    /// Every entry in a workspace, used by workspace export.
    pub async fn list_all_memory_entries(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<MemoryEntry>> {
        let rows = sqlx::query_as::<_, MemoryEntryRow>(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE workspace_id = ?
             ORDER BY created_at, id"
        ))
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(MemoryEntryRow::into_domain).collect()
    }

    /// Active pinned entries that apply to a turn in the given scopes.
    pub async fn list_pinned_memory_entries(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
        agent_id: Option<&AgentId>,
        limit: u32,
    ) -> CowResult<Vec<MemoryEntry>> {
        let mut sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries
             WHERE workspace_id = ? AND status = ? AND pinned = 1"
        );
        push_scope_clause(&mut sql, "channel_id", channel_id.is_some());
        push_scope_clause(&mut sql, "agent_id", agent_id.is_some());
        sql.push_str(" ORDER BY updated_at DESC, id LIMIT ?");

        let mut query = sqlx::query_as::<_, MemoryEntryRow>(&sql)
            .bind(&workspace_id.0)
            .bind(MemoryStatus::Active.as_str());
        if let Some(channel_id) = channel_id {
            query = query.bind(&channel_id.0);
        }
        if let Some(agent_id) = agent_id {
            query = query.bind(&agent_id.0);
        }
        let rows = query
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        rows.into_iter().map(MemoryEntryRow::into_domain).collect()
    }

    /// Search active entries in scope with FTS5, falling back to LIKE.
    ///
    /// Returns entries and their `bm25` score (lower is better). LIKE
    /// fallbacks score `0.0`, leaving recency and scope to order them.
    pub async fn search_memory_entries(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
        agent_id: Option<&AgentId>,
        query_text: &str,
        limit: u32,
    ) -> CowResult<Vec<(MemoryEntry, f64)>> {
        let Some(fts_query) = build_fts_query(query_text) else {
            return self
                .search_memory_entries_like(workspace_id, channel_id, agent_id, query_text, limit)
                .await;
        };

        let mut scope = String::new();
        push_scope_clause(&mut scope, "m.channel_id", channel_id.is_some());
        push_scope_clause(&mut scope, "m.agent_id", agent_id.is_some());
        let sql = format!(
            "SELECT {ENTRY_COLUMNS_M}, bm25(memory_entries_fts) AS score
             FROM memory_entries_fts
             JOIN memory_entries m ON m.id = memory_entries_fts.id
             WHERE memory_entries_fts MATCH ? AND m.workspace_id = ? AND m.status = ?{scope}
             ORDER BY score, m.updated_at DESC, m.id
             LIMIT ?"
        );
        let mut query = sqlx::query_as::<_, MemoryHitRow>(&sql)
            .bind(fts_query)
            .bind(&workspace_id.0)
            .bind(MemoryStatus::Active.as_str());
        if let Some(channel_id) = channel_id {
            query = query.bind(&channel_id.0);
        }
        if let Some(agent_id) = agent_id {
            query = query.bind(&agent_id.0);
        }
        let rows = query
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db)?;

        // The trigram tokenizer cannot match two-character terms; fall back so
        // short Chinese and Latin queries still recall.
        if rows.is_empty() {
            return self
                .search_memory_entries_like(workspace_id, channel_id, agent_id, query_text, limit)
                .await;
        }

        rows.into_iter()
            .map(|row| Ok((row.entry.into_domain()?, row.score)))
            .collect()
    }

    async fn search_memory_entries_like(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
        agent_id: Option<&AgentId>,
        query_text: &str,
        limit: u32,
    ) -> CowResult<Vec<(MemoryEntry, f64)>> {
        let needle = query_text.trim();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let mut scope = String::new();
        push_scope_clause(&mut scope, "channel_id", channel_id.is_some());
        push_scope_clause(&mut scope, "agent_id", agent_id.is_some());
        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries
             WHERE workspace_id = ? AND status = ?{scope}
               AND content LIKE ? ESCAPE '\\'
             ORDER BY pinned DESC, updated_at DESC, id
             LIMIT ?"
        );
        let mut query = sqlx::query_as::<_, MemoryEntryRow>(&sql)
            .bind(&workspace_id.0)
            .bind(MemoryStatus::Active.as_str());
        if let Some(channel_id) = channel_id {
            query = query.bind(&channel_id.0);
        }
        if let Some(agent_id) = agent_id {
            query = query.bind(&agent_id.0);
        }
        let rows = query
            .bind(like_pattern(needle))
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        rows.into_iter()
            .map(|row| Ok((MemoryEntryRow::into_domain(row)?, 0.0)))
            .collect()
    }

    /// Increment recall statistics for the entries injected in a prompt.
    pub async fn touch_memory_recalls(&self, ids: &[String]) -> CowResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE memory_entries
             SET recall_count = recall_count + 1, last_recalled_at = ?
             WHERE id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(Utc::now().to_rfc3339());
        for id in ids {
            query = query.bind(id);
        }
        query.execute(self.pool()).await.map_err(db)?;
        Ok(())
    }

    /// Read the workspace memory settings, creating defaults on first use.
    pub async fn get_memory_settings(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<MemorySettings> {
        let defaults = MemorySettings::new(workspace_id.clone());
        sqlx::query(
            "INSERT OR IGNORE INTO workspace_memory_settings
                (workspace_id, extraction_enabled, recall_enabled, notes_budget_tokens,
                 recall_budget_tokens, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace_id.0)
        .bind(if defaults.extraction_enabled {
            1i64
        } else {
            0i64
        })
        .bind(if defaults.recall_enabled { 1i64 } else { 0i64 })
        .bind(i64::from(defaults.notes_budget_tokens))
        .bind(i64::from(defaults.recall_budget_tokens))
        .bind(defaults.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;

        let row = sqlx::query_as::<_, MemorySettingsRow>(
            "SELECT workspace_id, extraction_enabled, recall_enabled, notes_budget_tokens,
                    recall_budget_tokens, updated_at
             FROM workspace_memory_settings WHERE workspace_id = ?",
        )
        .bind(&workspace_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        row.into_domain()
    }

    pub async fn update_memory_settings(&self, settings: &MemorySettings) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO workspace_memory_settings
                (workspace_id, extraction_enabled, recall_enabled, notes_budget_tokens,
                 recall_budget_tokens, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(workspace_id) DO UPDATE SET
                extraction_enabled = excluded.extraction_enabled,
                recall_enabled = excluded.recall_enabled,
                notes_budget_tokens = excluded.notes_budget_tokens,
                recall_budget_tokens = excluded.recall_budget_tokens,
                updated_at = excluded.updated_at",
        )
        .bind(&settings.workspace_id.0)
        .bind(if settings.extraction_enabled {
            1i64
        } else {
            0i64
        })
        .bind(if settings.recall_enabled { 1i64 } else { 0i64 })
        .bind(i64::from(clamp_memory_budget(settings.notes_budget_tokens)))
        .bind(i64::from(clamp_memory_budget(
            settings.recall_budget_tokens,
        )))
        .bind(settings.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }
}

/// Append a scope condition: a scoped turn sees unscoped and matching rows; an
/// unscoped turn sees only unscoped rows.
fn push_scope_clause(sql: &mut String, column: &str, scoped: bool) {
    if scoped {
        sql.push_str(&format!(" AND ({column} IS NULL OR {column} = ?)"));
    } else {
        sql.push_str(&format!(" AND {column} IS NULL"));
    }
}

/// Build an FTS5 OR query from trigrams and words in the text.
fn build_fts_query(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split(|character: char| !character.is_alphanumeric()) {
        let characters = word.chars().collect::<Vec<_>>();
        if characters.len() < 3 {
            continue;
        }
        let word = characters.iter().collect::<String>();
        if !terms.contains(&word) {
            terms.push(word);
        }
        for window in characters.windows(3) {
            let trigram = window.iter().collect::<String>();
            if !terms.contains(&trigram) {
                terms.push(trigram);
            }
        }
        if terms.len() >= 24 {
            break;
        }
    }
    terms.truncate(24);
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

fn like_pattern(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len() + 2);
    escaped.push('%');
    for character in needle.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}
