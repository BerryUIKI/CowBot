//! The HTTP router and its shared state.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use cowbot_core::identity::WorkspaceId;
use cowbot_store::Store;

use crate::api;
use crate::auth::{AuthService, AuthenticatedUser};
use crate::channels;
use crate::error::ApiError;
use crate::stream;
use crate::tasks;
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
}

impl AppState {
    pub fn new(store: Store, auth: Arc<AuthService>, bus: cowbot_bus::EventBus) -> Self {
        Self {
            store,
            auth,
            bus,
            session_decisions: cowbot_service::SessionDecisions::new(),
        }
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
}

/// Build the API router.
///
/// The authenticated routes take their identity from [`AuthenticatedUser`],
/// which extracts and verifies the bearer token itself, so a handler cannot
/// accidentally run without proving who is asking.
pub fn app(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(api::health))
        .route("/api/v1/auth/sign-in", post(api::sign_in));

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

/// A status assistant so a caller can tell "not found" from "unauthorized"
/// without parsing.
pub fn status_code(error: &ApiError) -> StatusCode {
    error.status
}
