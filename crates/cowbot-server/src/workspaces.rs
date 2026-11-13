//! Workspace, membership, and invite routes (Phase 2 milestone 2.2).
//!
//! Every route resolves the caller's membership before it reads or writes
//! anything, and a workspace the caller is not a member of answers `404` rather
//! than `403`, so membership cannot be probed from the outside (ADR-0044).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use cowbot_core::audit::{AuditEvent, AuditSource};
use cowbot_core::identity::{Invite, Membership, Role, UserId, Workspace, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::api::UserBody;
use crate::auth::{hash_token, random_token, AuthenticatedUser};
use crate::error::ApiError;
use crate::router::AppState;

/// How long an invitation stays usable.
const INVITE_DAYS: i64 = 7;

/// One workspace with the caller's place in it.
#[derive(Debug, Serialize)]
pub struct WorkspaceBody {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub role: Role,
}

/// A workspace in the sign-in answer, before any route has run.
#[derive(Debug, Serialize)]
pub struct SignInWorkspaceBody {
    pub id: String,
    pub name: String,
    pub role: Role,
}

/// Every workspace the caller belongs to.
pub async fn list_workspaces(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<WorkspaceBody>>, ApiError> {
    let mut result = Vec::new();
    for membership in state.store.list_memberships(&user.user.id).await? {
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
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
}

/// Create a workspace. The creator becomes its owner.
pub async fn create_workspace(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<WorkspaceBody>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "a workspace name must be 1 to 120 characters",
        ));
    }
    let workspace = Workspace::new(name);
    state.store.create_workspace(&workspace).await?;
    state
        .store
        .add_member(&Membership::new(
            user.user.id.clone(),
            workspace.id.clone(),
            Role::Owner,
        ))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(WorkspaceBody {
            id: workspace.id.0.clone(),
            name: workspace.name,
            created_at: workspace.created_at,
            role: Role::Owner,
        }),
    ))
}

/// One member of a workspace.
#[derive(Debug, Serialize)]
pub struct MemberBody {
    pub user: UserBody,
    pub role: Role,
}

pub async fn list_members(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<MemberBody>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let mut result = Vec::new();
    for membership in state.store.list_members(&workspace_id).await? {
        let Some(member) = state.store.get_user(&membership.user_id).await? else {
            continue;
        };
        result.push(MemberBody {
            user: UserBody::from(&member),
            role: membership.role,
        });
    }
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemberRequest {
    pub role: Role,
}

/// Change a member's role.
///
/// The actor must be able to manage the target's *current* role, and to grant
/// the new one. The last owner cannot be demoted, so a workspace always has
/// someone who can administer it.
pub async fn update_member(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, member_id)): Path<(String, String)>,
    Json(body): Json<UpdateMemberRequest>,
) -> Result<Json<MemberBody>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    let member_id = UserId::new(member_id);
    let target = require_member_by_id(&state, &workspace_id, &member_id).await?;

    if !actor.can_manage(target) {
        return Err(ApiError::forbidden(
            "your role does not allow changing this member",
        ));
    }
    if !actor.can_grant(body.role) {
        return Err(ApiError::forbidden(
            "your role does not allow granting that role",
        ));
    }
    if target == Role::Owner && body.role != Role::Owner {
        ensure_not_last_owner(&state, &workspace_id).await?;
    }

    state
        .store
        .set_member_role(&member_id, &workspace_id, body.role)
        .await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "member.role_changed",
        format!(
            "{}:{}->{}",
            member_id.0,
            target.as_str(),
            body.role.as_str()
        ),
    )
    .await?;

    let member = state
        .store
        .get_user(&member_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the member no longer exists"))?;
    Ok(Json(MemberBody {
        user: UserBody::from(&member),
        role: body.role,
    }))
}

/// Remove a member from a workspace.
pub async fn remove_member(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, member_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    let member_id = UserId::new(member_id);
    let target = require_member_by_id(&state, &workspace_id, &member_id).await?;
    if !actor.can_manage(target) {
        return Err(ApiError::forbidden(
            "your role does not allow removing this member",
        ));
    }
    if target == Role::Owner {
        ensure_not_last_owner(&state, &workspace_id).await?;
    }
    state.store.remove_member(&member_id, &workspace_id).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "member.removed",
        member_id.0.clone(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A newly created invitation. The token is returned once and never stored.
#[derive(Debug, Serialize)]
pub struct InviteBody {
    pub id: String,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateInviteRequest {
    pub role: Role,
}

pub async fn create_invite(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<InviteBody>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    if !actor.can_invite() {
        return Err(ApiError::forbidden(
            "your role does not allow inviting members",
        ));
    }
    if !actor.can_grant(body.role) {
        return Err(ApiError::forbidden(
            "your role does not allow inviting at that role",
        ));
    }
    let token = random_token();
    let expires_at = Utc::now() + Duration::days(INVITE_DAYS);
    let invite = Invite::new(
        workspace_id.clone(),
        hash_token(&token),
        body.role,
        expires_at,
    );
    state.store.create_invite(&invite).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "invite.created",
        format!("{}:{}", invite.id, body.role.as_str()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(InviteBody {
            id: invite.id,
            role: body.role,
            expires_at,
            token,
        }),
    ))
}

/// A listed invitation. The token hash stays server-side; only its id, role,
/// and expiry are shown.
#[derive(Debug, Serialize)]
pub struct InviteSummary {
    pub id: String,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
}

pub async fn list_invites(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<InviteSummary>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    if !actor.can_invite() {
        return Err(ApiError::forbidden(
            "your role does not allow reviewing invitations",
        ));
    }
    let invites = state.store.list_invites(&workspace_id).await?;
    Ok(Json(
        invites
            .into_iter()
            .map(|invite| InviteSummary {
                id: invite.id,
                role: invite.role,
                expires_at: invite.expires_at,
            })
            .collect(),
    ))
}

pub async fn revoke_invite(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, invite_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    if !actor.can_invite() {
        return Err(ApiError::forbidden(
            "your role does not allow revoking invitations",
        ));
    }
    // Delete by id only if it belongs to this workspace, so an id from another
    // workspace cannot be revoked.
    let invites = state.store.list_invites(&workspace_id).await?;
    if !invites.iter().any(|invite| invite.id == invite_id) {
        return Err(ApiError::not_found("the invitation does not exist"));
    }
    state.store.delete_invite(&invite_id).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "invite.revoked",
        invite_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    pub token: String,
}

/// Accept an invitation, joining the workspace at its role.
///
/// The invite is single-use: accepting deletes it, so a leaked token cannot be
/// replayed.
pub async fn accept_invite(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(body): Json<AcceptInviteRequest>,
) -> Result<(StatusCode, Json<WorkspaceBody>), ApiError> {
    let invite = state
        .store
        .find_invite_by_token_hash(&hash_token(body.token.trim()))
        .await?
        .ok_or_else(|| ApiError::not_found("the invitation is not valid"))?;
    if invite.expires_at <= Utc::now() {
        // An expired invite is dropped so it stops showing up in the list.
        state.store.delete_invite(&invite.id).await?;
        return Err(ApiError::bad_request("the invitation has expired"));
    }
    let existing = state
        .store
        .get_membership(&user.user.id, &invite.workspace_id)
        .await?;
    if existing.is_some() {
        return Err(ApiError::conflict(
            "you are already a member of this workspace",
        ));
    }
    state
        .store
        .add_member(&Membership::new(
            user.user.id.clone(),
            invite.workspace_id.clone(),
            invite.role,
        ))
        .await?;
    state.store.delete_invite(&invite.id).await?;
    audit(
        &state,
        &invite.workspace_id,
        &user.user.id.0,
        "member.joined",
        invite.role.as_str(),
    )
    .await?;
    let workspace = state
        .store
        .get_workspace(&invite.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the workspace no longer exists"))?;
    Ok((
        StatusCode::CREATED,
        Json(WorkspaceBody {
            id: workspace.id.0.clone(),
            name: workspace.name,
            created_at: workspace.created_at,
            role: invite.role,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct RenameWorkspaceRequest {
    pub name: String,
}

pub async fn rename_workspace(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<RenameWorkspaceRequest>,
) -> Result<Json<WorkspaceBody>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let actor = require_member(&state, &user, &workspace_id).await?;
    if !actor.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow renaming the workspace",
        ));
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "a workspace name must be 1 to 120 characters",
        ));
    }
    state.store.rename_workspace(&workspace_id, name).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "workspace.renamed",
        name,
    )
    .await?;
    let workspace = state
        .store
        .get_workspace(&workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the workspace no longer exists"))?;
    Ok(Json(WorkspaceBody {
        id: workspace.id.0.clone(),
        name: workspace.name,
        created_at: workspace.created_at,
        role: actor,
    }))
}

/// The caller's role in a workspace, or `404`.
///
/// A non-member gets `404` rather than `403`: a `403` would confirm the
/// workspace exists, which an unauthenticated probe could use to map a
/// deployment.
pub(crate) async fn require_member(
    state: &AppState,
    user: &AuthenticatedUser,
    workspace_id: &WorkspaceId,
) -> Result<Role, ApiError> {
    let membership = state
        .store
        .get_membership(&user.user.id, workspace_id)
        .await?;
    membership
        .map(|membership| membership.role)
        .ok_or_else(|| ApiError::not_found("the workspace was not found"))
}

async fn require_member_by_id(
    state: &AppState,
    workspace_id: &WorkspaceId,
    user_id: &UserId,
) -> Result<Role, ApiError> {
    state
        .store
        .get_membership(user_id, workspace_id)
        .await?
        .map(|membership| membership.role)
        .ok_or_else(|| ApiError::not_found("the member was not found"))
}

async fn ensure_not_last_owner(
    state: &AppState,
    workspace_id: &WorkspaceId,
) -> Result<(), ApiError> {
    if state.store.count_owners(workspace_id).await? <= 1 {
        return Err(ApiError::conflict(
            "a workspace must keep at least one owner",
        ));
    }
    Ok(())
}

/// Record an auditable membership change.
///
/// The source is `human` because an authenticated person made the request; a
/// future API-token route would record the token instead.
pub(crate) async fn audit(
    state: &AppState,
    workspace_id: &WorkspaceId,
    actor: &str,
    action: &str,
    target: impl Into<String>,
) -> Result<(), ApiError> {
    let mut event = AuditEvent::new(workspace_id.clone(), actor, action, target);
    event.source = Some(AuditSource::Human.as_str().to_string());
    state.store.append_audit_event(&event).await?;
    Ok(())
}
