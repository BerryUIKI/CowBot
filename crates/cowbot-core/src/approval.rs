use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::WorkspaceId;
use crate::task::TaskId;

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Unique identifier for an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Status of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// What Bypass permissions does with a Critical-risk tool call.
///
/// Every other approval mode blocks Critical calls regardless of this
/// setting. `Allow` is the workspace default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BypassCritical {
    #[default]
    Allow,
    Ask,
    Block,
}

impl BypassCritical {
    pub fn as_str(self) -> &'static str {
        match self {
            BypassCritical::Allow => "allow",
            BypassCritical::Ask => "ask",
            BypassCritical::Block => "block",
        }
    }
}

impl std::fmt::Display for BypassCritical {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Per-workspace approval settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePolicy {
    pub workspace_id: WorkspaceId,
    pub bypass_critical: BypassCritical,
}

impl WorkspacePolicy {
    pub fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id,
            bypass_critical: BypassCritical::default(),
        }
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Effective approval level for an assignment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLevel {
    #[default]
    Ask,
    AcceptEdits,
    SafeAuto,
    PlanOnly,
    Trusted,
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A human decision required before a tool call executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: ApprovalId,
    pub task_id: TaskId,
    pub tool_call_id: String,
    pub status: ApprovalStatus,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Approval {
    pub fn new(task_id: TaskId, tool_call_id: impl Into<String>) -> Self {
        Self {
            id: ApprovalId::random(),
            task_id,
            tool_call_id: tool_call_id.into(),
            status: ApprovalStatus::Pending,
            decided_by: None,
            decided_at: None,
            created_at: Utc::now(),
        }
    }
}
