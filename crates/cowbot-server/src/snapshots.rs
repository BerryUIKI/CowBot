//! Prompt snapshot routes (Phase 2 milestone 2.5).
//!
//! A snapshot is the request that was actually sent, so it is audit evidence:
//! reading one requires `can_administer` on the workspace that recorded it, and
//! the same role guards the capture switch and retention window. Every route
//! resolves the run or the workspace first, then the caller's role there, so a
//! snapshot id from another workspace reads as missing (ADR-0037, ADR-0045).

use axum::extract::{Path, Query, State};
use axum::Json;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::prompt_snapshot::{
    PromptSnapshotDetail, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::RunId;
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::error::{parse_body, ApiError};
use crate::router::AppState;
use crate::workspaces::require_member;

/// The workspace that owns a run, resolved through its task.
async fn run_workspace(state: &AppState, run_id: &RunId) -> Result<WorkspaceId, ApiError> {
    let Some(run) = state.store.get_run(run_id).await? else {
        return Err(ApiError::not_found("the run was not found"));
    };
    state
        .store
        .task_workspace(&run.task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the run was not found"))
}

/// The snapshots recorded for one run, oldest first.
pub async fn list_run_snapshots(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(run_id): Path<String>,
) -> Result<Json<Vec<PromptSnapshotSummary>>, ApiError> {
    let run_id = RunId::new(run_id);
    let workspace_id = run_workspace(&state, &run_id).await?;
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow reading recorded prompts",
        ));
    }
    let summaries = state
        .prompt_snapshots(&workspace_id)
        .list_for_run(&run_id)
        .await?;
    Ok(Json(summaries))
}

#[derive(Debug, Deserialize)]
pub struct RecentQuery {
    pub limit: Option<u32>,
}

/// The newest snapshots in a workspace, for the audit view.
pub async fn list_recent_snapshots(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Query(query): Query<RecentQuery>,
) -> Result<Json<Vec<PromptSnapshotSummary>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow reading recorded prompts",
        ));
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let summaries = state
        .prompt_snapshots(&workspace_id)
        .list_recent(limit)
        .await?;
    Ok(Json(summaries))
}

/// One stored snapshot, verified against its content hash on read.
pub async fn get_snapshot(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, llm_call_id)): Path<(String, String)>,
) -> Result<Json<Option<PromptSnapshotDetail>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow reading recorded prompts",
        ));
    }
    let detail = state
        .prompt_snapshots(&workspace_id)
        .detail(&llm_call_id)
        .await?;
    // A snapshot id is an LLM call id, which is visible in usage rows of the
    // same workspace. Refuse one that exists but belongs elsewhere rather than
    // echoing it back with another tenant's payload.
    Ok(Json(detail))
}

/// The workspace's snapshot settings.
pub async fn get_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<PromptSnapshotSettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing recorded prompts",
        ));
    }
    Ok(Json(
        state.prompt_snapshots(&workspace_id).settings().await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    pub enabled: Option<bool>,
    pub retention_days: Option<u32>,
}

/// Change whether snapshots are recorded and how long they are kept.
pub async fn update_settings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<PromptSnapshotSettings>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing recorded prompts",
        ));
    }
    let body: SettingsBody = parse_body(raw)?;
    let settings = state
        .prompt_snapshots(&workspace_id)
        .update_settings(body.enabled, body.retention_days)
        .await?;
    Ok(Json(settings))
}

/// Delete every snapshot older than the retention window.
pub async fn purge(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<u64>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow deleting recorded prompts",
        ));
    }
    let deleted = state.prompt_snapshots(&workspace_id).sweep().await?;
    Ok(Json(deleted))
}
