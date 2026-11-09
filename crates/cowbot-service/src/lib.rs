//! Application services for the local CowBot runtime.
//!
//! This crate contains the local bootstrap flow and the channel/message
//! services that keep SQLite persistence and the event bus in sync.

mod agents;
mod approvals;
mod assignments;
mod bootstrap;
mod channel_agents;
mod channels;
mod conversations;
mod crew_runner;
mod scheduler;
mod task_service;
mod templates;

pub use agents::{AgentService, AgentUpdate};
pub use approvals::{
    decision_for, decision_for_call, ApprovalDecision, ApprovalService, ScopedApprovalGate,
};
pub use assignments::AssignmentService;
pub use bootstrap::{bootstrap_local, bootstrap_local_named, LocalContext};
pub use channel_agents::{ChannelAgentService, ChannelTurnOptions, ChannelTurnReport};
pub use channels::ChannelService;
pub use conversations::ConversationService;
pub use crew_runner::CrewRunner;
pub use scheduler::{AssignmentScheduler, CrewRunOptions};
pub use task_service::TaskService;
pub use templates::{builtin_templates, TemplateService};
