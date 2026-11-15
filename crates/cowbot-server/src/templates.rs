//! Template routes (Phase 2 milestone 2.5).
//!
//! Templates are workspace data: a database migration added `workspace_id`, so
//! one workspace's templates are invisible to another. Reading is open to any
//! member; saving, deleting, and instantiating agents require the administer
//! role, because instantiating writes agent rows the whole workspace shares.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use cowbot_core::agent::AgentProfile;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::template::TemplateDocument;
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::error::{parse_body, ApiError};
use crate::router::AppState;
use crate::workspaces::require_member;

/// The workspace's templates, built-ins first by name.
pub async fn list_templates(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<TemplateDocument>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.templates(&workspace_id).list().await?))
}

pub async fn get_template(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, template_id)): Path<(String, String)>,
) -> Result<Json<Option<TemplateDocument>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(
        state.templates(&workspace_id).get(&template_id).await?,
    ))
}

/// Save a template, optionally by importing a `.cowtemplate` document.
pub async fn save_template(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<TemplateDocument>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing templates",
        ));
    }
    let body: SaveTemplateBody = parse_body(raw)?;
    let document = match (body.document, body.json) {
        (Some(document), _) => {
            state.templates(&workspace_id).save(&document).await?;
            document
        }
        (None, Some(json)) => state.templates(&workspace_id).import_json(&json).await?,
        (None, None) => {
            return Err(ApiError::bad_request(
                "send either a template document or a .cowtemplate string",
            ))
        }
    };
    Ok((StatusCode::CREATED, Json(document)))
}

#[derive(Debug, Deserialize)]
pub struct SaveTemplateBody {
    pub document: Option<TemplateDocument>,
    pub json: Option<String>,
}

pub async fn delete_template(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, template_id)): Path<(String, String)>,
) -> Result<Json<()>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing templates",
        ));
    }
    state.templates(&workspace_id).delete(&template_id).await?;
    Ok(Json(()))
}

/// A template as the `.cowtemplate` document a caller can save or share.
pub async fn export_template(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, template_id)): Path<(String, String)>,
) -> Result<Json<String>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let document = state
        .templates(&workspace_id)
        .get(&template_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the template was not found"))?;
    Ok(Json(cowbot_service::TemplateService::export_json(
        &document,
    )?))
}

/// Create the template's agents in this workspace.
pub async fn instantiate_template(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, template_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Vec<AgentProfile>>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow adding agents",
        ));
    }
    let profiles = state
        .templates(&workspace_id)
        .instantiate_agents(&workspace_id, &template_id)
        .await?;
    Ok((StatusCode::CREATED, Json(profiles)))
}
