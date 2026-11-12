use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::WorkspaceId;

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Where a permission or approval decision came from.
///
/// Stored on audit entries so the audit view can filter decisions by source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSource {
    Rule,
    Model,
    Session,
    Human,
    Policy,
    System,
}

impl AuditSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditSource::Rule => "rule",
            AuditSource::Model => "model",
            AuditSource::Session => "session",
            AuditSource::Human => "human",
            AuditSource::Policy => "policy",
            AuditSource::System => "system",
        }
    }
}

impl std::fmt::Display for AuditSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// An auditable action performed in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub actor: String,
    pub action: String,
    pub target: String,
    /// Decision source: `rule`, `model`, `session`, `human`, `policy`, or
    /// `system`. Older audit rows may not have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl AuditEvent {
    pub fn new(
        workspace_id: WorkspaceId,
        actor: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id,
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            source: None,
            created_at: Utc::now(),
        }
    }

    pub fn with_source(mut self, source: AuditSource) -> Self {
        self.source = Some(source.as_str().to_string());
        self
    }
}
