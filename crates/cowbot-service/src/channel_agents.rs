use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::approval::ApprovalStatus;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::{RouteSource, TurnId};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::mention::resolve_mentions;
use cowbot_core::task::{Task, TaskId, TaskStatus};
use cowbot_llm::LlmProvider;
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

use crate::channels::ChannelService;
use crate::compaction_locks::CompactionLocks;
use crate::conversations::ConversationService;
use crate::memory::MemoryService;
use crate::scheduler::{AssignmentScheduler, CrewRunOptions};
use crate::session_decisions::SessionDecisions;

/// How many channel messages are used as conversation context for one turn.
const HISTORY_LIMIT: u32 = 20;

/// Usable context window and tool-schema cost the caller knows for one agent.
///
/// The service does not own model metadata or the tool registry, so the
/// desktop supplies these values for the group post-routing capacity check.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantCapacity {
    pub agent_id: AgentId,
    /// Window left after the model's maximum output and the safety buffer.
    pub usable_window: u32,
    /// Tokens the agent's tool schemas take in every request.
    pub tool_tokens: u32,
}

/// Options for one channel turn.
#[derive(Debug, Clone)]
pub struct ChannelTurnOptions {
    /// Approval policy applied to the tool calls of every agent in the turn.
    pub approval_level: ApprovalLevel,
    /// Maximum number of agents that may run at the same time.
    pub max_concurrency: usize,
    /// Let Auto mode ask the active model for permission decisions.
    pub use_model_permissions: bool,
    /// Extra environment text appended to the platform context, for example
    /// registered projects the desktop can target with workspace tools.
    pub platform_context: Option<String>,
    /// Per-participant windows used by the group post-routing capacity check.
    ///
    /// Callers without model metadata (the CLI) pass nothing; the service then
    /// skips the percentage-based group gate.
    pub participant_capacity: Vec<ParticipantCapacity>,
}

impl Default for ChannelTurnOptions {
    fn default() -> Self {
        Self {
            approval_level: ApprovalLevel::Ask,
            max_concurrency: 4,
            use_model_permissions: false,
            platform_context: None,
            participant_capacity: Vec::new(),
        }
    }
}

/// What one channel turn did.
#[derive(Debug, Clone, Default)]
pub struct ChannelTurnReport {
    /// Agents that were addressed by the message.
    pub agents: Vec<AgentId>,
    /// Mentioned names that matched no agent.
    pub unknown_mentions: Vec<String>,
    /// Tasks created for the turn.
    pub tasks: Vec<TaskId>,
    /// Group responders whose answer was posted.
    pub replied: Vec<AgentId>,
    /// Group responders whose task failed.
    pub failed: Vec<AgentId>,
    /// Group responders skipped because their context branch was full.
    pub skipped: Vec<AgentId>,
    /// How a group message selected its responders.
    pub route: Option<RouteSource>,
    /// Persisted group turn row.
    pub turn_id: Option<TurnId>,
}

impl ChannelTurnReport {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

/// Turns channel messages that address agents into assignment-backed work.
///
/// The user's message is persisted by [`ChannelService`] before it reaches this
/// service. Every mentioned agent gets a task in the channel, the assignment
/// scheduler runs them, and each settled task posts its output back into the
/// timeline as a message from that agent. Failures and unknown mentions are
/// reported by the channel itself, so a user is never left without feedback.
#[derive(Clone)]
pub struct ChannelAgentService {
    store: Store,
    workspace_id: WorkspaceId,
    channels: ChannelService,
    conversations: ConversationService,
    memory: MemoryService,
    session_decisions: Option<Arc<SessionDecisions>>,
}

impl ChannelAgentService {
    pub fn new(store: Store, bus: EventBus, workspace_id: WorkspaceId) -> Self {
        Self {
            conversations: ConversationService::new(
                store.clone(),
                bus.clone(),
                workspace_id.clone(),
            ),
            memory: MemoryService::new(store.clone(), workspace_id.clone()),
            channels: ChannelService::new(store.clone(), bus),
            store,
            workspace_id,
            session_decisions: None,
        }
    }

    pub fn with_session_decisions(mut self, decisions: Option<Arc<SessionDecisions>>) -> Self {
        self.session_decisions = decisions.clone();
        self.conversations = self.conversations.with_session_decisions(decisions);
        self
    }

    /// Share the process-wide compaction registry with the desktop's
    /// conversation service, so both paths contend on the same branch locks.
    pub fn with_compaction_locks(mut self, locks: Arc<CompactionLocks>) -> Self {
        self.conversations = self.conversations.with_compaction_locks(locks);
        self
    }

    /// Answer `message` on behalf of every agent it mentions.
    ///
    /// Returns an empty report when the message addresses nobody, leaving the
    /// channel untouched. A message that mentions an unknown name, or a
    /// workspace without agents, records a system notice so the user knows why
    /// nothing happened.
    pub async fn reply(
        &self,
        channel_id: &ChannelId,
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        model: &str,
        options: &ChannelTurnOptions,
    ) -> CowResult<ChannelTurnReport> {
        let Some(channel) = self.store.get_channel(channel_id).await? else {
            return Err(CowError::Task(format!(
                "channel '{channel_id}' was not found"
            )));
        };

        // User-created conversations have their own participant set, fixed
        // model, and append-only context branch.
        if channel.is_conversation() {
            return self
                .conversations
                .reply(channel_id, message, llm, tools, options)
                .await;
        }

        let agents = self.store.list_agents(&self.workspace_id).await?;
        let direct = channel.agent_id.is_some();

        // A direct conversation has exactly one addressee: its agent answers
        // every message, so the user never has to type an @mention. Mentions
        // in a direct conversation are plain text, including @team.
        let addressees: Vec<AgentProfile> = if direct {
            let agent = channel
                .agent_id
                .as_ref()
                .and_then(|agent_id| agents.iter().find(|agent| &agent.id == agent_id));
            match agent {
                Some(agent) => vec![agent.clone()],
                None => {
                    self.post_notice(
                        &channel,
                        "This conversation's agent is no longer available.",
                    )
                    .await?;
                    return Ok(ChannelTurnReport::default());
                }
            }
        } else {
            let resolution = resolve_mentions(&message.content, &agents);

            if agents.is_empty() {
                if !resolution.is_empty() {
                    self.post_notice(
                        &channel,
                        "No agents in this workspace yet. Add a team from a template in Settings, then mention an agent by name.",
                    )
                    .await?;
                }
                return Ok(ChannelTurnReport {
                    unknown_mentions: resolution.unknown,
                    ..ChannelTurnReport::default()
                });
            }

            if !resolution.unknown.is_empty() {
                let available = agents
                    .iter()
                    .map(|agent| format!("@{}", agent.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.post_notice(
                    &channel,
                    &format!(
                        "No agent matched {}. Available agents: {}.",
                        resolution
                            .unknown
                            .iter()
                            .map(|name| format!("@{name}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        available
                    ),
                )
                .await?;
            }

            if resolution.agents.is_empty() {
                return Ok(ChannelTurnReport {
                    unknown_mentions: resolution.unknown,
                    ..ChannelTurnReport::default()
                });
            }

            resolution.agents
        };

        let requester = match &message.sender {
            Sender::User(id) => AssignmentRequester::User(id.clone()),
            _ => AssignmentRequester::System,
        };

        // A scoped scheduler claims only the roots enqueued here, so a turn
        // never picks up work from another turn or a visible crew run.
        let mut scheduler = AssignmentScheduler::new(
            self.store.clone(),
            self.channels.bus().clone(),
            self.workspace_id.clone(),
            options.max_concurrency,
        )
        .with_scope()
        .with_model_permissions(options.use_model_permissions)
        .with_session_decisions(self.session_decisions.clone());
        let run_options = CrewRunOptions {
            approval_level: options.approval_level,
            channel_id: Some(channel.id.clone()),
            max_concurrency: options.max_concurrency,
            use_model_permissions: options.use_model_permissions,
            capture_plan_task: None,
        };

        // History excludes this turn's own notices, so an "@Nobody" notice
        // posted above does not lead the prompt.
        let history = self.render_history(&channel, message, &agents).await?;

        let mut tasks = Vec::new();
        let platform = Self::platform_context(&agents, options.platform_context.as_deref());
        for agent in &addressees {
            scheduler.register_agent(agent.clone(), llm.clone(), tools.clone(), model);
            let memory = if direct {
                self.memory
                    .recall_for_prompt(Some(&channel.id), &agent.id, message.content.trim())
                    .await
            } else {
                None
            };
            let task = Self::turn_task(
                agent,
                &channel,
                message,
                &history,
                direct,
                &platform,
                memory.as_deref(),
            );
            let mut task_options = run_options.clone();
            if options.approval_level == ApprovalLevel::PlanOnly {
                task_options.capture_plan_task = Some(task.id.clone());
            }
            scheduler
                .enqueue(
                    &task,
                    requester.clone(),
                    AssignmentOrigin::User,
                    &task_options,
                )
                .await?;
            tasks.push(task.id);
        }

        // A failing agent must not silence the others: the scheduler drains
        // every root and the per-task status decides what gets posted.
        let outcome = scheduler.run_until_idle().await;

        let mut posted = Vec::new();
        for task_id in &tasks {
            if let Some(reply) = self.post_outcome(&channel, task_id).await? {
                posted.push(reply);
            }
        }

        outcome?;

        // Direct conversations extract one aggregate candidate set per settled
        // turn; plain channel mention turns keep their current behavior.
        if direct && !posted.is_empty() {
            self.memory
                .extract_after_turn(
                    &channel,
                    llm,
                    model,
                    message.content.trim(),
                    &posted.join("\n\n"),
                    channel.agent_id.as_ref(),
                    &message.id,
                )
                .await;
        }

        Ok(ChannelTurnReport {
            agents: addressees.iter().map(|agent| agent.id.clone()).collect(),
            unknown_mentions: Vec::new(),
            tasks,
            ..ChannelTurnReport::default()
        })
    }

    /// Environment shared by every channel turn.
    pub(crate) fn platform_context(agents: &[AgentProfile], extra: Option<&str>) -> String {
        let mut roster = agents
            .iter()
            .take(20)
            .map(|agent| format!("- {} — {}", agent.name, agent.role))
            .collect::<Vec<_>>();
        if agents.len() > 20 {
            roster.push(format!("- and {} more", agents.len() - 20));
        }
        if roster.is_empty() {
            roster.push("- (no agents yet)".to_string());
        }
        let extra = extra
            .map(|extra| format!("\n\n{extra}"))
            .unwrap_or_default();

        format!(
            "Environment: you are running inside CowBot, a local desktop workspace for a \
             one-person company. The user is the operator and sees your reply in the CowBot app.\n\n\
             Workspace agents:\n{roster}\n\n\
             Workspace actions (only when enabled for you): create_channel creates a group \
             channel; create_task queues work for another agent but does not run it; \
             instantiate_team adds agents from a template; list_agents, list_channels, and \
             list_templates inspect the workspace. delete_channel deletes a group channel \
             and always requires human approval; agent direct conversations cannot be \
             deleted. Deleting agents is a UI action you cannot perform.\n\n\
             Rendering: normal answers use Markdown. For an interactive interface, put a \
             complete HTML document in a ```html code block; it runs in a sandbox with inline \
             CSS and JavaScript but no network access, and it cannot send messages back.\n\n\
             Rules: reply in the language of the latest user message. Do not invent or refer to \
             external products such as Slack, Discord, Teams, or Feishu. If a request needs an \
             action you cannot perform, explain the matching CowBot UI action plainly.{extra}",
            roster = roster.join("\n"),
            extra = extra,
        )
    }

    fn turn_task(
        agent: &AgentProfile,
        channel: &Channel,
        message: &ChannelMessage,
        history: &str,
        direct: bool,
        platform: &str,
        memory: Option<&str>,
    ) -> Task {
        let author = match &message.sender {
            Sender::User(_) => "the user".to_string(),
            Sender::Agent(id) => format!("agent '{}'", id.0),
            Sender::System => "the system".to_string(),
        };
        let memory = memory
            .map(|memory| format!("\n\n{memory}"))
            .unwrap_or_default();

        let description = if direct {
            format!(
                "{platform}{memory}\n\n\
                 You are the agent in a direct conversation with {author}.\n\n\
                 Recent messages in this conversation:\n{history}\n\n\
                 Latest message from {author}:\n{content}\n\n\
                 Reply as yourself. Be concise, answer what was asked, and do not \
                 repeat the conversation history back to the user.",
                platform = platform,
                memory = memory,
                history = history,
                author = author,
                content = message.content.trim(),
            )
        } else {
            format!(
                "{platform}{memory}\n\n\
                 You were mentioned in the channel #{channel_name}.\n\n\
                 Recent channel messages:\n{history}\n\n\
                 Latest message from {author}:\n{content}\n\n\
                 Reply to the latest message as a teammate in this channel. \
                 Be concise, answer what was asked, and do not repeat the channel \
                 history back to the user.",
                platform = platform,
                memory = memory,
                channel_name = channel.name,
                history = history,
                author = author,
                content = message.content.trim(),
            )
        };

        Task::new(
            description,
            "A concise reply addressed to the user, ready to post in the channel.",
        )
        .with_title(message.content.lines().next().unwrap_or_default())
        .with_channel(channel.id.clone())
        .assign_to(agent.id.clone())
    }

    async fn post_outcome(&self, channel: &Channel, task_id: &TaskId) -> CowResult<Option<String>> {
        let Some(task) = self.store.get_task(task_id).await? else {
            return Ok(None);
        };
        let Some(assignee) = task.assigned_agent.clone() else {
            return Ok(None);
        };
        let name = self
            .store
            .get_agent(&assignee)
            .await?
            .map(|agent| agent.name)
            .unwrap_or_else(|| format!("Agent {}", assignee.0));

        match &task.status {
            TaskStatus::Completed { output } => {
                let content = output.trim();
                if content.is_empty() {
                    let approvals = self.store.list_approvals_for_task(&task.id).await?;
                    let expired = approvals
                        .iter()
                        .any(|approval| approval.status == ApprovalStatus::Expired);
                    if expired {
                        self.post_notice(
                            channel,
                            &format!(
                                "{name} did not reply because the approval request expired before a decision."
                            ),
                        )
                        .await?;
                    } else {
                        self.post_notice(channel, &format!("{name} finished without a reply."))
                            .await?;
                    }
                    Ok(None)
                } else {
                    self.channels
                        .post_message(&channel.id, Sender::Agent(assignee), content.to_string())
                        .await?;
                    Ok(Some(content.to_string()))
                }
            }
            TaskStatus::Failed { error } => {
                self.post_notice(channel, &format!("{name} could not answer: {error}"))
                    .await?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn render_history(
        &self,
        channel: &Channel,
        message: &ChannelMessage,
        agents: &[AgentProfile],
    ) -> CowResult<String> {
        let messages = self.store.list_messages(&channel.id, HISTORY_LIMIT).await?;

        let mut history = String::new();
        for entry in messages
            .iter()
            .filter(|entry| entry.id != message.id && entry.created_at < message.created_at)
        {
            let author = match &entry.sender {
                Sender::User(_) => "User".to_string(),
                Sender::Agent(id) => agents
                    .iter()
                    .find(|agent| &agent.id == id)
                    .map(|agent| agent.name.clone())
                    .unwrap_or_else(|| format!("Agent {}", id.0)),
                Sender::System => "System".to_string(),
            };
            history.push_str(&format!("{author}: {}\n", entry.content.trim()));
        }

        if history.is_empty() {
            Ok("(no earlier messages)".to_string())
        } else {
            Ok(history.trim_end().to_string())
        }
    }

    async fn post_notice(&self, channel: &Channel, content: &str) -> CowResult<()> {
        self.channels
            .post_message(&channel.id, Sender::System, content.to_string())
            .await?;
        Ok(())
    }
}
