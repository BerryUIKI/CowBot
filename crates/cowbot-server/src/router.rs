//! The HTTP router and its shared state.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use cowbot_core::identity::WorkspaceId;
use cowbot_store::Store;

use crate::api;
use crate::auth::{AuthService, AuthenticatedUser};
use crate::channels;
use crate::conversation_list;
use crate::conversations;
use crate::error::ApiError;
use crate::integrations;
use crate::memory;
use crate::packages;
use crate::snapshots;
use crate::stream;
use crate::tasks;
use crate::templates;
use crate::tokens;
use crate::workspaces;

/// Shared state every handler sees.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub auth: Arc<AuthService>,
    pub bus: cowbot_bus::EventBus,
    /// Session-scoped approval decisions, shared by every workspace.
    pub session_decisions: Arc<cowbot_service::SessionDecisions>,
    /// Runs channel turns in this process. `None` when the deployment
    /// configured no LLM, in which case a posted message is stored and no
    /// answer arrives — the behaviour every server release had before ADR-0055.
    pub turns: Option<Arc<crate::TurnExecutor>>,
    /// Serves the inbound surface a webhook transport needs (ADR-0062).
    ///
    /// `None` for every deployment that runs a socket or a poll, and for one with
    /// no model provider. The route answers 404 in that case, which is the same
    /// "nothing is running here" a caller gets from an integration this build
    /// cannot start.
    pub webhooks: Option<Arc<crate::webhooks::WebhookReceiver>>,
}

impl AppState {
    pub fn new(store: Store, auth: Arc<AuthService>, bus: cowbot_bus::EventBus) -> Self {
        Self {
            store,
            auth,
            bus,
            session_decisions: cowbot_service::SessionDecisions::new(),
            turns: None,
            webhooks: None,
        }
    }

    /// Attach a turn executor, enabling server-side answers (ADR-0055).
    pub fn with_turns(mut self, turns: Arc<crate::TurnExecutor>) -> Self {
        self.turns = Some(turns);
        self
    }

    /// Attach the webhook receiver, enabling the inbound surface (ADR-0062).
    pub fn with_webhooks(mut self, webhooks: Arc<crate::webhooks::WebhookReceiver>) -> Self {
        self.webhooks = Some(webhooks);
        self
    }

    /// The channel service. It is not workspace-scoped; every call names a
    /// channel whose workspace is checked by the caller.
    pub fn channels(&self) -> cowbot_service::ChannelService {
        cowbot_service::ChannelService::new(self.store.clone(), self.bus.clone())
    }

    /// The permission rule service for one workspace.
    pub fn permissions(&self, workspace_id: &WorkspaceId) -> cowbot_service::PermissionService {
        cowbot_service::PermissionService::new(self.store.clone(), workspace_id.clone())
    }

    /// The task service.
    pub fn tasks(&self) -> cowbot_service::TaskService {
        cowbot_service::TaskService::new(self.store.clone(), self.bus.clone())
    }

    /// The approval service for one workspace.
    pub fn approvals(&self, workspace_id: &WorkspaceId) -> cowbot_service::ApprovalService {
        cowbot_service::ApprovalService::new(
            self.store.clone(),
            self.bus.clone(),
            workspace_id.clone(),
        )
        .with_session_decisions(Some(self.session_decisions.clone()))
    }

    /// The agent service.
    pub fn agents(&self) -> cowbot_service::AgentService {
        cowbot_service::AgentService::new(self.store.clone(), self.bus.clone())
    }

    /// The conversation service for one workspace (milestone 2.5).
    pub fn conversations(&self, workspace_id: &WorkspaceId) -> cowbot_service::ConversationService {
        cowbot_service::ConversationService::new(
            self.store.clone(),
            self.bus.clone(),
            workspace_id.clone(),
        )
    }

    /// The read-only context preview service for one workspace.
    pub fn previews(&self, workspace_id: &WorkspaceId) -> cowbot_service::PreviewService {
        cowbot_service::PreviewService::new(self.store.clone(), workspace_id.clone())
    }

    /// The template service for one workspace (milestone 2.5).
    pub fn templates(&self, workspace_id: &WorkspaceId) -> cowbot_service::TemplateService {
        cowbot_service::TemplateService::new(self.store.clone(), workspace_id.clone())
    }

    /// The prompt-snapshot service for one workspace (milestone 2.5).
    pub fn prompt_snapshots(
        &self,
        workspace_id: &WorkspaceId,
    ) -> cowbot_service::PromptSnapshotService {
        cowbot_service::PromptSnapshotService::new(self.store.clone(), workspace_id.clone())
    }

    /// The memory service for one workspace (milestone 2.5).
    pub fn memory(&self, workspace_id: &WorkspaceId) -> cowbot_service::MemoryService {
        cowbot_service::MemoryService::new(self.store.clone(), workspace_id.clone())
    }
}

/// Build the API router.
///
/// The authenticated routes take their identity from [`AuthenticatedUser`],
/// which extracts and verifies the bearer token itself, so a handler cannot
/// accidentally run without proving who is asking.
pub fn app(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(api::health))
        .route("/api/v1/auth/sign-in", post(api::sign_in))
        // A webhook endpoint authenticates by verifying the body, never by a
        // session, so it sits on the public router (ADR-0062). The integration
        // id in the path chooses whose secret is used.
        .route(
            "/api/v1/integrations/{integration_id}/webhook",
            post(crate::webhooks::receive),
        );

    let authenticated = Router::new()
        .route("/api/v1/me", get(api::me))
        .route("/api/v1/auth/sign-out", post(api::sign_out))
        .route("/api/v1/auth/password", post(api::change_password))
        // Workspaces (ADR-0044)
        .route(
            "/api/v1/workspaces",
            get(workspaces::list_workspaces).post(workspaces::create_workspace),
        )
        .route("/api/v1/invites/accept", post(workspaces::accept_invite))
        .route(
            "/api/v1/workspaces/{workspace_id}",
            patch(workspaces::rename_workspace),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            get(workspaces::list_members),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            patch(workspaces::update_member).delete(workspaces::remove_member),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/invites",
            get(workspaces::list_invites).post(workspaces::create_invite),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/invites/{invite_id}",
            delete(workspaces::revoke_invite),
        )
        // Channels and messages (ADR-0045)
        .route(
            "/api/v1/workspaces/{workspace_id}/channels",
            get(channels::list_channels).post(channels::create_channel),
        )
        .route(
            "/api/v1/channels/{channel_id}",
            get(channels::get_channel)
                .patch(channels::update_channel)
                .delete(channels::delete_channel),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages",
            get(channels::list_messages).post(channels::post_message),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages/clear",
            post(channels::clear_messages),
        )
        // Tasks, runs, and approvals (ADR-0045)
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks",
            get(tasks::list_tasks).post(tasks::create_task),
        )
        .route("/api/v1/tasks/{task_id}", get(tasks::get_task))
        .route("/api/v1/tasks/{task_id}/cancel", post(tasks::cancel_task))
        .route("/api/v1/tasks/{task_id}/archive", post(tasks::archive_task))
        .route(
            "/api/v1/tasks/{task_id}/approve-plan",
            post(tasks::approve_task_plan),
        )
        .route(
            "/api/v1/tasks/{task_id}/assignments",
            get(tasks::list_assignments),
        )
        .route("/api/v1/tasks/{task_id}/runs", get(tasks::list_runs))
        .route("/api/v1/runs/{run_id}", get(tasks::get_run))
        .route(
            "/api/v1/runs/{run_id}/tool-calls",
            get(tasks::list_tool_calls),
        )
        .route(
            "/api/v1/assignments/{assignment_id}/events",
            get(tasks::list_assignment_events),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/audit",
            get(tasks::list_audit),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/approvals",
            get(tasks::pending_approvals),
        )
        .route(
            "/api/v1/approvals/{approval_id}/decide",
            post(tasks::decide_approval),
        )
        // Permission rules, policy, and agents (ADR-0045)
        .route(
            "/api/v1/workspaces/{workspace_id}/permission-rules",
            get(tasks::list_permission_rules).post(tasks::create_permission_rule),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/permission-rules/{rule_id}",
            patch(tasks::set_permission_rule_enabled).delete(tasks::delete_permission_rule),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/policy",
            get(tasks::workspace_policy).patch(tasks::set_bypass_critical),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/agents",
            get(tasks::list_agents).patch(tasks::update_agent),
        )
        // API tokens and the live event stream (ADR-0045)
        .route(
            "/api/v1/workspaces/{workspace_id}/tokens",
            get(tokens::list_tokens).post(tokens::create_token),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tokens/{token_id}",
            delete(tokens::revoke_token),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/stream",
            get(stream::stream),
        )
        // Memory and notes (milestone 2.5)
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/notes",
            get(memory::get_notes).put(memory::save_workspace_note),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/notes/{channel_id}",
            put(memory::save_conversation_note).delete(memory::delete_conversation_note),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/settings",
            get(memory::get_memory_settings).patch(memory::update_memory_settings),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/entries",
            get(memory::list_memory_entries).post(memory::add_memory_entry),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/entries/{entry_id}",
            patch(memory::update_memory_entry).delete(memory::delete_memory_entry),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/entries/{entry_id}/{action}",
            post(memory::set_memory_status),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/memory/recall",
            get(memory::preview_memory_recall),
        )
        // Conversation creation and listing (milestone 2.5)
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations",
            get(conversation_list::list_summaries).post(conversation_list::create_conversation),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}/participants",
            get(conversation_list::list_participants),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}/turns",
            get(conversation_list::list_turns),
        )
        // Conversation settings, context, and previews (milestone 2.5)
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}",
            get(conversations::get_settings).patch(conversations::update_settings),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}/context",
            get(conversations::get_context).delete(conversations::clear_context),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}/context/{agent_id}",
            axum::routing::delete(conversations::clear_participant_context),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/conversations/{channel_id}/preview",
            get(conversations::conversation_preview),
        )
        .route(
            "/api/v1/runs/{run_id}/preview",
            get(conversations::task_run_preview),
        )
        // Prompt snapshots (milestone 2.5)
        .route(
            "/api/v1/workspaces/{workspace_id}/snapshots",
            get(snapshots::list_recent_snapshots),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/snapshots/settings",
            get(snapshots::get_settings).patch(snapshots::update_settings),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/snapshots/purge",
            post(snapshots::purge),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/snapshots/{llm_call_id}",
            get(snapshots::get_snapshot),
        )
        .route(
            "/api/v1/runs/{run_id}/snapshots",
            get(snapshots::list_run_snapshots),
        )
        // Templates (milestone 2.5)
        .route(
            "/api/v1/workspaces/{workspace_id}/templates",
            get(templates::list_templates).post(templates::save_template),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/templates/{template_id}",
            get(templates::get_template).delete(templates::delete_template),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/templates/{template_id}/export",
            get(templates::export_template),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/templates/{template_id}/instantiate",
            post(templates::instantiate_template),
        )
        // IM integrations (ADR-0056)
        .route(
            "/api/v1/workspaces/{workspace_id}/integrations",
            get(integrations::list_integrations).post(integrations::create_integration),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/integrations/{integration_id}",
            delete(integrations::revoke_integration),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/integrations/{integration_id}/bindings",
            get(integrations::list_bindings).post(integrations::bind_conversation),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/integrations/{integration_id}/bindings/{external_id}",
            delete(integrations::unbind_conversation),
        );

    // Package transfer is workspace-level, not inside one workspace, so it
    // sits beside the workspace routes rather than under a path id.
    let authenticated = authenticated
        .route("/api/v1/packages/import", post(packages::import_workspace))
        .route(
            "/api/v1/workspaces/{workspace_id}/packages/export",
            get(packages::export_workspace),
        );

    public
        .merge(authenticated)
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Log each request's outcome at debug level, so a deployment has a trail
/// without a verbose default.
async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if response.status().is_server_error() {
        tracing::warn!(%method, %path, status = %response.status(), "Request failed");
    } else {
        tracing::debug!(%method, %path, status = %response.status(), "Request served");
    }
    response
}

/// Resolve the bearer token to the signed-in user.
///
/// Implemented as an extractor rather than a middleware so a handler's
/// signature says whether it needs an identity.
impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = api::bearer_token(&parts.headers) else {
            return Err(ApiError::unauthorized(
                "a bearer token is required; sign in first",
            ));
        };
        match state.auth.authenticate(token).await {
            Ok(Some(user)) => Ok(user),
            Ok(None) => Err(ApiError::unauthorized(
                "the session is expired or unknown; sign in again",
            )),
            Err(error) => Err(error.into()),
        }
    }
}

/// Serve the router on an existing listener until the future is dropped.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let router = app(state);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
}

/// Serve until `shutdown` resolves, for an embedded or test deployment.
pub async fn serve_with_shutdown<F>(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: F,
) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown)
        .await
}
