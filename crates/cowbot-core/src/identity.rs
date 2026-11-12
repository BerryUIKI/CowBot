use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Unique identifier for a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Role of a user within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

/// A platform user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: String,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn new(
        name: impl Into<String>,
        email: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: UserId::random(),
            name: name.into(),
            email: email.into(),
            password_hash: password_hash.into(),
            created_at: Utc::now(),
        }
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A workspace that owns channels, agents, tasks, and templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: WorkspaceId::random(),
            name: name.into(),
            created_at: Utc::now(),
        }
    }
}

/// Membership of a user in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub role: Role,
}

impl Membership {
    pub fn new(user_id: UserId, workspace_id: WorkspaceId, role: Role) -> Self {
        Self {
            user_id,
            workspace_id,
            role,
        }
    }
}

/// An authenticated server session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl Session {
    pub fn new(user_id: UserId, token_hash: impl Into<String>, expires_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            user_id,
            token_hash: token_hash.into(),
            expires_at,
            created_at: Utc::now(),
        }
    }
}

/// A long-lived API token bound to a user and workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiToken {
    pub fn new(
        user_id: UserId,
        workspace_id: WorkspaceId,
        token_hash: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            user_id,
            workspace_id,
            token_hash: token_hash.into(),
            scopes,
            revoked_at: None,
        }
    }
}

/// A single-use workspace invitation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub token_hash: String,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
}

impl Invite {
    pub fn new(
        workspace_id: WorkspaceId,
        token_hash: impl Into<String>,
        role: Role,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id,
            token_hash: token_hash.into(),
            role,
            expires_at,
        }
    }
}
