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

impl TaskStatus {
    /// Whether the task can no longer change without a new execution.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// Where a task came from.
///
/// Conversation turns reuse the task pipeline but are not work items, so the
/// desktop can list user-created tasks without the turns that produced them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSource {
    /// A work item the user created.
    #[default]
    User,
    /// A reply in a channel or an agent direct conversation.
    ChannelTurn,
    /// A reply in a user-created conversation session.
    ConversationTurn,
}

impl TaskSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ChannelTurn => "channel_turn",
            Self::ConversationTurn => "conversation_turn",
        }
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
    /// Channel the task was created in, when it has one. Used to attribute LLM
    /// usage to the conversation that caused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// Prerequisite task IDs whose outputs will be injected as context
    pub context_tasks: Vec<TaskId>,
    /// Task that started the run this task belongs to.
    ///
    /// A crew run sets this to the submitted task's id on every task it
    /// creates, so cards and worktrees have one owning task. Manual tasks and
    /// channel turns leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_group: Option<TaskId>,
    /// Where this task came from: a user-created work item or a conversation
    /// turn that reused the task pipeline.
    #[serde(default)]
    pub source: TaskSource,
    /// Current execution status
    pub status: TaskStatus,
    /// Whether the task was archived by the user
    #[serde(default)]
    pub archived: bool,
    /// Plan artifact produced by a Plan-mode run, stored with the task until
    /// the user approves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Whether the stored plan was approved to continue in Manual mode.
    #[serde(default)]
    pub plan_approved: bool,
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
            channel_id: None,
            context_tasks: Vec::new(),
            run_group: None,
            source: TaskSource::User,
            status: TaskStatus::Pending,
            archived: false,
            plan: None,
            plan_approved: false,
        }
    }

    pub fn with_channel(mut self, channel_id: ChannelId) -> Self {
        self.channel_id = Some(channel_id);
        self
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

    pub fn in_channel(mut self, channel_id: ChannelId) -> Self {
        self.channel_id = Some(channel_id);
        self
    }

    pub fn with_context_task(mut self, task_id: TaskId) -> Self {
        self.context_tasks.push(task_id);
        self
    }

    /// Attach this task to the run started by `task_id`.
    pub fn in_run_group(mut self, task_id: TaskId) -> Self {
        self.run_group = Some(task_id);
        self
    }

    /// Mark the task as a conversation turn instead of a user-created work item.
    pub fn from_source(mut self, source: TaskSource) -> Self {
        self.source = source;
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

    #[test]
    fn run_groups_are_optional_and_serialize_cleanly() {
        let owner = TaskId::new("owner");
        let task = Task::new("Child", "Output").in_run_group(owner.clone());
        assert_eq!(task.run_group, Some(owner));

        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(value["run_group"], "owner");

        let legacy: Task = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "description": "Old task",
            "expected_output": "Output",
            "assigned_agent": null,
            "context_tasks": [],
            "status": "pending",
        }))
        .unwrap();
        assert!(legacy.run_group.is_none());
        assert!(!serde_json::to_value(&legacy)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("run_group"));
    }

    #[test]
    fn task_source_defaults_to_user_and_round_trips() {
        let task = Task::new("Work", "Output");
        assert_eq!(task.source, TaskSource::User);
        assert_eq!(task.source.as_str(), "user");

        let turn = Task::new("Reply", "Output").from_source(TaskSource::ChannelTurn);
        assert_eq!(turn.source.as_str(), "channel_turn");
        let value = serde_json::to_value(&turn).unwrap();
        assert_eq!(value["source"], "channel_turn");

        let legacy: Task = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "description": "Old task",
            "expected_output": "Output",
            "assigned_agent": null,
            "context_tasks": [],
            "status": "pending",
        }))
        .unwrap();
        assert_eq!(legacy.source, TaskSource::User);
        assert_eq!(TaskSource::ConversationTurn.as_str(), "conversation_turn");
    }
}
