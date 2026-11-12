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
mod compaction_locks;
mod conversations;
mod crew_runner;
mod memory;
mod permissions;
mod permissions_rules;
mod preview;
mod prompt_snapshots;
mod remember;
mod scheduler;
mod session_decisions;
mod task_service;
mod templates;
mod worktrees;

pub use agents::{AgentService, AgentUpdate};
pub use approvals::{
    decision_for, decision_for_call, ApprovalDecision, ApprovalService, ScopedApprovalGate,
};
pub use assignments::AssignmentService;
pub use bootstrap::{bootstrap_local, bootstrap_local_named, LocalContext};
pub use channel_agents::{
    ChannelAgentService, ChannelTurnOptions, ChannelTurnReport, ParticipantCapacity,
};
pub use channels::ChannelService;
pub use compaction_locks::{CompactionGuard, CompactionLocks};
pub use conversations::ConversationService;
pub use crew_runner::CrewRunner;
pub use memory::{
    parse_extraction_response, ExtractionCandidate, MemoryFilter, MemorySave, MemoryService,
    MemorySettingsUpdate, MemoryUpdate, NewMemory, NoteSave, RecalledMemory,
};
pub use permissions::{
    ModelPermissionEvaluator, PermissionDecision, PermissionDecisionKind, PermissionEvaluator,
    PermissionRequest,
};
pub use permissions_rules::PermissionService;
pub use preview::{ConversationPreview, PreviewService, TaskRunPreview};
pub use prompt_snapshots::PromptSnapshotService;
pub use remember::{RememberTool, REMEMBER_TOOL_NAME};
pub use scheduler::{AssignmentScheduler, CrewRunOptions};
pub use session_decisions::{SessionDecisionEntry, SessionDecisions};
pub use task_service::TaskService;
pub use templates::{builtin_templates, TemplateService};
pub use worktrees::{slug_for, WorktreeService};
