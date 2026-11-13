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

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Ordering rank, so a caller can ask "is this role at least as strong".
    ///
    /// Owner outranks Admin outranks Member. A comparison is never the whole
    /// permission check — see [`Role::can_manage`], which also weighs the target
    /// role — but it makes the common case readable.
    pub fn rank(self) -> u8 {
        match self {
            Role::Owner => 3,
            Role::Admin => 2,
            Role::Member => 1,
        }
    }

    /// Whether this role may change a member with `target`'s role.
    ///
    /// An Owner may promote or remove anyone but the last owner (a rule the
    /// server enforces against the membership list, not here). An Admin may
    /// manage Members but never another Admin or an Owner, so an admin cannot
    /// escalate a peer or remove someone above them. A Member manages nobody.
    pub fn can_manage(self, target: Role) -> bool {
        match self {
            Role::Owner => true,
            Role::Admin => target == Role::Member,
            Role::Member => false,
        }
    }

    /// Whether this role may grant `role` to someone.
    ///
    /// Nobody may grant a role stronger than their own, and an Admin cannot
    /// mint another Admin even at equal rank: peer promotion would let one
    /// admin escalate another without an owner's say. Only an Owner grants the
    /// Owner role.
    pub fn can_grant(self, role: Role) -> bool {
        match self {
            Role::Owner => true,
            Role::Admin => role == Role::Member,
            Role::Member => false,
        }
    }

    /// Whether this role may invite people into the workspace.
    pub fn can_invite(self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }

    /// Whether this role may create, rename, or delete channels, agents, and
    /// tasks — the everyday workspace work.
    pub fn can_write(self) -> bool {
        self.rank() >= Role::Member.rank()
    }

    /// Whether this role may change workspace-level settings, such as its name.
    pub fn can_administer(self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }

    /// Whether this role may hand ownership to someone else.
    pub fn can_transfer_ownership(self) -> bool {
        self == Role::Owner
    }
}

impl std::str::FromStr for Role {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "owner" => Ok(Role::Owner),
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_manages_everyone_and_nobody_manages_an_owner() {
        assert!(Role::Owner.can_manage(Role::Owner));
        assert!(Role::Owner.can_manage(Role::Admin));
        assert!(Role::Owner.can_manage(Role::Member));
        assert!(!Role::Admin.can_manage(Role::Owner));
        assert!(!Role::Member.can_manage(Role::Owner));
    }

    #[test]
    fn an_admin_manages_members_but_not_peers() {
        assert!(Role::Admin.can_manage(Role::Member));
        assert!(!Role::Admin.can_manage(Role::Admin));
        assert!(!Role::Member.can_manage(Role::Member));
    }

    #[test]
    fn nobody_grants_a_role_stronger_than_their_own() {
        assert!(Role::Owner.can_grant(Role::Owner));
        assert!(Role::Admin.can_grant(Role::Member));
        assert!(!Role::Admin.can_grant(Role::Admin));
        assert!(!Role::Admin.can_grant(Role::Owner));
        assert!(!Role::Member.can_grant(Role::Member));
    }

    #[test]
    fn invitation_and_administration_need_admin_or_owner() {
        for role in [Role::Owner, Role::Admin] {
            assert!(role.can_invite());
            assert!(role.can_administer());
        }
        assert!(!Role::Member.can_invite());
        assert!(!Role::Member.can_administer());
        assert!(Role::Member.can_write());
    }

    #[test]
    fn only_an_owner_transfers_ownership() {
        assert!(Role::Owner.can_transfer_ownership());
        assert!(!Role::Admin.can_transfer_ownership());
        assert!(!Role::Member.can_transfer_ownership());
    }

    #[test]
    fn roles_round_trip_through_their_names() {
        for role in [Role::Owner, Role::Admin, Role::Member] {
            assert_eq!(role.as_str().parse::<Role>(), Ok(role));
            assert_eq!(role.to_string(), role.as_str());
        }
        assert!("superuser".parse::<Role>().is_err());
    }
}
