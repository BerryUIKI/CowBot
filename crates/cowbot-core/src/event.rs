use crate::agent::AgentId;
use crate::approval::Approval;
use crate::channel::{Channel, ChannelId, ChannelMessage};
use crate::risk::RiskLevel;
use crate::task::{Task, TaskId};
use crate::tool::{ToolCall, ToolResult};
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
    /// A new task has been created
    TaskCreated(Task),
    /// A task has been assigned to a specific agent
    TaskAssigned { task_id: TaskId, agent_id: AgentId },
    /// A task has successfully finished execution
    TaskCompleted { task_id: TaskId, output: String },
    /// A task failed during execution
    TaskFailed { task_id: TaskId, error: String },
    /// An agent requested a tool invocation (e.g. Code execution)
    ToolCallRequested { agent_id: AgentId, call: ToolCall },
    /// Tool execution result returned
    ToolCallExecuted {
        agent_id: AgentId,
        result: ToolResult,
    },
    /// A tool call is waiting for a human decision
    ApprovalRequested {
        approval: Approval,
        risk: RiskLevel,
        reason: String,
    },
    /// A pending approval was approved or rejected
    ApprovalDecided { approval: Approval },
    /// Intermediate reasoning or thought emitted by an agent
    AgentThought { agent_id: AgentId, thought: String },
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
