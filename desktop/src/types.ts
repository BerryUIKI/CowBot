// Types the desktop uses.
//
// API-shaped types come from `./generated`, which `scripts/generate-types.sh`
// produces from the Rust definitions in `crates/cowbot-core` (ADR-0040). Do not
// hand-write a type a Tauri command returns: annotate the Rust type with
// `ts(export)` and regenerate instead.
//
// Below the re-export block are the types that exist only in the desktop: view
// state, the composer's command surface, and the presentation helpers.

import type {
  AgentProfile,
  ApprovalLevel,
  AssignmentStatus,
  CacheUsageSummary,
  Channel,
  ChannelParticipant,
  CommandTarget,
  ConversationSettings,
  MemoryEntry,
  MemoryKind,
  MemoryStatus,
  Note,
  PromptKind,
  PromptPlan,
  RuleDecision,
  TaskWorktree,
  TokenSource,
  ToolCallRecord,
  Workspace,
} from "./generated/index";






export type {
  AgentId,
  AgentProfile,
  AgentTemplate,
  Approval,
  ApprovalId,
  ApprovalLevel,
  ApprovalStatus,
  Assignment,
  AssignmentBudgets,
  AssignmentEvent,
  AssignmentEventKind,
  AssignmentId,
  AssignmentMode,
  AssignmentOrigin,
  AssignmentOutcome,
  AssignmentRequester,
  AssignmentStatus,
  AuditEvent,
  AuditSource,
  BypassCritical,
  CacheUsageSummary,
  Channel,
  ChannelId,
  ChannelKind,
  ChannelMessage,
  ChannelParticipant,
  CommandArg,
  CommandSpec,
  CommandTarget,
  CompactionOutcome,
  ContextEntry,
  ContextEntryKind,
  ContextEventKind,
  ContextProjection,
  ContextRole,
  ContextToolCall,
  ConversationMode,
  ConversationParticipantContext,
  ConversationSettings,
  ConversationSettingsUpdate,
  ConversationTurn,
  LlmCallKind,
  LlmPromptSnapshot,
  MemoryEntry,
  MemoryKind,
  MemoryScope,
  MemorySettings,
  MemorySource,
  MemoryStatus,
  Note,
  PermissionRule,
  PromptKind,
  PromptMemory,
  PromptMessage,
  PromptPlan,
  PromptRole,
  PromptSlot,
  PromptSlotName,
  PromptSnapshotDetail,
  PromptSnapshotSettings,
  PromptSnapshotSummary,
  PromptTool,
  RecallCandidate,
  RecallPreview,
  RecallSkipReason,
  ReplyMode,
  RouteSource,
  RuleDecision,
  RuleSource,
  Run,
  RunId,
  RunStatus,
  Sender,
  SnapshotMessage,
  SnapshotToolCall,
  Task,
  TaskId,
  TaskSource,
  TaskTemplate,
  TaskWorktree,
  TaskWorktreeDiff,
  TemplateDocument,
  TemplateKind,
  TokenSource,
  ToolCallRecord,
  ToolDefinition,
  TurnId,
  Workspace,
  WorkspaceId,
  WorkspacePolicy,
  WorktreeStatus,
} from "./generated/index";

// Values that the generated surface does not carry: the desktop owns them.
//
// A few view types share a name with a core type the desktop also imports (the
// meter's `ConversationContext`, the composer's `CommandArgKind`), so they are
// declared locally rather than re-exported.
export type ContextUsageState = "neutral" | "warning" | "danger" | "full" | "unknown";
export type CommandArgKind = "text" | "participant" | "choice";

export interface ClientUser {
  id: string;
  name: string;
  email: string;
}

export interface ClientSession {
  user: ClientUser;
  workspace: Workspace;
}

export const approvalModes: ApprovalMode[] = [
  {
    value: "safe_auto",
    label: "Auto",
    description: "Low-risk tools run automatically; medium and high risk ask.",
  },
  {
    value: "ask",
    label: "Manual",
    description: "Read-only tools run; anything that changes state waits for approval.",
  },
  {
    value: "accept_edits",
    label: "Accept edits",
    description: "Workspace file edits run; other tools follow Manual.",
  },
  {
    value: "plan_only",
    label: "Plan",
    description: "Only read-only tools run; everything else is denied.",
  },
  {
    value: "trusted",
    label: "Bypass permissions",
    description:
      "Every tool call runs; critical-risk calls follow the Bypass setting in Settings → Approvals.",
  },
];

export type TaskStatus =
  | "pending"
  | "in_progress"
  | "cancelled"
  | { completed: { output: string } }
  | { failed: { error: string } };

export function isActiveOverviewStatus(status: TaskOverviewStatus): boolean {
  return status !== "completed" && status !== "failed" && status !== "cancelled";
}

export interface ConversationParticipantView {
  agent_id: string;
  agent_name: string;
  system_tokens: number;
  tool_tokens: number;
  history_tokens: number;
  latest_tokens: number;
  total_tokens: number;
  memory_tokens: number;
  active_entries: number;
  excluded_entries: number;
  percent_used: number | null;
  state: ContextUsageState;
  cache_last_run: CacheUsageSummary;
  cache_conversation: CacheUsageSummary;
}

export interface ConversationContext {
  channel_id: string;
  agent_id: string;
  model: string;
  provider_id: string | null;
  system_tokens: number;
  tool_tokens: number;
  history_tokens: number;
  latest_tokens: number;
  total_tokens: number;
  active_entries: number;
  excluded_entries: number;
  token_source: TokenSource;
  token_counter: string | null;
  memory_tokens: number;
  context_window: number | null;
  max_output_tokens: number | null;
  usable_tokens: number | null;
  percent_used: number | null;
  state: ContextUsageState;
  auto_compact: boolean;
  compact_threshold: number;
  router_enabled: boolean;
  default_responder: string | null;
  cache_last_turn: CacheUsageSummary;
  cache_conversation: CacheUsageSummary;
  participants: ConversationParticipantView[];
}

export interface ContextPreview {
  kind: PromptKind;
  channel_id: string | null;
  run_id: string | null;
  task_id: string | null;
  agent_id: string;
  agent_name: string;
  title: string;
  plan: PromptPlan;
  request_tokens: number;
  context_window: number | null;
  max_output_tokens: number | null;
  usable_tokens: number | null;
  percent_used: number | null;
  state: ContextUsageState;
  over_budget: boolean;
}

export interface MemoryNotes {
  workspace: Note | null;
  conversation: Note | null;
}

export interface NoteSave {
  note: Note;
  redacted: boolean;
}

export interface MemorySave {
  entry: MemoryEntry;
  redacted: boolean;
}

export interface MemorySettingsUpdate {
  extractionEnabled?: boolean;
  recallEnabled?: boolean;
  notesBudgetTokens?: number;
  recallBudgetTokens?: number;
}

export interface MemoryFilter {
  status?: MemoryStatus;
  agentId?: string;
  channelId?: string;
}

export interface MemoryEntryUpdate {
  content?: string;
  kind?: MemoryKind;
  pinned?: boolean;
}

export interface RecalledMemory {
  block: string;
  entry_ids: string[];
  tokens: number;
}

export interface ConversationSummary {
  channel: Channel;
  settings: ConversationSettings;
  participants: AgentProfile[];
}

export interface CowEvent {
  id: string;
  timestamp: string;
  payload: {
    type: string;
    payload?: unknown;
  };
}

export interface ApprovalMode {
  value: ApprovalLevel;
  label: string;
  description: string;
}

export interface ToolCallDetail {
  name: string;
  arguments: unknown;
}

export interface ApprovalRisk {
  risk: string;
  reason: string;
  rule?: ApprovalRuleMatch | null;
}

export interface ApprovalRuleMatch {
  rule_id: string;
  description: string;
  decision: RuleDecision;
}

export interface SessionDecisionEntry {
  key: string;
  decision: "approved" | "rejected" | "unknown";
}

export type ProviderProtocol = "openai" | "anthropic" | "openai-responses";

export interface ModelMetadata {
  context_window?: number;
  max_output_tokens?: number;
}

export interface ModelProvider {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  base_url: string;
  models: string[];
  model: string;
  model_metadata: Record<string, ModelMetadata>;
  has_api_key: boolean;
  active: boolean;
}

export interface ModelSettings {
  providers: ModelProvider[];
  active_provider_id: string | null;
  active_label: string;
  config_path: string;
}

export interface OnboardingState {
  completed: boolean;
}

export interface WorkspaceSummary {
  database: string;
  name: string;
  active: boolean;
  available: boolean;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  current_branch: string | null;
  available: boolean;
}

export interface ProjectBranches {
  current: string | null;
  branches: string[];
}

export type TaskOverviewStatus =
  | "running"
  | "needs_input"
  | "waiting"
  | "resume"
  | "queued"
  | "completed"
  | "failed"
  | "cancelled";

export interface TaskOverviewProgress {
  completed: number;
  total: number;
}

export interface TaskOverviewAssignment {
  id: string;
  assignee: string;
  status: AssignmentStatus;
  detail: string | null;
}

export interface TaskOverviewEntry {
  run_id: string;
  title: string;
  channel_id: string | null;
  status: TaskOverviewStatus;
  progress: TaskOverviewProgress;
  assignments: TaskOverviewAssignment[];
  worktree: TaskWorktree | null;
  latest_tool_call: ToolCallRecord | null;
  started_at: string;
  updated_at: string;
}

export interface AppInfo {
  version: string;
  autoCheckUpdates: boolean;
}

export interface CommandArgInfo {
  placeholder: string;
  required: boolean;
  kind: CommandArgKind;
  choices: string[];
}

export interface CommandInfo {
  name: string;
  aliases: string[];
  usage: string;
  summary: string;
  target: CommandTarget;
  args: CommandArgInfo[];
}

/// The face the desktop uses of a conversation participant.
///
/// The Tauri command returns `ChannelParticipant`, whose ids are branded
/// newtypes; the desktop needs them as strings, so this is the alias it has
/// always used. Regenerating does not change it.
export type ConversationParticipant = ChannelParticipant;

export type CommandParseResult =
  | { kind: "not_a_command" }
  | { kind: "invocation"; name: string; args: string[]; raw_args: string }
  | { kind: "unknown"; name: string }
  | { kind: "missing_argument"; name: string; usage: string };

/**
 * One `/btw` answer. It is deliberately not a {@link ChannelMessage}: nothing
 * was posted, so the panel that shows it is the only place it exists.
 */
export interface SideAnswer {
  agent_id: string;
  agent_name: string;
  content: string;
  model: string;
  call_id: string;
}

const taskStatusLabels = {
  pending: "Pending",
  in_progress: "In progress",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
} as const;

export function taskStatusKind(
  status: TaskStatus,
): "pending" | "in_progress" | "completed" | "failed" | "cancelled" {
  if (typeof status === "string") return status;
  return "completed" in status ? "completed" : "failed";
}

export function taskStatusLabel(status: TaskStatus): string {
  return taskStatusLabels[taskStatusKind(status)];
}

export function approvalLevelLabel(level: ApprovalLevel): string {
  switch (level) {
    case "ask":
      return "Manual";
    case "accept_edits":
      return "Accept edits";
    case "safe_auto":
      return "Auto";
    case "plan_only":
      return "Plan";
    case "trusted":
      return "Bypass permissions";
  }
}

