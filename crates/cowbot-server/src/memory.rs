//! Memory and notes routes (Phase 2 milestone 2.5).
//!
//! Notes, memory entries, settings, and the recall preview are workspace data,
//! so every route resolves the caller's role in the workspace it names. A
//! conversation note additionally requires the channel to belong to that
//! workspace; `MemoryService` checks the scope of an entry against the
//! workspace it was constructed with, which is the same rule the local
//! runtime applies.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{MemoryEntry, MemoryKind, MemorySettings, MemoryStatus, Note};
use cowbot_service::{
    MemoryFilter, MemorySave, MemorySettingsUpdate, MemoryUpdate, NewMemory, NoteSave,
    RecalledMemory,
};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use crate::error::{parse_body, ApiError};
use crate::router::AppState;
use crate::workspaces::require_member;

/// The notes a workspace and one conversation carry.
#[derive(Debug, Serialize)]
pub struct MemoryNotesBody {
    pub workspace: Option<Note>,
    pub conversation: Option<Note>,
}

#[derive(Debug, Deserialize)]
pub struct NotesQuery {
    pub channel_id: Option<String>,
}

/// Workspace note and, when a conversation is named, its note.
pub async fn get_notes(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Query(query): Query<NotesQuery>,
) -> Result<Json<MemoryNotesBody>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    let channel_id = match &query.channel_id {
        Some(id) => {
            let channel_id = ChannelId::new(id.clone());
            ensure_channel(&state, &workspace_id, &channel_id).await?;
            Some(channel_id)
        }
        None => None,
    };
    // A conversation note is a write-adjacent edit surface; members read it
    // with the rest of the workspace note, so the read role is enough.
    let _ = role;
    let (workspace, conversation) = state
        .memory(&workspace_id)
        .notes(channel_id.as_ref())
        .await?;
    Ok(Json(MemoryNotesBody {
        workspace,
        conversation,
    }))
}

/// Save the workspace note.
pub async fn save_workspace_note(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<NoteSave>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing the workspace note",
        ));
    }
    let body: NoteBody = parse_body(raw)?;
    let save = state
        .memory(&workspace_id)
        .save_workspace_note(&body.content)
        .await?;
    Ok(Json(save))
}

#[derive(Debug, Deserialize)]
pub struct NoteBody {
    pub content: String,
}

/// Save or delete one conversation's note.
pub async fn save_conversation_note(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<NoteSave>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing conversation notes",
        ));
    }
    let channel_id = ChannelId::new(channel_id);
    ensure_channel(&state, &workspace_id, &channel_id).await?;
    let body: NoteBody = parse_body(raw)?;
    let save = state
        .memory(&workspace_id)
        .save_conversation_note(&channel_id, &body.content)
        .await?;
    Ok(Json(save))
}

pub async fn delete_conversation_note(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, channel_id)): Path<(String, String)>,
) -> Result<Json<u64>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing conversation notes",
        ));
    }
    let channel_id = ChannelId::new(channel_id);
    ensure_channel(&state, &workspace_id, &channel_id).await?;
    let deleted = state
        .memory(&workspace_id)
        .delete_conversation_note(&channel_id)
        .await?;
    Ok(Json(deleted))
}

/// The workspace's memory settings.
pub async fn get_memory_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<MemorySettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.memory(&workspace_id).settings().await?))
}

#[derive(Debug, Deserialize)]
pub struct MemorySettingsBody {
    pub extraction_enabled: Option<bool>,
    pub recall_enabled: Option<bool>,
    pub notes_budget_tokens: Option<u32>,
    pub recall_budget_tokens: Option<u32>,
}

/// Change the workspace's memory settings.
pub async fn update_memory_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<MemorySettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing memory settings",
        ));
    }
    let body: MemorySettingsBody = parse_body(raw)?;
    let settings = state
        .memory(&workspace_id)
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: body.extraction_enabled,
            recall_enabled: body.recall_enabled,
            notes_budget_tokens: body.notes_budget_tokens,
            recall_budget_tokens: body.recall_budget_tokens,
        })
        .await?;
    Ok(Json(settings))
}

#[derive(Debug, Deserialize)]
pub struct EntriesQuery {
    pub status: Option<String>,
    pub agent_id: Option<String>,
    pub channel_id: Option<String>,
}

/// Memory entries of one workspace, newest first.
pub async fn list_memory_entries(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<Vec<MemoryEntry>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let status = match &query.status {
        Some(value) => Some(
            value
                .parse::<MemoryStatus>()
                .map_err(|_| ApiError::bad_request("unknown memory status"))?,
        ),
        None => None,
    };
    let filter = MemoryFilter {
        status,
        agent_id: query.agent_id.clone().map(AgentId::new),
        channel_id: query.channel_id.clone().map(ChannelId::new),
    };
    let entries = state.memory(&workspace_id).list_entries(&filter).await?;
    Ok(Json(entries))
}

#[derive(Debug, Deserialize)]
pub struct AddEntryBody {
    pub content: String,
    pub kind: Option<String>,
    pub agent_id: Option<String>,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub pinned: bool,
}

/// Add an entry attributed to the signed-in user.
pub async fn add_memory_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<MemorySave>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow adding memory",
        ));
    }
    let body: AddEntryBody = parse_body(raw)?;
    let kind = match &body.kind {
        Some(value) => value
            .parse::<MemoryKind>()
            .map_err(|_| ApiError::bad_request("unknown memory kind"))?,
        None => MemoryKind::Fact,
    };
    let save = state
        .memory(&workspace_id)
        .add_entry(NewMemory {
            content: body.content,
            kind,
            agent_id: body.agent_id.map(AgentId::new),
            channel_id: body.channel_id.map(ChannelId::new),
            pinned: body.pinned,
            source: cowbot_core::memory::MemorySource::User,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(save)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateEntryBody {
    pub content: Option<String>,
    pub kind: Option<String>,
    pub pinned: Option<bool>,
}

/// Edit an entry's content, kind, or pinning.
pub async fn update_memory_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, entry_id)): Path<(String, String)>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<MemorySave>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing memory",
        ));
    }
    let body: UpdateEntryBody = parse_body(raw)?;
    let kind = match &body.kind {
        Some(value) => Some(
            value
                .parse::<MemoryKind>()
                .map_err(|_| ApiError::bad_request("unknown memory kind"))?,
        ),
        None => None,
    };
    let save = state
        .memory(&workspace_id)
        .update_entry(
            &entry_id,
            &MemoryUpdate {
                content: body.content,
                kind,
                pinned: body.pinned,
            },
        )
        .await?;
    Ok(Json(save))
}

/// Move an entry to one of the terminal statuses the review queue uses.
pub async fn set_memory_status(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, entry_id, action)): Path<(String, String, String)>,
) -> Result<Json<MemoryEntry>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing memory",
        ));
    }
    let service = state.memory(&workspace_id);
    let entry = match action.as_str() {
        "approve" => service.approve_entry(&entry_id).await?,
        "dismiss" => service.dismiss_entry(&entry_id).await?,
        "archive" => service.archive_entry(&entry_id).await?,
        other => {
            return Err(ApiError::not_found(format!(
                "there is no memory action '{other}'"
            )))
        }
    };
    Ok(Json(entry))
}

pub async fn delete_memory_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, entry_id)): Path<(String, String)>,
) -> Result<Json<u64>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow editing memory",
        ));
    }
    let deleted = state.memory(&workspace_id).delete_entry(&entry_id).await?;
    Ok(Json(deleted))
}

#[derive(Debug, Deserialize)]
pub struct RecallQuery {
    pub agent_id: String,
    pub query: String,
    pub channel_id: Option<String>,
}

/// What the next turn would inject, without touching recall statistics.
pub async fn preview_memory_recall(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Query(query): Query<RecallQuery>,
) -> Result<Json<Option<RecalledMemory>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let recalled = state
        .memory(&workspace_id)
        .preview_recall(
            query
                .channel_id
                .as_ref()
                .map(|id| ChannelId::new(id.clone()))
                .as_ref(),
            &AgentId::new(query.agent_id.clone()),
            &query.query,
        )
        .await?;
    Ok(Json(recalled))
}

/// Refuse a channel id that belongs to another workspace, so a conversation
/// note cannot be read or written across the tenant boundary.
async fn ensure_channel(
    state: &AppState,
    workspace_id: &WorkspaceId,
    channel_id: &ChannelId,
) -> Result<(), ApiError> {
    if !state
        .store
        .channel_in_workspace(channel_id, workspace_id)
        .await?
    {
        return Err(ApiError::not_found("the channel was not found"));
    }
    Ok(())
}
