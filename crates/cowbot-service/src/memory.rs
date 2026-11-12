//! Agent memory and shared notes (M4, ADR-0033).
//!
//! The service owns note and entry writes, validation, redaction, settings, and
//! deterministic recall. Recall never blocks a turn: callers that need a prompt
//! block use [`MemoryService::recall_for_prompt`], which logs and continues
//! without memory when the store fails.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{
    plan_recalled_entries, rank_memories, redact_secrets, render_memory_block,
    render_notes_for_prompt, truncate_to_tokens, MemoryEntry, MemoryKind, MemorySettings,
    MemorySource, MemoryStatus, Note, RecallCandidate, RecallPreview, ScoredMemory,
    MEMORY_ENTRY_MAX_CHARS, MEMORY_EXTRACTED_MAX_CHARS, MEMORY_NOTE_MAX_CHARS,
};
use cowbot_core::usage::{estimate_tokens, LlmCallKind, LlmCallRecord};
use cowbot_llm::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage, LlmProvider};
use cowbot_store::Store;

use crate::prompt_snapshots::{seal_with_context, PromptSnapshotService, SnapshotContext};

/// How a memory-relevant turn query is truncated before recall.
const RECALL_QUERY_MAX_CHARS: usize = 500;

/// Most pinned and matched entries considered for one recall.
const RECALL_CANDIDATE_LIMIT: u32 = 20;

/// Most pending candidates one extraction may create.
const MAX_EXTRACTION_CANDIDATES: usize = 3;

/// Token budget for the transcript sent to the extractor.
const EXTRACTION_INPUT_TOKENS: u32 = 4_000;

/// Share of the transcript budget kept for the user message.
const EXTRACTION_USER_TOKENS: u32 = 1_000;

/// Instruction for the opt-in extraction call.
const EXTRACTION_SYSTEM_PROMPT: &str = "You maintain the long-term memory of a local agent \
workspace. Read one conversation turn and return the durable facts, user preferences, and \
decisions that will still matter in later conversations. Reply with one JSON object only: \
{\"memories\": [{\"content\": \"one self-contained sentence\", \"kind\": \"fact\"}]}. Kind is \
one of fact, preference, or decision. Return at most three entries. Skip transient details, \
restatements, and anything already listed as known memory. Return {\"memories\": []} when \
nothing is durable. Do not add any other text.";

/// One candidate parsed from an extraction response, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionCandidate {
    pub content: String,
    pub kind: MemoryKind,
}

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
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoteSave {
    pub note: Note,
    pub redacted: bool,
}

/// A saved memory entry and whether redaction changed its content.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemorySave {
    pub entry: MemoryEntry,
    pub redacted: bool,
}

/// The prompt block a turn would receive and the entries that shape it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RecalledMemory {
    pub block: String,
    pub entry_ids: Vec<String>,
    pub tokens: u32,
}

/// One recall computation shared by the prompt path and the inspector preview.
struct RecallDetails {
    preview: RecallPreview,
    selected_ids: Vec<String>,
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
        self.recall_for_prompt_memory(channel_id, agent_id, query)
            .await
            .map(|recalled| recalled.block)
    }

    /// The rendered block, its token estimate, and the selected entry ids.
    ///
    /// A prompt snapshot records the selection, not only the text, so the
    /// recall path returns the ids beside the block (ADR-0037).
    pub(crate) async fn recall_for_prompt_memory(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> Option<RecalledMemory> {
        match self.recall(channel_id, agent_id, query, true).await {
            Ok(Some(recalled)) => Some(recalled),
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

    /// What the next recall would inject, including every candidate it
    /// considered and why skipped candidates were left out.
    ///
    /// Computing this never touches recall statistics.
    pub async fn preview_recall_trace(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<RecallPreview> {
        Ok(self
            .recall_details(channel_id, agent_id, query)
            .await?
            .preview)
    }

    async fn recall(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
        touch: bool,
    ) -> CowResult<Option<RecalledMemory>> {
        let details = self.recall_details(channel_id, agent_id, query).await?;
        let Some(block) = details.preview.block.clone() else {
            return Ok(None);
        };

        if touch && !details.selected_ids.is_empty() {
            if let Err(error) = self.store.touch_memory_recalls(&details.selected_ids).await {
                tracing::warn!(
                    error = %error,
                    "Failed to record memory recall statistics"
                );
            }
        }

        Ok(Some(RecalledMemory {
            tokens: details.preview.estimated_tokens,
            block,
            entry_ids: details.selected_ids,
        }))
    }

    async fn recall_details(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<RecallDetails> {
        let settings = self.settings().await?;
        if !settings.recall_enabled {
            return Ok(RecallDetails {
                preview: RecallPreview {
                    enabled: false,
                    query: String::new(),
                    block: None,
                    estimated_tokens: 0,
                    budget_tokens: settings.recall_budget_tokens,
                    notes_budget_tokens: settings.notes_budget_tokens,
                    notes_tokens: 0,
                    candidates: Vec::new(),
                },
                selected_ids: Vec::new(),
            });
        }

        let (workspace_note, conversation_note) = self.notes(channel_id).await?;
        let notes = render_notes_for_prompt(
            workspace_note.as_ref(),
            conversation_note.as_ref(),
            settings.notes_budget_tokens,
        );
        let notes_tokens = notes.as_deref().map(estimate_tokens).unwrap_or(0);

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
        let plan = plan_recalled_entries(&ranked, settings.recall_budget_tokens);
        let selected_ids = plan
            .selected
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let block = render_memory_block(notes.as_deref(), &plan.selected);
        let estimated_tokens = block.as_deref().map(estimate_tokens).unwrap_or(0);
        let candidates = plan
            .candidates
            .iter()
            .map(|trace| RecallCandidate {
                entry_id: trace.entry.id.clone(),
                content: trace.entry.content.clone(),
                kind: trace.entry.kind,
                scope: trace.entry.scope(),
                pinned: trace.entry.pinned,
                score: trace.score,
                tokens: trace.tokens,
                selected: trace.selected,
                skip_reason: trace.skip_reason,
            })
            .collect();

        Ok(RecallDetails {
            preview: RecallPreview {
                enabled: true,
                query,
                block,
                estimated_tokens,
                budget_tokens: settings.recall_budget_tokens,
                notes_budget_tokens: settings.notes_budget_tokens,
                notes_tokens,
                candidates,
            },
            selected_ids,
        })
    }

    /// Extract up to three pending candidates from a settled conversation turn.
    ///
    /// Opt-in per workspace. The call is best-effort: a disabled setting, an
    /// empty reply set, or a provider, parse, or storage failure is logged and
    /// skipped, and never fails the turn. Candidates are stored as `pending`
    /// with the turn as their source reference, so nothing is injected until a
    /// user approves it.
    #[allow(clippy::too_many_arguments)]
    pub async fn extract_after_turn(
        &self,
        channel: &Channel,
        llm: Arc<dyn LlmProvider>,
        model: &str,
        user_message: &str,
        replies: &str,
        responder: Option<&AgentId>,
        source_ref: &str,
    ) -> Vec<MemoryEntry> {
        let settings = match self.settings().await {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(error = %error, "Memory extraction skipped: settings unavailable");
                return Vec::new();
            }
        };
        if !settings.extraction_enabled {
            return Vec::new();
        }
        if replies.trim().is_empty() {
            return Vec::new();
        }

        let transcript = extraction_transcript(channel, user_message, replies);
        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage::system(EXTRACTION_SYSTEM_PROMPT),
                ChatMessage::user(transcript),
            ],
            tools: Vec::new(),
            temperature: Some(0.0),
        };

        let response = match llm.chat_complete(&request).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "Memory extraction call failed");
                return Vec::new();
            }
        };
        self.record_extraction_usage(channel, model, responder, &request, &response)
            .await;

        let Some(content) = response
            .content
            .as_deref()
            .map(str::trim)
            .filter(|content| !content.is_empty())
        else {
            tracing::warn!("Memory extraction returned no text");
            return Vec::new();
        };
        let Some(candidates) = parse_extraction_response(content) else {
            tracing::warn!("Memory extraction returned no usable JSON");
            return Vec::new();
        };

        let existing = match self
            .store
            .list_memory_entries(
                &self.workspace_id,
                Some(MemoryStatus::Active),
                responder,
                Some(&channel.id),
            )
            .await
        {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(error = %error, "Memory extraction could not read existing entries");
                return Vec::new();
            }
        };
        let mut known = existing
            .iter()
            .filter(|entry| entry.agent_id.as_ref() == responder)
            .map(|entry| normalize_extracted_content(&entry.content))
            .collect::<HashSet<_>>();

        let mut created = Vec::new();
        for candidate in candidates {
            if created.len() >= MAX_EXTRACTION_CANDIDATES {
                break;
            }
            if candidate.content.chars().count() > MEMORY_EXTRACTED_MAX_CHARS {
                continue;
            }
            let redaction = redact_secrets(&candidate.content);
            let content = redaction.content.trim().to_string();
            if content.is_empty() || !known.insert(normalize_extracted_content(&content)) {
                continue;
            }
            let mut entry = MemoryEntry::candidate(
                self.workspace_id.clone(),
                responder.cloned(),
                Some(channel.id.clone()),
                candidate.kind,
                content,
            );
            entry.source_ref = Some(source_ref.to_string());
            if let Err(error) = self.store.insert_memory_entry(&entry).await {
                tracing::warn!(error = %error, "Failed to store an extracted memory candidate");
                continue;
            }
            created.push(entry);
        }
        created
    }

    async fn record_extraction_usage(
        &self,
        channel: &Channel,
        model: &str,
        responder: Option<&AgentId>,
        request: &ChatCompletionRequest,
        response: &ChatCompletionResponse,
    ) {
        let mut record = LlmCallRecord::new(LlmCallKind::MemoryExtract, model.to_string());
        record.channel_id = Some(channel.id.clone());
        record.agent_id = responder.cloned();
        record.usage = response.usage.clone();
        record.raw_usage = response.raw_usage.clone();
        if let Err(error) = self.store.record_llm_call(&record).await {
            tracing::warn!(
                error = %error,
                "Failed to record memory extraction usage"
            );
        }
        // The extraction prompt is a fixed instruction plus a transcript; the
        // snapshot records both so a human can see what was sent.
        let snapshots = PromptSnapshotService::new(self.store.clone(), self.workspace_id.clone());
        let counter = cowbot_llm::token_counter_for_model(&request.model);
        let slots = request
            .messages
            .first()
            .filter(|message| message.role == cowbot_llm::ChatRole::System)
            .map(|message| {
                vec![cowbot_core::prompt::PromptSlot {
                    name: cowbot_core::prompt::PromptSlotName::Instruction,
                    content: message.content.clone(),
                    tokens: counter.count(&message.content),
                }]
            })
            .unwrap_or_default();
        let context = SnapshotContext::default().with_slots(slots);
        let snapshot = seal_with_context(
            cowbot_llm::snapshot_request(&request.model, LlmCallKind::MemoryExtract, request),
            &context,
        );
        if let Err(error) = snapshots.record(&record.id, &snapshot).await {
            tracing::warn!(
                error = %error,
                "Failed to record the memory extraction prompt snapshot"
            );
        }
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

/// Parse an extractor response, keeping well-formed candidates only.
///
/// Code fences around the JSON are tolerated. Returns `None` when the reply is
/// not a JSON object with a `memories` array; individual invalid entries are
/// dropped so one bad candidate cannot discard the others.
pub fn parse_extraction_response(content: &str) -> Option<Vec<ExtractionCandidate>> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end]).ok()?;
    let memories = value.get("memories")?.as_array()?;
    Some(
        memories
            .iter()
            .filter_map(|item| {
                let content = item.get("content")?.as_str()?.trim();
                if content.is_empty() {
                    return None;
                }
                let kind = match item.get("kind")?.as_str()? {
                    "fact" => MemoryKind::Fact,
                    "preference" => MemoryKind::Preference,
                    "decision" => MemoryKind::Decision,
                    _ => return None,
                };
                Some(ExtractionCandidate {
                    content: content.to_string(),
                    kind,
                })
            })
            .collect(),
    )
}

/// Render the bounded extraction input: conversation name, user message, replies.
fn extraction_transcript(channel: &Channel, user_message: &str, replies: &str) -> String {
    let (user_message, _) = truncate_to_tokens(user_message.trim(), EXTRACTION_USER_TOKENS);
    let reply_budget = EXTRACTION_INPUT_TOKENS.saturating_sub(EXTRACTION_USER_TOKENS);
    let (replies, _) = truncate_to_tokens(replies.trim(), reply_budget);
    let transcript = format!(
        "Conversation: {name}\n\nUser message:\n{user_message}\n\nReplies:\n{replies}",
        name = channel.name,
    );
    truncate_to_tokens(&transcript, EXTRACTION_INPUT_TOKENS).0
}

/// Compare candidate text after whitespace and case normalization.
fn normalize_extracted_content(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
