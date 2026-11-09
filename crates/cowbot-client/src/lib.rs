//! Transport-neutral client boundary for CowBot.
//!
//! Local mode calls the core in-process. A future remote implementation will
//! speak to `cowbot-server` over HTTP and WebSocket behind the same [`Client`]
//! trait, so the desktop UI and CLI never depend on storage or service types
//! directly (ADR-0001, ADR-0007).

use async_trait::async_trait;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel};
use cowbot_core::assignment::{Assignment, AssignmentEvent, AssignmentId};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::conversation::{
    ConversationContext, ConversationSettings, ConversationSettingsUpdate,
};
use cowbot_core::error::CowResult;
use cowbot_core::event::Event;
use cowbot_core::identity::{UserId, Workspace};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use serde::Serialize;
use tokio::sync::mpsc;

mod local;

pub use cowbot_package::MAX_PACKAGE_BYTES;
pub use cowbot_service::AgentUpdate;
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

    /// Create a user-created single-agent conversation.
    ///
    /// `provider_id` is the configured provider that supplies `model`; the pair
    /// stays fixed for the conversation.
    async fn create_conversation(
        &self,
        title: Option<&str>,
        agent_id: &AgentId,
        provider_id: Option<String>,
        model: &str,
    ) -> CowResult<Channel>;
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
    /// Exclude every active context entry; display messages are kept.
    async fn clear_conversation_context(&self, channel_id: &ChannelId) -> CowResult<u64>;

    async fn create_task(&self, task: &NewTask) -> CowResult<Task>;
    async fn list_tasks(&self) -> CowResult<Vec<Task>>;
    async fn get_task(&self, task_id: &TaskId) -> CowResult<Option<Task>>;
    async fn cancel_task(&self, task_id: &TaskId, reason: &str) -> CowResult<()>;
    async fn list_assignments(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>>;

    async fn list_runs(&self, task_id: &TaskId) -> CowResult<Vec<Run>>;
    async fn get_run(&self, run_id: &RunId) -> CowResult<Option<Run>>;
    async fn list_tool_calls(&self, run_id: &RunId) -> CowResult<Vec<ToolCallRecord>>;
    async fn list_assignment_events(
        &self,
        assignment_id: &AssignmentId,
    ) -> CowResult<Vec<AssignmentEvent>>;
    async fn list_audit_events(&self, limit: u32) -> CowResult<Vec<AuditEvent>>;

    async fn pending_approvals(&self) -> CowResult<Vec<Approval>>;
    async fn decide_approval(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval>;

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
