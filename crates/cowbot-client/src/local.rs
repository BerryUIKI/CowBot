use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cowbot_agent::Crew;
use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, ApprovalStatus};
use cowbot_core::assignment::{
    Assignment, AssignmentEvent, AssignmentId, AssignmentOrigin, AssignmentRequester,
};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::{
    CompactionOutcome, ConversationContext, ConversationSettings, ConversationSettingsUpdate,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_llm::LlmProvider;
use cowbot_service::{
    bootstrap_local, bootstrap_local_named, AgentService, ApprovalService, AssignmentService,
    ChannelAgentService, ChannelService, ChannelTurnOptions, ChannelTurnReport,
    ConversationService, CrewRunOptions, CrewRunner, LocalContext, TaskService, TemplateService,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

use crate::{
    AgentUpdate, Client, ClientSession, ClientUser, EventStream, NewTask, WorkspaceImport,
};

/// In-process client backed by `cowbot-store` and `cowbot-service`.
pub struct LocalClient {
    store: Store,
    bus: EventBus,
    session: ClientSession,
    channels: ChannelService,
    channel_agents: ChannelAgentService,
    conversations: ConversationService,
    templates: TemplateService,
    tasks: TaskService,
    assignments: AssignmentService,
    approvals: ApprovalService,
    agents: AgentService,
}

impl LocalClient {
    /// Open (or create) the local SQLite workspace at `path`.
    pub async fn open(path: impl AsRef<Path>) -> CowResult<Self> {
        let store = Store::connect(path).await?;
        Self::initialize(store, EventBus::default_bus()).await
    }

    /// Open an in-memory workspace, primarily for tests and embedded use.
    pub async fn open_in_memory() -> CowResult<Self> {
        let store = Store::connect_in_memory().await?;
        Self::initialize(store, EventBus::default_bus()).await
    }

    async fn initialize(store: Store, bus: EventBus) -> CowResult<Self> {
        store.migrate().await?;
        let context = bootstrap_local(&store).await?;
        Self::build(store, bus, context).await
    }

    async fn build(store: Store, bus: EventBus, context: LocalContext) -> CowResult<Self> {
        let assignments = AssignmentService::new(store.clone(), bus.clone());
        assignments.reconcile_on_startup().await?;
        let approvals =
            ApprovalService::new(store.clone(), bus.clone(), context.workspace.id.clone());
        approvals.reconcile_on_startup().await?;

        let channels = ChannelService::new(store.clone(), bus.clone());
        let channel_agents =
            ChannelAgentService::new(store.clone(), bus.clone(), context.workspace.id.clone());
        let conversations =
            ConversationService::new(store.clone(), bus.clone(), context.workspace.id.clone());
        let templates = TemplateService::new(store.clone());
        let tasks = TaskService::new(store.clone(), bus.clone());
        let agents = AgentService::new(store.clone(), bus.clone());
        let session = ClientSession {
            user: ClientUser {
                id: context.user.id,
                name: context.user.name,
                email: context.user.email,
            },
            workspace: context.workspace,
        };

        Ok(Self {
            store,
            bus,
            session,
            channels,
            channel_agents,
            conversations,
            templates,
            tasks,
            assignments,
            approvals,
            agents,
        })
    }

    fn user_sender(&self) -> Sender {
        Sender::User(self.session.user.id.0.clone())
    }

    /// Export the current workspace as a `.cowpkg` archive.
    pub async fn export_workspace(&self) -> CowResult<Vec<u8>> {
        cowbot_package::export_workspace(&self.store, &self.session.workspace.id)
            .await
            .map_err(|error| CowError::Package(error.to_string()))
    }

    /// Create a new local database from a `.cowpkg` archive.
    ///
    /// Import keeps the package's entity ids, so callers must pass a fresh
    /// database path. Bootstrap reuses the imported owner and workspace, and
    /// the returned summary describes what was written.
    pub async fn create_from_package(
        path: impl AsRef<Path>,
        bytes: &[u8],
    ) -> CowResult<(Self, WorkspaceImport)> {
        let store = Store::connect(path).await?;
        store.migrate().await?;
        let summary = cowbot_package::import_workspace(&store, bytes)
            .await
            .map_err(|error| CowError::Package(error.to_string()))?;
        let context = bootstrap_local(&store).await?;
        let client = Self::build(store, EventBus::default_bus(), context).await?;

        Ok((
            client,
            WorkspaceImport {
                workspace_id: summary.workspace_id.0,
                workspace_name: summary.workspace_name,
                channels: summary.channels,
                messages: summary.messages,
                agents: summary.agents,
                tasks: summary.tasks,
                runs: summary.runs,
                audit_events: summary.audit_events,
            },
        ))
    }

    /// Create a new local database with an empty workspace named `name`.
    pub async fn create_workspace_database(path: impl AsRef<Path>, name: &str) -> CowResult<Self> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CowError::Config("workspace name is required".to_string()));
        }
        if name.chars().count() > 80 {
            return Err(CowError::Config(
                "workspace names are limited to 80 characters".to_string(),
            ));
        }

        let store = Store::connect(path).await?;
        store.migrate().await?;
        let context = bootstrap_local_named(&store, "Local Owner", name).await?;
        Self::build(store, EventBus::default_bus(), context).await
    }

    /// Execute a sequential crew through the local assignment runtime.
    ///
    /// Local-only: the crew carries in-process LLM and tool handles, so remote
    /// execution needs a server-side API (Phase 2.3) rather than this method.
    pub async fn run_crew(
        &self,
        crew: &mut Crew,
        approval_level: ApprovalLevel,
    ) -> CowResult<HashMap<TaskId, String>> {
        self.run_crew_with_options(
            crew,
            CrewRunOptions {
                approval_level,
                ..CrewRunOptions::default()
            },
        )
        .await
    }

    /// Execute a crew with explicit scheduling options.
    ///
    /// Local-only for the same reason as [`Self::run_crew`].
    pub async fn run_crew_with_options(
        &self,
        crew: &mut Crew,
        options: CrewRunOptions,
    ) -> CowResult<HashMap<TaskId, String>> {
        CrewRunner::new(self.store.clone(), self.bus.clone())
            .run_with_options(
                crew,
                &self.session.workspace.id,
                AssignmentRequester::User(self.session.user.id.0.clone()),
                AssignmentOrigin::User,
                options,
            )
            .await
    }
    /// Ensure every workspace agent has a direct conversation channel.
    ///
    /// Idempotent: existing conversations are reused, missing ones are created
    /// and published on the bus. The desktop shell calls this at startup and
    /// after instantiating agents from a template.
    pub async fn ensure_agent_channels(&self) -> CowResult<Vec<Channel>> {
        let workspace_id = &self.session.workspace.id;
        let agents = self.store.list_agents(workspace_id).await?;
        self.channels
            .ensure_agent_channels(workspace_id, &agents)
            .await?;
        self.channels.list_channels(workspace_id).await
    }

    /// Answer a channel message on behalf of every agent it mentions.
    ///
    /// The caller posts the message first; this method routes it through the
    /// assignment scheduler and writes each agent's answer back to the
    /// timeline. Local-only for the same reason as [`Self::run_crew`].
    pub async fn reply_to_mentions(
        &self,
        channel_id: &ChannelId,
        message: &ChannelMessage,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        model: &str,
        options: ChannelTurnOptions,
    ) -> CowResult<ChannelTurnReport> {
        self.channel_agents
            .reply(channel_id, message, llm, tools, model, &options)
            .await
    }

    /// Compact a conversation context with the caller's fixed model client.
    ///
    /// Local-only for the same reason as [`Self::reply_to_mentions`]: the LLM
    /// handle lives in the desktop runtime, not in the workspace database.
    pub async fn compact_conversation(
        &self,
        channel_id: &ChannelId,
        llm: Arc<dyn LlmProvider>,
        model: &str,
    ) -> CowResult<CompactionOutcome> {
        self.conversations.compact(channel_id, llm, model).await
    }
}

#[async_trait]
impl Client for LocalClient {
    async fn session(&self) -> CowResult<ClientSession> {
        Ok(self.session.clone())
    }

    fn subscribe(&self) -> EventStream {
        let mut broadcast_rx = self.bus.subscribe();
        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(event) => {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "client event stream lagged; events were skipped");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        EventStream::new(rx)
    }

    async fn list_channels(&self) -> CowResult<Vec<Channel>> {
        self.channels
            .list_channels(&self.session.workspace.id)
            .await
    }

    async fn get_channel(&self, channel_id: &ChannelId) -> CowResult<Option<Channel>> {
        self.store.get_channel(channel_id).await
    }

    async fn create_channel(&self, name: &str, topic: &str) -> CowResult<Channel> {
        self.channels
            .create_channel(&self.session.workspace.id, name, topic)
            .await
    }

    async fn rename_channel(
        &self,
        channel_id: &ChannelId,
        name: &str,
        topic: &str,
    ) -> CowResult<Channel> {
        self.channels
            .rename_channel(&self.session.workspace.id, channel_id, name, topic)
            .await
    }

    async fn delete_channel(&self, channel_id: &ChannelId) -> CowResult<Channel> {
        self.channels.delete_channel(channel_id).await
    }

    async fn set_channel_pinned(&self, channel_id: &ChannelId, pinned: bool) -> CowResult<Channel> {
        self.channels.set_channel_pinned(channel_id, pinned).await
    }

    async fn clear_channel_messages(&self, channel_id: &ChannelId) -> CowResult<u64> {
        self.channels.clear_channel_messages(channel_id).await
    }

    async fn post_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<ChannelMessage> {
        self.channels
            .post_message(channel_id, self.user_sender(), content)
            .await
    }

    async fn post_thread_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
        thread_id: &str,
    ) -> CowResult<ChannelMessage> {
        self.channels
            .post_thread_message(
                channel_id,
                self.user_sender(),
                content,
                Some(thread_id.to_string()),
            )
            .await
    }

    async fn list_messages(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ChannelMessage>> {
        self.channels.list_messages(channel_id, limit).await
    }

    async fn create_conversation(
        &self,
        title: Option<&str>,
        agent_id: &AgentId,
        provider_id: Option<String>,
        model: &str,
    ) -> CowResult<Channel> {
        self.conversations
            .create(title, agent_id, provider_id, model)
            .await
    }

    async fn conversation_settings(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<ConversationSettings> {
        self.conversations.settings(channel_id).await
    }

    async fn update_conversation_settings(
        &self,
        channel_id: &ChannelId,
        update: &ConversationSettingsUpdate,
    ) -> CowResult<ConversationSettings> {
        self.conversations.update_settings(channel_id, update).await
    }

    async fn conversation_context(&self, channel_id: &ChannelId) -> CowResult<ConversationContext> {
        self.conversations.context(channel_id).await
    }

    async fn clear_conversation_context(&self, channel_id: &ChannelId) -> CowResult<u64> {
        self.conversations.clear_context(channel_id).await
    }

    async fn create_task(&self, task: &NewTask) -> CowResult<Task> {
        let model = Task::new(task.description.clone(), task.expected_output.clone())
            .assign_to(task.assignee.clone());
        self.tasks
            .create(
                &self.session.workspace.id,
                task.channel_id.as_ref(),
                &model,
                AssignmentRequester::User(self.session.user.id.0.clone()),
                task.approval_level,
            )
            .await?;
        Ok(model)
    }

    async fn list_tasks(&self) -> CowResult<Vec<Task>> {
        self.tasks.list(&self.session.workspace.id).await
    }

    async fn get_task(&self, task_id: &TaskId) -> CowResult<Option<Task>> {
        self.tasks.get(task_id).await
    }

    async fn cancel_task(&self, task_id: &TaskId, reason: &str) -> CowResult<()> {
        self.tasks.cancel(task_id, reason).await
    }

    async fn list_assignments(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>> {
        self.assignments.list_for_task(task_id).await
    }

    async fn list_runs(&self, task_id: &TaskId) -> CowResult<Vec<Run>> {
        self.store.list_runs(task_id).await
    }

    async fn get_run(&self, run_id: &RunId) -> CowResult<Option<Run>> {
        self.store.get_run(run_id).await
    }

    async fn list_tool_calls(&self, run_id: &RunId) -> CowResult<Vec<ToolCallRecord>> {
        self.store.list_tool_calls(run_id).await
    }

    async fn list_assignment_events(
        &self,
        assignment_id: &AssignmentId,
    ) -> CowResult<Vec<AssignmentEvent>> {
        self.assignments.events(assignment_id).await
    }

    async fn list_audit_events(&self, limit: u32) -> CowResult<Vec<AuditEvent>> {
        self.store
            .list_audit_events(&self.session.workspace.id, limit)
            .await
    }

    async fn pending_approvals(&self) -> CowResult<Vec<Approval>> {
        self.approvals.pending_for_workspace().await
    }

    async fn decide_approval(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval> {
        let status = if approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Rejected
        };
        self.approvals
            .decide(approval_id, status, &self.session.user.id.0)
            .await
    }

    async fn list_templates(&self) -> CowResult<Vec<TemplateDocument>> {
        self.templates.list().await
    }

    async fn get_template(&self, id: &str) -> CowResult<Option<TemplateDocument>> {
        self.templates.get(id).await
    }

    async fn save_template(&self, document: &TemplateDocument) -> CowResult<()> {
        self.templates.save(document).await
    }

    async fn delete_template(&self, id: &str) -> CowResult<()> {
        self.templates.delete(id).await
    }

    async fn import_template(&self, json: &str) -> CowResult<TemplateDocument> {
        self.templates.import_json(json).await
    }

    async fn export_template(&self, id: &str) -> CowResult<String> {
        let document = self
            .templates
            .get(id)
            .await?
            .ok_or_else(|| CowError::Config(format!("template '{id}' was not found")))?;
        TemplateService::export_json(&document)
    }

    async fn instantiate_agents(&self, template_id: &str) -> CowResult<Vec<AgentProfile>> {
        self.templates
            .instantiate_agents(&self.session.workspace.id, template_id)
            .await
    }

    async fn list_agents(&self) -> CowResult<Vec<AgentProfile>> {
        self.store.list_agents(&self.session.workspace.id).await
    }

    async fn update_agent(&self, update: &AgentUpdate) -> CowResult<AgentProfile> {
        self.agents
            .update_agent(&self.session.workspace.id, update)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::agent::AgentId;
    use cowbot_core::approval::{Approval, ApprovalLevel, ApprovalStatus};
    use cowbot_core::assignment::{
        AssignmentEventKind, AssignmentMode, AssignmentOrigin, ReplyMode,
    };
    use cowbot_core::run::RunStatus;
    use cowbot_core::task::Task;
    use cowbot_llm::MockLlmProvider;
    use serde_json::json;

    #[tokio::test]
    async fn initialize_expires_orphaned_approvals() {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = bootstrap_local(&store).await.unwrap();
        let task = Task::new("Pending approval", "Output");
        store
            .create_task(&context.workspace.id, None, &task)
            .await
            .unwrap();
        let approval = Approval::new(task.id.clone(), "call-1");
        store.create_approval(&approval).await.unwrap();

        let client = LocalClient::initialize(store.clone(), EventBus::default_bus())
            .await
            .unwrap();

        assert!(store
            .list_pending_approvals(&task.id)
            .await
            .unwrap()
            .is_empty());
        let stored = store.get_approval(&approval.id).await.unwrap().unwrap();
        assert_eq!(stored.status, ApprovalStatus::Expired);
        assert_eq!(client.session().await.unwrap().user.name, "Local Owner");
    }

    #[tokio::test]
    async fn client_lists_and_decides_pending_approvals() {
        let store = Store::connect_in_memory().await.unwrap();
        let client = LocalClient::initialize(store, EventBus::default_bus())
            .await
            .unwrap();
        let workspace_id = client.session().await.unwrap().workspace.id;
        let task = Task::new("Approval task", "Output");
        client
            .store
            .create_task(&workspace_id, None, &task)
            .await
            .unwrap();
        let approval = Approval::new(task.id.clone(), "call-1");
        client.store.create_approval(&approval).await.unwrap();

        let pending = client.pending_approvals().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, approval.id);

        let decided = client.decide_approval(&approval.id, true).await.unwrap();
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(
            decided.decided_by,
            Some(client.session().await.unwrap().user.id.0)
        );
        assert!(client.pending_approvals().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn client_reads_run_and_audit_history() {
        let store = Store::connect_in_memory().await.unwrap();
        let client = LocalClient::initialize(store, EventBus::default_bus())
            .await
            .unwrap();
        let session = client.session().await.unwrap();
        let workspace_id = session.workspace.id;
        let agent = AgentId::new("agent-1");

        let task = Task::new("History task", "Output");
        client
            .store
            .create_task(&workspace_id, None, &task)
            .await
            .unwrap();
        let assignment = Assignment::new_root(
            task.id.clone(),
            AssignmentOrigin::User,
            AssignmentRequester::User(session.user.id.0.clone()),
            agent.clone(),
            AssignmentMode::Work,
            ReplyMode::Resume,
            ApprovalLevel::Ask,
        );
        client.store.create_assignment(&assignment).await.unwrap();

        let run = Run::new(task.id.clone(), agent);
        client.store.create_run(&run).await.unwrap();
        client
            .store
            .update_run_status(&run.id, RunStatus::Completed, None)
            .await
            .unwrap();

        let mut record = ToolCallRecord::new(
            run.id.clone(),
            "read_file",
            json!({ "path": "src/main.rs" }),
        );
        record.result = Some(json!({ "success": true, "output": "ok" }));
        client.store.record_tool_call(&record).await.unwrap();

        client
            .store
            .append_audit_event(&AuditEvent::new(
                workspace_id.clone(),
                "system",
                "test.action",
                "target",
            ))
            .await
            .unwrap();

        let runs = client.list_runs(&task.id).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Completed);
        assert_eq!(client.get_run(&run.id).await.unwrap().unwrap().id, run.id);

        let calls = client.list_tool_calls(&run.id).await.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, record.id);

        let events = client.list_assignment_events(&assignment.id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, AssignmentEventKind::Enqueued);

        let audit = client.list_audit_events(10).await.unwrap();
        assert!(audit.iter().any(|event| event.action == "test.action"));
    }

    #[tokio::test]
    async fn workspace_package_round_trips_into_a_new_database() {
        let client = LocalClient::open_in_memory().await.unwrap();
        client.create_channel("general", "General").await.unwrap();
        let bytes = client.export_workspace().await.unwrap();

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("imported.db");
        let (imported, summary) = LocalClient::create_from_package(&path, &bytes)
            .await
            .unwrap();

        assert_eq!(summary.channels, 1);
        assert!(summary.workspace_name.ends_with("(imported)"));

        let session = imported.session().await.unwrap();
        assert_eq!(session.workspace.id.0, summary.workspace_id);
        assert_eq!(session.workspace.name, summary.workspace_name);

        let channels = imported.list_channels().await.unwrap();
        assert!(channels.iter().any(|channel| channel.name == "general"));
    }

    #[tokio::test]
    async fn update_agent_roundtrips_through_the_client() {
        let client = LocalClient::open_in_memory().await.unwrap();
        let added = client
            .instantiate_agents("builtin-general-assistant")
            .await
            .unwrap();
        let agent = added.first().unwrap();

        let updated = client
            .update_agent(&AgentUpdate {
                id: agent.id.clone(),
                name: "chief".to_string(),
                role: "Chief of Staff".to_string(),
                tools: vec!["read_file".to_string()],
            })
            .await
            .unwrap();
        assert_eq!(updated.name, "chief");

        let channels = client.list_channels().await.unwrap();
        assert!(channels.iter().any(|channel| {
            channel.agent_id.as_ref() == Some(&agent.id) && channel.name == "chief"
        }));
    }

    #[tokio::test]
    async fn channel_management_roundtrips_through_the_client() {
        let client = LocalClient::open_in_memory().await.unwrap();
        let channel = client.create_channel("general", "").await.unwrap();
        assert!(!channel.pinned);

        let renamed = client
            .rename_channel(&channel.id, "ops", "Operations")
            .await
            .unwrap();
        assert_eq!(renamed.name, "ops");
        assert_eq!(renamed.topic, "Operations");

        let pinned = client.set_channel_pinned(&channel.id, true).await.unwrap();
        assert!(pinned.pinned);

        client.post_message(&channel.id, "hello").await.unwrap();
        assert_eq!(client.clear_channel_messages(&channel.id).await.unwrap(), 1);

        client.delete_channel(&channel.id).await.unwrap();
        assert!(client.list_channels().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ensure_agent_channels_provisions_one_conversation_per_agent() {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = bootstrap_local(&store).await.unwrap();
        let agent = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
        store
            .upsert_agent(&context.workspace.id, &agent)
            .await
            .unwrap();

        let client = LocalClient::initialize(store, EventBus::default_bus())
            .await
            .unwrap();

        let channels = client.ensure_agent_channels().await.unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].agent_id.as_ref(), Some(&agent.id));

        let again = client.ensure_agent_channels().await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, channels[0].id);
    }

    #[tokio::test]
    async fn new_workspace_databases_use_the_requested_name() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("team.db");

        let client = LocalClient::create_workspace_database(&path, "Team Workspace")
            .await
            .unwrap();
        let session = client.session().await.unwrap();

        assert_eq!(session.workspace.name, "Team Workspace");
        assert_eq!(session.user.name, "Local Owner");
        assert!(client.list_channels().await.unwrap().is_empty());
        assert!(LocalClient::create_workspace_database(&path, "  ")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn conversation_operations_roundtrip_through_the_client() {
        let client = LocalClient::open_in_memory().await.unwrap();
        let added = client
            .instantiate_agents("builtin-general-assistant")
            .await
            .unwrap();
        let agent = added.first().unwrap();

        let channel = client
            .create_conversation(
                Some("Design review"),
                &agent.id,
                Some("provider-1".to_string()),
                "mock-model",
            )
            .await
            .unwrap();
        assert_eq!(
            channel.kind,
            cowbot_core::conversation::ChannelKind::Conversation
        );
        assert_eq!(
            client.get_channel(&channel.id).await.unwrap().unwrap().kind,
            cowbot_core::conversation::ChannelKind::Conversation
        );

        let settings = client.conversation_settings(&channel.id).await.unwrap();
        assert_eq!(settings.model, "mock-model");
        assert_eq!(settings.provider_id.as_deref(), Some("provider-1"));
        assert!(!settings.auto_compact);

        let updated = client
            .update_conversation_settings(
                &channel.id,
                &cowbot_core::conversation::ConversationSettingsUpdate {
                    auto_compact: Some(true),
                    compact_threshold: Some(70),
                },
            )
            .await
            .unwrap();
        assert!(updated.auto_compact);
        assert_eq!(updated.compact_threshold, 70);

        let message = client
            .post_message(&channel.id, "hello there")
            .await
            .unwrap();
        let report = client
            .reply_to_mentions(
                &channel.id,
                &message,
                Arc::new(MockLlmProvider::echo()),
                ToolRegistry::new(),
                "mock-model",
                ChannelTurnOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.agents, vec![agent.id.clone()]);

        let context = client.conversation_context(&channel.id).await.unwrap();
        assert_eq!(context.agent_id, agent.id);
        assert_eq!(context.projection.active_entries, 2);
        assert!(context.projection.total_tokens() > 0);

        let cleared = client
            .clear_conversation_context(&channel.id)
            .await
            .unwrap();
        assert!(cleared >= 2);
        let context = client.conversation_context(&channel.id).await.unwrap();
        assert_eq!(context.projection.active_entries, 0);
    }
}
