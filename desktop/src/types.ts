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
  IntegrationKind,
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
  RoutingMode,
} from "./generated/index";

import { t, type TranslationKey } from "./i18n/catalog";






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
  RoutingMode,
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
  PendingApproval,
  ToolResult,
  TurnEvent,
  TurnTarget,
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
    labelKey: "approval.auto",
    descriptionKey: "approval.autoDescription",
  },
  {
    value: "ask",
    labelKey: "approval.manual",
    descriptionKey: "approval.manualDescription",
  },
  {
    value: "accept_edits",
    labelKey: "approval.acceptEdits",
    descriptionKey: "approval.acceptEditsDescription",
  },
  {
    value: "plan_only",
    labelKey: "approval.plan",
    descriptionKey: "approval.planDescription",
  },
  {
    value: "trusted",
    labelKey: "approval.bypass",
    descriptionKey: "approval.bypassDescription",
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
  /// Display name of the provider, when one supplies the model.
  provider_name: string | null;
  /// Whether this conversation follows the default model (ADR-0049).
  model_follows_default: boolean;
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
  /// How an unmentioned group message picks its responder (ADR-0042).
  routing_mode: RoutingMode;
  /// Participants kept in the roster but never selected.
  muted_participants: string[];
  /// Whether group replies are stripped of speaker labels and cross-talk.
  reply_cleanup: boolean;
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
  labelKey: TranslationKey;
  descriptionKey: TranslationKey;
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
  /** Absolute path of the workspace database the app opened. */
  database: string;
  /** Folder that holds workspace databases. */
  databaseDir: string;
}

/** What the desktop needs before it can open the workspace database. */
export type StartupStatus =
  | { state: "ready" }
  | {
      state: "upgrade";
      current: number;
      target: number;
      database: string;
      /** File that receives a backup before the upgrade runs. */
      backup: string;
    }
  | {
      state: "newer";
      current: number;
      target: number;
      database: string;
      /** Copies this build can open, newest first. */
      backups: string[];
      /** Where the refused database moves if the user starts fresh. */
      replaced: string;
    }
  | {
      state: "failed";
      message: string;
      database: string;
      backups: string[];
      replaced: string | null;
    }
  /**
   * The device opens a server workspace and the server could not be reached.
   *
   * The local database was never inspected, so there is no upgrade or recovery
   * to offer: the choice is to retry or switch back to the local workspace.
   */
  | {
      state: "serverUnavailable";
      message: string;
      baseUrl: string;
    };

export type StartupAction =
  | { kind: "upgrade" }
  | { kind: "upgradeWithBackup" }
  | { kind: "restore"; backup: string }
  | { kind: "fresh" }
  | { kind: "retry" }
  | { kind: "goLocal" };

export interface WorkspaceSummary {
  database: string;
  name: string;
  active: boolean;
  available: boolean;
}

/**
 * Where this device's data comes from, as Settings → Workspace shows it.
 *
 * The session token is never part of this: the view learns only which account
 * and workspace are connected.
 */
export interface ServerSettings {
  /** `local` or `server`: which backend the app opened. */
  mode: "local" | "server";
  base_url: string | null;
  email: string | null;
  workspace_id: string | null;
  workspace_name: string | null;
}

/** One workspace the signed-in server account can enter. */
export interface ServerWorkspace {
  id: string;
  name: string;
}

/** What a sign-in reachable without saving the configuration reports. */
export interface ServerProbe {
  workspaces: ServerWorkspace[];
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
  /** Schema version of the workspace database this build provides. */
  databaseSchema: number;
  /** `local` or `server`: which backend this device opened. */
  backend: "local" | "server";
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

const taskStatusLabels: Record<
  "pending" | "in_progress" | "completed" | "failed" | "cancelled",
  TranslationKey
> = {
  pending: "taskStatus.pending",
  in_progress: "taskStatus.inProgress",
  completed: "taskStatus.completed",
  failed: "taskStatus.failed",
  cancelled: "taskStatus.cancelled",
};

export function taskStatusKind(
  status: TaskStatus,
): "pending" | "in_progress" | "completed" | "failed" | "cancelled" {
  if (typeof status === "string") return status;
  return "completed" in status ? "completed" : "failed";
}

export function taskStatusLabel(status: TaskStatus): string {
  return t(taskStatusLabels[taskStatusKind(status)]);
}

export function approvalLevelLabel(level: ApprovalLevel): string {
  switch (level) {
    case "ask":
      return t("approval.manual");
    case "accept_edits":
      return t("approval.acceptEdits");
    case "safe_auto":
      return t("approval.auto");
    case "plan_only":
      return t("approval.plan");
    case "trusted":
      return t("approval.bypass");
  }
}

/** An IM integration as the server returns it (ADR-0056). */
export interface IntegrationView {
  id: string;
  workspace_id: string;
  kind: IntegrationKind;
  label: string;
  created_at: string;
  revoked_at: string | null;
  /** Whether this client's backend can actually run the kind. */
  supported: boolean;
  binding_count: number;
}

/** One platform conversation feeding a CowBot channel. */
export interface BindingView {
  integration_id: string;
  external_id: string;
  channel_id: string;
  agent_id: string | null;
  /** What the adapter said on bind; absent on a list. */
  health?: { status: string; detail: string | null } | null;
}
