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
  context_window: number | null;
  max_output_tokens: number | null;
  usable_tokens: number | null;
  percent_used: number | null;
  state: ContextUsageState;
  auto_compact: boolean;
  compact_threshold: number;
  cache_last_turn: CacheUsageSummary;
  cache_conversation: CacheUsageSummary;
}

export interface CompactionOutcome {
  compacted: boolean;
  summarized_entries: number;
  tokens_before: number;
  tokens_after: number;
  summary: string | null;
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

export interface Task {
  id: string;
  title: string;
  description: string;
  expected_output: string;
  assigned_agent: string | null;
  channel_id: string | null;
  context_tasks: string[];
  run_group: string | null;
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

