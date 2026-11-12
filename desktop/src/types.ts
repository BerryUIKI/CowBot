export interface ClientUser {
  id: string;
  name: string;
  email: string;
}

export interface Workspace {
  id: string;
  name: string;
  created_at: string;
}

export interface ClientSession {
  user: ClientUser;
  workspace: Workspace;
}

export type ChannelKind = "channel" | "direct" | "conversation";

export interface Channel {
  id: string;
  name: string;
  topic: string;
  created_at: string;
  agent_id: string | null;
  pinned: boolean;
  kind: ChannelKind;
}

export interface ConversationSettings {
  channel_id: string;
  mode: "single" | "group";
  provider_id: string | null;
  model: string;
  default_responder: string | null;
  router_enabled: boolean;
  auto_compact: boolean;
  compact_threshold: number;
  created_at: string;
  updated_at: string;
}

export interface CacheUsageSummary {
  calls: number;
  cached_input_tokens: number;
  total_input_tokens: number;
}

export type ContextUsageState = "neutral" | "warning" | "danger" | "full" | "unknown";

/** Where a token count came from: an exact tokenizer or the estimator. */
export type TokenSource = "tiktoken" | "provider" | "estimate";

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

/** A read-only plan of the request one turn or task run would send. */
export type PromptKind = "conversation" | "group" | "task_run";
export type PromptRole = "system" | "user" | "assistant" | "tool";
export type PromptSlotName =
  | "agent"
  | "platform"
  | "roster"
  | "default_responder"
  | "peer_convention"
  | "conversation_convention"
  | "mode_instruction"
  | "instruction";

export interface PromptSlot {
  name: PromptSlotName;
  content: string;
  tokens: number;
}

export interface PromptMessage {
  role: PromptRole;
  content: string;
  tokens: number;
  entry_id?: string | null;
  latest: boolean;
  tool_calls: string[];
  tool_call_id?: string | null;
}

export interface PromptTool {
  name: string;
  description: string;
  parameters: unknown;
  tokens: number;
}

export interface PromptMemory {
  block: string;
  tokens: number;
}

export type MemoryScope =
  | "workspace"
  | "agent"
  | "conversation"
  | "agent_in_conversation";

export type RecallSkipReason =
  | { kind: "inactive" }
  | { kind: "over_budget"; remaining_tokens: number };

export interface RecallCandidate {
  entry_id: string;
  content: string;
  kind: MemoryKind;
  scope: MemoryScope;
  pinned: boolean;
  score?: number | null;
  tokens: number;
  selected: boolean;
  skip_reason?: RecallSkipReason | null;
}

export interface RecallPreview {
  enabled: boolean;
  /** The trimmed search query; empty means only notes and pinned entries. */
  query: string;
  block?: string | null;
  estimated_tokens: number;
  budget_tokens: number;
  notes_budget_tokens: number;
  notes_tokens: number;
  candidates: RecallCandidate[];
}

export interface ContextProjection {
  system_tokens: number;
  tool_tokens: number;
  history_tokens: number;
  latest_tokens: number;
  active_entries: number;
  excluded_entries: number;
  token_source: TokenSource;
  token_counter: string | null;
}

export interface PromptPlan {
  kind: PromptKind;
  model: string;
  provider_id?: string | null;
  slots: PromptSlot[];
  messages: PromptMessage[];
  memory?: PromptMemory | null;
  recall: RecallPreview;
  tools: PromptTool[];
  projection: ContextProjection;
  auto_compact: boolean;
  compact_threshold: number;
}

/** A conversation or task-run preview with its window metadata. */
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

/** Why an LLM call was made (ADR-0037). */
export type LlmCallKind =
  | "agent"
  | "router"
  | "summary"
  | "compact"
  | "memory_extract";

export interface SnapshotToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

/** One message exactly as the provider received it. */
export interface SnapshotMessage {
  role: PromptRole;
  content: string;
  name?: string | null;
  tool_calls: SnapshotToolCall[];
  tool_call_id?: string | null;
}

/**
 * A sealed, redacted record of one model request.
 *
 * Unlike a `PromptPlan`, which rebuilds the next request, this is the request
 * that was actually sent and cannot be reconstructed.
 */
export interface LlmPromptSnapshot {
  schema_version: number;
  call_kind: LlmCallKind;
  model: string;
  messages: SnapshotMessage[];
  tools: ToolDefinition[];
  slots: PromptSlot[];
  memory?: PromptMemory | null;
  selected_memory_ids: string[];
  projection?: ContextProjection | null;
  token_count: number;
  content_hash: string;
  redacted: boolean;
  truncated: boolean;
  omitted_messages: number;
  created_at: string;
}

/** What a stored snapshot is, without its payload. */
export interface PromptSnapshotSummary {
  llm_call_id: string;
  run_id: string | null;
  task_id: string | null;
  channel_id: string | null;
  agent_id: string | null;
  call_kind: LlmCallKind;
  model: string;
  token_count: number;
  byte_size: number;
  truncated: boolean;
  redacted: boolean;
  created_at: string;
}

/** A stored snapshot together with the call it belongs to. */
export interface PromptSnapshotDetail {
  summary: PromptSnapshotSummary;
  snapshot: LlmPromptSnapshot;
  /** True when the stored payload no longer matches its content hash. */
  hash_mismatch: boolean;
}

export interface PromptSnapshotSettings {
  workspace_id: string;
  enabled: boolean;
  retention_days: number;
  updated_at: string;
}

export type MemoryKind = "fact" | "preference" | "decision" | "summary" | "note";
export type MemoryStatus = "active" | "pending" | "archived";
export type MemorySource = "user" | "agent" | "extraction" | "compaction";

export interface MemoryNote {
  id: string;
  workspace_id: string;
  channel_id?: string;
  title: string;
  content: string;
  created_at: string;
  updated_at: string;
}

export interface MemoryNotes {
  workspace: MemoryNote | null;
  conversation: MemoryNote | null;
}

export interface NoteSave {
  note: MemoryNote;
  redacted: boolean;
}

export interface MemoryEntry {
  id: string;
  workspace_id: string;
  agent_id?: string;
  channel_id?: string;
  kind: MemoryKind;
  content: string;
  source: MemorySource;
  source_ref?: string;
  status: MemoryStatus;
  pinned: boolean;
  tokens: number;
  recall_count: number;
  last_recalled_at?: string;
  created_at: string;
  updated_at: string;
}

export interface MemorySave {
  entry: MemoryEntry;
  redacted: boolean;
}

export interface MemorySettings {
  workspace_id: string;
  extraction_enabled: boolean;
  recall_enabled: boolean;
  notes_budget_tokens: number;
  recall_budget_tokens: number;
  updated_at: string;
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

export type RouteSource = "mention" | "router" | "default" | "silent" | "unmatched";

export interface ConversationParticipant {
  channel_id: string;
  agent_id: string;
  position: number;
  joined_at: string;
}

export interface ConversationTurn {
  id: string;
  channel_id: string;
  message_id: string;
  source: RouteSource;
  selected_agents: string[];
  unknown_mentions: string[];
  reason: string | null;
  router_call_id: string | null;
  task_ids: string[];
  mirrored_at: string | null;
  created_at: string;
}

export interface ConversationSummary {
  channel: Channel;
  settings: ConversationSettings;
  participants: AgentProfile[];
}

export interface CompactionOutcome {
  compacted: boolean;
  summarized_entries: number;
  tokens_before: number;
  tokens_after: number;
  summary: string | null;
  in_progress: boolean;
  reason: string | null;
}

export type Sender =
  | { type: "Agent"; id: string }
  | { type: "User"; id: string }
  | { type: "System" };

export interface ChannelMessage {
  id: string;
  channel_id: string;
  sender: Sender;
  content: string;
  thread_id: string | null;
  created_at: string;
}

export interface CowEvent {
  id: string;
  timestamp: string;
  payload: {
    type: string;
    payload?: unknown;
  };
}

export type ApprovalLevel =
  | "ask"
  | "accept_edits"
  | "safe_auto"
  | "plan_only"
  | "trusted";

export type BypassCritical = "allow" | "ask" | "block";

export interface WorkspacePolicy {
  workspace_id: string;
  bypass_critical: BypassCritical;
}

export interface ApprovalMode {
  value: ApprovalLevel;
  label: string;
  description: string;
}

/// Modes in the order Claude presents them.
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

export type TaskSource = "user" | "channel_turn" | "conversation_turn";

export interface Task {
  id: string;
  title: string;
  description: string;
  expected_output: string;
  assigned_agent: string | null;
  channel_id: string | null;
  context_tasks: string[];
  run_group: string | null;
  source: TaskSource;
  status: TaskStatus;
  archived: boolean;
  plan?: string | null;
  plan_approved?: boolean;
}

export type AssignmentStatus =
  | "queued"
  | "running"
  | "waiting"
  | "resume"
  | "needs_input"
  | "completed"
  | "failed"
  | "cancelled";

export type AssignmentOutcome =
  | { status: "completed"; detail: { output: string } }
  | { status: "failed"; detail: { error: string } }
  | { status: "cancelled"; detail: { reason: string } };

export interface Assignment {
  id: string;
  root_id: string;
  parent_id: string | null;
  depth: number;
  task_id: string;
  assignee: string;
  status: AssignmentStatus;
  executions: number;
  restarts: number;
  wakes: number;
  approval_level: ApprovalLevel;
  outcome: AssignmentOutcome | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export type AssignmentEventKind =
  | "enqueued"
  | "started"
  | "waiting"
  | "resumed"
  | "needs_input"
  | "completed"
  | "failed"
  | "cancelled"
  | "restarted"
  | "stalled"
  | "budget_exhausted";

export interface AssignmentEvent {
  id: string;
  assignment_id: string;
  kind: AssignmentEventKind;
  detail: string | null;
  created_at: string;
}

export type RunStatus =
  | "running"
  | "waiting_approval"
  | "completed"
  | "failed"
  | "cancelled";

export interface Run {
  id: string;
  task_id: string;
  agent_id: string;
  status: RunStatus;
  started_at: string;
  finished_at: string | null;
}

export interface ToolCallRecord {
  id: string;
  run_id: string;
  name: string;
  arguments: unknown;
  result: unknown;
  approved_by: string | null;
  created_at: string;
}

export type ApprovalStatus = "pending" | "approved" | "rejected" | "expired";

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

export type RuleDecision = "allow" | "ask" | "deny";

export type RuleSource = "local" | "imported";

export interface PermissionRule {
  id: string;
  workspace_id: string;
  tool: string;
  command: string | null;
  decision: RuleDecision;
  enabled: boolean;
  source: RuleSource;
  created_at: string;
  updated_at: string;
}

export interface AuditEvent {
  id: string;
  workspace_id: string;
  actor: string;
  action: string;
  target: string;
  source?: string | null;
  created_at: string;
}

export interface Approval {
  id: string;
  task_id: string;
  tool_call_id: string;
  status: ApprovalStatus;
  decided_by: string | null;
  decided_at: string | null;
  created_at: string;
}

export interface SessionDecisionEntry {
  key: string;
  decision: "approved" | "rejected" | "unknown";
}

export interface AgentProfile {
  id: string;
  name: string;
  role: string;
  goal: string;
  backstory: string;
  model: string | null;
  tools: string[];
  allow_delegation: boolean;
  max_iterations: number;
  verbose: boolean;
}

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: unknown;
}

export interface TemplateDocument {
  id: string;
  name: string;
  description: string;
  kind: string;
  agents: unknown[];
  tasks: unknown[];
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

export type WorktreeStatus = "requested" | "ready" | "removed" | "missing";

export interface TaskWorktree {
  task_id: string;
  project_id: string;
  project_path: string;
  base_branch: string;
  branch: string;
  worktree_path: string;
  status: WorktreeStatus;
  created_at: string;
  updated_at: string;
}

export interface TaskWorktreeDiff {
  status: string;
  stat: string;
  patch: string;
  truncated: boolean;
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

/** Whether an overview run still has work to show in the active sections. */
export function isActiveOverviewStatus(status: TaskOverviewStatus): boolean {
  return status !== "completed" && status !== "failed" && status !== "cancelled";
}

export interface AppInfo {
  version: string;
  autoCheckUpdates: boolean;
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

