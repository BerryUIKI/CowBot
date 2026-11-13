//! HTTP handlers.
//!
//! This milestone exposes the account surface only: health, sign-in, sign-out,
//! the current identity, the workspaces it belongs to, and a password change.
//! Channel, task, and approval routes follow in 2.2 alongside membership
//! enforcement, so nothing here can act on a workspace it does not prove access
//! to.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use cowbot_core::identity::{Role, UserId};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthenticatedUser, IssuedSession};
use crate::error::ApiError;
use crate::router::AppState;

/// Whether the server is up, and the API version a client should speak.
#[derive(Debug, Serialize)]
pub struct HealthBody {
    pub status: &'static str,
    pub api_version: u32,
}

pub async fn health() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        api_version: API_VERSION,
    })
}

/// The API version this server implements.
///
/// A client checks it before speaking; the desktop's remote mode will pin a
/// compatible range rather than assume the newest.
pub const API_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct SignInRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct SignInBody {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub user: UserBody,
    pub workspaces: Vec<WorkspaceBody>,
}

/// A user as the API returns it. The password hash never crosses the wire.
#[derive(Debug, Serialize)]
pub struct UserBody {
    pub id: String,
    pub name: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

impl From<&cowbot_core::identity::User> for UserBody {
    fn from(user: &cowbot_core::identity::User) -> Self {
        Self {
            id: user.id.0.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            created_at: user.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorkspaceBody {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub role: Role,
}

/// Sign in and receive a bearer token.
pub async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SignInRequest>,
) -> Result<Json<SignInBody>, ApiError> {
    if body.email.trim().is_empty() || body.password.is_empty() {
        return Err(ApiError::bad_request("email and password are required"));
    }
    // The reverse proxy in front of a deployment is expected to set
    // `X-Forwarded-For`; without one the whole server shares a single bucket.
    let source = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();
    let issued = match state
        .auth
        .sign_in(body.email.trim(), &body.password, &source)
        .await
    {
        Ok(issued) => issued,
        Err(cowbot_core::error::CowError::Config(message))
            if message.contains("too many sign-in attempts") =>
        {
            return Err(ApiError::too_many_requests(message));
        }
        Err(_) => {
            return Err(ApiError::unauthorized(
                "the email or password is not correct",
            ))
        }
    };
    let authenticated = state
        .auth
        .authenticate(&issued.token)
        .await?
        .ok_or_else(|| ApiError::internal("the new session could not be read back"))?;
    let workspaces = workspaces_for(&state, &authenticated).await?;
    Ok(Json(sign_in_body(
        issued,
        UserBody::from(&authenticated.user),
        workspaces,
    )))
}

fn sign_in_body(
    issued: IssuedSession,
    user: UserBody,
    workspaces: Vec<WorkspaceBody>,
) -> SignInBody {
    SignInBody {
        token: issued.token,
        expires_at: issued.expires_at,
        user,
        workspaces,
    }
}

async fn workspaces_for(
    state: &AppState,
    user: &AuthenticatedUser,
) -> Result<Vec<WorkspaceBody>, ApiError> {
    let mut result = Vec::new();
    for membership in state.auth.memberships(&user.user.id).await? {
        let Some(workspace) = state.store.get_workspace(&membership.workspace_id).await? else {
            continue;
        };
        result.push(WorkspaceBody {
            id: workspace.id.0.clone(),
            name: workspace.name,
            created_at: workspace.created_at,
            role: membership.role,
        });
    }
    Ok(result)
}

/// The signed-in user, refreshed from the store.
#[derive(Debug, Serialize)]
pub struct MeBody {
    pub session_expires_at: DateTime<Utc>,
    pub user: UserBody,
    pub workspaces: Vec<WorkspaceBody>,
}

pub async fn me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<MeBody>, ApiError> {
    let workspaces = workspaces_for(&state, &user).await?;
    Ok(Json(MeBody {
        session_expires_at: user.expires_at,
        user: UserBody::from(&user.user),
        workspaces,
    }))
}

/// End the current session.
pub async fn sign_out(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    state.auth.sign_out(&user.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Change the password and re-issue this session.
///
/// Every other session for the user is dropped, so a token obtained before the
/// change stops working.
pub async fn change_password(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<SignInBody>, ApiError> {
    state
        .auth
        .change_password(&user.user, &body.current_password, &body.new_password)
        .await?;
    let issued = state.auth.issue(&UserId(user.user.id.0.clone())).await?;
    let workspaces = workspaces_for(&state, &user).await?;
    Ok(Json(sign_in_body(
        issued,
        UserBody::from(&user.user),
        workspaces,
    )))
}

/// Extract a bearer token from the request headers.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}
