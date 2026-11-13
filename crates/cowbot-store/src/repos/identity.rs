use chrono::{DateTime, Utc};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::{
    ApiToken, Invite, Membership, Role, Session, User, UserId, Workspace, WorkspaceId,
};

use crate::error::{db, parse_datetime, parse_json, parse_optional_datetime};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    name: String,
    email: String,
    password_hash: String,
    created_at: String,
}

impl UserRow {
    fn into_domain(self) -> CowResult<User> {
        Ok(User {
            id: UserId(self.id),
            name: self.name,
            email: self.email,
            password_hash: self.password_hash,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: String,
    name: String,
    created_at: String,
}

impl WorkspaceRow {
    fn into_domain(self) -> CowResult<Workspace> {
        Ok(Workspace {
            id: WorkspaceId(self.id),
            name: self.name,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct MembershipRow {
    user_id: String,
    workspace_id: String,
    role: String,
}

impl MembershipRow {
    fn into_domain(self) -> CowResult<Membership> {
        Ok(Membership {
            user_id: UserId(self.user_id),
            workspace_id: WorkspaceId(self.workspace_id),
            role: role_from_str(&self.role)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    user_id: String,
    token_hash: String,
    expires_at: String,
    created_at: String,
}

impl SessionRow {
    fn into_domain(self) -> CowResult<Session> {
        Ok(Session {
            id: self.id,
            user_id: UserId(self.user_id),
            token_hash: self.token_hash,
            expires_at: parse_datetime(&self.expires_at)?,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ApiTokenRow {
    id: String,
    user_id: String,
    workspace_id: String,
    token_hash: String,
    scopes: String,
    revoked_at: Option<String>,
}

impl ApiTokenRow {
    fn into_domain(self) -> CowResult<ApiToken> {
        Ok(ApiToken {
            id: self.id,
            user_id: UserId(self.user_id),
            workspace_id: WorkspaceId(self.workspace_id),
            token_hash: self.token_hash,
            scopes: parse_json(&self.scopes, "api_tokens.scopes")?,
            revoked_at: parse_optional_datetime(self.revoked_at.as_deref())?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: String,
    workspace_id: String,
    token_hash: String,
    role: String,
    expires_at: String,
}

impl InviteRow {
    fn into_domain(self) -> CowResult<Invite> {
        Ok(Invite {
            id: self.id,
            workspace_id: WorkspaceId(self.workspace_id),
            token_hash: self.token_hash,
            role: role_from_str(&self.role)?,
            expires_at: parse_datetime(&self.expires_at)?,
        })
    }
}

fn role_to_str(role: Role) -> &'static str {
    role.as_str()
}

fn role_from_str(value: &str) -> CowResult<Role> {
    match value {
        "owner" => Ok(Role::Owner),
        "admin" => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        other => Err(CowError::Storage(format!("unknown role '{other}'"))),
    }
}

impl Store {
    pub async fn create_user(&self, user: &User) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO users (id, name, email, password_hash, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&user.id.0)
        .bind(&user.name)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(user.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_user(&self, id: &UserId) -> CowResult<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, name, email, password_hash, created_at FROM users WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(UserRow::into_domain).transpose()
    }

    pub async fn get_user_by_email(&self, email: &str) -> CowResult<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, name, email, password_hash, created_at FROM users WHERE email = ?",
        )
        .bind(email)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(UserRow::into_domain).transpose()
    }

    pub async fn update_user(&self, id: &UserId, name: &str, email: &str) -> CowResult<()> {
        sqlx::query("UPDATE users SET name = ?, email = ? WHERE id = ?")
            .bind(name)
            .bind(email)
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn list_users(&self) -> CowResult<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, name, email, password_hash, created_at
             FROM users ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(UserRow::into_domain).collect()
    }

    pub async fn create_workspace(&self, workspace: &Workspace) -> CowResult<()> {
        sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?, ?, ?)")
            .bind(&workspace.id.0)
            .bind(&workspace.name)
            .bind(workspace.created_at.to_rfc3339())
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_workspace(&self, id: &WorkspaceId) -> CowResult<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, name, created_at FROM workspaces WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(WorkspaceRow::into_domain).transpose()
    }

    pub async fn list_workspaces(&self) -> CowResult<Vec<Workspace>> {
        let rows = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, name, created_at FROM workspaces ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(WorkspaceRow::into_domain).collect()
    }

    pub async fn add_member(&self, membership: &Membership) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO memberships (user_id, workspace_id, role) VALUES (?, ?, ?)
             ON CONFLICT (user_id, workspace_id) DO UPDATE SET role = excluded.role",
        )
        .bind(&membership.user_id.0)
        .bind(&membership.workspace_id.0)
        .bind(role_to_str(membership.role))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Set a member's role, or add the membership if it does not exist.
    pub async fn set_member_role(
        &self,
        user_id: &UserId,
        workspace_id: &WorkspaceId,
        role: Role,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO memberships (user_id, workspace_id, role) VALUES (?, ?, ?)
             ON CONFLICT(user_id, workspace_id) DO UPDATE SET role = excluded.role",
        )
        .bind(&user_id.0)
        .bind(&workspace_id.0)
        .bind(role_to_str(role))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Remove a membership. The user and their sessions are untouched.
    pub async fn remove_member(
        &self,
        user_id: &UserId,
        workspace_id: &WorkspaceId,
    ) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM memberships WHERE user_id = ? AND workspace_id = ?")
            .bind(&user_id.0)
            .bind(&workspace_id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Count the owners of a workspace, so the last one cannot be removed.
    pub async fn count_owners(&self, workspace_id: &WorkspaceId) -> CowResult<u64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memberships WHERE workspace_id = ? AND role = 'owner'",
        )
        .bind(&workspace_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(count.max(0) as u64)
    }

    /// Every invite for a workspace, newest first, for an admin to review.
    pub async fn list_invites(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Invite>> {
        let rows = sqlx::query_as::<_, InviteRow>(
            "SELECT id, workspace_id, token_hash, role, expires_at
             FROM invites WHERE workspace_id = ? ORDER BY expires_at DESC",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(InviteRow::into_domain).collect()
    }

    /// Rename a workspace.
    pub async fn rename_workspace(&self, id: &WorkspaceId, name: &str) -> CowResult<()> {
        sqlx::query("UPDATE workspaces SET name = ? WHERE id = ?")
            .bind(name)
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_membership(
        &self,
        user_id: &UserId,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Option<Membership>> {
        let row = sqlx::query_as::<_, MembershipRow>(
            "SELECT user_id, workspace_id, role FROM memberships
             WHERE user_id = ? AND workspace_id = ?",
        )
        .bind(&user_id.0)
        .bind(&workspace_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(MembershipRow::into_domain).transpose()
    }

    pub async fn list_members(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Membership>> {
        let rows = sqlx::query_as::<_, MembershipRow>(
            "SELECT user_id, workspace_id, role FROM memberships
             WHERE workspace_id = ? ORDER BY user_id",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(MembershipRow::into_domain).collect()
    }

    /// Every workspace and role a user has.
    pub async fn list_memberships(&self, user_id: &UserId) -> CowResult<Vec<Membership>> {
        let rows = sqlx::query_as::<_, MembershipRow>(
            "SELECT user_id, workspace_id, role FROM memberships
             WHERE user_id = ? ORDER BY workspace_id",
        )
        .bind(&user_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(MembershipRow::into_domain).collect()
    }

    /// Replace a user's password hash.
    ///
    /// Session invalidation is the caller's job: the server drops every other
    /// session when the password changes (ADR-0043).
    pub async fn update_password(&self, id: &UserId, password_hash: &str) -> CowResult<()> {
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Delete every session of a user.
    pub async fn delete_user_sessions(&self, user_id: &UserId) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(&user_id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn create_session(&self, session: &Session) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&session.id)
        .bind(&session.user_id.0)
        .bind(&session.token_hash)
        .bind(session.expires_at.to_rfc3339())
        .bind(session.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_session_by_token_hash(&self, token_hash: &str) -> CowResult<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, user_id, token_hash, expires_at, created_at
             FROM sessions WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(SessionRow::into_domain).transpose()
    }

    pub async fn delete_session(&self, id: &str) -> CowResult<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn create_api_token(&self, token: &ApiToken) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO api_tokens (id, user_id, workspace_id, token_hash, scopes, revoked_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&token.id)
        .bind(&token.user_id.0)
        .bind(&token.workspace_id.0)
        .bind(&token.token_hash)
        .bind(serde_json::to_string(&token.scopes)?)
        .bind(token.revoked_at.map(|value| value.to_rfc3339()))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn revoke_api_token(&self, id: &str) -> CowResult<()> {
        sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_api_token_by_hash(&self, token_hash: &str) -> CowResult<Option<ApiToken>> {
        let row = sqlx::query_as::<_, ApiTokenRow>(
            "SELECT id, user_id, workspace_id, token_hash, scopes, revoked_at
             FROM api_tokens WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ApiTokenRow::into_domain).transpose()
    }

    /// One API token by id.
    pub async fn get_api_token(&self, id: &str) -> CowResult<Option<ApiToken>> {
        let row = sqlx::query_as::<_, ApiTokenRow>(
            "SELECT id, user_id, workspace_id, token_hash, scopes, revoked_at
             FROM api_tokens WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ApiTokenRow::into_domain).transpose()
    }

    /// A user's tokens in one workspace, newest first.
    pub async fn list_api_tokens(
        &self,
        user_id: &UserId,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<ApiToken>> {
        let rows = sqlx::query_as::<_, ApiTokenRow>(
            "SELECT id, user_id, workspace_id, token_hash, scopes, revoked_at
             FROM api_tokens WHERE user_id = ? AND workspace_id = ? ORDER BY id",
        )
        .bind(&user_id.0)
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ApiTokenRow::into_domain).collect()
    }

    pub async fn create_invite(&self, invite: &Invite) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO invites (id, workspace_id, token_hash, role, expires_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&invite.id)
        .bind(&invite.workspace_id.0)
        .bind(&invite.token_hash)
        .bind(role_to_str(invite.role))
        .bind(invite.expires_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn find_invite_by_token_hash(&self, token_hash: &str) -> CowResult<Option<Invite>> {
        let row = sqlx::query_as::<_, InviteRow>(
            "SELECT id, workspace_id, token_hash, role, expires_at
             FROM invites WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(InviteRow::into_domain).transpose()
    }

    /// Move an invitation's expiry, for tests and for an admin shortening one.
    pub async fn set_invite_expiry(&self, id: &str, expires_at: DateTime<Utc>) -> CowResult<()> {
        sqlx::query("UPDATE invites SET expires_at = ? WHERE id = ?")
            .bind(expires_at.to_rfc3339())
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn delete_invite(&self, id: &str) -> CowResult<()> {
        sqlx::query("DELETE FROM invites WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }
}
