use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::approval::ApprovalStatus;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::mention::resolve_mentions;
use cowbot_core::task::{Task, TaskId, TaskStatus};
use cowbot_llm::LlmProvider;
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

use crate::channels::ChannelService;
use crate::conversations::ConversationService;
use crate::scheduler::{AssignmentScheduler, CrewRunOptions};

/// How many channel messages are used as conversation context for one turn.
const HISTORY_LIMIT: u32 = 20;

/// Options for one channel turn.
#[derive(Debug, Clone)]
pub struct ChannelTurnOptions {
    /// Approval policy applied to the tool calls of every agent in the turn.
    pub approval_level: ApprovalLevel,
    /// Maximum number of agents that may run at the same time.
    pub max_concurrency: usize,
}

impl Default for ChannelTurnOptions {
    fn default() -> Self {
        Self {
            approval_level: ApprovalLevel::Ask,
            max_concurrency: 4,
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
}

impl ChannelAgentService {
    pub fn new(store: Store, bus: EventBus, workspace_id: WorkspaceId) -> Self {
        Self {
            conversations: ConversationService::new(
                store.clone(),
                bus.clone(),
                workspace_id.clone(),
            ),
            channels: ChannelService::new(store.clone(), bus),
            store,
            workspace_id,
        }
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
        .with_scope();
        let run_options = CrewRunOptions {
            approval_level: options.approval_level,
            channel_id: Some(channel.id.clone()),
            max_concurrency: options.max_concurrency,
        };

        // History excludes this turn's own notices, so an "@Nobody" notice
        // posted above does not lead the prompt.
        let history = self.render_history(&channel, message, &agents).await?;

        let mut tasks = Vec::new();
        let platform = Self::platform_context(&agents);
        for agent in &addressees {
            scheduler.register_agent(agent.clone(), llm.clone(), tools.clone(), model);
            let task = Self::turn_task(agent, &channel, message, &history, direct, &platform);
            scheduler
                .enqueue(
                    &task,
                    requester.clone(),
                    AssignmentOrigin::User,
                    &run_options,
                )
                .await?;
            tasks.push(task.id);
        }

        // A failing agent must not silence the others: the scheduler drains
        // every root and the per-task status decides what gets posted.
        let outcome = scheduler.run_until_idle().await;

        for task_id in &tasks {
            self.post_outcome(&channel, task_id).await?;
        }

        outcome?;
        Ok(ChannelTurnReport {
            agents: addressees.iter().map(|agent| agent.id.clone()).collect(),
            unknown_mentions: Vec::new(),
            tasks,
        })
    }

    /// Environment shared by every channel turn.
    pub(crate) fn platform_context(agents: &[AgentProfile]) -> String {
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
             action you cannot perform, explain the matching CowBot UI action plainly.",
            roster = roster.join("\n"),
        )
    }

    fn turn_task(
        agent: &AgentProfile,
        channel: &Channel,
        message: &ChannelMessage,
        history: &str,
        direct: bool,
        platform: &str,
    ) -> Task {
        let author = match &message.sender {
            Sender::User(_) => "the user".to_string(),
            Sender::Agent(id) => format!("agent '{}'", id.0),
            Sender::System => "the system".to_string(),
        };

        let description = if direct {
            format!(
                "{platform}\n\n\
                 You are the agent in a direct conversation with {author}.\n\n\
                 Recent messages in this conversation:\n{history}\n\n\
                 Latest message from {author}:\n{content}\n\n\
                 Reply as yourself. Be concise, answer what was asked, and do not \
                 repeat the conversation history back to the user.",
                platform = platform,
                history = history,
                author = author,
                content = message.content.trim(),
            )
        } else {
            format!(
                "{platform}\n\n\
                 You were mentioned in the channel #{channel_name}.\n\n\
                 Recent channel messages:\n{history}\n\n\
                 Latest message from {author}:\n{content}\n\n\
                 Reply to the latest message as a teammate in this channel. \
                 Be concise, answer what was asked, and do not repeat the channel \
                 history back to the user.",
                platform = platform,
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

    async fn post_outcome(&self, channel: &Channel, task_id: &TaskId) -> CowResult<()> {
        let Some(task) = self.store.get_task(task_id).await? else {
            return Ok(());
        };
        let Some(assignee) = task.assigned_agent.clone() else {
            return Ok(());
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
                } else {
                    self.channels
                        .post_message(&channel.id, Sender::Agent(assignee), content.to_string())
                        .await?;
                }
            }
            TaskStatus::Failed { error } => {
                self.post_notice(channel, &format!("{name} could not answer: {error}"))
                    .await?;
            }
            _ => {}
        }
        Ok(())
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
