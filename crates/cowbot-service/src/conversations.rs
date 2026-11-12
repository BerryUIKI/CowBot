use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::{
    compaction_split_by, context_entry_tokens, context_usage_state, project_context,
    CacheUsageSummary, CompactionOutcome, CompactionWrite, ContextEntry, ContextEntryKind,
    ContextEvent, ContextEventKind, ContextRole, ConversationContext, ConversationMode,
    ConversationParticipantContext, ConversationSettings, ConversationSettingsUpdate,
    ConversationTurn, RouteSource, CONTEXT_SUMMARY_INPUT_TOKENS, MAX_GROUP_PARTICIPANTS,
    MAX_ROUTED_AGENTS, MIN_GROUP_PARTICIPANTS,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::mention::resolve_mentions;
use cowbot_core::prompt::{PromptMemory, PromptSlot, PromptSlotName};
use cowbot_core::task::{Task, TaskId, TaskSource};
use cowbot_core::usage::{LlmCallKind, LlmCallRecord, TokenCounter, UsageTotals};
use cowbot_llm::{
    token_counter_for_model, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole,
    LlmProvider,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

use crate::channel_agents::{
    ChannelAgentService, ChannelTurnOptions, ChannelTurnReport, ParticipantCapacity,
};
use crate::channels::{normalize_channel_name, ChannelService};
use crate::compaction_locks::CompactionLocks;
use crate::memory::{MemoryService, RecalledMemory};
use crate::prompt_snapshots::{seal_with_context, PromptSnapshotService, SnapshotContext};
use crate::scheduler::{AssignmentScheduler, ConversationTurnRequest, CrewRunOptions};
use crate::session_decisions::SessionDecisions;

/// Longest conversation title accepted before a uniqueness suffix is added.
const MAX_TITLE_CHARS: usize = 68;

/// Time one router completion may take before the turn falls back.
const ROUTER_TIMEOUT: Duration = Duration::from_secs(20);

/// Time one compaction summary request may take. The per-branch lock is held
/// for this long at most, so a hung provider cannot block the branch until
/// the process restarts.
const COMPACTION_TIMEOUT: Duration = Duration::from_secs(120);

/// Display messages shown to the router.
const ROUTER_HISTORY_LIMIT: usize = 10;

/// Token budget for the router's rendered history.
const ROUTER_INPUT_TOKENS: u32 = 4_000;

/// Characters kept from one history message before rendering.
const ROUTER_MESSAGE_CHARS: usize = 800;

/// The router asks for one JSON object naming at most three participants.
const ROUTER_SYSTEM_PROMPT: &str = "You route messages in a group conversation. Given the \
participant roster, recent messages, and the latest user message, choose the participants who \
should answer. Reply with one JSON object only, for example {\"agents\": [\"Planner\"], \
\"reason\": \"why they should answer\"}. Name at most three participants exactly as they appear \
in the roster, or return an empty list when nobody should answer. Do not add any other text.";

/// One router decision attempt.
struct RouterDecision {
    agents: Vec<AgentProfile>,
    reason: Option<String>,
    /// Id shared by the turn row and the persisted router usage record.
    call_id: String,
    /// Why the router produced no usable selection, if it did.
    failure: Option<String>,
}

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
    memory: MemoryService,
    session_decisions: Option<Arc<SessionDecisions>>,
    compaction_locks: Arc<CompactionLocks>,
}

impl ConversationService {
    pub fn new(store: Store, bus: EventBus, workspace_id: WorkspaceId) -> Self {
        Self {
            channels: ChannelService::new(store.clone(), bus.clone()),
            memory: MemoryService::new(store.clone(), workspace_id.clone()),
            store,
            bus,
            workspace_id,
            session_decisions: None,
            compaction_locks: Arc::new(CompactionLocks::new()),
        }
    }

    /// Share the process-wide compaction registry with another service
    /// instance. The desktop and the channel agent service both handle
    /// conversation turns, so they must contend on the same per-branch locks.
    pub fn with_compaction_locks(mut self, locks: Arc<CompactionLocks>) -> Self {
        self.compaction_locks = locks;
        self
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
        self.create_with(
            title,
            std::slice::from_ref(agent_id),
            provider_id,
            model,
            None,
        )
        .await
    }

    /// Create a group conversation with an ordered participant list.
    ///
    /// The participant order is selection order and is preserved for routing,
    /// display, and the default responder. Membership is fixed after creation.
    pub async fn create_group(
        &self,
        title: Option<&str>,
        agent_ids: &[AgentId],
        provider_id: Option<String>,
        model: &str,
        default_responder: Option<AgentId>,
    ) -> CowResult<Channel> {
        if !(MIN_GROUP_PARTICIPANTS..=MAX_GROUP_PARTICIPANTS).contains(&agent_ids.len()) {
            return Err(CowError::Config(format!(
                "a group conversation needs between {MIN_GROUP_PARTICIPANTS} and \
                 {MAX_GROUP_PARTICIPANTS} participants"
            )));
        }
        self.create_with(title, agent_ids, provider_id, model, default_responder)
            .await
    }

    async fn create_with(
        &self,
        title: Option<&str>,
        agent_ids: &[AgentId],
        provider_id: Option<String>,
        model: &str,
        default_responder: Option<AgentId>,
    ) -> CowResult<Channel> {
        if agent_ids.is_empty() {
            return Err(CowError::Config(
                "a conversation needs at least one participant".into(),
            ));
        }
        let unique: std::collections::HashSet<&AgentId> = agent_ids.iter().collect();
        if unique.len() != agent_ids.len() {
            return Err(CowError::Config(
                "a participant may appear only once in a conversation".into(),
            ));
        }

        let mut agents = Vec::with_capacity(agent_ids.len());
        for agent_id in agent_ids {
            let agent =
                self.store.get_agent(agent_id).await?.ok_or_else(|| {
                    CowError::Config(format!("agent '{}' was not found", agent_id.0))
                })?;
            agents.push(agent);
        }

        if let Some(responder) = &default_responder {
            if !agent_ids.contains(responder) {
                return Err(CowError::Config(
                    "the default responder must be a participant".into(),
                ));
            }
        }

        let model = model.trim();
        if model.is_empty() {
            return Err(CowError::Config(
                "a conversation needs a model; configure a model provider first".into(),
            ));
        }

        let names = agents
            .iter()
            .map(|agent| agent.name.clone())
            .collect::<Vec<_>>();
        let fallback_title = match names.as_slice() {
            [agent] => format!("Conversation with {agent}"),
            _ => names.join(", "),
        };
        let base = title
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(&fallback_title);
        let base = truncate_title(&normalize_channel_name(base)?);
        let name = self.unique_name(&base).await?;

        let topic = match names.as_slice() {
            [agent] => format!("Conversation with {agent}"),
            _ => format!("Group conversation with {}", names.join(", ")),
        };
        let channel = Channel::conversation(name, topic);
        let settings = if agents.len() >= MIN_GROUP_PARTICIPANTS {
            ConversationSettings::group(
                channel.id.clone(),
                provider_id,
                model.to_string(),
                default_responder,
            )
        } else {
            ConversationSettings::new(channel.id.clone(), provider_id, model.to_string())
        };
        self.store
            .create_conversation(&self.workspace_id, &channel, &settings, agent_ids)
            .await?;
        self.bus
            .publish(Event::new(EventPayload::ChannelCreated(channel.clone())));
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
        if let Some(router_enabled) = update.router_enabled {
            settings.router_enabled = router_enabled;
        }
        if let Some(responder) = &update.default_responder {
            let participants = self.store.list_channel_participants(channel_id).await?;
            if !participants
                .iter()
                .any(|participant| &participant.agent_id == responder)
            {
                return Err(CowError::Config(
                    "the default responder must be a participant".into(),
                ));
            }
            settings.default_responder = Some(responder.clone());
        }
        settings.updated_at = chrono::Utc::now();
        self.store.update_conversation_settings(&settings).await?;
        Ok(settings)
    }

    /// Token breakdown, cache summaries, and fixed model of a conversation.
    ///
    /// Group conversations return one entry per participant; the top-level
    /// fields mirror the first participant for older callers.
    pub async fn context(&self, channel_id: &ChannelId) -> CowResult<ConversationContext> {
        let channel = self.conversation_channel(channel_id).await?;
        let participants = self.load_participants(&channel).await?;
        if participants.is_empty() {
            return Err(CowError::Config(format!(
                "conversation '{}' has no participant",
                channel.name
            )));
        }
        let settings = self.settings(channel_id).await?;
        let agents = self.store.list_agents(&self.workspace_id).await?;
        let counter = token_counter_for_model(&settings.model);
        let default_name = resolve_default_responder(&settings, &participants)
            .0
            .map(|agent| agent.name.clone());
        let cache_conversation =
            cache_summary(&self.store.sum_llm_usage_for_channel(channel_id).await?);
        let group = participants.len() >= MIN_GROUP_PARTICIPANTS;

        let mut contexts = Vec::with_capacity(participants.len());
        for agent in &participants {
            let entries = self
                .store
                .list_context_entries(channel_id, &agent.id)
                .await?;
            let system_tokens = if group {
                counter.count_message(
                    &group_system_prompt(
                        counter.as_ref(),
                        agent,
                        &participants,
                        default_name.as_deref(),
                        None,
                        &agents,
                    )
                    .text,
                )
            } else {
                counter.count_message(
                    &conversation_system_prompt(counter.as_ref(), agent, &agents).text,
                )
            };
            let projection = project_context(counter.as_ref(), system_tokens, 0, &entries);
            // What this participant's next turn would inject, without touching
            // recall statistics. An empty query leaves notes and pinned entries.
            let memory_tokens = self
                .memory
                .preview_recall(Some(channel_id), &agent.id, "")
                .await?
                .map(|recalled| recalled.tokens)
                .unwrap_or(0);
            let cache_last_run = cache_summary(
                &self
                    .store
                    .latest_run_usage_for_channel_agent(channel_id, &agent.id)
                    .await?,
            );
            contexts.push(ConversationParticipantContext {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                projection,
                memory_tokens,
                cache_last_run,
                cache_conversation: cache_conversation.clone(),
            });
        }

        let first = contexts
            .first()
            .expect("participants are checked to be non-empty");
        Ok(ConversationContext {
            channel_id: channel.id,
            agent_id: first.agent_id.clone(),
            model: settings.model,
            provider_id: settings.provider_id,
            projection: first.projection.clone(),
            cache_last_turn: first.cache_last_run.clone(),
            cache_conversation,
            participants: contexts,
        })
    }

    /// Answer `message` in a conversation.
    ///
    /// The participant count is authoritative: one participant is served by the
    /// single-agent path (imported rows with a stale mode included), and two or
    /// more use group routing.
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
        let participants = self.store.list_channel_participants(&channel.id).await?;
        match participants.len() {
            0 => {
                self.channels
                    .post_system_notice(
                        &channel.id,
                        "This conversation has no agent participant. Create a new conversation.",
                    )
                    .await?;
                let mut report = ChannelTurnReport::default();
                if settings.mode == ConversationMode::Group {
                    let turn = ConversationTurn::new(
                        channel.id.clone(),
                        message.id.clone(),
                        RouteSource::Silent,
                    );
                    self.record_turn(&turn).await?;
                    report.route = Some(turn.source);
                    report.turn_id = Some(turn.id);
                }
                Ok(report)
            }
            1 => {
                self.reply_single(&channel, &settings, message, llm, tools, options)
                    .await
            }
            _ => {
                self.reply_group(&channel, &settings, message, llm, tools, options)
                    .await
            }
        }
    }

    async fn reply_single(
        &self,
        channel: &Channel,
        settings: &ConversationSettings,
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        options: &ChannelTurnOptions,
    ) -> CowResult<ChannelTurnReport> {
        let Some(agent) = self.participant_or_none(channel).await? else {
            self.channels
                .post_system_notice(
                    &channel.id,
                    "This conversation has no agent participant. Create a new conversation.",
                )
                .await?;
            return Ok(ChannelTurnReport::default());
        };
        let agents = self.store.list_agents(&self.workspace_id).await?;
        let counter = token_counter_for_model(&settings.model);

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
        let memory = self
            .memory
            .recall_for_prompt_memory(Some(&channel.id), &agent.id, message.content.trim())
            .await;
        let assembled = assemble_conversation_messages(
            counter.as_ref(),
            &agent,
            &agents,
            &entries,
            memory.as_ref().map(|memory| memory.block.as_str()),
        );
        let snapshot_context =
            snapshot_context_for(&assembled.slots, memory.as_ref(), counter.as_ref());
        let messages = assembled.messages;

        let task = Task::new(
            message.content.trim(),
            "A concise reply addressed to the user in this conversation.",
        )
        .assign_to(agent.id.clone())
        .in_channel(channel.id.clone())
        .from_source(TaskSource::ConversationTurn);
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
        scheduler.register_agent(agent.clone(), llm.clone(), tools, settings.model.clone());

        let mut run_options = CrewRunOptions {
            approval_level: options.approval_level,
            channel_id: Some(channel.id.clone()),
            max_concurrency: options.max_concurrency,
            use_model_permissions: options.use_model_permissions,
            capture_plan_task: None,
        };
        if options.approval_level == ApprovalLevel::PlanOnly {
            run_options.capture_plan_task = Some(task.id.clone());
        }
        scheduler
            .enqueue_conversation_turn(
                &task,
                ConversationTurnRequest {
                    messages,
                    channel_id: channel.id.clone(),
                    agent_id: agent.id.clone(),
                    snapshot_context,
                    record_context: true,
                },
                requester,
                AssignmentOrigin::User,
                &run_options,
            )
            .await?;

        let outcome = scheduler.run_until_idle().await;
        // The turn runner posts a conversation reply with its text and its
        // stopped marker, so this path only needs the text for memory
        // extraction (ADR-0039).
        let reply = self.store.task_output(&task_id).await?;
        outcome?;

        // A settled turn may contribute pending memory candidates. Extraction
        // is opt-in and best-effort, so it never changes the reply outcome.
        if let Some(reply) = reply {
            self.memory
                .extract_after_turn(
                    channel,
                    llm,
                    &settings.model,
                    message.content.trim(),
                    &reply,
                    Some(&agent.id),
                    &message.id,
                )
                .await;
        }

        Ok(ChannelTurnReport {
            agents: vec![agent.id],
            unknown_mentions: Vec::new(),
            tasks: vec![task_id],
            ..ChannelTurnReport::default()
        })
    }

    /// Route one group message and fan it out to its responders.
    ///
    /// Explicit mentions always win. Without a mention the default responder
    /// answers; the model router arrives in M3.4 behind `router_enabled`.
    async fn reply_group(
        &self,
        channel: &Channel,
        settings: &ConversationSettings,
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        options: &ChannelTurnOptions,
    ) -> CowResult<ChannelTurnReport> {
        let participants = self.load_participants(channel).await?;
        let counter = token_counter_for_model(&settings.model);
        if participants.is_empty() {
            self.channels
                .post_system_notice(
                    &channel.id,
                    "This conversation has no available participants. Create a new group \
                     conversation.",
                )
                .await?;
            let turn =
                ConversationTurn::new(channel.id.clone(), message.id.clone(), RouteSource::Silent);
            self.record_turn(&turn).await?;
            return Ok(ChannelTurnReport {
                route: Some(turn.source),
                turn_id: Some(turn.id),
                ..ChannelTurnReport::default()
            });
        }
        let agents = self.store.list_agents(&self.workspace_id).await?;

        // Every participant stays current for later turns, including the ones
        // that do not answer this message.
        for participant in &participants {
            let mut entry = ContextEntry::new(
                channel.id.clone(),
                participant.id.clone(),
                ContextRole::User,
                message.content.trim(),
            );
            entry.recount();
            self.store.append_context_entry(&entry).await?;
        }

        let resolution = resolve_mentions(&message.content, &participants);
        let unknown = resolution.unknown.clone();
        let mut turn =
            ConversationTurn::new(channel.id.clone(), message.id.clone(), RouteSource::Silent);
        turn.unknown_mentions = unknown.clone();
        let mut skipped = Vec::new();
        let mut auto_compacted = Vec::new();

        let selected = if !resolution.agents.is_empty() {
            turn.source = RouteSource::Mention;
            if !resolution.unknown.is_empty() {
                self.post_participant_notice(channel, &resolution.unknown, &participants)
                    .await?;
            }
            resolution.agents
        } else if !resolution.unknown.is_empty() {
            turn.source = RouteSource::Unmatched;
            self.post_participant_notice(channel, &resolution.unknown, &participants)
                .await?;
            self.record_turn(&turn).await?;
            return Ok(ChannelTurnReport {
                unknown_mentions: unknown,
                route: Some(turn.source),
                turn_id: Some(turn.id),
                ..ChannelTurnReport::default()
            });
        } else {
            let mut routed = None;
            if settings.router_enabled {
                let decision = self
                    .route_with_model(
                        channel,
                        &participants,
                        message,
                        llm.clone(),
                        &settings.model,
                    )
                    .await?;
                turn.router_call_id = Some(decision.call_id.clone());
                if let Some(failure) = decision.failure {
                    turn.reason = Some(failure.clone());
                    let fallback = if resolve_default_responder(settings, &participants)
                        .0
                        .is_some()
                    {
                        " Falling back to the default responder."
                    } else {
                        " The message was left unanswered."
                    };
                    self.channels
                        .post_system_notice(
                            &channel.id,
                            &format!("Routing failed: {failure}.{fallback}"),
                        )
                        .await?;
                } else {
                    turn.source = RouteSource::Router;
                    turn.reason = decision.reason;
                    routed = Some(decision.agents);
                }
            }

            match routed {
                Some(agents) => agents,
                None => {
                    let (responder, stale) = resolve_default_responder(settings, &participants);
                    let Some(responder) = responder else {
                        turn.source = RouteSource::Silent;
                        self.record_turn(&turn).await?;
                        return Ok(ChannelTurnReport {
                            unknown_mentions: unknown,
                            route: Some(turn.source),
                            turn_id: Some(turn.id),
                            ..ChannelTurnReport::default()
                        });
                    };
                    if stale {
                        self.channels
                            .post_system_notice(
                                &channel.id,
                                &format!(
                                    "The configured default responder is no longer a participant; \
                                     {} answered instead.",
                                    responder.name
                                ),
                            )
                            .await?;
                    }
                    turn.source = RouteSource::Default;
                    vec![responder.clone()]
                }
            }
        };

        let default_name = resolve_default_responder(settings, &participants)
            .0
            .map(|agent| agent.name.clone());
        let capacities: HashMap<&AgentId, &ParticipantCapacity> = options
            .participant_capacity
            .iter()
            .map(|capacity| (&capacity.agent_id, capacity))
            .collect();

        let mut responders = Vec::new();
        for agent in &selected {
            let capacity = capacities.get(&agent.id);
            let window = capacity
                .map(|capacity| capacity.usable_window)
                .filter(|window| *window > 0);
            if let Some(window) = window {
                let tool_tokens = capacity.map(|capacity| capacity.tool_tokens).unwrap_or(0);
                let mut compaction_notice = None;
                if settings.auto_compact
                    && self
                        .branch_crosses_threshold(
                            channel,
                            agent,
                            &participants,
                            default_name.as_deref(),
                            options.platform_context.as_deref(),
                            &agents,
                            counter.as_ref(),
                            tool_tokens,
                            window,
                            settings.compact_threshold,
                        )
                        .await?
                {
                    match self
                        .compact_branch(channel, agent, llm.clone(), &settings.model, false)
                        .await
                    {
                        Ok(outcome) => {
                            if outcome.compacted {
                                auto_compacted.push(agent.name.clone());
                            } else if outcome.in_progress {
                                compaction_notice = Some(
                                    "its context is being compacted; try again shortly".to_string(),
                                );
                            } else if outcome
                                .reason
                                .as_deref()
                                .is_some_and(|reason| reason.contains("invalid sequence numbers"))
                            {
                                compaction_notice = Some(
                                    "its context branch cannot be compacted; start a new \
                                     conversation or re-import the workspace"
                                        .to_string(),
                                );
                            }
                        }
                        Err(error) => {
                            // Automatic compaction is an optimization. A
                            // provider or store failure must not abort the
                            // user's turn; the capacity check below decides.
                            tracing::warn!(
                                error = %error,
                                agent = %agent.name,
                                "Automatic compaction failed; answering without it"
                            );
                        }
                    }
                }
                let entries = self
                    .store
                    .list_context_entries(&channel.id, &agent.id)
                    .await?;
                let system_tokens = counter.count_message(
                    &group_system_prompt(
                        counter.as_ref(),
                        agent,
                        &participants,
                        default_name.as_deref(),
                        options.platform_context.as_deref(),
                        &agents,
                    )
                    .text,
                );
                let projection =
                    project_context(counter.as_ref(), system_tokens, tool_tokens, &entries);
                if projection.total_tokens() >= window {
                    skipped.push(agent.id.clone());
                    let notice = match compaction_notice {
                        Some(detail) => format!("{} was skipped because {}.", agent.name, detail),
                        None => format!(
                            "{} was skipped because its context is full. Clear or compact \
                             its context to include it again.",
                            agent.name
                        ),
                    };
                    self.channels
                        .post_system_notice(&channel.id, &notice)
                        .await?;
                    continue;
                }
            }
            responders.push(agent.clone());
        }

        if !auto_compacted.is_empty() {
            self.channels
                .post_system_notice(
                    &channel.id,
                    &format!(
                        "Context compacted automatically for {}.",
                        auto_compacted.join(", ")
                    ),
                )
                .await?;
        }

        turn.selected_agents = responders
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        if responders.is_empty() && !skipped.is_empty() {
            turn.reason = Some("every selected participant's context is full".to_string());
            self.channels
                .post_system_notice(
                    &channel.id,
                    "No participant had room to answer. Compact or clear a context branch and \
                     try again.",
                )
                .await?;
            self.record_turn(&turn).await?;
            return Ok(ChannelTurnReport {
                unknown_mentions: unknown,
                skipped,
                route: Some(turn.source),
                turn_id: Some(turn.id),
                ..ChannelTurnReport::default()
            });
        }

        let requester = match &message.sender {
            Sender::User(id) => AssignmentRequester::User(id.clone()),
            _ => AssignmentRequester::System,
        };
        let max_concurrency = responders.len().clamp(1, MAX_GROUP_PARTICIPANTS);
        let mut scheduler = AssignmentScheduler::new(
            self.store.clone(),
            self.bus.clone(),
            self.workspace_id.clone(),
            max_concurrency,
        )
        .with_scope()
        .with_model_permissions(options.use_model_permissions)
        .with_session_decisions(self.session_decisions.clone());
        let run_options = CrewRunOptions {
            approval_level: options.approval_level,
            channel_id: Some(channel.id.clone()),
            max_concurrency,
            use_model_permissions: options.use_model_permissions,
            capture_plan_task: None,
        };
        let platform = options.platform_context.as_deref();

        // Persist the route before the drain so the audit survives a restart
        // even when a run fails halfway through.
        self.store.insert_conversation_turn(&turn).await?;

        let mut tasks = Vec::new();
        for agent in &responders {
            scheduler.register_agent(
                agent.clone(),
                llm.clone(),
                tools.clone(),
                settings.model.clone(),
            );
            let entries = self
                .store
                .list_context_entries(&channel.id, &agent.id)
                .await?;
            let memory = self
                .memory
                .recall_for_prompt_memory(Some(&channel.id), &agent.id, message.content.trim())
                .await;
            let assembled = assemble_group_messages(
                counter.as_ref(),
                agent,
                &participants,
                default_name.as_deref(),
                platform,
                &agents,
                &entries,
                memory.as_ref().map(|memory| memory.block.as_str()),
            );
            let snapshot_context =
                snapshot_context_for(&assembled.slots, memory.as_ref(), counter.as_ref());
            let messages = assembled.messages;
            let task = Task::new(
                message.content.trim(),
                "A concise reply addressed to the user in this group conversation.",
            )
            .in_channel(channel.id.clone())
            .assign_to(agent.id.clone())
            .from_source(TaskSource::ConversationTurn);
            let mut task_options = run_options.clone();
            if options.approval_level == ApprovalLevel::PlanOnly {
                task_options.capture_plan_task = Some(task.id.clone());
            }
            scheduler
                .enqueue_conversation_turn(
                    &task,
                    ConversationTurnRequest {
                        messages,
                        channel_id: channel.id.clone(),
                        agent_id: agent.id.clone(),
                        snapshot_context,
                        record_context: true,
                    },
                    requester.clone(),
                    AssignmentOrigin::User,
                    &task_options,
                )
                .await?;
            tasks.push(task.id);
        }
        turn.task_ids = tasks.clone();
        self.store.update_conversation_turn(&turn).await?;
        self.bus
            .publish(Event::new(EventPayload::ConversationRouted(turn.clone())));

        // Replies post as their tasks settle, so fast agents answer without
        // waiting for the slowest participant.
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let consumer = tokio::spawn(self.clone().consume_group_settlements(
            channel.clone(),
            participants.clone(),
            receiver,
        ));
        let outcome = scheduler.run_until_idle_observed(sender).await;
        let settlement = consumer
            .await
            .map_err(|error| CowError::Task(format!("reply poster failed: {error}")))?;
        if let Err(error) = outcome {
            tracing::warn!(
                error = %error,
                "Group turn finished with an error; task statuses are authoritative"
            );
        }

        turn.mirrored_at = Some(chrono::Utc::now());
        self.store.update_conversation_turn(&turn).await?;

        // One aggregate extraction per settled group turn. The entry is
        // conversation-shared because several participants may have answered.
        if !settlement.replies.is_empty() {
            let replies = settlement
                .replies
                .iter()
                .map(|(agent_id, content)| {
                    let name = participants
                        .iter()
                        .find(|participant| &participant.id == agent_id)
                        .map(|participant| participant.name.clone())
                        .unwrap_or_else(|| format!("Agent {}", agent_id.0));
                    format!("{name}: {content}")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            self.memory
                .extract_after_turn(
                    channel,
                    llm,
                    &settings.model,
                    message.content.trim(),
                    &replies,
                    None,
                    &turn.id.0,
                )
                .await;
        }

        Ok(ChannelTurnReport {
            agents: responders
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<Vec<_>>(),
            unknown_mentions: unknown,
            tasks,
            replied: settlement.replied,
            failed: settlement.failed,
            skipped,
            route: Some(turn.source),
            turn_id: Some(turn.id),
        })
    }

    async fn consume_group_settlements(
        self,
        channel: Channel,
        participants: Vec<AgentProfile>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<TaskId>,
    ) -> GroupSettlement {
        let mut settlement = GroupSettlement::default();
        while let Some(task_id) = receiver.recv().await {
            match self
                .settle_group_task(&channel, &task_id, &participants)
                .await
            {
                Ok(Some(settled)) => {
                    if settled.replied {
                        settlement.replied.push(settled.agent_id.clone());
                        if let Some(content) = settled.content {
                            settlement.replies.push((settled.agent_id, content));
                        }
                    } else {
                        settlement.failed.push(settled.agent_id);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        task = %task_id.0,
                        "Failed to post a group reply"
                    );
                }
            }
        }
        settlement
    }

    async fn settle_group_task(
        &self,
        channel: &Channel,
        task_id: &TaskId,
        participants: &[AgentProfile],
    ) -> CowResult<Option<SettledGroupReply>> {
        let Some(task) = self.store.get_task(task_id).await? else {
            return Ok(None);
        };
        let Some(assignee) = task.assigned_agent.clone() else {
            return Ok(None);
        };
        let name = match participants.iter().find(|agent| agent.id == assignee) {
            Some(agent) => agent.name.clone(),
            None => self
                .store
                .get_agent(&assignee)
                .await?
                .map(|agent| agent.name)
                .unwrap_or_else(|| format!("Agent {}", assignee.0)),
        };

        // The turn runner already posted the reply, including a stopped one, so
        // settling mirrors it into the other branches and reports the outcome
        // instead of posting a second copy (ADR-0039).
        match &task.status {
            cowbot_core::task::TaskStatus::Completed { output } => {
                let content = output.trim();
                if content.is_empty() {
                    return Ok(Some(SettledGroupReply {
                        agent_id: assignee,
                        replied: false,
                        content: None,
                    }));
                }
                for participant in participants
                    .iter()
                    .filter(|participant| participant.id != assignee)
                {
                    let peer = ContextEntry::peer(
                        channel.id.clone(),
                        participant.id.clone(),
                        assignee.clone(),
                        &name,
                        content,
                    );
                    self.store.append_context_entry(&peer).await?;
                }
                Ok(Some(SettledGroupReply {
                    agent_id: assignee,
                    replied: true,
                    content: Some(content.to_string()),
                }))
            }
            cowbot_core::task::TaskStatus::Failed { .. } => Ok(Some(SettledGroupReply {
                agent_id: assignee,
                replied: false,
                content: None,
            })),
            _ => Ok(None),
        }
    }

    async fn post_participant_notice(
        &self,
        channel: &Channel,
        rejected: &[String],
        participants: &[AgentProfile],
    ) -> CowResult<()> {
        let rejected = rejected
            .iter()
            .map(|name| format!("@{name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let available = participants
            .iter()
            .map(|agent| format!("@{}", agent.name))
            .collect::<Vec<_>>()
            .join(", ");
        self.channels
            .post_system_notice(
                &channel.id,
                &format!("No participant matched {rejected}. Available participants: {available}."),
            )
            .await?;
        Ok(())
    }

    async fn record_turn(&self, turn: &ConversationTurn) -> CowResult<()> {
        self.store.insert_conversation_turn(turn).await?;
        self.bus
            .publish(Event::new(EventPayload::ConversationRouted(turn.clone())));
        Ok(())
    }

    async fn load_participants(&self, channel: &Channel) -> CowResult<Vec<AgentProfile>> {
        let links = self.store.list_channel_participants(&channel.id).await?;
        let mut participants = Vec::with_capacity(links.len());
        for link in links {
            if let Some(agent) = self.store.get_agent(&link.agent_id).await? {
                participants.push(agent);
            }
        }
        Ok(participants)
    }

    #[allow(clippy::too_many_arguments)]
    async fn branch_crosses_threshold(
        &self,
        channel: &Channel,
        agent: &AgentProfile,
        participants: &[AgentProfile],
        default_name: Option<&str>,
        platform_context: Option<&str>,
        agents: &[AgentProfile],
        counter: &dyn TokenCounter,
        tool_tokens: u32,
        window: u32,
        compact_threshold: u32,
    ) -> CowResult<bool> {
        let entries = self
            .store
            .list_context_entries(&channel.id, &agent.id)
            .await?;
        let system_tokens = counter.count_message(
            &group_system_prompt(
                counter,
                agent,
                participants,
                default_name,
                platform_context,
                agents,
            )
            .text,
        );
        let projection = project_context(counter, system_tokens, tool_tokens, &entries);
        let (percent, _) = context_usage_state(projection.total_tokens(), Some(window));
        Ok(percent
            .map(|percent| percent >= compact_threshold as f32)
            .unwrap_or(false))
    }

    /// Ask the conversation's fixed model which participants should answer.
    ///
    /// One completion, temperature 0.0, a 20-second timeout, and no retries.
    /// Every failure is returned as a decision with `failure` set so the caller
    /// can post a notice and fall back to the default responder.
    async fn route_with_model(
        &self,
        channel: &Channel,
        participants: &[AgentProfile],
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        model: &str,
    ) -> CowResult<RouterDecision> {
        let call_id = uuid::Uuid::new_v4().to_string();
        let roster = participants
            .iter()
            .map(|participant| {
                format!(
                    "- {} — {}: {}",
                    participant.name, participant.role, participant.goal
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let counter = token_counter_for_model(model);
        let history = self
            .render_router_history(channel, message, participants, counter.as_ref())
            .await?;
        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage::system(ROUTER_SYSTEM_PROMPT),
                ChatMessage::user(format!(
                    "Participants:\n{roster}\n\nRecent messages:\n{history}\n\n\
                     Latest message from the user:\n{}\n\nReply with JSON only.",
                    message.content.trim()
                )),
            ],
            tools: Vec::new(),
            temperature: Some(0.0),
        };

        let response = match complete_with_timeout(llm, &request, ROUTER_TIMEOUT).await {
            Ok(response) => response,
            Err(failure) => {
                return Ok(RouterDecision {
                    agents: Vec::new(),
                    reason: None,
                    call_id,
                    failure: Some(failure),
                })
            }
        };

        self.record_router_usage(channel, model, &call_id, &request, &response)
            .await;

        let Some(content) = response
            .content
            .as_deref()
            .map(str::trim)
            .filter(|content| !content.is_empty())
        else {
            return Ok(RouterDecision {
                agents: Vec::new(),
                reason: None,
                call_id,
                failure: Some("the router returned no text".to_string()),
            });
        };

        match parse_router_response(content) {
            Ok((names, reason)) => {
                let agents = select_participants(&names, participants);
                if agents.is_empty() {
                    Ok(RouterDecision {
                        agents,
                        reason,
                        call_id,
                        failure: Some("the router named no participant".to_string()),
                    })
                } else {
                    Ok(RouterDecision {
                        agents,
                        reason,
                        call_id,
                        failure: None,
                    })
                }
            }
            Err(failure) => Ok(RouterDecision {
                agents: Vec::new(),
                reason: None,
                call_id,
                failure: Some(failure),
            }),
        }
    }

    async fn record_router_usage(
        &self,
        channel: &Channel,
        model: &str,
        call_id: &str,
        request: &ChatCompletionRequest,
        response: &ChatCompletionResponse,
    ) {
        let mut record = LlmCallRecord::new(LlmCallKind::Router, model.to_string());
        record.id = call_id.to_string();
        record.channel_id = Some(channel.id.clone());
        record.usage = response.usage.clone();
        record.raw_usage = response.raw_usage.clone();
        if let Err(error) = self.store.record_llm_call(&record).await {
            tracing::warn!(error = %error, "Failed to record router usage");
        }
        self.record_prompt_snapshot(
            &record.id,
            request,
            LlmCallKind::Router,
            instruction_context(request),
        )
        .await;
    }

    async fn render_router_history(
        &self,
        channel: &Channel,
        message: &ChannelMessage,
        participants: &[AgentProfile],
        counter: &dyn TokenCounter,
    ) -> CowResult<String> {
        let messages = self.store.list_messages(&channel.id, 50).await?;
        let mut lines = Vec::new();
        let mut tokens = 0_u32;
        for entry in messages.iter().rev() {
            if entry.id == message.id || entry.created_at >= message.created_at {
                continue;
            }
            let author = match &entry.sender {
                Sender::User(_) => "User".to_string(),
                Sender::Agent(id) => participants
                    .iter()
                    .find(|participant| &participant.id == id)
                    .map(|participant| participant.name.clone())
                    .unwrap_or_else(|| format!("Agent {}", id.0)),
                Sender::System => continue,
            };
            let line = format!(
                "{author}: {}",
                truncate_chars(entry.content.trim(), ROUTER_MESSAGE_CHARS)
            );
            let cost = counter.count(&line);
            if tokens.saturating_add(cost) > ROUTER_INPUT_TOKENS && !lines.is_empty() {
                break;
            }
            tokens = tokens.saturating_add(cost);
            lines.push(line);
            if lines.len() >= ROUTER_HISTORY_LIMIT {
                break;
            }
        }
        lines.reverse();
        Ok(if lines.is_empty() {
            "(no earlier messages)".to_string()
        } else {
            lines.join("\n")
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
        self.compact_branch(&channel, &agent, llm, model, true)
            .await
    }

    /// Summarize one participant's context branch.
    ///
    /// Manual group compaction targets one participant; a clear-all action
    /// repeats this per branch.
    pub async fn compact_participant(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        llm: Arc<dyn LlmProvider>,
        model: &str,
    ) -> CowResult<CompactionOutcome> {
        let channel = self.conversation_channel(channel_id).await?;
        let agent = self
            .load_participants(&channel)
            .await?
            .into_iter()
            .find(|agent| &agent.id == agent_id)
            .ok_or_else(|| {
                CowError::Config(format!(
                    "agent '{}' is not a participant of '{}'",
                    agent_id.0, channel.name
                ))
            })?;
        self.compact_branch(&channel, &agent, llm, model, true)
            .await
    }

    async fn compact_branch(
        &self,
        channel: &Channel,
        agent: &AgentProfile,
        llm: Arc<dyn LlmProvider>,
        model: &str,
        announce: bool,
    ) -> CowResult<CompactionOutcome> {
        // Claim the branch before reading it. The lock covers the model call
        // and the guarded write, so a second request cannot summarize the
        // same prefix and then discover the branch moved.
        let key = (channel.id.clone(), agent.id.clone());
        let Some(_guard) = self.compaction_locks.try_acquire(key) else {
            return Ok(CompactionOutcome {
                in_progress: true,
                reason: Some("A compaction is already running for this branch.".to_string()),
                ..CompactionOutcome::default()
            });
        };

        let entries = self
            .store
            .list_context_entries(&channel.id, &agent.id)
            .await?;
        let counter = token_counter_for_model(model);
        let active = entries
            .iter()
            .filter(|entry| !entry.excluded)
            .cloned()
            .collect::<Vec<_>>();
        let tokens_before = active
            .iter()
            .fold(0_u32, |total, entry| total.saturating_add(entry.tokens));

        // The store's renumber uses negative sequences as a staging range.
        // Reject a branch with non-positive rows before paying for a summary;
        // the store still guards the write for a cross-process import or edit.
        if entries.iter().any(|entry| entry.seq <= 0) {
            return Ok(CompactionOutcome {
                compacted: false,
                summarized_entries: 0,
                tokens_before,
                tokens_after: tokens_before,
                reason: Some(
                    "This context branch has invalid sequence numbers; start a new \
                     conversation or re-import the workspace."
                        .to_string(),
                ),
                ..CompactionOutcome::default()
            });
        }

        let split = compaction_split_by(&active, |entry| {
            context_entry_tokens(counter.as_ref(), entry)
        });
        if split == 0 {
            return Ok(CompactionOutcome {
                compacted: false,
                summarized_entries: 0,
                tokens_before,
                tokens_after: tokens_before,
                reason: Some("There are no older entries to summarize.".to_string()),
                ..CompactionOutcome::default()
            });
        }

        let summarized = &active[..split];
        // `compaction_split_by` guarantees at least one kept entry, so this is
        // the sequence the summary will occupy after the tail shifts.
        let cutoff_seq = active[split].seq;
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
        let response = match tokio::time::timeout(COMPACTION_TIMEOUT, llm.chat_complete(&request))
            .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Ok(CompactionOutcome {
                    compacted: false,
                    summarized_entries: 0,
                    tokens_before,
                    tokens_after: tokens_before,
                    reason: Some("The summary request timed out; nothing was written.".to_string()),
                    ..CompactionOutcome::default()
                });
            }
        };
        self.record_compaction_usage(channel, agent, model, &request, &response)
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

        let mut event = ContextEvent::new(
            channel.id.clone(),
            agent.id.clone(),
            ContextEventKind::Compact,
            tokens_before,
            0,
        );
        event.detail = Some(format!("summarized {} entries", summarized.len()));
        let excluded_ids = summarized
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let write = self
            .store
            .compact_context(&summary, &excluded_ids, cutoff_seq, &mut event)
            .await?;

        match write {
            CompactionWrite::Applied {
                tokens_before,
                tokens_after,
                ..
            } => {
                if announce {
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
                }

                Ok(CompactionOutcome {
                    compacted: true,
                    summarized_entries: summarized.len() as u32,
                    tokens_before,
                    tokens_after,
                    summary: Some(summary_text),
                    in_progress: false,
                    reason: None,
                })
            }
            CompactionWrite::Stale => Ok(CompactionOutcome {
                compacted: false,
                summarized_entries: 0,
                tokens_before: event.tokens_before,
                tokens_after: event.tokens_before,
                reason: Some(
                    "The context changed while it was being summarized; nothing was written."
                        .to_string(),
                ),
                ..CompactionOutcome::default()
            }),
            CompactionWrite::Empty => Ok(CompactionOutcome {
                compacted: false,
                summarized_entries: 0,
                tokens_before: event.tokens_before,
                tokens_after: event.tokens_before,
                reason: Some(
                    "The context was cleared while it was being summarized; nothing was written."
                        .to_string(),
                ),
                ..CompactionOutcome::default()
            }),
        }
    }

    /// Exclude every active entry from future model requests.
    ///
    /// The rows and display messages stay; the model starts from an empty
    /// context after this call. Group conversations clear every participant
    /// branch and record one event per branch.
    pub async fn clear_context(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let channel = self.conversation_channel(channel_id).await?;
        let participants = self.load_participants(&channel).await?;
        if participants.is_empty() {
            return Err(CowError::Config(format!(
                "conversation '{}' has no participant",
                channel.name
            )));
        }
        let mut tokens_by_agent = Vec::with_capacity(participants.len());
        for agent in &participants {
            let entries = self
                .store
                .list_context_entries(&channel.id, &agent.id)
                .await?;
            let tokens_before = entries
                .iter()
                .filter(|entry| !entry.excluded)
                .fold(0_u32, |total, entry| total.saturating_add(entry.tokens));
            tokens_by_agent.push((agent.id.clone(), tokens_before));
        }
        let cleared = self.store.exclude_all_context_entries(&channel.id).await?;
        if cleared > 0 {
            for (agent_id, tokens_before) in &tokens_by_agent {
                let event = ContextEvent::new(
                    channel.id.clone(),
                    agent_id.clone(),
                    ContextEventKind::Clear,
                    *tokens_before,
                    0,
                );
                self.store.record_context_event(&event).await?;
            }
            let notice = if participants.len() == 1 {
                "Context cleared. Earlier messages stay visible but are no longer sent to the \
                 model."
                    .to_string()
            } else {
                format!(
                    "Context cleared for all {} participants. Earlier messages stay visible but \
                     are no longer sent to the model.",
                    participants.len()
                )
            };
            self.channels
                .post_system_notice(&channel.id, &notice)
                .await?;
        }
        Ok(cleared)
    }

    /// Exclude one participant's active entries from future model requests.
    pub async fn clear_participant_context(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<u64> {
        let channel = self.conversation_channel(channel_id).await?;
        let agent = self
            .load_participants(&channel)
            .await?
            .into_iter()
            .find(|agent| &agent.id == agent_id)
            .ok_or_else(|| {
                CowError::Config(format!(
                    "agent '{}' is not a participant of '{}'",
                    agent_id.0, channel.name
                ))
            })?;
        let (cleared, tokens_before) = self
            .store
            .exclude_active_context_entries(&channel.id, &agent.id)
            .await?;
        if cleared == 0 {
            return Ok(0);
        }
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
                &format!(
                    "Context cleared for {}. Earlier messages stay visible but are no longer \
                     sent to the model.",
                    agent.name
                ),
            )
            .await?;
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
        request: &ChatCompletionRequest,
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
        self.record_prompt_snapshot(
            &record.id,
            request,
            LlmCallKind::Compact,
            instruction_context(request),
        )
        .await;
    }

    /// Store the sealed request behind one call, logging and continuing on
    /// failure. Usage is already persisted by the caller.
    async fn record_prompt_snapshot(
        &self,
        llm_call_id: &str,
        request: &ChatCompletionRequest,
        call_kind: LlmCallKind,
        context: SnapshotContext,
    ) {
        let snapshots = PromptSnapshotService::new(self.store.clone(), self.workspace_id.clone());
        let snapshot = seal_with_context(
            cowbot_llm::snapshot_request(&request.model, call_kind, request),
            &context,
        );
        if let Err(error) = snapshots.record(llm_call_id, &snapshot).await {
            tracing::warn!(error = %error, "Failed to record a prompt snapshot");
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

/// Run one router completion under its timeout.
///
/// The timeout is a separate helper so the failure paths are testable with a
/// short duration; the caller always uses [`ROUTER_TIMEOUT`].
async fn complete_with_timeout(
    llm: Arc<dyn LlmProvider>,
    request: &ChatCompletionRequest,
    timeout: Duration,
) -> Result<ChatCompletionResponse, String> {
    match tokio::time::timeout(timeout, llm.chat_complete(request)).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(format!("the router call failed: {error}")),
        Err(_) => Err("the router timed out".to_string()),
    }
}

/// Parse the router's JSON answer into names and an optional reason.
fn parse_router_response(content: &str) -> Result<(Vec<String>, Option<String>), String> {
    let (Some(start), Some(end)) = (content.find('{'), content.rfind('}')) else {
        return Err("the router returned invalid JSON".to_string());
    };
    if end < start {
        return Err("the router returned invalid JSON".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end])
        .map_err(|_| "the router returned invalid JSON".to_string())?;

    let mut names = Vec::new();
    match value.get("agents") {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(name) = item.as_str() {
                    names.push(name.to_string());
                }
            }
        }
        Some(serde_json::Value::String(name)) => names.push(name.clone()),
        _ => {}
    }
    if let Some(name) = value.get("agent").and_then(serde_json::Value::as_str) {
        names.push(name.to_string());
    }
    if names.is_empty() {
        return Err("the router selected nobody".to_string());
    }

    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string);
    Ok((names, reason))
}

/// Match router names against the roster, preserving the router's order.
fn select_participants(names: &[String], participants: &[AgentProfile]) -> Vec<AgentProfile> {
    let mut selected: Vec<AgentProfile> = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(agent) = participants.iter().find(|agent| {
            agent.name.eq_ignore_ascii_case(name) || agent.id.0.eq_ignore_ascii_case(name)
        }) else {
            continue;
        };
        if selected.iter().any(|existing| existing.id == agent.id) {
            continue;
        }
        selected.push(agent.clone());
        if selected.len() >= MAX_ROUTED_AGENTS {
            break;
        }
    }
    selected
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
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

/// Resolve the participant that answers an unaddressed group message.
///
/// A stored responder that is no longer a participant falls back to the
/// participant named `main` (the built-in Main Assistant), then to the first
/// participant in order. The boolean reports that fallback happened so the
/// turn can explain it. An unset responder stays silent.
pub(crate) fn resolve_default_responder<'a>(
    settings: &ConversationSettings,
    participants: &'a [AgentProfile],
) -> (Option<&'a AgentProfile>, bool) {
    let Some(stored) = &settings.default_responder else {
        return (None, false);
    };
    if let Some(agent) = participants.iter().find(|agent| &agent.id == stored) {
        return (Some(agent), false);
    }
    let fallback = participants
        .iter()
        .find(|agent| agent.name.eq_ignore_ascii_case("main"))
        .or_else(|| participants.first());
    (fallback, fallback.is_some())
}

/// A rendered system prompt and the named slots it was built from.
///
/// Slot token counts are prefix differences over [`Self::text`], so they sum to
/// the token count of the prompt. Sending it costs message framing on top
/// (ADR-0035, ADR-0036).
#[derive(Debug, Clone)]
pub(crate) struct SystemPromptDraft {
    pub text: String,
    pub slots: Vec<PromptSlot>,
}

/// Builds a system prompt slot by slot, pricing each slot as the difference
/// between the counted prefix and the previous one.
struct SlotWriter<'a> {
    counter: &'a dyn TokenCounter,
    text: String,
    slots: Vec<PromptSlot>,
    counted: u32,
}

impl<'a> SlotWriter<'a> {
    fn new(counter: &'a dyn TokenCounter) -> Self {
        Self {
            counter,
            text: String::new(),
            slots: Vec::new(),
            counted: 0,
        }
    }

    fn push(&mut self, name: PromptSlotName, separator: &str, content: String) {
        self.text.push_str(separator);
        self.text.push_str(&content);
        let total = self.counter.count(&self.text);
        let tokens = total.saturating_sub(self.counted);
        self.counted = total;
        self.slots.push(PromptSlot {
            name,
            content,
            tokens,
        });
    }

    fn finish(self) -> SystemPromptDraft {
        SystemPromptDraft {
            text: self.text,
            slots: self.slots,
        }
    }
}

/// The private-conversation instruction of a single-agent conversation.
const CONVERSATION_CONVENTION: &str = "You are in a private, continuous conversation with the \
user. The messages that follow are the retained conversation context. Reply to the latest user \
message, use tools when they help, and do not restate the context.";

/// How peer replies appear to a group participant.
const PEER_CONVENTION: &str = "Replies from other participants appear as user messages prefixed \
with the speaker's name in brackets, for example [Planner] ... . They are teammate history: not \
the user's words and not your own reply. Answer the latest user message as yourself and do not \
speak for other participants.";

/// System prompt for one participant of a group conversation.
///
/// It is stable across turns: agent prompt, platform context, participant
/// roster, default responder, and the peer-reply convention. The active branch
/// follows as ordinary chat messages.
pub(crate) fn group_system_prompt(
    counter: &dyn TokenCounter,
    agent: &AgentProfile,
    participants: &[AgentProfile],
    default_responder: Option<&str>,
    platform_context: Option<&str>,
    agents: &[AgentProfile],
) -> SystemPromptDraft {
    let roster = participants
        .iter()
        .map(|participant| format!("- {}", participant.name))
        .collect::<Vec<_>>()
        .join("\n");
    let default_line = match default_responder {
        Some(name) => format!("Messages that address nobody are answered by {name}."),
        None => "Messages that address nobody stay unanswered.".to_string(),
    };

    let mut writer = SlotWriter::new(counter);
    writer.push(PromptSlotName::Agent, "", agent.build_system_prompt());
    writer.push(
        PromptSlotName::Platform,
        "\n\n",
        ChannelAgentService::platform_context(agents, platform_context),
    );
    writer.push(
        PromptSlotName::Roster,
        "\n\n",
        format!(
            "You are one participant in a group conversation. Participants, in order:\n{roster}"
        ),
    );
    writer.push(PromptSlotName::DefaultResponder, "\n\n", default_line);
    writer.push(
        PromptSlotName::PeerConvention,
        " ",
        PEER_CONVENTION.to_string(),
    );
    writer.finish()
}

/// Assemble a group participant's request from its active branch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_group_messages(
    counter: &dyn TokenCounter,
    agent: &AgentProfile,
    participants: &[AgentProfile],
    default_responder: Option<&str>,
    platform_context: Option<&str>,
    agents: &[AgentProfile],
    entries: &[ContextEntry],
    memory: Option<&str>,
) -> AssembledRequest {
    let prompt = group_system_prompt(
        counter,
        agent,
        participants,
        default_responder,
        platform_context,
        agents,
    );
    let mut messages = vec![ChatMessage::system(prompt.text)];
    if let Some(memory) = memory.filter(|memory| !memory.trim().is_empty()) {
        messages.push(ChatMessage::system(memory));
    }
    messages.extend(
        entries
            .iter()
            .filter(|entry| !entry.excluded)
            .map(entry_to_message),
    );
    AssembledRequest {
        messages,
        slots: prompt.slots,
    }
}

/// Outcomes of one group drain, in settle order.
#[derive(Default)]
struct GroupSettlement {
    replied: Vec<AgentId>,
    failed: Vec<AgentId>,
    /// Reply text posted by each agent, in settle order.
    replies: Vec<(AgentId, String)>,
}

/// One settled group task and what was posted for it.
struct SettledGroupReply {
    agent_id: AgentId,
    replied: bool,
    content: Option<String>,
}

/// System prompt for a single-agent conversation turn.
pub(crate) fn conversation_system_prompt(
    counter: &dyn TokenCounter,
    agent: &AgentProfile,
    agents: &[AgentProfile],
) -> SystemPromptDraft {
    let mut writer = SlotWriter::new(counter);
    writer.push(PromptSlotName::Agent, "", agent.build_system_prompt());
    writer.push(
        PromptSlotName::Platform,
        "\n\n",
        ChannelAgentService::platform_context(agents, None),
    );
    writer.push(
        PromptSlotName::ConversationConvention,
        "\n\n",
        CONVERSATION_CONVENTION.to_string(),
    );
    writer.finish()
}

/// A rendered model request and the named system-prompt slots behind it.
///
/// The slots travel with the request so a prompt snapshot can record what the
/// system prompt was assembled from, not only its finished text (ADR-0037).
#[derive(Debug, Clone)]
pub(crate) struct AssembledRequest {
    pub messages: Vec<ChatMessage>,
    pub slots: Vec<PromptSlot>,
}

/// Assemble the model request from the conversation's active branch.
///
/// The order is deterministic: system prompt first, then entries by sequence.
pub(crate) fn assemble_conversation_messages(
    counter: &dyn TokenCounter,
    agent: &AgentProfile,
    agents: &[AgentProfile],
    entries: &[ContextEntry],
    memory: Option<&str>,
) -> AssembledRequest {
    let prompt = conversation_system_prompt(counter, agent, agents);
    let mut messages = vec![ChatMessage::system(prompt.text)];
    if let Some(memory) = memory.filter(|memory| !memory.trim().is_empty()) {
        messages.push(ChatMessage::system(memory));
    }
    messages.extend(
        entries
            .iter()
            .filter(|entry| !entry.excluded)
            .map(entry_to_message),
    );
    AssembledRequest {
        messages,
        slots: prompt.slots,
    }
}

/// What a prompt snapshot records beside the finished request.
///
/// The slot names, their contents, and the memory entry ids are lost once the
/// messages are assembled, so the turn captures them here (ADR-0037).
fn snapshot_context_for(
    slots: &[PromptSlot],
    recalled: Option<&RecalledMemory>,
    counter: &dyn TokenCounter,
) -> SnapshotContext {
    let memory = recalled.map(|recalled| PromptMemory {
        block: recalled.block.clone(),
        tokens: counter.count_message(&recalled.block),
    });
    let selected = recalled
        .map(|recalled| recalled.entry_ids.clone())
        .unwrap_or_default();
    SnapshotContext::default()
        .with_slots(slots.to_vec())
        .with_memory(memory, selected)
}

/// The fixed system prompt of a router, compaction, or extraction call as one
/// named slot, so the snapshot explains a prompt that was not assembled from
/// agent and platform slots.
fn instruction_context(request: &ChatCompletionRequest) -> SnapshotContext {
    let Some(system) = request
        .messages
        .first()
        .filter(|message| message.role == ChatRole::System)
    else {
        return SnapshotContext::default();
    };
    let counter = token_counter_for_model(&request.model);
    SnapshotContext::default().with_slots(vec![PromptSlot {
        name: PromptSlotName::Instruction,
        content: system.content.clone(),
        tokens: counter.count(&system.content),
    }])
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
    use cowbot_core::usage::ESTIMATE_COUNTER;
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
    fn system_prompt_slots_render_the_original_text() {
        let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
        let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
        let participants = vec![planner.clone(), coder.clone()];

        let group = group_system_prompt(
            &ESTIMATE_COUNTER,
            &planner,
            &participants,
            Some("Coder"),
            None,
            &participants,
        );
        assert_eq!(
            group.text,
            format!(
                "{}\n\n{}\n\nYou are one participant in a group conversation. Participants, in \
                 order:\n- Planner\n- Coder\n\nMessages that address nobody are answered by Coder. \
                 Replies from other participants appear as user messages prefixed with the \
                 speaker's name in brackets, for example [Planner] ... . They are teammate \
                 history: not the user's words and not your own reply. Answer the latest user \
                 message as yourself and do not speak for other participants.",
                planner.build_system_prompt(),
                ChannelAgentService::platform_context(&participants, None),
            )
        );
        assert_eq!(
            group.slots.iter().map(|slot| slot.tokens).sum::<u32>(),
            ESTIMATE_COUNTER.count(&group.text)
        );

        let single = conversation_system_prompt(&ESTIMATE_COUNTER, &planner, &participants);
        assert_eq!(
            single.text,
            format!(
                "{}\n\n{}\n\nYou are in a private, continuous conversation with the user. The \
                 messages that follow are the retained conversation context. Reply to the latest \
                 user message, use tools when they help, and do not restate the context.",
                planner.build_system_prompt(),
                ChannelAgentService::platform_context(&participants, None),
            )
        );
        assert_eq!(
            single.slots.iter().map(|slot| slot.tokens).sum::<u32>(),
            ESTIMATE_COUNTER.count(&single.text)
        );
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

        let first = assemble_conversation_messages(
            &ESTIMATE_COUNTER,
            &agent,
            std::slice::from_ref(&agent),
            &entries,
            None,
        );
        let second = assemble_conversation_messages(
            &ESTIMATE_COUNTER,
            &agent,
            std::slice::from_ref(&agent),
            &entries,
            None,
        );

        let first = first.messages;
        let second = second.messages;

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
    fn assembly_inserts_memory_after_the_system_prompt() {
        let agent = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
        let entries = vec![entry(ContextRole::User, "question")];

        let messages = assemble_conversation_messages(
            &ESTIMATE_COUNTER,
            &agent,
            std::slice::from_ref(&agent),
            &entries,
            Some(cowbot_core::memory::MEMORY_BLOCK_HEADER),
        )
        .messages;
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(
            messages[1].content,
            cowbot_core::memory::MEMORY_BLOCK_HEADER
        );
        assert_eq!(messages[2].content, "question");

        let blank = assemble_conversation_messages(
            &ESTIMATE_COUNTER,
            &agent,
            std::slice::from_ref(&agent),
            &entries,
            Some("  "),
        )
        .messages;
        assert_eq!(
            blank[1].content, "question",
            "a blank memory block adds no message"
        );
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
            &ESTIMATE_COUNTER,
            &agent,
            std::slice::from_ref(&agent),
            &[assistant, tool],
            None,
        )
        .messages;
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

    #[test]
    fn router_response_parses_fenced_json() {
        let (names, reason) = parse_router_response(
            "```json\n{\"agents\":[\"Planner\",\"Coder\"],\"reason\":\"both\"}\n```",
        )
        .unwrap();
        assert_eq!(names, vec!["Planner", "Coder"]);
        assert_eq!(reason.as_deref(), Some("both"));
    }

    #[test]
    fn router_selection_preserves_order_deduplicates_and_caps_at_three() {
        let participants = vec![
            AgentProfile::new("Planner", "Architect", "Plan", "Careful"),
            AgentProfile::new("Coder", "Engineer", "Ship", "Careful"),
            AgentProfile::new("Reviewer", "Reviewer", "Review", "Careful"),
            AgentProfile::new("Writer", "Writer", "Write", "Careful"),
        ];
        let names = [
            "writer".to_string(),
            "Planner".to_string(),
            "WRITER".to_string(),
            "Ghost".to_string(),
            "Coder".to_string(),
            "Reviewer".to_string(),
        ];

        let selected = select_participants(&names, &participants);
        let picked = selected
            .iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(picked, vec!["Writer", "Planner", "Coder"]);
    }

    #[tokio::test]
    async fn router_timeout_is_reported() {
        struct SlowLlm;

        #[async_trait::async_trait]
        impl LlmProvider for SlowLlm {
            fn name(&self) -> &str {
                "slow"
            }

            async fn chat_complete(
                &self,
                _request: &ChatCompletionRequest,
            ) -> CowResult<ChatCompletionResponse> {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(ChatCompletionResponse {
                    content: Some("late".to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: None,
                    usage: None,
                    raw_usage: None,
                })
            }
        }

        let request = ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![ChatMessage::user("route this")],
            tools: Vec::new(),
            temperature: Some(0.0),
        };
        let failure = complete_with_timeout(Arc::new(SlowLlm), &request, Duration::from_millis(10))
            .await
            .unwrap_err();
        assert_eq!(failure, "the router timed out");
    }
}
