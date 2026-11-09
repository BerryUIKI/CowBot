use crate::agent::AgentId;
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

impl TaskStatus {
    /// Whether the task can no longer change without a new execution.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// Task abstraction inspired by CrewAI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task ID
    pub id: TaskId,
    /// Short title shown in lists and headings
    #[serde(default)]
    pub title: String,
    /// Detailed description of what must be executed
    pub description: String,
    /// Clear definition of the expected deliverable
    pub expected_output: String,
    /// Agent assigned to execute this task
    pub assigned_agent: Option<AgentId>,
    /// Prerequisite task IDs whose outputs will be injected as context
    pub context_tasks: Vec<TaskId>,
    /// Current execution status
    pub status: TaskStatus,
    /// Whether the task was archived by the user
    #[serde(default)]
    pub archived: bool,
}

impl Task {
    pub fn new(description: impl Into<String>, expected_output: impl Into<String>) -> Self {
        let description = description.into();
        Self {
            id: TaskId::random(),
            title: title_from_description(&description),
            description,
            expected_output: expected_output.into(),
            assigned_agent: None,
            context_tasks: Vec::new(),
            status: TaskStatus::Pending,
            archived: false,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        let title = title.into().trim().to_string();
        if !title.is_empty() {
            self.title = truncate_title(&title);
        }
        self
    }

    pub fn assign_to(mut self, agent_id: AgentId) -> Self {
        self.assigned_agent = Some(agent_id);
        self
    }

    pub fn with_context_task(mut self, task_id: TaskId) -> Self {
        self.context_tasks.push(task_id);
        self
    }
}

fn title_from_description(description: &str) -> String {
    let line = description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    truncate_title(line)
}

fn truncate_title(title: &str) -> String {
    const MAX_CHARS: usize = 80;
    let mut chars = title.chars();
    let mut truncated: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        truncated.push('…');
    }
    if truncated.is_empty() {
        "Task".to_string()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_titles_use_the_first_non_empty_line() {
        let task = Task::new("\n  First line  \nSecond line", "Output");
        assert_eq!(task.title, "First line");
    }

    #[test]
    fn task_titles_truncate_long_descriptions() {
        let description = "x".repeat(100);
        let task = Task::new(description, "Output");
        assert_eq!(task.title.chars().count(), 81);
        assert!(task.title.ends_with('…'));
    }

    #[test]
    fn explicit_titles_are_trimmed_and_fall_back_when_empty() {
        let task = Task::new("Description", "Output").with_title("  Short title  ");
        assert_eq!(task.title, "Short title");

        let task = Task::new("Description", "Output").with_title("   ");
        assert_eq!(task.title, "Description");
    }

    #[test]
    fn terminal_statuses_are_recognized() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::InProgress.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(TaskStatus::Completed {
            output: "done".to_string()
        }
        .is_terminal());
        assert!(TaskStatus::Failed {
            error: "failed".to_string()
        }
        .is_terminal());
    }
}
