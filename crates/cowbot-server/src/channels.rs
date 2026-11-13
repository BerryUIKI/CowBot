//! Channel and message routes.
//!
//! A channel belongs to exactly one workspace, so every route resolves the
//! channel first and then checks the caller's membership in *that* workspace —
//! not the workspace in the path, which a caller could forge (ADR-0045).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::identity::Role;
use cowbot_core::identity::WorkspaceId;
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::require_member;

/// A channel as the API returns it, with the caller's role in its workspace so
/// a client can decide what to offer without a second request.
#[derive(Debug, Serialize)]
pub struct ChannelBody {
    pub id: String,
    pub workspace_id: Option<String>,
    pub name: String,
    pub topic: String,
    pub kind: String,
    pub agent_id: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub role: Role,
}

fn channel_body(channel: Channel, role: Role) -> ChannelBody {
    ChannelBody {
        id: channel.id.0,
        // The channel row does not carry its workspace; the list route fills it
        // from the path, and the single-channel route leaves it out.
        workspace_id: None,
        name: channel.name,
        topic: channel.topic,
        kind: channel.kind.as_str().to_string(),
        agent_id: channel.agent_id.map(|agent| agent.0),
        pinned: channel.pinned,
        created_at: channel.created_at,
        role,
    }
}

/// Channels of one workspace.
pub async fn list_channels(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<ChannelBody>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    let channels = state.store.list_channels(&workspace_id).await?;
    Ok(Json(
        channels
            .into_iter()
            .map(|channel| {
                let mut body = channel_body(channel, role);
                body.workspace_id = Some(workspace_id.0.clone());
                body
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    #[serde(default)]
    pub topic: String,
}

/// Create a channel. Requires write access.
pub async fn create_channel(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateChannelRequest>,
) -> Result<(StatusCode, Json<ChannelBody>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow creating channels",
        ));
    }
    let channel = state
        .channels()
        .create_channel(&workspace_id, &body.name, &body.topic)
        .await?;
    let mut body = channel_body(channel, role);
    body.workspace_id = Some(workspace_id.0.clone());
    Ok((StatusCode::CREATED, Json(body)))
}

/// One channel, with its role.
pub async fn get_channel(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
) -> Result<Json<ChannelBody>, ApiError> {
    let (channel, role) = channel_and_role(&state, &user, &ChannelId::new(channel_id)).await?;
    Ok(Json(channel_body(channel, role)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub name: Option<String>,
    pub topic: Option<String>,
    pub pinned: Option<bool>,
}

/// Rename, retopic, or pin a channel. Requires write access.
pub async fn update_channel(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
    Json(body): Json<UpdateChannelRequest>,
) -> Result<Json<ChannelBody>, ApiError> {
    let channel_id = ChannelId::new(channel_id);
    let (channel, role) = channel_and_role(&state, &user, &channel_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow changing channels",
        ));
    }
    let Some(workspace_id) = state.store.channel_workspace(&channel_id).await? else {
        return Err(ApiError::not_found("the channel was not found"));
    };
    let mut updated = channel;
    let name = body.name.as_deref().unwrap_or(&updated.name);
    let topic = body.topic.as_deref().unwrap_or(&updated.topic);
    if body.name.is_some() || body.topic.is_some() {
        updated = state
            .channels()
            .rename_channel(&workspace_id, &channel_id, name, topic)
            .await?;
    }
    if let Some(pinned) = body.pinned {
        updated = state
            .channels()
            .set_channel_pinned(&channel_id, pinned)
            .await?;
    }
    Ok(Json(channel_body(updated, role)))
}

/// Delete a channel. Requires administration, because it removes history.
pub async fn delete_channel(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let channel_id = ChannelId::new(channel_id);
    let (_, role) = channel_and_role(&state, &user, &channel_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow deleting channels",
        ));
    }
    state.channels().delete_channel(&channel_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A message as the API returns it.
#[derive(Debug, Serialize)]
pub struct MessageBody {
    pub id: String,
    pub channel_id: String,
    pub sender: Sender,
    pub content: String,
    pub thread_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ChannelMessage> for MessageBody {
    fn from(message: ChannelMessage) -> Self {
        Self {
            id: message.id,
            channel_id: message.channel_id.0,
            sender: message.sender,
            content: message.content,
            thread_id: message.thread_id,
            created_at: message.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListMessagesQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub thread_id: Option<String>,
}

fn default_limit() -> u32 {
    50
}

/// Messages of a channel, oldest first, capped at `limit`.
pub async fn list_messages(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
    Query(query): Query<ListMessagesQuery>,
) -> Result<Json<Vec<MessageBody>>, ApiError> {
    let channel_id = ChannelId::new(channel_id);
    channel_and_role(&state, &user, &channel_id).await?;
    let limit = query.limit.clamp(1, 500);
    let messages = state.store.list_messages(&channel_id, limit).await?;
    // Thread filtering is a view concern: the stored list already carries each
    // message's thread id, so the server narrows it here.
    let messages = match &query.thread_id {
        Some(thread_id) => messages
            .into_iter()
            .filter(|message| message.thread_id.as_deref() == Some(thread_id.as_str()))
            .collect::<Vec<_>>(),
        None => messages,
    };
    Ok(Json(messages.into_iter().map(MessageBody::from).collect()))
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    pub thread_id: Option<String>,
}

/// Post a message. Requires write access.
pub async fn post_message(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
    Json(body): Json<PostMessageRequest>,
) -> Result<(StatusCode, Json<MessageBody>), ApiError> {
    let channel_id = ChannelId::new(channel_id);
    let (_, role) = channel_and_role(&state, &user, &channel_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow posting in this channel",
        ));
    }
    // The sender is the authenticated user, never a value from the body, so a
    // caller cannot post as another participant.
    let sender = Sender::User(user.user.id.0.clone());
    let message = state
        .channels()
        .post_thread_message(&channel_id, sender, &body.content, body.thread_id.clone())
        .await?;
    Ok((StatusCode::CREATED, Json(MessageBody::from(message))))
}

/// Clear a channel's messages. Requires administration.
pub async fn clear_messages(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(channel_id): Path<String>,
) -> Result<Json<ClearBody>, ApiError> {
    let channel_id = ChannelId::new(channel_id);
    let (_, role) = channel_and_role(&state, &user, &channel_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow clearing a channel",
        ));
    }
    let cleared = state.channels().clear_channel_messages(&channel_id).await?;
    Ok(Json(ClearBody { cleared }))
}

#[derive(Debug, Serialize)]
pub struct ClearBody {
    pub cleared: u64,
}

/// The channel and the caller's role in the workspace that owns it.
///
/// A channel in a workspace the caller is not a member of is reported as `404`,
/// which is indistinguishable from an unknown channel.
pub(crate) async fn channel_and_role(
    state: &AppState,
    user: &AuthenticatedUser,
    channel_id: &ChannelId,
) -> Result<(Channel, Role), ApiError> {
    let Some(channel) = state.store.get_channel(channel_id).await? else {
        return Err(ApiError::not_found("the channel was not found"));
    };
    let Some(workspace_id) = state.store.channel_workspace(channel_id).await? else {
        return Err(ApiError::not_found("the channel was not found"));
    };
    let role = require_member(state, user, &workspace_id).await?;
    Ok((channel, role))
}
