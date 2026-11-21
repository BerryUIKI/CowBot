//! IM integrations: the deployment-side record of one IM connection and the
//! conversations it feeds into a workspace (ADR-0056).
//!
//! An integration is one bot connection to one IM tenant; a binding maps one
//! external conversation to one CowBot channel; a principal is the CowBot user
//! a platform account speaks as. The adapter itself, and the pipeline that
//! normalises what it reads, live in `cowbot-integrations`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::identity::{UserId, WorkspaceId};

/// The platforms an adapter can be written for.
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationKind {
    Mattermost,
    Telegram,
    Slack,
    Feishu,
}

impl IntegrationKind {
    /// Every supported platform, in the order the roadmap names them.
    pub const ALL: &'static [IntegrationKind] = &[
        IntegrationKind::Mattermost,
        IntegrationKind::Telegram,
        IntegrationKind::Slack,
        IntegrationKind::Feishu,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IntegrationKind::Mattermost => "mattermost",
            IntegrationKind::Telegram => "telegram",
            IntegrationKind::Slack => "slack",
            IntegrationKind::Feishu => "feishu",
        }
    }

    /// The CowBot user id a provisioned IM principal gets.
    ///
    /// Derived rather than generated, so two messages from the same IM user
    /// arriving at once provision one row instead of racing to create two: both
    /// compute the same id, and the second insert is a no-op.
    pub fn bridge_user_id(self, integration: &IntegrationId, external_id: &str) -> UserId {
        UserId::new(format!(
            "bridge:{}:{}:{external_id}",
            self.as_str(),
            integration.0
        ))
    }

    /// The address a provisioned principal gets.
    ///
    /// It is unique per integration, because two tenants of one platform can
    /// hand out the same external id. It is not usable for sign-in: the user row
    /// carries no password hash, and an empty hash never verifies.
    pub fn bridge_email(self, integration: &IntegrationId, external_id: &str) -> String {
        format!(
            "bridge:{}:{}:{external_id}@bridge.local",
            self.as_str(),
            integration.0
        )
    }
}

impl std::str::FromStr for IntegrationKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mattermost" => Ok(IntegrationKind::Mattermost),
            "telegram" => Ok(IntegrationKind::Telegram),
            "slack" => Ok(IntegrationKind::Slack),
            "feishu" => Ok(IntegrationKind::Feishu),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for IntegrationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Unique identifier for one IM connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IntegrationId(pub String);

impl IntegrationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for IntegrationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One IM connection, as a read returns it.
///
/// Deliberately without its configuration: the bot token and signing secret are
/// only obtainable through `Store::integration_config`, a method that says what
/// it hands back, so no accidental `Serialize` of this record can carry a
/// credential into an API response or a log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationRecord {
    pub id: IntegrationId,
    pub workspace_id: WorkspaceId,
    pub kind: IntegrationKind,
    pub label: String,
    pub created_at: DateTime<Utc>,
    /// When the connection was retired. A revoked integration is not started.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl IntegrationRecord {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// A new IM connection, as a write takes it.
///
/// Write-only on purpose: it is the one type that holds the adapter's
/// configuration, so it carries no `Serialize` and its `Debug` prints a
/// placeholder instead of the token.
#[derive(Clone)]
pub struct NewIntegration {
    pub id: IntegrationId,
    pub workspace_id: WorkspaceId,
    pub kind: IntegrationKind,
    pub label: String,
    /// Adapter configuration as JSON — endpoint, bot token, signing secret.
    pub config: String,
    pub created_at: DateTime<Utc>,
}

impl NewIntegration {
    pub fn new(
        workspace_id: &WorkspaceId,
        kind: IntegrationKind,
        label: impl Into<String>,
        config: impl Into<String>,
    ) -> Self {
        Self {
            id: IntegrationId::random(),
            workspace_id: workspace_id.clone(),
            kind,
            label: label.into(),
            config: config.into(),
            created_at: Utc::now(),
        }
    }
}

impl std::fmt::Debug for NewIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewIntegration")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("config", &"[sealed]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// The mapping from one external conversation to one CowBot channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationBinding {
    pub integration_id: IntegrationId,
    /// The platform's own conversation id: a channel, chat, or room.
    pub external_id: String,
    pub channel_id: ChannelId,
    /// The agent a mention of this bot addresses, when the binding routes to a
    /// group channel.
    ///
    /// A direct conversation needs none: it answers every message. A group
    /// channel needs it, because the mention has to become a CowBot `@Name` —
    /// and without one, a mentioned bot is dropped and the IM user has to type
    /// the agent's CowBot name instead.
    pub agent_id: Option<AgentId>,
}

/// The CowBot user a platform account speaks as.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationPrincipal {
    pub integration_id: IntegrationId,
    pub external_id: String,
    pub user_id: UserId,
    /// The name the platform shows for this account, kept for an operator to
    /// recognise the mapping.
    pub display_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_identity_is_derived_from_the_integration_and_the_account() {
        let integration = IntegrationId::new("i1");
        let other = IntegrationId::new("i2");

        // Derived, so a repeated read of the same account agrees with itself.
        assert_eq!(
            IntegrationKind::Slack.bridge_user_id(&integration, "U024"),
            IntegrationKind::Slack.bridge_user_id(&integration, "U024")
        );
        // Two tenants of one platform can hand out the same account id.
        assert_ne!(
            IntegrationKind::Slack.bridge_user_id(&integration, "U024"),
            IntegrationKind::Slack.bridge_user_id(&other, "U024")
        );
        assert_ne!(
            IntegrationKind::Slack.bridge_user_id(&integration, "U024"),
            IntegrationKind::Telegram.bridge_user_id(&integration, "U024")
        );
        assert!(IntegrationKind::Slack
            .bridge_email(&integration, "U024")
            .ends_with("@bridge.local"));
    }

    #[test]
    fn a_new_integration_never_prints_its_configuration() {
        let record = NewIntegration::new(
            &WorkspaceId::new("w1"),
            IntegrationKind::Telegram,
            "Support",
            r#"{"bot_token":"123:secret"}"#,
        );
        let printed = format!("{record:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(printed.contains("[sealed]"));
    }

    #[test]
    fn every_kind_round_trips_through_its_name() {
        for kind in IntegrationKind::ALL {
            assert_eq!(kind.as_str().parse::<IntegrationKind>(), Ok(*kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!("irc".parse::<IntegrationKind>(), Err(()));
    }

    #[test]
    fn a_revoked_integration_is_not_active() {
        let mut record = IntegrationRecord {
            id: IntegrationId::random(),
            workspace_id: WorkspaceId::new("w1"),
            kind: IntegrationKind::Slack,
            label: "Support".into(),
            created_at: Utc::now(),
            revoked_at: None,
        };
        assert!(record.is_active());
        record.revoked_at = Some(Utc::now());
        assert!(!record.is_active());
    }
}
