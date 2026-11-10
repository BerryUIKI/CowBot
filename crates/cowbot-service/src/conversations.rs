use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::{
    compaction_split, project_context, CacheUsageSummary, CompactionOutcome, ContextEntry,
    ContextEntryKind, ContextEvent, ContextEventKind, ContextRole, ConversationContext,
    ConversationMode, ConversationSettings, ConversationSettingsUpdate,
    CONTEXT_SUMMARY_INPUT_TOKENS,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::Task;
use cowbot_core::usage::{estimate_tokens, LlmCallKind, LlmCallRecord, UsageTotals};
use cowbot_llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

use crate::channel_agents::{ChannelAgentService, ChannelTurnOptions, ChannelTurnReport};
use crate::channels::{normalize_channel_name, ChannelService};
use crate::scheduler::{AssignmentScheduler, CrewRunOptions};
use crate::session_decisions::SessionDecisions;

/// Longest conversation title accepted before a uniqueness suffix is added.
const MAX_TITLE_CHARS: usize = 68;

/// Application service for user-created conversation sessions (ADR-0030).
///
/// A single-agent conversation has one participant, a fixed model, and an
/// append-only context branch per participant. Display messages stay in
/// `messages`; the model request is assembled from `context_entries`.
#[derive(Clone)]
pub struct ConversationService {
    store: Store,
    bus: EventBus,
    workspace_id: WorkspaceId,
    channels: ChannelService,
    session_decisions: Option<Arc<SessionDecisions>>,
}

impl ConversationService {
    pub fn new(store: Store, bus: EventBus, workspace_id: WorkspaceId) -> Self {
        Self {
            channels: ChannelService::new(store.clone(), bus.clone()),
            store,
            bus,
            workspace_id,
            session_decisions: None,
        }
    }

    pub fn with_session_decisions(mut self, decisions: Option<Arc<SessionDecisions>>) -> Self {
        self.session_decisions = decisions;
        self
    }

    /// Create a single-agent conversation with a fixed provider and model.
    pub async fn create(
        &self,
        title: Option<&str>,
        agent_id: &AgentId,
        provider_id: Option<String>,
        model: &str,
    ) -> CowResult<Channel> {
        let agent = self
            .store
            .get_agent(agent_id)
            .await?
            .ok_or_else(|| CowError::Config(format!("agent '{}' was not found", agent_id.0)))?;
        let model = model.trim();
        if model.is_empty() {
            return Err(CowError::Config(
                "a conversation needs a model; configure a model provider first".into(),
            ));
        }

        let fallback_title = format!("Conversation with {}", agent.name);
        let base = title
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(&fallback_title);
        let base = truncate_title(&normalize_channel_name(base)?);
        let name = self.unique_name(&base).await?;

        let channel = Channel::conversation(name, format!("Conversation with {}", agent.name));
        let settings =
            ConversationSettings::new(channel.id.clone(), provider_id, model.to_string());
        self.store
            .create_conversation(
                &self.workspace_id,
                &channel,
                &settings,
                std::slice::from_ref(agent_id),
            )
            .await?;
        self.bus.publish(cowbot_core::event::Event::new(
            cowbot_core::event::EventPayload::ChannelCreated(channel.clone()),
        ));
        Ok(channel)
    }

    pub async fn settings(&self, channel_id: &ChannelId) -> CowResult<ConversationSettings> {
        self.store
            .get_conversation_settings(channel_id)
            .await?
            .ok_or_else(|| {
                CowError::Config(format!("conversation '{}' has no settings", channel_id.0))
            })
    }

    pub async fn update_settings(
        &self,
        channel_id: &ChannelId,
        update: &ConversationSettingsUpdate,
    ) -> CowResult<ConversationSettings> {
        let mut settings = self.settings(channel_id).await?;
        if let Some(auto_compact) = update.auto_compact {
            settings.auto_compact = auto_compact;
        }
        if let Some(threshold) = update.compact_threshold {
            if !(1..=100).contains(&threshold) {
                return Err(CowError::Config(
                    "the compaction threshold must be between 1 and 100".into(),
                ));
            }
            settings.compact_threshold = threshold;
        }
        settings.updated_at = chrono::Utc::now();
        self.store.update_conversation_settings(&settings).await?;
        Ok(settings)
    }

    /// Token breakdown, cache summaries, and fixed model of a conversation.
    pub async fn context(&self, channel_id: &ChannelId) -> CowResult<ConversationContext> {
        let channel = self.conversation_channel(channel_id).await?;
        let agent = self.single_participant(&channel).await?;
        let settings = self.settings(channel_id).await?;
        let agents = self.store.list_agents(&self.workspace_id).await?;
        let entries = self
            .store
            .list_context_entries(channel_id, &agent.id)
            .await?;
        let system_tokens = estimate_tokens(&conversation_system_prompt(&agent, &agents));
        let projection = project_context(system_tokens, 0, &entries);
        let cache_last_turn =
            cache_summary(&self.store.latest_run_usage_for_channel(channel_id).await?);
        let cache_conversation =
            cache_summary(&self.store.sum_llm_usage_for_channel(channel_id).await?);

        Ok(ConversationContext {
            channel_id: channel.id,
            agent_id: agent.id,
            model: settings.model,
            provider_id: settings.provider_id,
            projection,
            cache_last_turn,
            cache_conversation,
        })
    }

    /// Answer `message` in a conversation with its participant agent.
    pub async fn reply(
        &self,
        channel_id: &ChannelId,
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        options: &ChannelTurnOptions,
    ) -> CowResult<ChannelTurnReport> {
        let channel = self.conversation_channel(channel_id).await?;
        let settings = self.settings(channel_id).await?;
        if settings.mode != ConversationMode::Single {
            return Err(CowError::Config(
                "group conversations are not supported yet".into(),
            ));
        }
        let Some(agent) = self.participant_or_none(&channel).await? else {
            self.channels
                .post_system_notice(
                    &channel.id,
                    "This conversation has no agent participant. Create a new conversation.",
                )
                .await?;
            return Ok(ChannelTurnReport::default());
        };
        let agents = self.store.list_agents(&self.workspace_id).await?;

        // The user message enters the branch before assembly, so the model
        // request is exactly the active branch plus a fresh system prompt.
        let mut user_entry = ContextEntry::new(
            channel.id.clone(),
            agent.id.clone(),
            ContextRole::User,
            message.content.trim(),
        );
        user_entry.recount();
        self.store.append_context_entry(&user_entry).await?;

        let entries = self
            .store
            .list_context_entries(&channel.id, &agent.id)
            .await?;
        let messages = assemble_conversation_messages(&agent, &agents, &entries);

        let task = Task::new(
            message.content.trim(),
            "A concise reply addressed to the user in this conversation.",
        )
        .assign_to(agent.id.clone())
        .in_channel(channel.id.clone());
        let task_id = task.id.clone();

        let requester = match &message.sender {
            Sender::User(id) => AssignmentRequester::User(id.clone()),
            _ => AssignmentRequester::System,
        };
        let mut scheduler = AssignmentScheduler::new(
            self.store.clone(),
            self.bus.clone(),
            self.workspace_id.clone(),
            options.max_concurrency,
        )
        .with_scope()
        .with_model_permissions(options.use_model_permissions)
        .with_session_decisions(self.session_decisions.clone());
        scheduler.register_agent(agent.clone(), llm, tools, settings.model.clone());

        let run_options = CrewRunOptions {
            approval_level: options.approval_level,
            channel_id: Some(channel.id.clone()),
            max_concurrency: options.max_concurrency,
            use_model_permissions: options.use_model_permissions,
        };
        scheduler
            .enqueue_conversation_turn(
                &task,
                requester,
                AssignmentOrigin::User,
                &run_options,
                messages,
                channel.id.clone(),
                agent.id.clone(),
            )
            .await?;

        let outcome = scheduler.run_until_idle().await;
        self.channels.post_task_outcome(&channel, &task_id).await?;
        outcome?;

        Ok(ChannelTurnReport {
            agents: vec![agent.id],
            unknown_mentions: Vec::new(),
            tasks: vec![task_id],
        })
    }

    /// Summarize older context entries and exclude them from future requests.
    ///
    /// Display messages are never touched. A system notice tells the user what
    /// changed, and a context event records the token counts.
    pub async fn compact(
        &self,
        channel_id: &ChannelId,
        llm: Arc<dyn LlmProvider>,
        model: &str,
    ) -> CowResult<CompactionOutcome> {
        let channel = self.conversation_channel(channel_id).await?;
        let agent = self.single_participant(&channel).await?;
        let entries = self
            .store
            .list_context_entries(&channel.id, &agent.id)
            .await?;
        let active = entries
            .iter()
            .filter(|entry| !entry.excluded)
            .cloned()
            .collect::<Vec<_>>();
        let tokens_before = active
            .iter()
            .fold(0_u32, |total, entry| total.saturating_add(entry.tokens));

        let split = compaction_split(&active);
        if split == 0 {
            return Ok(CompactionOutcome {
                compacted: false,
                summarized_entries: 0,
                tokens_before,
                tokens_after: tokens_before,
                summary: None,
            });
        }

        let summarized = &active[..split];
        let kept = &active[split..];
        let transcript = summary_transcript(summarized);
        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage::system(
                    "You compress conversation history for an AI assistant. Write a concise \
                     summary that preserves decisions, facts, open questions, and user \
                     preferences. Do not add new information. Reply with the summary only.",
                ),
                ChatMessage::user(format!(
                    "Summarize the earlier part of this conversation:\n\n{transcript}"
                )),
            ],
            tools: Vec::new(),
            temperature: Some(0.2),
        };
        let response = llm.chat_complete(&request).await?;
        self.record_compaction_usage(&channel, &agent, model, &response)
            .await;

        let summary_text = response
            .content
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                "The earlier conversation was summarized, but the model returned no text."
                    .to_string()
            });
        let summary = ContextEntry::summary(
            channel.id.clone(),
            agent.id.clone(),
            format!("Summary of the earlier conversation:\n\n{summary_text}"),
        );
        let kept_tokens = kept
            .iter()
            .fold(0_u32, |total, entry| total.saturating_add(entry.tokens));
        let tokens_after = kept_tokens.saturating_add(summary.tokens);

        let mut event = ContextEvent::new(
            channel.id.clone(),
            agent.id.clone(),
            ContextEventKind::Compact,
            tokens_before,
            tokens_after,
        );
        event.detail = Some(format!("summarized {} entries", summarized.len()));
        let excluded_ids = summarized
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        self.store
            .compact_context(&summary, &excluded_ids, &event)
            .await?;

        self.channels
            .post_system_notice(
                &channel.id,
                &format!(
                    "Context compacted: {} earlier entries were summarized ({} → {} tokens). \
                     Earlier messages stay visible but are no longer sent to the model.",
                    summarized.len(),
                    tokens_before,
                    tokens_after
                ),
            )
            .await?;

        Ok(CompactionOutcome {
            compacted: true,
            summarized_entries: summarized.len() as u32,
            tokens_before,
            tokens_after,
            summary: Some(summary_text),
        })
    }

    /// Exclude every active entry from future model requests.
    ///
    /// The rows and display messages stay; the model starts from an empty
    /// context after this call.
    pub async fn clear_context(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let channel = self.conversation_channel(channel_id).await?;
        let agent = self.single_participant(&channel).await?;
        let entries = self
            .store
            .list_context_entries(&channel.id, &agent.id)
            .await?;
        let tokens_before = entries
            .iter()
            .filter(|entry| !entry.excluded)
            .fold(0_u32, |total, entry| total.saturating_add(entry.tokens));
        let cleared = self.store.exclude_all_context_entries(&channel.id).await?;
        if cleared > 0 {
            let event = ContextEvent::new(
                channel.id.clone(),
                agent.id.clone(),
                ContextEventKind::Clear,
                tokens_before,
                0,
            );
            self.store.record_context_event(&event).await?;
            self.channels
                .post_system_notice(
                    &channel.id,
                    "Context cleared. Earlier messages stay visible but are no longer sent to \
                     the model.",
                )
                .await?;
        }
        Ok(cleared)
    }

    async fn conversation_channel(&self, channel_id: &ChannelId) -> CowResult<Channel> {
        let Some(channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Config(format!(
                "conversation '{}' was not found",
                channel_id.0
            )));
        };
        if !channel.is_conversation() {
            return Err(CowError::Config(format!(
                "'{}' is not a conversation",
                channel.name
            )));
        }
        Ok(channel)
    }

    async fn participant_or_none(&self, channel: &Channel) -> CowResult<Option<AgentProfile>> {
        let participants = self.store.list_channel_participants(&channel.id).await?;
        let Some(participant) = participants.first() else {
            return Ok(None);
        };
        self.store.get_agent(&participant.agent_id).await
    }

    async fn single_participant(&self, channel: &Channel) -> CowResult<AgentProfile> {
        let participants = self.store.list_channel_participants(&channel.id).await?;
        match participants.as_slice() {
            [participant] => self
                .store
                .get_agent(&participant.agent_id)
                .await?
                .ok_or_else(|| {
                    CowError::Config(format!(
                        "the agent of conversation '{}' no longer exists",
                        channel.name
                    ))
                }),
            [] => Err(CowError::Config(format!(
                "conversation '{}' has no participant",
                channel.name
            ))),
            _ => Err(CowError::Config(
                "group conversations are not supported yet".into(),
            )),
        }
    }

    async fn record_compaction_usage(
        &self,
        channel: &Channel,
        agent: &AgentProfile,
        model: &str,
        response: &cowbot_llm::ChatCompletionResponse,
    ) {
        let mut record = LlmCallRecord::new(LlmCallKind::Compact, model.to_string());
        record.channel_id = Some(channel.id.clone());
        record.agent_id = Some(agent.id.clone());
        record.usage = response.usage.clone();
        record.raw_usage = response.raw_usage.clone();
        if let Err(error) = self.store.record_llm_call(&record).await {
            tracing::warn!(error = %error, "Failed to record compaction usage");
        }
    }

    async fn unique_name(&self, base: &str) -> CowResult<String> {
        let mut candidate = base.to_string();
        let mut suffix = 2_u32;
        while self
            .store
            .channel_name_taken(&self.workspace_id, &candidate, None)
            .await?
        {
            candidate = format!("{base} ({suffix})");
            suffix += 1;
            if suffix > 10_000 {
                return Err(CowError::Config(
                    "could not find a free conversation name".into(),
                ));
            }
        }
        Ok(candidate)
    }
}

fn truncate_title(title: &str) -> String {
    let chars = title.chars().collect::<Vec<_>>();
    if chars.len() <= MAX_TITLE_CHARS {
        return title.to_string();
    }
    chars[..MAX_TITLE_CHARS]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn cache_summary(totals: &UsageTotals) -> CacheUsageSummary {
    CacheUsageSummary {
        calls: totals.calls,
        cached_input_tokens: totals.cached_input_tokens,
        total_input_tokens: totals.total_input_tokens(),
    }
}

/// System prompt for a conversation turn.
pub(crate) fn conversation_system_prompt(agent: &AgentProfile, agents: &[AgentProfile]) -> String {
    format!(
        "{}\n\n{}\n\n\
         You are in a private, continuous conversation with the user. The messages that \
         follow are the retained conversation context. Reply to the latest user message, \
         use tools when they help, and do not restate the context.",
        agent.build_system_prompt(),
        ChannelAgentService::platform_context(agents)
    )
}

/// Assemble the model request from the conversation's active branch.
///
/// The order is deterministic: system prompt first, then entries by sequence.
pub(crate) fn assemble_conversation_messages(
    agent: &AgentProfile,
    agents: &[AgentProfile],
    entries: &[ContextEntry],
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(conversation_system_prompt(
        agent, agents,
    ))];
    messages.extend(
        entries
            .iter()
            .filter(|entry| !entry.excluded)
            .map(entry_to_message),
    );
    messages
}

fn entry_to_message(entry: &ContextEntry) -> ChatMessage {
    let mut message = match entry.role {
        ContextRole::System => ChatMessage::system(entry.content.clone()),
        ContextRole::User => ChatMessage::user(entry.content.clone()),
        ContextRole::Assistant => ChatMessage::assistant(entry.content.clone()),
        ContextRole::Tool => ChatMessage::tool(
            entry.content.clone(),
            entry.tool_call_id.clone().unwrap_or_default(),
        ),
    };
    message.tool_calls = entry.tool_calls.as_ref().map(|calls| {
        calls
            .iter()
            .map(|call| cowbot_llm::LlmToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect()
    });
    message.tool_call_id = entry.tool_call_id.clone();
    message
}

/// Render summarized entries newest-first within the summarizer's input budget.
fn summary_transcript(entries: &[ContextEntry]) -> String {
    let mut selected: Vec<&ContextEntry> = Vec::new();
    let mut tokens = 0_u32;
    for entry in entries.iter().rev() {
        if tokens.saturating_add(entry.tokens) > CONTEXT_SUMMARY_INPUT_TOKENS
            && !selected.is_empty()
        {
            break;
        }
        tokens = tokens.saturating_add(entry.tokens);
        selected.push(entry);
    }
    selected.reverse();

    let mut transcript = String::new();
    if selected.len() < entries.len() {
        transcript.push_str("(Earlier messages were omitted from this summary.)\n\n");
    }
    for entry in selected {
        transcript.push_str(&render_entry(entry));
        transcript.push('\n');
    }
    transcript.trim_end().to_string()
}

fn render_entry(entry: &ContextEntry) -> String {
    let role = match entry.role {
        ContextRole::System => "system",
        ContextRole::User => "user",
        ContextRole::Assistant => "assistant",
        ContextRole::Tool => "tool",
    };
    let mut rendered = format!("[{role}] {}", entry.content.trim());
    if let Some(calls) = &entry.tool_calls {
        for call in calls {
            rendered.push_str(&format!(
                "\n  tool call {} {} {}",
                call.id, call.name, call.arguments
            ));
        }
    }
    if entry.kind == ContextEntryKind::Summary {
        rendered.insert_str(0, "[previous summary] ");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::conversation::ContextToolCall;
    use cowbot_llm::ChatRole;

    fn entry(role: ContextRole, content: &str) -> ContextEntry {
        let mut entry = ContextEntry::new(
            ChannelId::new("channel"),
            AgentId::new("agent"),
            role,
            content,
        );
        entry.recount();
        entry
    }

    #[test]
    fn assembly_is_deterministic_and_skips_excluded_entries() {
        let agent = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
        let mut excluded = entry(ContextRole::User, "forgotten");
        excluded.excluded = true;
        let entries = vec![
            entry(ContextRole::User, "first question"),
            excluded,
            entry(ContextRole::Assistant, "first answer"),
            entry(ContextRole::User, "latest question"),
        ];

        let first = assemble_conversation_messages(&agent, std::slice::from_ref(&agent), &entries);
        let second = assemble_conversation_messages(&agent, std::slice::from_ref(&agent), &entries);

        assert_eq!(first.len(), second.len());
        for (left, right) in first.iter().zip(second.iter()) {
            assert_eq!(left.role, right.role);
            assert_eq!(left.content, right.content);
        }
        assert_eq!(first[0].role, ChatRole::System);
        assert_eq!(first[1].content, "first question");
        assert_eq!(first[2].content, "first answer");
        assert_eq!(first[3].content, "latest question");
        assert!(!first.iter().any(|message| message.content == "forgotten"));
    }

    #[test]
    fn assembly_preserves_tool_messages() {
        let agent = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
        let mut assistant = entry(ContextRole::Assistant, "reading");
        assistant.tool_calls = Some(vec![ContextToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "a.txt" }),
        }]);
        assistant.recount();
        let mut tool = entry(ContextRole::Tool, "file contents");
        tool.tool_call_id = Some("call-1".into());
        tool.recount();

        let messages = assemble_conversation_messages(
            &agent,
            std::slice::from_ref(&agent),
            &[assistant, tool],
        );
        assert_eq!(
            messages[1].tool_calls.as_ref().unwrap()[0].name,
            "read_file"
        );
        assert_eq!(messages[2].role, ChatRole::Tool);
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-1"));
    }

    #[test]
    fn summary_transcript_keeps_recent_entries_within_budget() {
        let entries = vec![
            entry(ContextRole::User, "old"),
            entry(ContextRole::Assistant, "answer"),
        ];
        let transcript = summary_transcript(&entries);
        assert!(transcript.contains("[user] old"));
        assert!(transcript.contains("[assistant] answer"));
        assert!(!transcript.contains("omitted"));
    }
}
