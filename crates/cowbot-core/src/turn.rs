use crate::agent::AgentId;
use crate::approval::Approval;
use crate::cancel::CancelSignal;
use crate::channel::ChannelId;
use crate::conversation::TurnId;
use crate::task::TaskId;
use crate::tool::{ToolCall, ToolResult};
use serde::{Deserialize, Serialize};

/// Where a turn came from.
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnTarget {
    /// A reply in a direct chat, group conversation, or channel.
    Conversation {
        channel_id: ChannelId,
        agent_id: AgentId,
        /// The message the user posted that started the turn.
        message_id: String,
    },
    /// A task the user ran.
    Task { task_id: TaskId, agent_id: AgentId },
}

/// A tool call that is waiting for a human decision.
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub approval: Approval,
    /// The call the decision applies to, so the UI can show it without a
    /// second lookup.
    pub call: ToolCall,
}

/// One event in a turn's lifecycle.
///
/// Every variant except [`TurnEvent::TurnStarted`] carries the turn id, and
/// [`TurnEvent::is_terminal`] marks the two that end it. The final message is
/// persisted before a terminal event is published (ADR-0041).
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    TurnStarted {
        turn_id: TurnId,
        target: TurnTarget,
        /// Every agent that answers in this turn, so the UI can show a
        /// placeholder before the first delta arrives.
        agent_ids: Vec<AgentId>,
    },
    TextDelta {
        turn_id: TurnId,
        text: String,
    },
    ReasoningDelta {
        turn_id: TurnId,
        text: String,
    },
    ToolCallStarted {
        turn_id: TurnId,
        call: ToolCall,
    },
    ToolCallFinished {
        turn_id: TurnId,
        result: ToolResult,
    },
    ApprovalRequired {
        turn_id: TurnId,
        approval: PendingApproval,
    },
    TurnDone {
        turn_id: TurnId,
        /// The final persisted text. A turn that produced no text carries an
        /// empty string; a stopped turn keeps what it produced (§4).
        message: String,
        stopped: bool,
    },
    TurnError {
        turn_id: TurnId,
        error: String,
        /// Text that was produced before the failure, when any.
        partial: String,
    },
}

impl TurnEvent {
    /// The turn this event belongs to.
    pub fn turn_id(&self) -> &TurnId {
        match self {
            Self::TurnStarted { turn_id, .. }
            | Self::TextDelta { turn_id, .. }
            | Self::ReasoningDelta { turn_id, .. }
            | Self::ToolCallStarted { turn_id, .. }
            | Self::ToolCallFinished { turn_id, .. }
            | Self::ApprovalRequired { turn_id, .. }
            | Self::TurnDone { turn_id, .. }
            | Self::TurnError { turn_id, .. } => turn_id,
        }
    }

    /// Whether this event ends its turn.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TurnDone { .. } | Self::TurnError { .. })
    }
}

/// How a turn ended.
#[derive(Debug, Clone)]
pub enum TurnOutcome {
    /// The agent finished on its own; `stopped` is set when a cancellation
    /// interrupted it after some text had been produced.
    Done { message: String, stopped: bool },
    /// The turn failed; `partial` is the text produced before the failure.
    Failed { error: String, partial: String },
}

impl TurnOutcome {
    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::Done { stopped: true, .. })
    }
}

/// Turn-level cancellation.
///
/// One runner owns the lifecycle of every turn: chat, group, and task paths
/// register their `TurnId` here, stop through it, and never implement their own
/// stop path. `turn` is the cancellation scope the agent loop and provider
/// stream observe.
pub trait TurnControl: Send + Sync {
    /// Register a turn before anything is published for it.
    fn open(&self, turn: TurnId, channel_id: Option<ChannelId>);
    /// Trip the turn's cancel signal, if it is still open. Idempotent: an
    /// unknown or finished turn is a no-op that reports `false`.
    fn cancel(&self, turn: &TurnId) -> bool;
    /// Cancel every open turn in a channel.
    fn cancel_channel(&self, channel_id: &ChannelId) -> usize;
    /// The cancellation scope for a turn opened by [`Self::open`].
    fn turn(&self, turn: &TurnId) -> CancelSignal;
    /// Forget a turn once its terminal event has been published.
    fn close(&self, turn: &TurnId);
    /// Whether the turn has been stopped.
    fn is_cancelled(&self, turn: &TurnId) -> bool;
}
