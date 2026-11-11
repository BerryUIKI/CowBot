//! Agent memory and shared notes (M4, ADR-0033).
//!
//! The service owns note and entry writes, validation, redaction, settings, and
//! deterministic recall. Recall never blocks a turn: callers that need a prompt
//! block use [`MemoryService::recall_for_prompt`], which logs and continues
//! without memory when the store fails.

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{
    rank_memories, redact_secrets, render_memory_block, render_notes_for_prompt,
    select_recalled_entries, MemoryEntry, MemoryKind, MemorySettings, MemorySource, MemoryStatus,
    Note, ScoredMemory, MEMORY_ENTRY_MAX_CHARS, MEMORY_NOTE_MAX_CHARS,
};
use cowbot_core::usage::estimate_tokens;
use cowbot_store::Store;

/// How a memory-relevant turn query is truncated before recall.
const RECALL_QUERY_MAX_CHARS: usize = 500;

/// Most pinned and matched entries considered for one recall.
const RECALL_CANDIDATE_LIMIT: u32 = 20;

/// Parameters for creating a memory entry.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub content: String,
    pub kind: MemoryKind,
    pub agent_id: Option<AgentId>,
    pub channel_id: Option<ChannelId>,
    pub pinned: bool,
    pub source: MemorySource,
}

impl NewMemory {
    /// A user-authored fact with no scope.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            kind: MemoryKind::Fact,
            agent_id: None,
            channel_id: None,
            pinned: false,
            source: MemorySource::User,
        }
    }

    pub fn with_kind(mut self, kind: MemoryKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_scope(mut self, agent_id: Option<AgentId>, channel_id: Option<ChannelId>) -> Self {
        self.agent_id = agent_id;
        self.channel_id = channel_id;
        self
    }

    pub fn with_pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.source = source;
        self
    }
}

/// Editable parts of an existing entry.
#[derive(Debug, Clone, Default)]
pub struct MemoryUpdate {
    pub content: Option<String>,
    pub kind: Option<MemoryKind>,
    pub pinned: Option<bool>,
}

/// Optional filters for listing entries.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub status: Option<MemoryStatus>,
    pub agent_id: Option<AgentId>,
    pub channel_id: Option<ChannelId>,
}

/// Editable workspace memory settings; `None` keeps the current value.
#[derive(Debug, Clone, Default)]
pub struct MemorySettingsUpdate {
    pub extraction_enabled: Option<bool>,
    pub recall_enabled: Option<bool>,
    pub notes_budget_tokens: Option<u32>,
    pub recall_budget_tokens: Option<u32>,
}

/// A saved note and whether redaction changed its content.
#[derive(Debug, Clone)]
pub struct NoteSave {
    pub note: Note,
    pub redacted: bool,
}

/// A saved memory entry and whether redaction changed its content.
#[derive(Debug, Clone)]
pub struct MemorySave {
    pub entry: MemoryEntry,
    pub redacted: bool,
}

/// The prompt block a turn would receive and the entries that shape it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecalledMemory {
    pub block: String,
    pub entry_ids: Vec<String>,
    pub tokens: u32,
}

/// Notes, memory entries, settings, and recall for one workspace.
#[derive(Clone)]
pub struct MemoryService {
    store: Store,
    workspace_id: WorkspaceId,
}

impl MemoryService {
    pub fn new(store: Store, workspace_id: WorkspaceId) -> Self {
        Self {
            store,
            workspace_id,
        }
    }

    /// The workspace note and, when a conversation is selected, its note.
    pub async fn notes(
        &self,
        channel_id: Option<&ChannelId>,
    ) -> CowResult<(Option<Note>, Option<Note>)> {
        let workspace = self.store.get_note(&self.workspace_id, None).await?;
        let conversation = match channel_id {
            Some(channel_id) => {
                self.store
                    .get_note(&self.workspace_id, Some(channel_id))
                    .await?
            }
            None => None,
        };
        Ok((workspace, conversation))
    }

    /// Save the workspace note, redacting credential-shaped content.
    pub async fn save_workspace_note(&self, content: &str) -> CowResult<NoteSave> {
        let content = validate_content(content, MEMORY_NOTE_MAX_CHARS, "a note")?;
        let redaction = redact_secrets(&content);
        let mut note = Note::workspace(self.workspace_id.clone(), redaction.content);
        note.updated_at = Utc::now();
        let note = self.store.save_note(&note).await?;
        Ok(NoteSave {
            note,
            redacted: redaction.redacted,
        })
    }

    /// Save a conversation note; only user-created conversations accept one.
    pub async fn save_conversation_note(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<NoteSave> {
        self.require_conversation(channel_id).await?;
        let content = validate_content(content, MEMORY_NOTE_MAX_CHARS, "a note")?;
        let redaction = redact_secrets(&content);
        let mut note = Note::conversation(
            self.workspace_id.clone(),
            channel_id.clone(),
            redaction.content,
        );
        note.updated_at = Utc::now();
        let note = self.store.save_note(&note).await?;
        Ok(NoteSave {
            note,
            redacted: redaction.redacted,
        })
    }

    pub async fn delete_conversation_note(&self, channel_id: &ChannelId) -> CowResult<u64> {
        self.store
            .delete_note(&self.workspace_id, Some(channel_id))
            .await
    }

    pub async fn settings(&self) -> CowResult<MemorySettings> {
        self.store.get_memory_settings(&self.workspace_id).await
    }

    pub async fn update_settings(
        &self,
        update: &MemorySettingsUpdate,
    ) -> CowResult<MemorySettings> {
        let mut settings = self.settings().await?;
        if let Some(enabled) = update.extraction_enabled {
            settings.extraction_enabled = enabled;
        }
        if let Some(enabled) = update.recall_enabled {
            settings.recall_enabled = enabled;
        }
        if update.notes_budget_tokens.is_some() || update.recall_budget_tokens.is_some() {
            settings.set_budgets(
                update
                    .notes_budget_tokens
                    .unwrap_or(settings.notes_budget_tokens),
                update
                    .recall_budget_tokens
                    .unwrap_or(settings.recall_budget_tokens),
            );
        } else {
            settings.updated_at = Utc::now();
        }
        self.store.update_memory_settings(&settings).await?;
        Ok(settings)
    }

    pub async fn list_entries(&self, filter: &MemoryFilter) -> CowResult<Vec<MemoryEntry>> {
        self.store
            .list_memory_entries(
                &self.workspace_id,
                filter.status,
                filter.agent_id.as_ref(),
                filter.channel_id.as_ref(),
            )
            .await
    }

    pub async fn pending_entries(&self) -> CowResult<Vec<MemoryEntry>> {
        self.list_entries(&MemoryFilter {
            status: Some(MemoryStatus::Pending),
            ..MemoryFilter::default()
        })
        .await
    }

    /// Create an entry after validating its content and scope.
    pub async fn add_entry(&self, new: NewMemory) -> CowResult<MemorySave> {
        let content = validate_content(&new.content, MEMORY_ENTRY_MAX_CHARS, "a memory entry")?;
        self.validate_scope(new.agent_id.as_ref(), new.channel_id.as_ref())
            .await?;
        let redaction = redact_secrets(&content);
        let mut entry = MemoryEntry::new(
            self.workspace_id.clone(),
            new.agent_id.clone(),
            new.channel_id.clone(),
            new.kind,
            redaction.content,
            new.source,
        );
        entry.pinned = new.pinned;
        entry.recount();
        self.store.insert_memory_entry(&entry).await?;
        Ok(MemorySave {
            entry,
            redacted: redaction.redacted,
        })
    }

    /// Edit the content, kind, or pinning of an entry.
    pub async fn update_entry(&self, id: &str, update: &MemoryUpdate) -> CowResult<MemorySave> {
        let mut entry = self.require_entry(id).await?;
        let mut redacted = false;
        if let Some(content) = &update.content {
            let content = validate_content(content, MEMORY_ENTRY_MAX_CHARS, "a memory entry")?;
            let outcome = redact_secrets(&content);
            redacted = outcome.redacted;
            entry.content = outcome.content;
            entry.recount();
        }
        if let Some(kind) = update.kind {
            entry.kind = kind;
        }
        if let Some(pinned) = update.pinned {
            entry.pinned = pinned;
        }
        entry.updated_at = Utc::now();
        self.store.update_memory_entry(&entry).await?;
        Ok(MemorySave { entry, redacted })
    }

    /// Approve a pending extraction candidate.
    pub async fn approve_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_status(id, MemoryStatus::Active).await
    }

    /// Dismiss a pending candidate, keeping it visible as archived.
    pub async fn dismiss_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_status(id, MemoryStatus::Archived).await
    }

    pub async fn archive_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_status(id, MemoryStatus::Archived).await
    }

    pub async fn delete_entry(&self, id: &str) -> CowResult<u64> {
        self.require_entry(id).await?;
        self.store.delete_memory_entry(id).await
    }

    /// What the next turn would inject, without touching recall statistics.
    pub async fn preview_recall(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<Option<RecalledMemory>> {
        self.recall(channel_id, agent_id, query, false).await
    }

    /// Recall memory for a turn and update the injected entries' statistics.
    pub async fn recall_for_turn(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<Option<RecalledMemory>> {
        self.recall(channel_id, agent_id, query, true).await
    }

    /// Recall a prompt block for an agent, logging failures instead of
    /// failing the turn.
    pub(crate) async fn recall_for_prompt(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> Option<String> {
        match self.recall(channel_id, agent_id, query, true).await {
            Ok(Some(recalled)) => Some(recalled.block),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Memory recall failed; continuing without memory"
                );
                None
            }
        }
    }

    async fn recall(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
        touch: bool,
    ) -> CowResult<Option<RecalledMemory>> {
        let settings = self.settings().await?;
        if !settings.recall_enabled {
            return Ok(None);
        }

        let (workspace_note, conversation_note) = self.notes(channel_id).await?;
        let notes = render_notes_for_prompt(
            workspace_note.as_ref(),
            conversation_note.as_ref(),
            settings.notes_budget_tokens,
        );

        let query = query
            .trim()
            .chars()
            .take(RECALL_QUERY_MAX_CHARS)
            .collect::<String>();
        let pinned = self
            .store
            .list_pinned_memory_entries(
                &self.workspace_id,
                channel_id,
                Some(agent_id),
                RECALL_CANDIDATE_LIMIT,
            )
            .await?;
        let hits = if query.is_empty() {
            Vec::new()
        } else {
            self.store
                .search_memory_entries(
                    &self.workspace_id,
                    channel_id,
                    Some(agent_id),
                    &query,
                    RECALL_CANDIDATE_LIMIT,
                )
                .await?
        };

        let mut candidates = Vec::with_capacity(pinned.len() + hits.len());
        for entry in &pinned {
            candidates.push(ScoredMemory { entry, score: None });
        }
        for (entry, score) in &hits {
            candidates.push(ScoredMemory {
                entry,
                score: Some(*score),
            });
        }

        let ranked = rank_memories(candidates);
        let selected = select_recalled_entries(&ranked, settings.recall_budget_tokens);
        let entry_ids = selected
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let Some(block) = render_memory_block(notes.as_deref(), &selected) else {
            return Ok(None);
        };

        if touch && !entry_ids.is_empty() {
            if let Err(error) = self.store.touch_memory_recalls(&entry_ids).await {
                tracing::warn!(
                    error = %error,
                    "Failed to record memory recall statistics"
                );
            }
        }

        Ok(Some(RecalledMemory {
            tokens: estimate_tokens(&block),
            block,
            entry_ids,
        }))
    }

    async fn set_status(&self, id: &str, status: MemoryStatus) -> CowResult<MemoryEntry> {
        let mut entry = self.require_entry(id).await?;
        entry.status = status;
        entry.updated_at = Utc::now();
        self.store.update_memory_entry(&entry).await?;
        Ok(entry)
    }

    async fn require_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        let Some(entry) = self.store.get_memory_entry(id).await? else {
            return Err(CowError::Config(format!(
                "memory entry '{id}' was not found"
            )));
        };
        if entry.workspace_id != self.workspace_id {
            return Err(CowError::Config(format!(
                "memory entry '{id}' was not found"
            )));
        }
        Ok(entry)
    }

    async fn require_conversation(&self, channel_id: &ChannelId) -> CowResult<Channel> {
        let Some(channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "conversation '{}' was not found",
                channel_id.0
            )));
        };
        if !self
            .store
            .channel_in_workspace(channel_id, &self.workspace_id)
            .await?
        {
            return Err(CowError::Config(format!(
                "conversation '{}' was not found",
                channel_id.0
            )));
        }
        if !channel.is_conversation() {
            return Err(CowError::Config(
                "notes can only be attached to user-created conversations".into(),
            ));
        }
        Ok(channel)
    }

    async fn validate_scope(
        &self,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
    ) -> CowResult<()> {
        if let Some(agent_id) = agent_id {
            if !self
                .store
                .agent_in_workspace(agent_id, &self.workspace_id)
                .await?
            {
                return Err(CowError::Config(format!(
                    "agent '{}' is not in this workspace",
                    agent_id.0
                )));
            }
        }
        if let Some(channel_id) = channel_id {
            if !self
                .store
                .channel_in_workspace(channel_id, &self.workspace_id)
                .await?
            {
                return Err(CowError::Config(format!(
                    "channel '{}' is not in this workspace",
                    channel_id.0
                )));
            }
        }
        Ok(())
    }
}

fn validate_content(content: &str, max: usize, label: &str) -> CowResult<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(CowError::Config(format!("{label} cannot be empty")));
    }
    let length = trimmed.chars().count();
    if length > max {
        return Err(CowError::Config(format!(
            "{label} must be at most {max} characters (got {length})"
        )));
    }
    Ok(trimmed.to_string())
}
