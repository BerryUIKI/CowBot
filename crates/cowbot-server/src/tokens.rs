//! API tokens: long-lived credentials a script or integration uses instead of
//! a browser session (ADR-0045).
//!
//! A token is bound to one user and one workspace, and carries scopes. It is
//! random 256 bits, stored only as a SHA-256 hash, shown once, and revoked by
//! setting `revoked_at` rather than deleting the row, so an audit can still say
//! which token acted.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use cowbot_core::identity::{ApiToken, Role, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::auth::{hash_token, random_token, AuthenticatedUser};
use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::require_member;

/// The only scope this milestone defines: full access to the token's workspace,
/// bounded by the owner's current role.
pub const SCOPE_WORKSPACE: &str = "workspace";

/// A token as the API lists it. The hash and the secret never appear.
#[derive(Debug, Serialize)]
pub struct TokenSummary {
    pub id: String,
    pub workspace_id: String,
    pub scopes: Vec<String>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A newly created token. `token` is the only time the secret is returned.
#[derive(Debug, Serialize)]
pub struct CreatedToken {
    pub id: String,
    pub workspace_id: String,
    pub scopes: Vec<String>,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
}

/// Create a token for the caller in a workspace they administer.
///
/// Requiring `can_administer` keeps a Member from minting a credential that
/// outlives their session; the token still acts as its owner, so the owner's
/// role is re-checked on every request.
pub async fn create_token(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow creating API tokens",
        ));
    }
    let scopes = body
        .scopes
        .unwrap_or_else(|| vec![SCOPE_WORKSPACE.to_string()]);
    if scopes.is_empty() || scopes.iter().any(|scope| scope != SCOPE_WORKSPACE) {
        return Err(ApiError::bad_request(format!(
            "the only supported scope is '{SCOPE_WORKSPACE}'"
        )));
    }
    let secret = random_token();
    let token = ApiToken::new(
        user.user.id.clone(),
        workspace_id.clone(),
        hash_token(&secret),
        scopes.clone(),
    );
    state.store.create_api_token(&token).await?;
    crate::workspaces::audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "token.created",
        token.id.clone(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            id: token.id,
            workspace_id: workspace_id.0,
            scopes,
            token: secret,
        }),
    ))
}

/// The caller's own tokens in one workspace.
pub async fn list_tokens(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<TokenSummary>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let tokens = state
        .store
        .list_api_tokens(&user.user.id, &workspace_id)
        .await?;
    Ok(Json(
        tokens
            .into_iter()
            .map(|token| TokenSummary {
                id: token.id,
                workspace_id: token.workspace_id.0,
                scopes: token.scopes,
                revoked_at: token.revoked_at,
            })
            .collect(),
    ))
}

/// Revoke one of the caller's tokens.
///
/// An admin may also revoke a token they can see through the workspace, but a
/// token belonging to someone else is only revocable by an Owner — revoking a
/// peer admin's credential is an ownership-level action.
pub async fn revoke_token(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, token_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    let Some(token) = state.store.get_api_token(&token_id).await? else {
        return Err(ApiError::not_found("the token was not found"));
    };
    if token.workspace_id != workspace_id {
        return Err(ApiError::not_found("the token was not found"));
    }
    let owns_it = token.user_id == user.user.id;
    if !owns_it && !matches!(role, Role::Owner) {
        return Err(ApiError::forbidden(
            "only the owner of a token, or a workspace owner, may revoke it",
        ));
    }
    state.store.revoke_api_token(&token_id).await?;
    crate::workspaces::audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "token.revoked",
        token_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
