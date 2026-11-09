use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentId;
use crate::approval::ApprovalLevel;
use crate::task::TaskId;

/// Maximum assignment tree depth allowed by the local runtime.
pub const MAX_ASSIGNMENT_DEPTH: u32 = 8;

/// Inherit an approval level for delegated work.
///
/// `PlanOnly` is treated as a hard read-only boundary and always wins.
/// Otherwise the more permissive of the parent and requested levels wins,
/// ordered `Ask < AcceptEdits < SafeAuto < Trusted`.
pub fn inherit_approval_level(parent: ApprovalLevel, requested: ApprovalLevel) -> ApprovalLevel {
    use ApprovalLevel::{AcceptEdits, Ask, PlanOnly, SafeAuto, Trusted};
    match (parent, requested) {
        (PlanOnly, _) | (_, PlanOnly) => PlanOnly,
        (Trusted, _) | (_, Trusted) => Trusted,
        (SafeAuto, _) | (_, SafeAuto) => SafeAuto,
        (AcceptEdits, _) | (_, AcceptEdits) => AcceptEdits,
        (Ask, Ask) => Ask,
    }
}

/// Unique identifier for an assignment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssignmentId(pub String);

impl AssignmentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for AssignmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where the work originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentOrigin {
    User,
    Agent,
    System,
    Routine,
    External,
}

/// How the assigned work should be performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentMode {
    Work,
    Consult,
    Notify,
}

/// How the requester expects to receive the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    Resume,
    Poll,
    None,
}

/// Lifecycle status of an assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStatus {
    Queued,
    Running,
    Waiting,
    Resume,
    NeedsInput,
    Completed,
    Failed,
    Cancelled,
}

impl AssignmentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            AssignmentStatus::Completed | AssignmentStatus::Failed | AssignmentStatus::Cancelled
        )
    }

    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// Who requested the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum AssignmentRequester {
    User(String),
    Agent(AgentId),
    System,
}

/// Final result of an assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum AssignmentOutcome {
    Completed { output: String },
    Failed { error: String },
    Cancelled { reason: String },
}

/// Per-root budgets. Child assignments copy the root's budgets; enforcement
/// always reads the root totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentBudgets {
    pub requests: u32,
    pub executions: u32,
    pub wakes: u32,
}

impl Default for AssignmentBudgets {
    fn default() -> Self {
        Self {
            requests: 64,
            executions: 24,
            wakes: 32,
        }
    }
}

/// A single unit of agent work with a parent/root tree position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: AssignmentId,
    pub root_id: AssignmentId,
    pub parent_id: Option<AssignmentId>,
    pub depth: u32,
    pub task_id: TaskId,
    pub origin: AssignmentOrigin,
    pub requester: AssignmentRequester,
    pub assignee: AgentId,
    pub mode: AssignmentMode,
    pub reply: ReplyMode,
    pub status: AssignmentStatus,
    pub executions: u32,
    pub restarts: u32,
    pub wakes: u32,
    pub budgets: AssignmentBudgets,
    pub approval_level: ApprovalLevel,
    pub outcome: Option<AssignmentOutcome>,
    pub stall_strikes: u32,
    pub repeated_errors: u32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Assignment {
    #[allow(clippy::too_many_arguments)]
    pub fn new_root(
        task_id: TaskId,
        origin: AssignmentOrigin,
        requester: AssignmentRequester,
        assignee: AgentId,
        mode: AssignmentMode,
        reply: ReplyMode,
        approval_level: ApprovalLevel,
    ) -> Self {
        let id = AssignmentId::random();
        let now = Utc::now();
        Self {
            id: id.clone(),
            root_id: id,
            parent_id: None,
            depth: 0,
            task_id,
            origin,
            requester,
            assignee,
            mode,
            reply,
            status: AssignmentStatus::Queued,
            executions: 0,
            restarts: 0,
            wakes: 0,
            budgets: AssignmentBudgets::default(),
            approval_level,
            outcome: None,
            stall_strikes: 0,
            repeated_errors: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_child(
        parent: &Assignment,
        task_id: TaskId,
        requester: AssignmentRequester,
        assignee: AgentId,
        mode: AssignmentMode,
        reply: ReplyMode,
        approval_level: ApprovalLevel,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: AssignmentId::random(),
            root_id: parent.root_id.clone(),
            parent_id: Some(parent.id.clone()),
            depth: parent.depth + 1,
            task_id,
            origin: parent.origin,
            requester,
            assignee,
            mode,
            reply,
            status: AssignmentStatus::Queued,
            executions: 0,
            restarts: 0,
            wakes: 0,
            budgets: parent.budgets,
            approval_level,
            outcome: None,
            stall_strikes: 0,
            repeated_errors: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

/// Kinds of append-only assignment events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentEventKind {
    Enqueued,
    Started,
    Waiting,
    Resumed,
    NeedsInput,
    Completed,
    Failed,
    Cancelled,
    Restarted,
    Stalled,
    BudgetExhausted,
}

/// An append-only event recording an assignment transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentEvent {
    pub id: String,
    pub assignment_id: AssignmentId,
    pub kind: AssignmentEventKind,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl AssignmentEvent {
    pub fn new(
        assignment_id: AssignmentId,
        kind: AssignmentEventKind,
        detail: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            assignment_id,
            kind,
            detail,
            created_at: Utc::now(),
        }
    }
}
