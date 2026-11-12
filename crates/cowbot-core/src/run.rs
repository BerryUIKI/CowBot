use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::agent::{AgentId, AgentProfile};
use crate::task::TaskId;

/// Unique identifier for a task run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl RunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a single agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

/// A single execution of a task by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The agent profile as it was when the run started.
    ///
    /// Standard crew roles are transient profiles that are never written to
    /// the agent table, so a run could otherwise not name its own agent. The
    /// snapshot also lets the context inspector rebuild the run's request
    /// after the profile changes (ADR-0036).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<AgentProfile>,
}

impl Run {
    pub fn new(task_id: TaskId, agent_id: AgentId) -> Self {
        Self {
            id: RunId::random(),
            task_id,
            agent_id,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            agent_profile: None,
        }
    }

    /// Attach the profile that executed the run.
    pub fn with_agent_profile(mut self, profile: AgentProfile) -> Self {
        self.agent_profile = Some(profile);
        self
    }
}

/// A persisted tool invocation associated with a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub run_id: RunId,
    pub name: String,
    pub arguments: Value,
    pub result: Option<Value>,
    pub approved_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ToolCallRecord {
    pub fn new(run_id: RunId, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            run_id,
            name: name.into(),
            arguments,
            result: None,
            approved_by: None,
            created_at: Utc::now(),
        }
    }
}
