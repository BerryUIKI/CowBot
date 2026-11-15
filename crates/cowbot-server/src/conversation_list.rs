//! Conversation creation, summaries, participants, and turns (2.5).
//!
//! Creating a conversation writes a channel, its settings, and its participant
//! rows, so it requires `can_write`. Reading is open to any member, but only
//! for a conversation that belongs to the workspace in the path: a channel id
//! from another tenant reads as missing.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::channel::{Channel, ChannelId};
use cowbot_core::conversation::{ChannelParticipant, ConversationSettings, ConversationTurn};
use cowbot_core::identity::WorkspaceId;
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::error::{parse_body, ApiError};
use crate::router::AppState;
use crate::workspaces::require_member;

/// Refuse a conversation that belongs to another workspace.
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

#[derive(Debug, Deserialize)]
pub struct CreateConversationBody {
    pub title: Option<String>,
    pub agent_ids: Vec<String>,
    pub provider_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub router_enabled: bool,
    pub default_responder: Option<String>,
}

/// Create a single or group conversation.
pub async fn create_conversation(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<Channel>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow creating conversations",
        ));
    }
    let body: CreateConversationBody = parse_body(raw)?;
    let agent_ids: Vec<AgentId> = body.agent_ids.into_iter().map(AgentId::new).collect();
    let default_responder = body.default_responder.map(AgentId::new);
    let service = state.conversations(&workspace_id);
    let channel = if agent_ids.len() == 1 {
        service
            .create(
                body.title.as_deref(),
                &agent_ids[0],
                body.provider_id.clone(),
                &body.model,
            )
            .await?
    } else {
        service
            .create_group(
                body.title.as_deref(),
                &agent_ids,
                body.provider_id.clone(),
                &body.model,
                default_responder,
            )
            .await?
    };
    if body.router_enabled {
        let update = cowbot_core::conversation::ConversationSettingsUpdate {
            router_enabled: Some(true),
            ..Default::default()
        };
        service.update_settings(&channel.id, &update).await?;
    }
    // `create` validates that every participant belongs to this workspace, so
    // the response is safe to return as-is.
    Ok((StatusCode::CREATED, Json(channel)))
}

/// One line per conversation in the workspace, with its participants.
pub async fn list_summaries(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<ConversationSummaryBody>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channels = state.store.list_channels(&workspace_id).await?;
    let agents = state.store.list_agents(&workspace_id).await?;
    let mut summaries = Vec::new();
    for channel in channels
        .into_iter()
        .filter(|channel| channel.is_conversation())
    {
        let Some(settings) = state.store.get_conversation_settings(&channel.id).await? else {
            continue;
        };
        let participants = state
            .store
            .list_channel_participants(&channel.id)
            .await?
            .into_iter()
            // A participant whose agent was deleted no longer has a profile,
            // so it is dropped rather than rendering a blank member.
            .filter_map(|participant| {
                agents
                    .iter()
                    .find(|agent| agent.id == participant.agent_id)
                    .cloned()
            })
            .collect();
        summaries.push(ConversationSummaryBody {
            channel,
            settings,
            participants,
        });
    }
    Ok(Json(summaries))
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationSummaryBody {
    pub channel: Channel,
    pub settings: ConversationSettings,
    pub participants: Vec<AgentProfile>,
}

pub async fn list_participants(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
) -> Result<Json<Vec<ChannelParticipant>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    Ok(Json(
        state.store.list_channel_participants(&channel_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct TurnsQuery {
    pub limit: Option<u32>,
}

/// The persisted routing turns of a conversation, newest first.
pub async fn list_turns(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
    Query(query): Query<TurnsQuery>,
) -> Result<Json<Vec<ConversationTurn>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let channel_id = ChannelId::new(channel_id);
    ensure_conversation(&state, &workspace_id, &channel_id).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(
        state
            .store
            .list_conversation_turns(&channel_id, limit)
            .await?,
    ))
}
