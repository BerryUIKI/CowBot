//! Running a channel turn inside the server (ADR-0055).
//!
//! Before this module the server could store a message that mentions an agent
//! and never answer it: the reply ran in the desktop process, and the server had
//! no LLM at all. An IM adapter needs the opposite — a bot must answer when
//! nobody's desktop is open — so this is the prerequisite for the integration
//! work.
//!
//! The turn logic itself is unchanged. [`ChannelAgentService::reply`] has always
//! taken its provider, tool registry, and model as arguments; the CLI constructs
//! all three itself. What was missing was a place to run it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cowbot_bus::EventBus;
use cowbot_core::channel::{ChannelId, ChannelMessage};
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;
use cowbot_llm::LlmProvider;
use cowbot_service::{ChannelAgentService, ChannelTurnOptions, SessionDecisions};
use cowbot_store::Store;
use cowbot_tools::{
    CodeExecutionTool, ReadFileTool, SubprocessSandbox, ToolRegistry, WriteFileTool,
};
use tracing::{info, warn};

use crate::config::ServerLlmConfig;

/// Answers the channel turns that mention agents.
///
/// One executor serves the whole process. It is deliberately not built per
/// request: [`ChannelAgentService`] owns the open-turn registry and the
/// compaction branch locks, so a per-request instance would give every request
/// its own copy and silently break both `cancel_turn` and the "one compaction at
/// a time per branch" guarantee.
///
/// The service is workspace-scoped, so the executor keeps one per workspace
/// behind a `Mutex`. The lock is held only long enough to look the entry up.
pub struct TurnExecutor {
    store: Store,
    bus: EventBus,
    llm: Arc<dyn LlmProvider>,
    model: String,
    /// A tool set a server can offer safely: the sandbox, and file access
    /// rooted at one directory the server owns.
    tools: ToolRegistry,
    session_decisions: Arc<SessionDecisions>,
    services: Mutex<HashMap<WorkspaceId, Arc<ChannelAgentService>>>,
}

impl std::fmt::Debug for TurnExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnExecutor")
            .field("model", &self.model)
            .field("tools", &self.tools.definitions().len())
            .finish_non_exhaustive()
    }
}

impl TurnExecutor {
    /// Build an executor for a deployment that configured an LLM.
    ///
    /// `workspace_dir` roots the file tools and the sandbox working directory.
    /// A server has no user project folder, so a turn that writes a file must be
    /// confined to a directory the server owns rather than the host filesystem.
    pub async fn new(
        store: Store,
        bus: EventBus,
        config: &ServerLlmConfig,
        workspace_dir: &Path,
    ) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(workspace_dir)?;
        let root = workspace_dir.to_path_buf();

        let sandbox = Arc::new(SubprocessSandbox::new(Some(root.clone())));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(CodeExecutionTool::new(sandbox)));
        tools.register(Arc::new(ReadFileTool::new(Some(root.clone()))));
        tools.register(Arc::new(WriteFileTool::new(Some(root))));

        Ok(Arc::new(Self {
            store,
            bus,
            llm: config.provider(),
            model: config.model.clone(),
            tools,
            session_decisions: SessionDecisions::new(),
            services: Mutex::new(HashMap::new()),
        }))
    }

    /// The model every turn is answered with.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The tool names this executor offers, for tests and for the startup log.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        names.sort();
        names
    }

    /// The service for one workspace, created on first use.
    ///
    /// Shared so the open-turn registry and the branch locks are the same
    /// objects for every request.
    fn service(&self, workspace_id: &WorkspaceId) -> Arc<ChannelAgentService> {
        let mut services = self
            .services
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        services
            .entry(workspace_id.clone())
            .or_insert_with(|| {
                Arc::new(
                    ChannelAgentService::new(
                        self.store.clone(),
                        self.bus.clone(),
                        workspace_id.clone(),
                    )
                    .with_session_decisions(Some(self.session_decisions.clone())),
                )
            })
            .clone()
    }

    /// Answer `message` on behalf of the agents it mentions.
    ///
    /// Runs in the caller's task; [`Self::spawn_reply`] is the version a request
    /// handler uses so it can answer the client immediately.
    pub async fn reply(&self, channel_id: &ChannelId, message: &ChannelMessage) -> CowResult<()> {
        let Some(workspace_id) = self.store.channel_workspace(channel_id).await? else {
            // The channel was deleted between the post and the turn. Nothing to
            // answer, and no one to tell.
            return Ok(());
        };
        let service = self.service(&workspace_id);
        let report = service
            .reply(
                channel_id,
                message,
                self.llm.clone(),
                self.tools.clone(),
                &self.model,
                &ChannelTurnOptions::default(),
            )
            .await?;

        if report.is_empty() {
            return Ok(());
        }
        info!(
            channel = %channel_id.0,
            addressed = report.agents.len(),
            replied = report.replied.len(),
            failed = report.failed.len(),
            skipped = report.skipped.len(),
            "Answered a server-side channel turn"
        );
        for agent in &report.failed {
            warn!(channel = %channel_id.0, agent = %agent.0, "An agent failed in a channel turn");
        }
        for agent in &report.skipped {
            warn!(channel = %channel_id.0, agent = %agent.0, "An agent was skipped: its context is full");
        }
        Ok(())
    }

    /// Start [`Self::reply`] in the background and return at once.
    ///
    /// A turn can take far longer than a client will wait, so the request path
    /// stores the message, starts this, and returns. The answer reaches every
    /// client through the existing `MessagePosted` event on the workspace
    /// socket, which is what makes this a change the desktop and the IM adapter
    /// both see without a new protocol.
    pub fn spawn_reply(self: &Arc<Self>, channel_id: ChannelId, message: ChannelMessage) {
        let executor = self.clone();
        tokio::spawn(async move {
            if let Err(error) = executor.reply(&channel_id, &message).await {
                // A turn that cannot start is reported, never swallowed: the
                // client is waiting for an answer that will not come.
                warn!(
                    channel = %channel_id.0,
                    error = %error,
                    "A server-side channel turn failed to run"
                );
                if let Err(notice_error) = executor
                    .post_failure_notice(&channel_id, &error.to_string())
                    .await
                {
                    warn!(error = %notice_error, "Failed to post a turn failure notice");
                }
            }
        });
    }

    /// Tell the channel a turn could not run, so the user is not left guessing.
    async fn post_failure_notice(&self, channel_id: &ChannelId, error: &str) -> CowResult<()> {
        // `post_system_notice` is crate-private to the service; a system sender
        // through the public posting method is the same message.
        cowbot_service::ChannelService::new(self.store.clone(), self.bus.clone())
            .post_message(
                channel_id,
                cowbot_core::channel::Sender::System,
                format!("This turn could not run: {error}"),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::agent::AgentProfile;
    use cowbot_service::bootstrap_local;

    async fn store_with_workspace() -> (Store, WorkspaceId) {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = bootstrap_local(&store).await.unwrap();
        (store, context.workspace.id)
    }

    async fn executor(store: Store) -> Arc<TurnExecutor> {
        let directory = tempfile::tempdir().unwrap();
        let config = ServerLlmConfig {
            protocol: crate::config::ServerLlmProtocol::Mock,
            base_url: None,
            api_key: None,
            model: "mock".to_string(),
        };
        TurnExecutor::new(store, EventBus::default_bus(), &config, directory.path())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn one_service_per_workspace_is_reused() {
        // The cache is what keeps cancel_turn and the compaction locks working:
        // a per-request service would give each request its own registry.
        let (store, workspace_id) = store_with_workspace().await;
        let executor = executor(store).await;

        let first = executor.service(&workspace_id);
        let second = executor.service(&workspace_id);
        assert!(Arc::ptr_eq(&first, &second));

        let other = executor.service(&WorkspaceId::new("another-workspace"));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[tokio::test]
    async fn the_tool_profile_is_the_narrow_one() {
        // A server turn must not reach the desktop's workspace tools: they
        // assume a local project folder the server does not have.
        let (store, _) = store_with_workspace().await;
        let executor = executor(store).await;
        assert_eq!(
            executor.tool_names(),
            vec![
                "execute_code".to_string(),
                "read_file".to_string(),
                "write_file".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn a_channel_that_no_longer_exists_is_not_an_error() {
        // The channel can be deleted between the post and the turn; that is a
        // no-op rather than a failure to report.
        let (store, _) = store_with_workspace().await;
        let executor = executor(store).await;
        let message = ChannelMessage::new(
            ChannelId::new("missing-channel"),
            cowbot_core::channel::Sender::User("someone".to_string()),
            "@Anyone hello",
        );
        executor
            .reply(&ChannelId::new("missing-channel"), &message)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_server_turn_answers_a_mention() {
        // The whole point of S0: a message that mentions an agent produces an
        // agent message, with no desktop in the loop. The mock provider echoes,
        // so this runs without a key.
        let (store, workspace_id) = store_with_workspace().await;
        let agent = AgentProfile::new("Helper", "assistant", "answer questions", "a test agent");
        store.upsert_agent(&workspace_id, &agent).await.unwrap();
        let channel = cowbot_service::ChannelService::new(store.clone(), EventBus::default_bus())
            .create_channel(&workspace_id, "general", "test channel")
            .await
            .unwrap();
        let executor = executor(store.clone()).await;

        let message = cowbot_service::ChannelService::new(store.clone(), EventBus::default_bus())
            .post_thread_message(
                &channel.id,
                cowbot_core::channel::Sender::User("someone".to_string()),
                &format!("@{} what is the plan?", agent.name),
                None,
            )
            .await
            .unwrap();

        executor.reply(&channel.id, &message).await.unwrap();

        let messages = store.list_messages(&channel.id, 50).await.unwrap();
        let answered = messages
            .iter()
            .any(|stored| matches!(stored.sender, cowbot_core::channel::Sender::Agent(_)));
        assert!(answered, "the agent's answer should be in the timeline");
    }
}
