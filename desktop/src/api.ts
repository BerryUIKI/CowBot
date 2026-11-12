import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AgentProfile,
  AppInfo,
  Approval,
  ApprovalLevel,
  Assignment,
  AssignmentEvent,
  AuditEvent,
  Channel,
  ChannelMessage,
  ClientSession,
  CommandInfo,
  CommandParseResult,
  SideAnswer,
  CompactionOutcome,
  ContextPreview,
  ConversationContext,
  ConversationParticipant,
  ConversationSummary,
  ConversationTurn,
  CowEvent,
  MemoryEntry,
  MemoryEntryUpdate,
  MemoryFilter,
  MemoryKind,
  MemoryNotes,
  MemorySave,
  MemorySettings,
  MemorySettingsUpdate,
  ModelMetadata,
  ModelSettings,
  NoteSave,
  OnboardingState,
  PermissionRule,
  Project,
  ProjectBranches,
  PromptSnapshotDetail,
  PromptSnapshotSettings,
  PromptSnapshotSummary,
  ProviderProtocol,
  RecalledMemory,
  RuleDecision,
  Run,
  SessionDecisionEntry,
  Task,
  TaskOverviewEntry,
  TaskWorktree,
  TaskWorktreeDiff,
  TemplateDocument,
  ToolCallRecord,
  ToolDefinition,
  WorkspacePolicy,
  WorkspaceSummary,
} from "./types";

export function getSession(): Promise<ClientSession> {
  return invoke<ClientSession>("get_session");
}

export function updateUserProfile(
  name: string,
  email: string,
): Promise<ClientSession> {
  return invoke<ClientSession>("update_user_profile", { name, email });
}

export function listChannels(): Promise<Channel[]> {
  return invoke<Channel[]>("list_channels");
}

export function ensureAgentChannels(): Promise<Channel[]> {
  return invoke<Channel[]>("ensure_agent_channels");
}

export function createChannel(name: string, topic: string): Promise<Channel> {
  return invoke<Channel>("create_channel", { name, topic });
}

export function renameChannel(
  channelId: string,
  name: string,
  topic: string,
): Promise<Channel> {
  return invoke<Channel>("rename_channel", { channelId, name, topic });
}

export function deleteChannel(channelId: string): Promise<Channel> {
  return invoke<Channel>("delete_channel", { channelId });
}

export function setChannelPinned(channelId: string, pinned: boolean): Promise<Channel> {
  return invoke<Channel>("set_channel_pinned", { channelId, pinned });
}

export function clearChannelMessages(channelId: string): Promise<number> {
  return invoke<number>("clear_channel_messages", { channelId });
}

export function listMessages(channelId: string, limit = 200): Promise<ChannelMessage[]> {
  return invoke<ChannelMessage[]>("list_messages", { channelId, limit });
}

export function createConversation(
  title: string | null,
  agentId: string,
): Promise<Channel> {
  return invoke<Channel>("create_conversation", { title, agentId });
}

/** Create a group conversation with an ordered participant list. */
export function createGroupConversation(
  title: string | null,
  agentIds: string[],
  routerEnabled: boolean,
  defaultResponder: string | null,
): Promise<Channel> {
  return invoke<Channel>("create_conversation", {
    title,
    agentIds,
    routerEnabled,
    defaultResponder,
  });
}

export function listConversationSummaries(): Promise<ConversationSummary[]> {
  return invoke<ConversationSummary[]>("list_conversation_summaries");
}

export function listConversationParticipants(
  channelId: string,
): Promise<ConversationParticipant[]> {
  return invoke<ConversationParticipant[]>("list_conversation_participants", {
    channelId,
  });
}

export function listConversationTurns(
  channelId: string,
  limit = 50,
): Promise<ConversationTurn[]> {
  return invoke<ConversationTurn[]>("list_conversation_turns", { channelId, limit });
}

export function getConversationContext(channelId: string): Promise<ConversationContext> {
  return invoke<ConversationContext>("get_conversation_context", { channelId });
}

/** Rebuild what one participant's next turn would send. */
export function getConversationContextPreview(
  channelId: string,
  agentId: string,
  draft?: string,
  planMode = false,
): Promise<ContextPreview> {
  return invoke<ContextPreview>("get_conversation_context_preview", {
    channelId,
    agentId,
    draft: draft ?? null,
    planMode,
  });
}

/** Rebuild the first request one task run sent. */
export function getTaskRunContextPreview(runId: string): Promise<ContextPreview> {
  return invoke<ContextPreview>("get_task_run_context_preview", { runId });
}

/** The redacted prompt snapshots recorded for a run, oldest first. */
export function listRunPromptSnapshots(
  runId: string,
): Promise<PromptSnapshotSummary[]> {
  return invoke<PromptSnapshotSummary[]>("list_run_prompt_snapshots", { runId });
}

/** The newest prompt snapshots in the workspace, for the audit view. */
export function listRecentPromptSnapshots(
  limit = 100,
): Promise<PromptSnapshotSummary[]> {
  return invoke<PromptSnapshotSummary[]>("list_recent_prompt_snapshots", { limit });
}

/** One recorded prompt snapshot with the call links it belongs to. */
export function getLlmPromptSnapshot(
  callId: string,
): Promise<PromptSnapshotDetail | null> {
  return invoke<PromptSnapshotDetail | null>("get_llm_prompt_snapshot", { callId });
}

export function getPromptSnapshotSettings(): Promise<PromptSnapshotSettings> {
  return invoke<PromptSnapshotSettings>("get_prompt_snapshot_settings");
}

export function updatePromptSnapshotSettings(
  enabled: boolean | null,
  retentionDays: number | null,
): Promise<PromptSnapshotSettings> {
  return invoke<PromptSnapshotSettings>("update_prompt_snapshot_settings", {
    enabled,
    retentionDays,
  });
}

/** Delete expired snapshots now and report how many went. */
export function purgePromptSnapshots(): Promise<number> {
  return invoke<number>("purge_prompt_snapshots");
}

export function updateConversationSettings(
  channelId: string,
  autoCompact: boolean,
  compactThreshold: number,
  routerEnabled?: boolean,
  defaultResponder?: string | null,
): Promise<ConversationContext> {
  return invoke<ConversationContext>("update_conversation_settings", {
    channelId,
    autoCompact,
    compactThreshold,
    routerEnabled,
    defaultResponder,
  });
}

export function compactConversation(
  channelId: string,
  agentId?: string,
): Promise<CompactionOutcome> {
  return invoke<CompactionOutcome>("compact_conversation", { channelId, agentId });
}

export function clearConversationContext(
  channelId: string,
  agentId?: string,
): Promise<number> {
  return invoke<number>("clear_conversation_context", { channelId, agentId });
}

export function getMemoryNotes(channelId?: string): Promise<MemoryNotes> {
  return invoke<MemoryNotes>("get_memory_notes", { channelId });
}

export function saveMemoryNote(content: string, channelId?: string): Promise<NoteSave> {
  return invoke<NoteSave>("save_memory_note", { channelId, content });
}

export function deleteMemoryNote(channelId: string): Promise<number> {
  return invoke<number>("delete_memory_note", { channelId });
}

export function getMemorySettings(): Promise<MemorySettings> {
  return invoke<MemorySettings>("get_memory_settings");
}

export function updateMemorySettings(
  update: MemorySettingsUpdate,
): Promise<MemorySettings> {
  return invoke<MemorySettings>("update_memory_settings", {
    extractionEnabled: update.extractionEnabled,
    recallEnabled: update.recallEnabled,
    notesBudgetTokens: update.notesBudgetTokens,
    recallBudgetTokens: update.recallBudgetTokens,
  });
}

export function listMemoryEntries(filter: MemoryFilter = {}): Promise<MemoryEntry[]> {
  return invoke<MemoryEntry[]>("list_memory_entries", {
    status: filter.status,
    agentId: filter.agentId,
    channelId: filter.channelId,
  });
}

export function addMemoryEntry(
  content: string,
  kind: MemoryKind,
  options: { agentId?: string; channelId?: string; pinned?: boolean } = {},
): Promise<MemorySave> {
  return invoke<MemorySave>("add_memory_entry", { content, kind, ...options });
}

export function updateMemoryEntry(
  entryId: string,
  update: MemoryEntryUpdate,
): Promise<MemorySave> {
  return invoke<MemorySave>("update_memory_entry", { entryId, ...update });
}

export function approveMemoryEntry(entryId: string): Promise<MemoryEntry> {
  return invoke<MemoryEntry>("approve_memory_entry", { entryId });
}

export function dismissMemoryEntry(entryId: string): Promise<MemoryEntry> {
  return invoke<MemoryEntry>("dismiss_memory_entry", { entryId });
}

export function archiveMemoryEntry(entryId: string): Promise<MemoryEntry> {
  return invoke<MemoryEntry>("archive_memory_entry", { entryId });
}

export function deleteMemoryEntry(entryId: string): Promise<number> {
  return invoke<number>("delete_memory_entry", { entryId });
}

export function previewMemoryRecall(
  agentId: string,
  query: string,
  channelId?: string,
): Promise<RecalledMemory | null> {
  return invoke<RecalledMemory | null>("preview_memory_recall", {
    channelId,
    agentId,
    query,
  });
}

export function postMessage(
  channelId: string,
  content: string,
  approvalLevel: ApprovalLevel,
): Promise<ChannelMessage> {
  return invoke<ChannelMessage>("post_message", { channelId, content, approvalLevel });
}

export function onCowbotEvent(handler: (event: CowEvent) => void): Promise<UnlistenFn> {
  return listen<CowEvent>("cowbot-event", (event) => handler(event.payload));
}

export function listAgents(): Promise<AgentProfile[]> {
  return invoke<AgentProfile[]>("list_agents");
}

export function listTemplates(): Promise<TemplateDocument[]> {
  return invoke<TemplateDocument[]>("list_templates");
}

export function instantiateAgents(templateId: string): Promise<AgentProfile[]> {
  return invoke<AgentProfile[]>("instantiate_agents", { templateId });
}

export function updateAgent(
  agentId: string,
  name: string,
  role: string,
  tools: string[],
): Promise<AgentProfile> {
  return invoke<AgentProfile>("update_agent", { agentId, name, role, tools });
}

export function listAvailableTools(): Promise<ToolDefinition[]> {
  return invoke<ToolDefinition[]>("list_available_tools");
}

export function createTask(input: {
  description: string;
  expectedOutput: string;
  assignee: string;
  channelId: string | null;
  approvalLevel: ApprovalLevel;
  worktree: { projectId: string; baseBranch: string } | null;
}): Promise<Task> {
  return invoke<Task>("create_task", {
    description: input.description,
    expectedOutput: input.expectedOutput,
    assignee: input.assignee,
    channelId: input.channelId,
    approvalLevel: input.approvalLevel,
    worktree: input.worktree,
  });
}

export function listTasks(): Promise<Task[]> {
  return invoke<Task[]>("list_tasks");
}

export function listRunTasks(taskId: string): Promise<Task[]> {
  return invoke<Task[]>("list_run_tasks", { taskId });
}

export function getTaskWorktree(taskId: string): Promise<TaskWorktree | null> {
  return invoke<TaskWorktree | null>("get_task_worktree", { taskId });
}

export function taskWorktreeDiff(taskId: string): Promise<TaskWorktreeDiff> {
  return invoke<TaskWorktreeDiff>("task_worktree_diff", { taskId });
}

export function discardTaskWorktree(taskId: string): Promise<TaskWorktree> {
  return invoke<TaskWorktree>("discard_task_worktree", { taskId });
}

export function listTaskOverview(): Promise<TaskOverviewEntry[]> {
  return invoke<TaskOverviewEntry[]>("list_task_overview");
}

export function runTask(taskId: string, approvalLevel: ApprovalLevel): Promise<void> {
  return invoke<void>("run_task", { taskId, approvalLevel });
}

export interface RunFinished {
  task_id: string;
  ok: boolean;
  error: string | null;
}

export function onRunFinished(handler: (event: RunFinished) => void): Promise<UnlistenFn> {
  return listen<RunFinished>("cowbot-run-finished", (event) => handler(event.payload));
}

export interface ChannelReplyFinished {
  message_id: string;
  reply_count: number;
  error: string | null;
  replied: string[];
  failed: string[];
  skipped: string[];
  selected: string[];
  route: string | null;
  turn_id: string | null;
}

export function onChannelReply(handler: (event: ChannelReplyFinished) => void): Promise<UnlistenFn> {
  return listen<ChannelReplyFinished>("cowbot-channel-reply", (event) =>
    handler(event.payload),
  );
}

export function cancelTask(taskId: string, reason: string): Promise<void> {
  return invoke<void>("cancel_task", { taskId, reason });
}

export function archiveTask(taskId: string): Promise<Task> {
  return invoke<Task>("archive_task", { taskId });
}

export function unarchiveTask(taskId: string): Promise<Task> {
  return invoke<Task>("unarchive_task", { taskId });
}

export function approveTaskPlan(taskId: string): Promise<Task> {
  return invoke<Task>("approve_task_plan", { taskId });
}

export function listAssignments(taskId: string): Promise<Assignment[]> {
  return invoke<Assignment[]>("list_assignments", { taskId });
}

export function listAssignmentEvents(assignmentId: string): Promise<AssignmentEvent[]> {
  return invoke<AssignmentEvent[]>("list_assignment_events", { assignmentId });
}

export function listRuns(taskId: string): Promise<Run[]> {
  return invoke<Run[]>("list_runs", { taskId });
}

export function listToolCalls(runId: string): Promise<ToolCallRecord[]> {
  return invoke<ToolCallRecord[]>("list_tool_calls", { runId });
}

export function pendingApprovals(): Promise<Approval[]> {
  return invoke<Approval[]>("pending_approvals");
}

export function decideApproval(
  approvalId: string,
  approved: boolean,
  forSession = false,
): Promise<Approval> {
  return invoke<Approval>("decide_approval", { approvalId, approved, forSession });
}

export function getSessionDecisions(): Promise<SessionDecisionEntry[]> {
  return invoke<SessionDecisionEntry[]>("session_decisions");
}

export function clearSessionDecisions(): Promise<void> {
  return invoke<void>("clear_session_decisions");
}

export function listPermissionRules(): Promise<PermissionRule[]> {
  return invoke<PermissionRule[]>("list_permission_rules");
}

export function createPermissionRule(
  tool: string,
  command: string | null,
  decision: RuleDecision,
): Promise<PermissionRule> {
  return invoke<PermissionRule>("create_permission_rule", {
    tool,
    command,
    decision,
  });
}

export function setPermissionRuleEnabled(
  ruleId: string,
  enabled: boolean,
): Promise<PermissionRule> {
  return invoke<PermissionRule>("set_permission_rule_enabled", {
    ruleId,
    enabled,
  });
}

export function deletePermissionRule(ruleId: string): Promise<void> {
  return invoke<void>("delete_permission_rule", { ruleId });
}

export function getWorkspacePolicy(): Promise<WorkspacePolicy> {
  return invoke<WorkspacePolicy>("get_workspace_policy");
}

export function setBypassCritical(
  bypassCritical: WorkspacePolicy["bypass_critical"],
): Promise<WorkspacePolicy> {
  return invoke<WorkspacePolicy>("set_bypass_critical", { bypassCritical });
}

export function getAuditEvents(
  limit = 200,
  source: string | null = null,
): Promise<AuditEvent[]> {
  return invoke<AuditEvent[]>("list_audit_events", { limit, source });
}

export function getModelSettings(): Promise<ModelSettings> {
  return invoke<ModelSettings>("get_model_settings");
}

export function saveModelProvider(input: {
  providerId: string | null;
  protocol: ProviderProtocol;
  name: string;
  baseUrl: string;
  models: string[];
  model: string;
  modelMetadata: Record<string, ModelMetadata>;
  apiKey: string;
  clearApiKey: boolean;
}): Promise<ModelSettings> {
  return invoke<ModelSettings>("save_model_provider", {
    draft: {
      providerId: input.providerId,
      protocol: input.protocol,
      name: input.name,
      baseUrl: input.baseUrl,
      models: input.models,
      model: input.model,
      modelMetadata: input.modelMetadata,
      apiKey: input.apiKey,
      clearApiKey: input.clearApiKey,
    },
  });
}

export function deleteModelProvider(providerId: string): Promise<ModelSettings> {
  return invoke<ModelSettings>("delete_model_provider", { providerId });
}

export function activateModelProvider(providerId: string | null): Promise<ModelSettings> {
  return invoke<ModelSettings>("activate_model_provider", { providerId });
}

export function setActiveModel(
  providerId: string,
  model: string,
): Promise<ModelSettings> {
  return invoke<ModelSettings>("set_active_model", { providerId, model });
}

export function testModelProvider(input: {
  providerId: string | null;
  protocol: ProviderProtocol;
  baseUrl: string;
  model: string;
  apiKey: string;
}): Promise<string> {
  return invoke<string>("test_model_provider", {
    providerId: input.providerId,
    protocol: input.protocol,
    baseUrl: input.baseUrl,
    model: input.model,
    apiKey: input.apiKey,
  });
}

export function fetchProviderModels(input: {
  providerId: string | null;
  protocol: ProviderProtocol;
  baseUrl: string;
  apiKey: string;
}): Promise<string[]> {
  return invoke<string[]>("fetch_provider_models", {
    providerId: input.providerId,
    protocol: input.protocol,
    baseUrl: input.baseUrl,
    apiKey: input.apiKey,
  });
}

export function getOnboardingState(): Promise<OnboardingState> {
  return invoke<OnboardingState>("get_onboarding_state");
}

export function completeOnboarding(): Promise<OnboardingState> {
  return invoke<OnboardingState>("complete_onboarding");
}

export function listWorkspaces(): Promise<WorkspaceSummary[]> {
  return invoke<WorkspaceSummary[]>("list_workspaces");
}

export function switchWorkspace(database: string): Promise<void> {
  return invoke<void>("switch_workspace", { database });
}

export function exportWorkspace(path: string): Promise<string> {
  return invoke<string>("export_workspace", { path });
}

export function importWorkspace(path: string): Promise<void> {
  return invoke<void>("import_workspace", { path });
}

export function createWorkspace(name: string): Promise<void> {
  return invoke<void>("create_workspace", { name });
}

export function listProjects(): Promise<Project[]> {
  return invoke<Project[]>("list_projects");
}

export function listCommands(): Promise<CommandInfo[]> {
  return invoke<CommandInfo[]>("list_commands");
}

export function parseCommand(input: string): Promise<CommandParseResult> {
  return invoke<CommandParseResult>("parse_command", { input });
}

export function askSideQuestion(
  channelId: string,
  question: string,
  options: {
    agentId?: string;
    model?: string;
    approvalLevel?: ApprovalLevel;
  } = {},
): Promise<SideAnswer> {
  return invoke<SideAnswer>("ask_side_question", {
    channelId,
    question,
    agentId: options.agentId,
    model: options.model,
    approvalLevel: options.approvalLevel,
  });
}

export function addProject(path: string): Promise<Project[]> {
  return invoke<Project[]>("add_project", { path });
}

export function removeProject(projectId: string): Promise<Project[]> {
  return invoke<Project[]>("remove_project", { projectId });
}

export function listProjectBranches(projectId: string): Promise<ProjectBranches> {
  return invoke<ProjectBranches>("list_project_branches", { projectId });
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("get_app_info");
}

export function setAutoCheckUpdates(enabled: boolean): Promise<void> {
  return invoke<void>("set_auto_check_updates", { enabled });
}

export function restartApp(): Promise<void> {
  return invoke<void>("restart_app");
}
