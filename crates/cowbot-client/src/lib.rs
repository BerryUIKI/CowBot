//! Transport-neutral client boundary for CowBot.
//!
//! Local mode calls the core in-process. A future remote implementation will
//! speak to `cowbot-server` over HTTP and WebSocket behind the same [`Client`]
//! trait, so the desktop UI and CLI never depend on storage or service types
//! directly (ADR-0001, ADR-0007).

use async_trait::async_trait;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, BypassCritical, WorkspacePolicy};
use cowbot_core::assignment::{Assignment, AssignmentEvent, AssignmentId};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::conversation::{
    ChannelParticipant, ConversationContext, ConversationSettings, ConversationSettingsUpdate,
    ConversationTurn, TurnId,
};
use cowbot_core::error::CowResult;
use cowbot_core::event::Event;
use cowbot_core::identity::{UserId, Workspace};
use cowbot_core::memory::{MemoryEntry, MemorySettings, Note};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::prompt_snapshot::{
    PromptSnapshotDetail, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_core::tool::ToolDefinition;
use serde::Serialize;
use tokio::sync::mpsc;

mod local;

pub use cowbot_package::MAX_PACKAGE_BYTES;
pub use cowbot_service::{
    AgentUpdate, ConversationPreview, MemoryFilter, MemorySave, MemorySettingsUpdate, MemoryUpdate,
    NewMemory, NoteSave, RecalledMemory, TaskRunPreview,
};
pub use local::LocalClient;

/// User identity as seen by the client.
///
/// Credentials never cross this boundary; a remote implementation receives a
/// session token instead of a password hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientUser {
    pub id: UserId,
    pub name: String,
    pub email: String,
}

/// The authenticated user and workspace a client operates in.
#[derive(Debug, Clone, Serialize)]
pub struct ClientSession {
    pub user: ClientUser,
    pub workspace: Workspace,
}

/// One remembered session approval decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionDecisionView {
    pub key: String,
    pub decision: String,
}

/// Summary of a workspace imported from a `.cowpkg` package.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceImport {
    pub workspace_id: String,
    pub workspace_name: String,
    pub channels: usize,
    pub messages: usize,
    pub agents: usize,
    pub tasks: usize,
    pub runs: usize,
    pub audit_events: usize,
}

/// Parameters for creating a task and its root assignment.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub description: String,
    pub expected_output: String,
    pub assignee: AgentId,
    pub channel_id: Option<ChannelId>,
    pub approval_level: ApprovalLevel,
}

impl NewTask {
    pub fn new(
        description: impl Into<String>,
        expected_output: impl Into<String>,
        assignee: AgentId,
    ) -> Self {
        Self {
            description: description.into(),
            expected_output: expected_output.into(),
            assignee,
            channel_id: None,
            approval_level: ApprovalLevel::Ask,
        }
    }

    pub fn in_channel(mut self, channel_id: ChannelId) -> Self {
        self.channel_id = Some(channel_id);
        self
    }

    pub fn with_approval_level(mut self, approval_level: ApprovalLevel) -> Self {
        self.approval_level = approval_level;
        self
    }
}

/// Parameters for creating a user-created conversation.
#[derive(Debug, Clone, Serialize)]
pub struct NewConversation {
    pub title: Option<String>,
    /// Participants in selection order; one makes a single conversation.
    pub agent_ids: Vec<AgentId>,
    pub provider_id: Option<String>,
    pub model: String,
    /// Route unaddressed group messages with the model; single ignores it.
    #[serde(default)]
    pub router_enabled: bool,
    /// Participant that answers unaddressed group messages.
    #[serde(default)]
    pub default_responder: Option<AgentId>,
}

impl NewConversation {
    /// A single-agent conversation.
    pub fn single(
        title: Option<String>,
        agent_id: AgentId,
        provider_id: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            title,
            agent_ids: vec![agent_id],
            provider_id,
            model: model.into(),
            router_enabled: false,
            default_responder: None,
        }
    }
}

/// A conversation channel with its fixed settings and participant profiles.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummary {
    pub channel: Channel,
    pub settings: ConversationSettings,
    pub participants: Vec<AgentProfile>,
}

/// The workspace note and, when one conversation is selected, its note.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryNotes {
    pub workspace: Option<Note>,
    pub conversation: Option<Note>,
}

/// A live stream of platform events for the UI.
pub struct EventStream {
    receiver: mpsc::Receiver<Event>,
}
impl EventStream {
    pub(crate) fn new(receiver: mpsc::Receiver<Event>) -> Self {
        Self { receiver }
    }

    /// Wait for the next event. Returns `None` once the stream is closed.
    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
}

/// Unified client interface for local and future remote modes.
#[async_trait]
pub trait Client: Send + Sync {
    /// The local user and workspace this client is bound to.
    async fn session(&self) -> CowResult<ClientSession>;

    /// Update the local user's display name and email.
    async fn update_user_profile(&self, name: &str, email: &str) -> CowResult<ClientSession>;

    /// Subscribe to live platform events.
    ///
    /// Local mode starts a forwarding task, so call this from within a Tokio
    /// runtime context.
    fn subscribe(&self) -> EventStream;

    async fn list_channels(&self) -> CowResult<Vec<Channel>>;
    async fn get_channel(&self, channel_id: &ChannelId) -> CowResult<Option<Channel>>;
    async fn create_channel(&self, name: &str, topic: &str) -> CowResult<Channel>;
    async fn rename_channel(
        &self,
        channel_id: &ChannelId,
        name: &str,
        topic: &str,
    ) -> CowResult<Channel>;
    async fn delete_channel(&self, channel_id: &ChannelId) -> CowResult<Channel>;
    async fn set_channel_pinned(&self, channel_id: &ChannelId, pinned: bool) -> CowResult<Channel>;
    async fn clear_channel_messages(&self, channel_id: &ChannelId) -> CowResult<u64>;
    async fn post_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<ChannelMessage>;
    async fn post_thread_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
        thread_id: &str,
    ) -> CowResult<ChannelMessage>;
    async fn list_messages(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ChannelMessage>>;

    /// Create a user-created conversation with one to eight participants.
    ///
    /// One participant creates a single conversation; two or more create a
    /// group whose membership is fixed. `provider_id` is the configured
    /// provider that supplies `model`; the pair stays fixed for the
    /// conversation.
    async fn create_conversation(&self, conversation: &NewConversation) -> CowResult<Channel>;
    /// Conversations with their fixed settings and participant profiles.
    async fn list_conversation_summaries(&self) -> CowResult<Vec<ConversationSummary>>;
    /// Persisted participants of a conversation, in participant order.
    async fn list_conversation_participants(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<Vec<ChannelParticipant>>;
    /// Persisted routing decisions of a conversation, most recent first.
    async fn list_conversation_turns(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ConversationTurn>>;
    async fn conversation_settings(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<ConversationSettings>;
    async fn update_conversation_settings(
        &self,
        channel_id: &ChannelId,
        update: &ConversationSettingsUpdate,
    ) -> CowResult<ConversationSettings>;
    /// Token breakdown and cache summaries of a conversation context.
    async fn conversation_context(&self, channel_id: &ChannelId) -> CowResult<ConversationContext>;
    /// The exact request one participant's next turn would send.
    ///
    /// The preview never writes: no context entry, recall statistic, or LLM
    /// call. `draft` is the composer's pending message; `platform_context` is
    /// the caller's extra environment text; `plan_mode` adds the Plan-mode
    /// instruction a turn in that mode carries; `tools` are the schemas the
    /// caller would send.
    async fn conversation_context_preview(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        draft: Option<&str>,
        platform_context: Option<&str>,
        plan_mode: bool,
        tools: &[ToolDefinition],
    ) -> CowResult<ConversationPreview>;
    /// The request a task run started with, rebuilt from stored data.
    async fn task_run_context_preview(
        &self,
        run_id: &RunId,
        model: &str,
        tools: &[ToolDefinition],
    ) -> CowResult<Option<TaskRunPreview>>;
    /// The redacted prompt snapshots recorded for a run, oldest first.
    ///
    /// Unlike a preview, a snapshot is the request that was actually sent and
    /// cannot be rebuilt (ADR-0037).
    async fn list_run_prompt_snapshots(
        &self,
        run_id: &RunId,
    ) -> CowResult<Vec<PromptSnapshotSummary>>;
    /// The newest prompt snapshots in the workspace, for the audit view.
    async fn list_recent_prompt_snapshots(
        &self,
        limit: u32,
    ) -> CowResult<Vec<PromptSnapshotSummary>>;
    /// One stored prompt snapshot with the call links it belongs to.
    async fn prompt_snapshot(&self, llm_call_id: &str) -> CowResult<Option<PromptSnapshotDetail>>;
    /// The workspace prompt-snapshot settings.
    async fn prompt_snapshot_settings(&self) -> CowResult<PromptSnapshotSettings>;
    /// Change whether snapshots are stored and how long they are kept.
    ///
    /// The new retention window applies immediately, so lowering it deletes
    /// the snapshots that just fell outside it.
    async fn update_prompt_snapshot_settings(
        &self,
        enabled: Option<bool>,
        retention_days: Option<u32>,
    ) -> CowResult<PromptSnapshotSettings>;
    /// Delete expired snapshots now and report how many went.
    async fn purge_prompt_snapshots(&self) -> CowResult<u64>;
    /// Exclude every active context entry; display messages are kept.
    async fn clear_conversation_context(&self, channel_id: &ChannelId) -> CowResult<u64>;
    /// Exclude one participant's active context entries.
    async fn clear_conversation_participant_context(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<u64>;

    /// The workspace note and, when a conversation is selected, its note.
    async fn memory_notes(&self, channel_id: Option<&ChannelId>) -> CowResult<MemoryNotes>;
    /// Save the workspace note, redacting credential-shaped content.
    async fn save_workspace_note(&self, content: &str) -> CowResult<NoteSave>;
    /// Save a conversation note; only user-created conversations accept one.
    async fn save_conversation_note(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<NoteSave>;
    async fn delete_conversation_note(&self, channel_id: &ChannelId) -> CowResult<u64>;
    async fn memory_settings(&self) -> CowResult<MemorySettings>;
    async fn update_memory_settings(
        &self,
        update: &MemorySettingsUpdate,
    ) -> CowResult<MemorySettings>;
    /// Entries matching the optional status and exact scope filters.
    async fn list_memory_entries(&self, filter: &MemoryFilter) -> CowResult<Vec<MemoryEntry>>;
    /// Create an entry after validating its content and scope.
    async fn add_memory_entry(&self, new: NewMemory) -> CowResult<MemorySave>;
    /// Edit the content, kind, or pinning of an entry.
    async fn update_memory_entry(&self, id: &str, update: &MemoryUpdate) -> CowResult<MemorySave>;
    /// Approve a pending extraction candidate.
    async fn approve_memory_entry(&self, id: &str) -> CowResult<MemoryEntry>;
    /// Dismiss a pending candidate, keeping it visible as archived.
    async fn dismiss_memory_entry(&self, id: &str) -> CowResult<MemoryEntry>;
    async fn archive_memory_entry(&self, id: &str) -> CowResult<MemoryEntry>;
    async fn delete_memory_entry(&self, id: &str) -> CowResult<u64>;
    /// What the next turn would inject, without touching recall statistics.
    async fn preview_memory_recall(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<Option<RecalledMemory>>;

    async fn create_task(&self, task: &NewTask) -> CowResult<Task>;
    async fn list_tasks(&self) -> CowResult<Vec<Task>>;
    /// Tasks created by the run that `task_id` started.
    async fn list_run_tasks(&self, task_id: &TaskId) -> CowResult<Vec<Task>>;
    async fn get_task(&self, task_id: &TaskId) -> CowResult<Option<Task>>;
    async fn cancel_task(&self, task_id: &TaskId, reason: &str) -> CowResult<()>;
    /// Stop a running turn: a conversation reply, a group reply, or a task run.
    ///
    /// Idempotent: an unknown or already finished turn returns `false` and does
    /// nothing. A stopped turn keeps the text it produced, stored with a
    /// stopped marker (ADR-0039).
    async fn cancel_turn(&self, turn_id: &TurnId) -> CowResult<bool>;
    async fn archive_task(&self, task_id: &TaskId) -> CowResult<Task>;
    async fn unarchive_task(&self, task_id: &TaskId) -> CowResult<Task>;
    /// Approve the Plan-mode artifact stored with a task.
    async fn approve_task_plan(&self, task_id: &TaskId) -> CowResult<Task>;
    async fn list_assignments(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>>;

    async fn list_runs(&self, task_id: &TaskId) -> CowResult<Vec<Run>>;
    async fn get_run(&self, run_id: &RunId) -> CowResult<Option<Run>>;
    async fn list_tool_calls(&self, run_id: &RunId) -> CowResult<Vec<ToolCallRecord>>;
    async fn list_assignment_events(
        &self,
        assignment_id: &AssignmentId,
    ) -> CowResult<Vec<AssignmentEvent>>;
    async fn list_audit_events(
        &self,
        limit: u32,
        source: Option<&str>,
    ) -> CowResult<Vec<AuditEvent>>;

    /// Workspace-level allow / ask / deny rules for tool calls.
    async fn list_permission_rules(&self) -> CowResult<Vec<PermissionRule>>;
    async fn create_permission_rule(
        &self,
        tool: &str,
        command: Option<&str>,
        decision: RuleDecision,
    ) -> CowResult<PermissionRule>;
    async fn set_permission_rule_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> CowResult<PermissionRule>;
    async fn delete_permission_rule(&self, id: &str) -> CowResult<()>;
    async fn workspace_policy(&self) -> CowResult<WorkspacePolicy>;
    async fn set_bypass_critical(
        &self,
        bypass_critical: BypassCritical,
    ) -> CowResult<WorkspacePolicy>;

    async fn pending_approvals(&self) -> CowResult<Vec<Approval>>;
    async fn decide_approval(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval>;
    /// Decide an approval and remember it for the rest of this process.
    async fn decide_approval_for_session(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval>;
    async fn session_decisions(&self) -> CowResult<Vec<SessionDecisionView>>;
    async fn clear_session_decisions(&self) -> CowResult<()>;

    async fn list_templates(&self) -> CowResult<Vec<TemplateDocument>>;
    async fn get_template(&self, id: &str) -> CowResult<Option<TemplateDocument>>;
    async fn save_template(&self, document: &TemplateDocument) -> CowResult<()>;
    async fn delete_template(&self, id: &str) -> CowResult<()>;
    async fn import_template(&self, json: &str) -> CowResult<TemplateDocument>;
    async fn export_template(&self, id: &str) -> CowResult<String>;
    async fn instantiate_agents(&self, template_id: &str) -> CowResult<Vec<AgentProfile>>;
    async fn list_agents(&self) -> CowResult<Vec<AgentProfile>>;
    async fn update_agent(&self, update: &AgentUpdate) -> CowResult<AgentProfile>;
}
