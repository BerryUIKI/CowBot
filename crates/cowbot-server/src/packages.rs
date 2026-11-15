//! Package import and export routes (Phase 2 milestone 2.5).
//!
//! A `.cowpkg` package carries a whole workspace. Importing one creates a new
//! workspace with the importing user as its Owner, so it is a workspace-level
//! action rather than a change inside an existing one: any signed-in user may
//! do it, exactly as `POST /workspaces` lets any signed-in user start a team
//! (ADR-0044). Export needs `can_administer` on the workspace it reads.
//!
//! The archive is verified by `cowbot-package` before a single row is written,
//! and a package whose ids collide with this database is refused by the
//! importer itself, so a failed upload leaves no partial workspace.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use cowbot_core::identity::WorkspaceId;
use cowbot_package::MAX_PACKAGE_BYTES;
use serde::Serialize;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::require_member;

/// What an import created, mirroring the package crate's summary.
#[derive(Debug, Serialize)]
pub struct ImportBody {
    pub workspace_id: String,
    pub workspace_name: String,
    pub channels: usize,
    pub messages: usize,
    pub agents: usize,
    pub tasks: usize,
    pub runs: usize,
    pub audit_events: usize,
    pub conversations: usize,
    pub context_entries: usize,
    pub permission_rules: usize,
    pub memory_notes: usize,
    pub memory_entries: usize,
}

/// Import a package as a new workspace the caller owns.
pub async fn import_workspace(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    body: Bytes,
) -> Result<(StatusCode, Json<ImportBody>), ApiError> {
    if body.is_empty() {
        return Err(ApiError::bad_request("the request body is empty"));
    }
    if body.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "package_too_large",
            format!(
                "the package is larger than {} MB",
                MAX_PACKAGE_BYTES / (1024 * 1024)
            ),
        ));
    }
    let summary = cowbot_package::import_workspace(&state.store, &body)
        .await
        .map_err(package_error)?;
    // The importer creates a placeholder owner so a package always lands with
    // an accountable member. On a server the importing user is that owner: a
    // second Owner would leave an account nobody can sign in as holding the
    // workspace, so replace the placeholder rather than sitting beside it.
    let owners = state.store.list_members(&summary.workspace_id).await?;
    for membership in owners
        .iter()
        .filter(|member| member.user_id != user.user.id)
    {
        state
            .store
            .remove_member(&membership.user_id, &summary.workspace_id)
            .await?;
    }
    state
        .store
        .add_member(&cowbot_core::identity::Membership::new(
            user.user.id.clone(),
            summary.workspace_id.clone(),
            cowbot_core::identity::Role::Owner,
        ))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ImportBody {
            workspace_id: summary.workspace_id.0,
            workspace_name: summary.workspace_name,
            channels: summary.channels,
            messages: summary.messages,
            agents: summary.agents,
            tasks: summary.tasks,
            runs: summary.runs,
            audit_events: summary.audit_events,
            conversations: summary.conversations,
            context_entries: summary.context_entries,
            permission_rules: summary.permission_rules,
            memory_notes: summary.memory_notes,
            memory_entries: summary.memory_entries,
        }),
    ))
}

/// Download a workspace as a `.cowpkg` package.
pub async fn export_workspace(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow exporting this workspace",
        ));
    }
    let bytes = cowbot_package::export_workspace(&state.store, &workspace_id)
        .await
        .map_err(package_error)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.cowbot.package".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"workspace.cowpkg\"".to_string(),
            ),
        ],
        bytes,
    ))
}

/// Map a package failure onto the API's error model.
///
/// A package problem is the caller's to fix — a corrupt archive, a mismatched
/// checksum, a version this build cannot read, a colliding id — so it is a
/// `400` with the package's own message, which names the offending path or id.
/// A store failure inside the import keeps the generic `500`.
fn package_error(error: cowbot_package::PackageError) -> ApiError {
    use cowbot_package::PackageError;
    match error {
        PackageError::Store(inner) => ApiError::from(inner),
        PackageError::Io(_) => ApiError::internal("the server could not read the package"),
        other => ApiError::bad_request(other.to_string()),
    }
}
