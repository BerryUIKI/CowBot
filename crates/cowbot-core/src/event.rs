use crate::agent::{AgentId, AgentProfile};
use crate::approval::Approval;
use crate::assignment::{AssignmentEventKind, AssignmentId, AssignmentStatus};
use crate::channel::{Channel, ChannelId, ChannelMessage};
use crate::conversation::ConversationTurn;
use crate::permission::PermissionRuleMatch;
use crate::risk::RiskLevel;
use crate::task::{Task, TaskId};
use crate::tool::{ToolCall, ToolResult};
use crate::turn::TurnEvent;
use crate::worktree::WorktreeStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// High-level event types distributed across the CowBot message bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum EventPayload {
    /// A chat/collaboration message posted to a channel
    MessagePosted(ChannelMessage),
    /// A new collaboration channel was created
    ChannelCreated(Channel),
    /// A channel's metadata or pin state changed
    ChannelUpdated(Channel),
    /// A channel was deleted
    ChannelDeleted { channel_id: ChannelId },
    /// A channel's messages were cleared; its history is empty
    ChannelCleared { channel_id: ChannelId },
    /// A group conversation message was routed to its responders
    ConversationRouted(ConversationTurn),
    /// An agent profile was updated
    AgentUpdated(AgentProfile),
    /// A new task has been created
    TaskCreated(Task),
    /// A task's metadata changed, for example its archived flag
    TaskUpdated(Task),
    /// A task has been assigned to a specific agent
    TaskAssigned { task_id: TaskId, agent_id: AgentId },
    /// A task has successfully finished execution
    TaskCompleted { task_id: TaskId, output: String },
    /// A task failed during execution
    TaskFailed { task_id: TaskId, error: String },
    /// An assignment changed lifecycle state
    AssignmentUpdated {
        assignment_id: AssignmentId,
        task_id: TaskId,
        status: AssignmentStatus,
        kind: AssignmentEventKind,
        detail: Option<String>,
    },
    /// An agent requested a tool invocation (e.g. Code execution)
    ToolCallRequested {
        agent_id: AgentId,
        task_id: TaskId,
        call: ToolCall,
    },
    /// Tool execution result returned
    ToolCallExecuted {
        agent_id: AgentId,
        task_id: TaskId,
        result: ToolResult,
    },
    /// A task's git worktree changed lifecycle state
    WorktreeUpdated {
        task_id: TaskId,
        status: WorktreeStatus,
    },
    /// A tool call is waiting for a human decision
    ApprovalRequested {
        approval: Approval,
        risk: RiskLevel,
        reason: String,
        /// The workspace rule that required approval, when one matched.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<PermissionRuleMatch>,
    },
    /// A pending approval was approved or rejected
    ApprovalDecided { approval: Approval },
    /// Intermediate reasoning or thought emitted by an agent
    AgentThought { agent_id: AgentId, thought: String },
    /// One event in a turn's streaming lifecycle (ADR-0041).
    ///
    /// Direct chat, group reply, channel mention, and task turns all publish
    /// this same variant, so a client needs a single subscription.
    Turn(TurnEvent),
    /// Direct user prompt submitted to the platform
    UserPrompt {
        channel_id: ChannelId,
        prompt: String,
    },
    /// System broadcast notice
    SystemNotice { message: String },
}

/// An envelope wrapping an EventPayload with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
}

impl Event {
    pub fn new(payload: EventPayload) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            payload,
        }
    }
}
