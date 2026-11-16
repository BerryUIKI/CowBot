//! Conversation routes: settings, the context meter, and the two read-only
//! context previews (Phase 2 milestone 2.5).
//!
//! A conversation is a channel with a `kind` of `conversation`, so each route
//! resolves the channel, then the workspace that owns it, then the caller's
//! role — the same order the channel routes use (ADR-0045). The previews only
//! read; they never append a context entry or a recall statistic (ADR-0036).

use axum::extract::{Path, Query, State};
use axum::Json;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::conversation::{ConversationContext, ConversationSettings};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::run::RunId;
use cowbot_core::tool::ToolDefinition;
use cowbot_service::{ConversationPreview, TaskRunPreview};
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::error::{parse_body, ApiError};
use crate::router::AppState;
use crate::workspaces::require_member;

/// Refuse to act on a channel that another workspace owns.
async fn ensure_conversation(
    state: &AppState,
    workspace_id: &WorkspaceId,
    channel_id: &ChannelId,
) -> Result<(), ApiError> {
    if !state
        .store
        .channel_in_workspace(channel_id, workspace_id)
        .await?
    {
        return Err(ApiError::not_found("the conversation was not found"));
    }
    Ok(())
}

pub async fn get_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
) -> Result<Json<ConversationSettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    Ok(Json(
        state
            .conversations(&workspace_id)
            .settings(&channel_id)
            .await?,
    ))
}

/// The editable subset of a conversation's settings.
///
/// Kept as plain options so a caller patches only what it changed; the service
/// applies each field it receives and validates the threshold it clamps.
#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    pub auto_compact: Option<bool>,
    pub compact_threshold: Option<u32>,
    pub router_enabled: Option<bool>,
    pub routing_mode: Option<cowbot_core::conversation::RoutingMode>,
    pub muted_participants: Option<Vec<String>>,
    pub reply_cleanup: Option<bool>,
    pub default_responder: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub model_follows_default: Option<bool>,
}

pub async fn update_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<ConversationSettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow changing this conversation",
        ));
    }
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    let body: SettingsBody = parse_body(raw)?;
    let update = cowbot_core::conversation::ConversationSettingsUpdate {
        auto_compact: body.auto_compact,
        compact_threshold: body.compact_threshold,
        router_enabled: body.router_enabled,
        routing_mode: body.routing_mode,
        muted_participants: body
            .muted_participants
            .map(|ids| ids.into_iter().map(AgentId::new).collect()),
        reply_cleanup: body.reply_cleanup,
        default_responder: body.default_responder.map(AgentId::new),
        provider_id: body.provider_id,
        model: body.model,
        model_follows_default: body.model_follows_default,
    };
    let settings = state
        .conversations(&workspace_id)
        .update_settings(&channel_id, &update)
        .await?;
    Ok(Json(settings))
}

/// The context meter for every participant of a conversation.
pub async fn get_context(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
) -> Result<Json<ConversationContext>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    Ok(Json(
        state
            .conversations(&workspace_id)
            .context(&channel_id)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    pub agent_id: String,
    pub draft: Option<String>,
    pub platform_context: Option<String>,
    #[serde(default)]
    pub plan_mode: bool,
    /// Tool schemas as a JSON array, because the server has no tool registry:
    /// the caller sends the schemas it would attach to the request.
    #[serde(default)]
    pub tools: Option<String>,
}

/// Decode the tool schemas a preview request names.
///
/// A malformed value is the caller's mistake, so it is a `400` rather than a
/// silently empty tool list that would make the preview quietly wrong.
fn decode_tools(raw: Option<&str>) -> Result<Vec<ToolDefinition>, ApiError> {
    match raw {
        None => Ok(Vec::new()),
        Some(raw) => serde_json::from_str(raw)
            .map_err(|_| ApiError::bad_request("tools must be a JSON array of schemas")),
    }
}

/// Rebuild the request one participant's next turn would send.
///
/// The tool schemas come from the query because the server does not own a tool
/// registry — the caller sends the schemas it would attach, exactly as the
/// local preview takes them from its own registry.
pub async fn conversation_preview(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
    Query(query): Query<PreviewQuery>,
) -> Result<Json<ConversationPreview>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    let tools = decode_tools(query.tools.as_deref())?;
    let preview = state
        .previews(&workspace_id)
        .conversation(
            &channel_id,
            &AgentId::new(query.agent_id),
            query.draft.as_deref(),
            query.platform_context.as_deref(),
            query.plan_mode,
            &tools,
        )
        .await?;
    Ok(Json(preview))
}

#[derive(Debug, Deserialize)]
pub struct RunPreviewQuery {
    pub model: String,
    #[serde(default)]
    pub tools: Option<String>,
}

/// Rebuild the request a task run started with.
pub async fn task_run_preview(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(run_id): Path<String>,
    Query(query): Query<RunPreviewQuery>,
) -> Result<Json<Option<TaskRunPreview>>, ApiError> {
    let run_id = RunId::new(run_id);
    // Resolve the run to its task, then the task to its workspace: a run id
    // from another workspace must read as missing, not as forbidden.
    let Some(run) = state.store.get_run(&run_id).await? else {
        return Err(ApiError::not_found("the run was not found"));
    };
    let workspace_id = state
        .store
        .task_workspace(&run.task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the run was not found"))?;
    require_member(&state, &user, &workspace_id).await?;
    let tools = decode_tools(query.tools.as_deref())?;
    let preview = state
        .previews(&workspace_id)
        .task_run(&run_id, &query.model, &tools)
        .await?;
    Ok(Json(preview))
}

/// Clear every participant's context branch in a conversation.
pub async fn clear_context(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
) -> Result<Json<u64>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow clearing this conversation",
        ));
    }
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    let cleared = state
        .conversations(&workspace_id)
        .clear_context(&channel_id)
        .await?;
    Ok(Json(cleared))
}

/// Clear one participant's context branch.
pub async fn clear_participant_context(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id, agent_id)): Path<(String, String, String)>,
) -> Result<Json<u64>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow clearing this conversation",
        ));
    }
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    let cleared = state
        .conversations(&workspace_id)
        .clear_participant_context(&channel_id, &AgentId::new(agent_id))
        .await?;
    Ok(Json(cleared))
}
