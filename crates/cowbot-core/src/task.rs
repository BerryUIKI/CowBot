use crate::agent::AgentId;
use crate::channel::ChannelId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a task in the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed { output: String },
    Failed { error: String },
    Cancelled,
}

/// Task abstraction inspired by CrewAI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task ID
    pub id: TaskId,
    /// Detailed description of what must be executed
    pub description: String,
    /// Clear definition of the expected deliverable
    pub expected_output: String,
    /// Agent assigned to execute this task
    pub assigned_agent: Option<AgentId>,
    /// Prerequisite task IDs whose outputs will be injected as context
    pub context_tasks: Vec<TaskId>,
    /// Channel the task was created in, when any. Used to attribute LLM usage
    /// to the conversation that caused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// Current execution status
    pub status: TaskStatus,
}

impl Task {
    pub fn new(description: impl Into<String>, expected_output: impl Into<String>) -> Self {
        Self {
            id: TaskId::random(),
            description: description.into(),
            expected_output: expected_output.into(),
            assigned_agent: None,
            context_tasks: Vec::new(),
            channel_id: None,
            status: TaskStatus::Pending,
        }
    }

    pub fn assign_to(mut self, agent_id: AgentId) -> Self {
        self.assigned_agent = Some(agent_id);
        self
    }

    pub fn in_channel(mut self, channel_id: ChannelId) -> Self {
        self.channel_id = Some(channel_id);
        self
    }

    pub fn with_context_task(mut self, task_id: TaskId) -> Self {
        self.context_tasks.push(task_id);
        self
    }
}
