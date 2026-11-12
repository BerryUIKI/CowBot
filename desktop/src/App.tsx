import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent as ReactMouseEvent,
} from "react";

import ApprovalCard from "./ApprovalCard";
import ApprovalModeMenu from "./ApprovalModeMenu";
import Avatar from "./Avatar";
import BackgroundTaskCard from "./BackgroundTaskCard";
import GroupAvatar from "./GroupAvatar";
import Inspector from "./Inspector";
import type { InspectorTab } from "./Inspector";
import ContextMeter from "./ContextMeter";
import ContextInspector from "./ContextInspector";
import type { ContextTarget } from "./ContextInspector";
import ContextMenu from "./ContextMenu";
import type { ContextMenuItem } from "./ContextMenu";
import Modal from "./Modal";
import RememberMessageModal from "./RememberMessageModal";
import RichText from "./RichText";
import Onboarding from "./Onboarding";
import SettingsView, { settingsSections } from "./SettingsView";
import type { SettingsSection } from "./SettingsView";
import SlashCommandMenu from "./SlashCommandMenu";
import { planCommand } from "./commandActions";
import type { CommandAction } from "./commandActions";
import {
  acceptedDraft,
  filterCommands,
  isCommandDraft,
  loadCommands,
  parseCommandDraft,
  slashQuery,
} from "./command";
import TaskDetail from "./TaskDetail";
import TaskList from "./TaskList";import type { AssignmentWithEvents, RunWithCalls } from "./TaskDetail";
import UpdateBanner from "./UpdateBanner";
import {
  addMemoryEntry,
  addProject,
  archiveTask,
  approveTaskPlan,
  cancelTask,
  clearChannelMessages,
  clearConversationContext,
  compactConversation,
  completeOnboarding,
  createChannel,
  createGroupConversation,
  createTask,
  decideApproval,
  deleteChannel,
  discardTaskWorktree,
  ensureAgentChannels,
  getConversationContext,
  getOnboardingState,
  getSession,
  getTaskWorktree,
  instantiateAgents,
  listAgents,
  listAssignmentEvents,
  listAssignments,
  listAvailableTools,
  listChannels,
  listConversationSummaries,
  listConversationTurns,
  listMessages,
  listProjectBranches,
  listProjects,
  listRunTasks,
  listRuns,
  listTaskOverview,
  listTasks,
  listTemplates,
  listToolCalls,
  getModelSettings,
  onCowbotEvent,
  onChannelReply,
  onRunFinished,
  pendingApprovals,
  postMessage,
  removeProject,
  renameChannel,
  runTask,
  saveMemoryNote,
  setActiveModel,
  setChannelPinned,
  unarchiveTask,
  updateAgent,
  updateConversationSettings,
} from "./api";
import type { ChannelReplyFinished } from "./api";
import icon from "./assets/cowbot-icon.svg";
import wordmark from "./assets/cowbot-wordmark-dark.svg";
import { errorMessage } from "./errors";
import { dayKey, formatDayLabel, formatListTime, formatTime } from "./format";
import type {
  AgentProfile,
  Approval,
  ApprovalLevel,
  Channel,
  ChannelMessage,
  ClientSession,
  ConversationContext,
  ConversationSummary,
  ConversationTurn,
  CommandInfo,
  ModelSettings,
  OnboardingState,
  Project,
  ProjectBranches,
  RouteSource,
  Task,
  TaskOverviewEntry,
  TaskWorktree,
  TemplateDocument,
  ToolCallDetail,
  ToolDefinition,
  ApprovalRisk,
} from "./types";
import { approvalLevelLabel, isActiveOverviewStatus } from "./types";
import { useUpdater } from "./updater";

type View = "chat" | "tasks" | "settings";

interface PendingReply {
  channelId: string;
  messageId: string;
  agents: AgentProfile[];
}

interface MenuState {
  kind: "agent" | "channel" | "conversation" | "task";
  channel: Channel | null;
  agent: AgentProfile | null;
  task: Task | null;
  x: number;
  y: number;
}

type DialogState =
  | { kind: "rename-channel"; channel: Channel }
  | { kind: "delete-channel"; channel: Channel }
  | { kind: "clear-history"; channel: Channel; agent: AgentProfile }
  | { kind: "edit-agent"; agent: AgentProfile }
  | null;

const broadcastNames = ["team", "all", "everyone", "here"];

function senderName(message: ChannelMessage, session: ClientSession | null): string {
  const sender = message.sender;
  if (sender.type === "System") return "System";
  if (sender.type === "User") {
    return session && sender.id === session.user.id ? "You" : "Teammate";
  }
  return `Agent ${sender.id.slice(0, 8)}`;
}

function senderLabel(
  message: ChannelMessage,
  session: ClientSession | null,
  agents: AgentProfile[],
): string {
  const sender = message.sender;
  if (sender.type === "Agent") {
    return agents.find((agent) => agent.id === sender.id)?.name ?? senderName(message, session);
  }
  return senderName(message, session);
}

function agentName(agents: AgentProfile[], id: string | null): string {
  if (!id) return "Unassigned";
  return agents.find((agent) => agent.id === id)?.name ?? `Agent ${id.slice(0, 8)}`;
}

/** Compact plain text for sidebar previews; strips Markdown and HTML. */
function plainTextPreview(content: string): string {
  return content
    .replace(/```[\s\S]*?```/g, " [code] ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/<[^>]+>/g, " ")
    .replace(/[*_~#>]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/** Preview of the branch a worktree task will create. */
function worktreeBranchPreview(description: string): string {
  let slug = "";
  let pendingDash = false;
  for (const character of description.toLowerCase()) {
    if (/[a-z0-9]/.test(character)) {
      if (pendingDash && slug) slug += "-";
      pendingDash = false;
      slug += character;
      if (slug.length >= 32) break;
    } else if (slug) {
      pendingDash = true;
    }
  }
  const trimmed = slug.replace(/-+$/, "") || "task";
  return `cowbot/${trimmed}-xxxx`;
}

function mentionedNames(content: string): string[] {
  const names: string[] = [];
  for (const match of content.matchAll(/(^|[^\w@])@([\p{L}\p{N}_-]+)/gu)) {
    const name = match[2];
    if (!names.some((existing) => existing.toLowerCase() === name.toLowerCase())) {
      names.push(name);
    }
  }
  return names;
}

/** Agents a group-channel message addresses, mirroring the backend resolver. */
function addressedAgents(content: string, agents: AgentProfile[]): AgentProfile[] {
  const names = mentionedNames(content);
  const addressed: AgentProfile[] = [];
  const add = (agent: AgentProfile) => {
    if (!addressed.some((existing) => existing.name === agent.name)) {
      addressed.push(agent);
    }
  };

  for (const name of names) {
    if (broadcastNames.includes(name.toLowerCase())) {
      agents.forEach(add);
      continue;
    }
    const matched = agents.find(
      (agent) =>
        agent.name.toLowerCase() === name.toLowerCase() ||
        agent.id.toLowerCase() === name.toLowerCase(),
    );
    if (matched) add(matched);
  }
  return addressed;
}

function previewTime(message: ChannelMessage | null | undefined): string {
  return message?.created_at ?? "";
}

function byRecentActivity(
  left: { channel: Channel; agent?: AgentProfile; name?: string },
  right: { channel: Channel; agent?: AgentProfile; name?: string },
  previews: Record<string, ChannelMessage | null>,
): number {
  if (left.channel.pinned !== right.channel.pinned) {
    return left.channel.pinned ? -1 : 1;
  }
  const leftTime = previewTime(previews[left.channel.id]);
  const rightTime = previewTime(previews[right.channel.id]);
  if (leftTime !== rightTime) return leftTime < rightTime ? 1 : -1;
  const leftName = left.agent?.name ?? left.name ?? left.channel.name;
  const rightName = right.agent?.name ?? right.name ?? right.channel.name;
  return leftName.localeCompare(rightName);
}

function PinMark() {
  return (
    <svg
      className="pin-mark"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      aria-hidden="true"
    >
      <path d="M12 16v5" />
      <path d="M8 3h8l-1 6 3 3H6l3-3-1-6z" />
    </svg>
  );
}

export default function App() {
  const [view, setView] = useState<View>("chat");
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("model");
  const [session, setSession] = useState<ClientSession | null>(null);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChannelMessage[]>([]);
  const [previews, setPreviews] = useState<Record<string, ChannelMessage | null>>({});
  const [query, setQuery] = useState("");
  const [tasks, setTasks] = useState<Task[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [taskAssignments, setTaskAssignments] = useState<AssignmentWithEvents[]>([]);
  const [taskRuns, setTaskRuns] = useState<RunWithCalls[]>([]);
  const [taskRunTasks, setTaskRunTasks] = useState<Task[]>([]);
  const [taskWorktree, setTaskWorktree] = useState<TaskWorktree | null>(null);
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [templates, setTemplates] = useState<TemplateDocument[]>([]);
  const [onboarding, setOnboarding] = useState<OnboardingState | null>(null);
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const [commandCatalogue, setCommandCatalogue] = useState<CommandInfo[]>([]);
  const [commandIndex, setCommandIndex] = useState(0);
  const [commandDismissed, setCommandDismissed] = useState(false);
  const [commandHint, setCommandHint] = useState<string | null>(null);
  const [commandExpanded, setCommandExpanded] = useState(false);
  const [contextOpenSignal, setContextOpenSignal] = useState(0);
  const [pendingReply, setPendingReply] = useState<PendingReply | null>(null);
  const [creatingChannel, setCreatingChannel] = useState(false);
  const [channelName, setChannelName] = useState("");
  const [channelTopic, setChannelTopic] = useState("");
  const [creatingConversation, setCreatingConversation] = useState(false);
  const [conversationTitle, setConversationTitle] = useState("");
  const [conversationIds, setConversationIds] = useState<string[]>([]);
  const [conversationDefaultResponder, setConversationDefaultResponder] =
    useState("");
  const [conversationRouter, setConversationRouter] = useState(false);
  const [conversationSummaries, setConversationSummaries] = useState<
    Record<string, ConversationSummary>
  >({});
  const [conversationTurns, setConversationTurns] = useState<
    Record<string, ConversationTurn>
  >({});
  const [replyOutcomes, setReplyOutcomes] = useState<
    Record<string, ChannelReplyFinished>
  >({});
  const [conversationContext, setConversationContext] =
    useState<ConversationContext | null>(null);
  const [contextTarget, setContextTarget] = useState<ContextTarget | null>(null);
  const [contextBusy, setContextBusy] = useState(false);
  const [contextVersion, setContextVersion] = useState(0);
  const [rememberMessage, setRememberMessage] = useState<ChannelMessage | null>(
    null,
  );
  const [notice, setNotice] = useState<string | null>(null);
  const [creatingTask, setCreatingTask] = useState(false);
  const [taskDescription, setTaskDescription] = useState("");
  const [taskOutput, setTaskOutput] = useState("");
  const [taskAssignee, setTaskAssignee] = useState("");
  const [taskChannel, setTaskChannel] = useState("");
  const [taskApproval, setTaskApproval] = useState<ApprovalLevel>("ask");
  const [worktreeEnabled, setWorktreeEnabled] = useState(false);
  const [taskProject, setTaskProject] = useState("");
  const [taskBaseBranch, setTaskBaseBranch] = useState("");
  const [projectBranches, setProjectBranches] = useState<ProjectBranches | null>(null);
  const [projectBranchesBusy, setProjectBranchesBusy] = useState(false);
  const [channelApproval, setChannelApproval] = useState<ApprovalLevel>("ask");
  const [templateId, setTemplateId] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [runningTasks, setRunningTasks] = useState<Set<string>>(() => new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [dialogName, setDialogName] = useState("");
  const [dialogTopic, setDialogTopic] = useState("");
  const [dialogRole, setDialogRole] = useState("");
  const [dialogTools, setDialogTools] = useState<string[]>([]);
  const [availableTools, setAvailableTools] = useState<ToolDefinition[]>([]);
  const [toolCallDetails, setToolCallDetails] = useState<Record<string, ToolCallDetail>>({});
  const [approvalDetails, setApprovalDetails] = useState<Record<string, ApprovalRisk>>({});
  const [modelSettings, setModelSettings] = useState<ModelSettings | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [overview, setOverview] = useState<TaskOverviewEntry[]>([]);
  const [overviewNow, setOverviewNow] = useState(() => Date.now());
  const [inspectorTab, setInspectorTab] = useState<InspectorTab | null>(() => {
    try {
      const stored = window.localStorage.getItem("cowbot.inspector-tab");
      if (
        stored === "approvals" ||
        stored === "background" ||
        stored === "task"
      ) {
        return stored;
      }
      if (stored === "closed") return null;
      if (
        window.localStorage.getItem("cowbot.background-panel") === "open"
      ) {
        return "background";
      }
      return null;
    } catch {
      return null;
    }
  });
  const [conversationsOpen, setConversationsOpen] = useState(() => {
    try {
      return window.localStorage.getItem("cowbot.conversations-open") === "open";
    } catch {
      return false;
    }
  });
  const [modelMenu, setModelMenu] = useState<{ x: number; y: number } | null>(null);  const [dialogError, setDialogError] = useState<string | null>(null);
  const [dialogBusy, setDialogBusy] = useState(false);
  const endRef = useRef<HTMLDivElement | null>(null);
  const channelNameRef = useRef<HTMLInputElement | null>(null);
  const taskDescriptionRef = useRef<HTMLTextAreaElement | null>(null);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const overviewTimer = useRef<number | null>(null);
  const updater = useUpdater();

  const selectedChannel = useMemo(
    () => channels.find((channel) => channel.id === selectedId) ?? null,
    [channels, selectedId],
  );

  const selectedAgent = useMemo(() => {
    if (!selectedChannel) return null;
    const agentId =
      selectedChannel.kind === "conversation"
        ? conversationContext?.agent_id ?? null
        : selectedChannel.agent_id;
    return agentId ? agents.find((agent) => agent.id === agentId) ?? null : null;
  }, [agents, conversationContext, selectedChannel]);

  const selectedTask = useMemo(
    () => tasks.find((task) => task.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
  );

  const availableProjects = useMemo(
    () => projects.filter((project) => project.available),
    [projects],
  );

  const onboardingOpen = onboarding !== null && !onboarding.completed;

  const refreshPreviews = useCallback(async (list: Channel[]) => {
    const entries = await Promise.all(
      list.map(async (channel) => {
        try {
          const [message] = await listMessages(channel.id, 1);
          return [channel.id, message ?? null] as const;
        } catch {
          return [channel.id, null] as const;
        }
      }),
    );
    setPreviews(Object.fromEntries(entries));
  }, []);

  const applyChannels = useCallback(
    (next: Channel[], preferredId?: string) => {
      setChannels(next);
      setSelectedId((current) => {
        if (preferredId && next.some((channel) => channel.id === preferredId)) {
          return preferredId;
        }
        if (current && next.some((channel) => channel.id === current)) {
          return current;
        }
        const firstDirect = next.find((channel) => channel.kind === "direct");
        return (firstDirect ?? next[0])?.id ?? null;
      });
      void refreshPreviews(next);
    },
    [refreshPreviews],
  );

  const refreshChannels = useCallback(
    async (preferredId?: string) => {
      applyChannels(await listChannels(), preferredId);
    },
    [applyChannels],
  );

  const refreshMessages = useCallback(async (channelId: string | null) => {
    if (!channelId) {
      setMessages([]);
      return;
    }
    setMessages(await listMessages(channelId));
  }, []);

  const refreshTasks = useCallback(async (preferredId?: string) => {
    const next = await listTasks();
    setTasks(next);
    setSelectedTaskId((current) => {
      if (preferredId && next.some((task) => task.id === preferredId)) {
        return preferredId;
      }
      if (current && next.some((task) => task.id === current)) {
        return current;
      }
      return next[0]?.id ?? null;
    });
  }, []);

  const refreshTaskDetail = useCallback(async (taskId: string | null) => {
    if (!taskId) {
      setTaskAssignments([]);
      setTaskRuns([]);
      setTaskRunTasks([]);
      setTaskWorktree(null);
      return;
    }

    const [assignments, runs, runTasks, worktree] = await Promise.all([
      listAssignments(taskId),
      listRuns(taskId),
      listRunTasks(taskId),
      getTaskWorktree(taskId),
    ]);
    const [assignmentsWithEvents, runsWithCalls] = await Promise.all([
      Promise.all(
        assignments.map(async (assignment) => ({
          assignment,
          events: await listAssignmentEvents(assignment.id),
        })),
      ),
      Promise.all(
        runs.map(async (run) => ({
          run,
          calls: await listToolCalls(run.id),
        })),
      ),
    ]);
    setTaskAssignments(assignmentsWithEvents);
    setTaskRuns(runsWithCalls);
    setTaskRunTasks(runTasks);
    setTaskWorktree(worktree);
  }, []);

  const refreshApprovals = useCallback(async () => {
    setApprovals(await pendingApprovals());
  }, []);

  const refreshAgents = useCallback(async () => {
    setAgents(await listAgents());
  }, []);

  const refreshOverview = useCallback(async () => {
    setOverview(await listTaskOverview());
  }, []);

  const scheduleOverviewRefresh = useCallback(() => {
    if (overviewTimer.current !== null) {
      window.clearTimeout(overviewTimer.current);
    }
    overviewTimer.current = window.setTimeout(() => {
      overviewTimer.current = null;
      refreshOverview().catch((err) => setError(errorMessage(err)));
    }, 250);
  }, [refreshOverview]);

  useEffect(
    () => () => {
      if (overviewTimer.current !== null) {
        window.clearTimeout(overviewTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    const interval = window.setInterval(
      () => setOverviewNow(Date.now()),
      30_000,
    );
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        "cowbot.inspector-tab",
        inspectorTab ?? "closed",
      );
      window.localStorage.removeItem("cowbot.background-panel");
    } catch {
      // Storage can be unavailable in restricted webviews; the panel still works.
    }
  }, [inspectorTab]);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        "cowbot.conversations-open",
        conversationsOpen ? "open" : "closed",
      );
    } catch {
      // Storage can be unavailable in restricted webviews; the section still works.
    }
  }, [conversationsOpen]);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const [
          nextSession,
          nextChannels,
          nextTasks,
          nextApprovals,
          nextAgents,
          nextTemplates,
          nextOnboarding,
          nextProjects,
          nextOverview,
          nextSummaries,
        ] = await Promise.all([
          getSession(),
          ensureAgentChannels(),
          listTasks(),
          pendingApprovals(),
          listAgents(),
          listTemplates(),
          getOnboardingState(),
          listProjects(),
          listTaskOverview(),
          listConversationSummaries(),
        ]);
        if (cancelled) return;
        setSession(nextSession);
        applyChannels(nextChannels);
        setTasks(nextTasks);
        setSelectedTaskId(nextTasks[0]?.id ?? null);
        setApprovals(nextApprovals);
        setAgents(nextAgents);
        setTemplates(nextTemplates);
        setOnboarding(nextOnboarding);
        setProjects(nextProjects);
        setOverview(nextOverview);
        setConversationSummaries(
          Object.fromEntries(
            nextSummaries.map((summary) => [summary.channel.id, summary]),
          ),
        );
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [applyChannels]);

  useEffect(() => {
    refreshMessages(selectedId).catch((err) => setError(errorMessage(err)));
  }, [refreshMessages, selectedId]);

  useEffect(() => {
    if (!selectedChannel || selectedChannel.kind !== "conversation") {
      setConversationContext(null);
      return;
    }
    let cancelled = false;
    getConversationContext(selectedChannel.id)
      .then((context) => {
        if (!cancelled) setConversationContext(context);
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, [contextVersion, selectedChannel]);

  // A preview belongs to one conversation; switching chats clears it so the
  // inspector never shows another conversation's request.
  useEffect(() => {
    setContextTarget(null);
  }, [selectedId]);

  useEffect(() => {
    refreshTaskDetail(selectedTaskId).catch((err) => setError(errorMessage(err)));
  }, [refreshTaskDetail, selectedTaskId]);

  useEffect(() => {
    if (!selectedChannel || selectedChannel.kind !== "conversation") {
      setConversationTurns({});
      return;
    }
    let cancelled = false;
    listConversationTurns(selectedChannel.id, 100)
      .then((turns) => {
        if (!cancelled) {
          setConversationTurns(
            Object.fromEntries(turns.map((turn) => [turn.message_id, turn])),
          );
        }
      })
      .catch(() => {
        // Route details are supplementary; the thread still renders.
      });
    return () => {
      cancelled = true;
    };
  }, [contextVersion, selectedChannel]);

  useEffect(() => {
    if (conversationIds.length === 0) {
      if (conversationDefaultResponder) setConversationDefaultResponder("");
      return;
    }
    if (conversationIds.includes(conversationDefaultResponder)) return;
    const selected = conversationIds
      .map((id) => agents.find((agent) => agent.id === id) ?? null)
      .filter((agent): agent is AgentProfile => agent !== null);
    const preferred =
      selected.find((agent) => agent.name.toLowerCase() === "main") ?? selected[0];
    setConversationDefaultResponder(preferred?.id ?? "");
  }, [agents, conversationDefaultResponder, conversationIds]);

  useEffect(() => {
    if (!taskAssignee && agents.length > 0) setTaskAssignee(agents[0].id);
  }, [agents, taskAssignee]);

  useEffect(() => {
    if (!templateId && templates.length > 0) setTemplateId(templates[0].id);
  }, [templates, templateId]);

  // The catalogue is fetched once per session. If it fails, slash mode stays
  // off and drafts are sent as ordinary messages; the failure is reported once.
  useEffect(() => {
    let cancelled = false;
    loadCommands()
      .then((catalogue) => {
        if (!cancelled) setCommandCatalogue(catalogue);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setNotice(
          `Slash commands are unavailable; drafts starting with / are sent as messages. ${errorMessage(err)}`,
        );
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!creatingTask || !worktreeEnabled) return;
    if (!taskProject && availableProjects.length > 0) {
      setTaskProject(availableProjects[0].id);
    }
  }, [availableProjects, creatingTask, taskProject, worktreeEnabled]);

  useEffect(() => {
    if (!worktreeEnabled || !taskProject) {
      setProjectBranches(null);
      return;
    }
    let cancelled = false;
    setProjectBranchesBusy(true);
    listProjectBranches(taskProject)
      .then((listing) => {
        if (cancelled) return;
        setProjectBranches(listing);
        setTaskBaseBranch((current) =>
          listing.branches.includes(current)
            ? current
            : listing.current ?? listing.branches[0] ?? "",
        );
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setProjectBranchesBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [taskProject, worktreeEnabled]);

  useEffect(() => {
    listAvailableTools()
      .then((tools) =>
        setAvailableTools(
          [...tools].sort((left, right) => left.name.localeCompare(right.name)),
        ),
      )
      .catch((err) => setError(errorMessage(err)));
  }, []);

  useEffect(() => {
    if (view !== "chat") return;
    let cancelled = false;
    getModelSettings()
      .then((settings) => {
        if (!cancelled) setModelSettings(settings);
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, [view]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    onCowbotEvent((event) => {
      const type = event.payload.type;
      if (
        type === "TaskCreated" ||
        type === "TaskUpdated" ||
        type === "TaskAssigned" ||
        type === "TaskCompleted" ||
        type === "TaskFailed" ||
        type === "AssignmentUpdated" ||
        type === "WorktreeUpdated" ||
        type === "ToolCallRequested" ||
        type === "ToolCallExecuted"
      ) {
        scheduleOverviewRefresh();
      }
      if (type === "ChannelCreated") {
        const channel = event.payload.payload as Channel | undefined;
        if (channel?.agent_id) {
          refreshAgents().catch((err) => setError(errorMessage(err)));
        }
        refreshChannels().catch((err) => setError(errorMessage(err)));
      }
      if (type === "ChannelUpdated" || type === "ChannelDeleted") {
        refreshChannels().catch((err) => setError(errorMessage(err)));
      }
      if (type === "AgentUpdated") {
        refreshAgents().catch((err) => setError(errorMessage(err)));
        refreshChannels().catch((err) => setError(errorMessage(err)));
      }
      if (type === "ToolCallRequested") {
        const payload = event.payload.payload as
          | { call?: { id?: string; name?: string; arguments?: unknown } }
          | undefined;
        const call = payload?.call;
        if (call?.id && call.name) {
          setToolCallDetails((current) => ({
            ...current,
            [call.id as string]: { name: call.name as string, arguments: call.arguments },
          }));
        }
      }
      if (type === "ApprovalRequested") {
        const payload = event.payload.payload as
          | {
              approval?: { id?: string };
              risk?: string;
              reason?: string;
              rule?: { rule_id?: string; description?: string; decision?: string } | null;
            }
          | undefined;
        const approval = payload?.approval;
        if (approval?.id) {
          setApprovalDetails((current) => ({
            ...current,
            [approval.id as string]: {
              risk: payload?.risk ?? "unknown",
              reason: payload?.reason ?? "",
              rule:
                payload?.rule && payload.rule.description
                  ? {
                      rule_id: payload.rule.rule_id ?? "",
                      description: payload.rule.description,
                      decision:
                        (payload.rule.decision as
                          | "allow"
                          | "ask"
                          | "deny") ?? "ask",
                    }
                  : null,
            },
          }));
        }
      }
      if (type === "MessagePosted") {
        const message = event.payload.payload as ChannelMessage | undefined;
        if (message && typeof message.channel_id === "string") {
          setPreviews((current) => ({ ...current, [message.channel_id]: message }));
        }
        setContextVersion((version) => version + 1);
        refreshMessages(selectedId).catch((err) => setError(errorMessage(err)));
      }
      if (
        type === "TaskCreated" ||
        type === "TaskUpdated" ||
        type === "TaskAssigned" ||
        type === "TaskCompleted" ||
        type === "TaskFailed" ||
        type === "AssignmentUpdated" ||
        type === "WorktreeUpdated"
      ) {
        refreshTasks().catch((err) => setError(errorMessage(err)));
        if (selectedTaskId) {
          refreshTaskDetail(selectedTaskId).catch((err) => setError(errorMessage(err)));
        }
      }
      if (type === "ApprovalRequested" || type === "ApprovalDecided") {
        refreshApprovals().catch((err) => setError(errorMessage(err)));
      }
    })
      .then((stop) => {
        if (cancelled) {
          stop();
        } else {
          unlisten = stop;
        }
      })
      .catch((err) => setError(errorMessage(err)));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [
    refreshAgents,
    refreshApprovals,
    refreshChannels,
    refreshMessages,
    refreshTaskDetail,
    refreshTasks,
    scheduleOverviewRefresh,
    selectedId,
    selectedTaskId,
  ]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    onRunFinished((event) => {
      setRunningTasks((current) => {
        const next = new Set(current);
        next.delete(event.task_id);
        return next;
      });
      if (!event.ok && event.error) {
        setError(event.error);
      }
      refreshTasks().catch((err) => setError(errorMessage(err)));
      if (selectedTaskId) {
        refreshTaskDetail(selectedTaskId).catch((err) => setError(errorMessage(err)));
      }
    })
      .then((stop) => {
        if (cancelled) {
          stop();
        } else {
          unlisten = stop;
        }
      })
      .catch((err) => setError(errorMessage(err)));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refreshTaskDetail, refreshTasks, selectedTaskId]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    onChannelReply((event) => {
      setPendingReply((current) =>
        current && current.messageId === event.message_id ? null : current,
      );
      setReplyOutcomes((current) => ({
        ...current,
        [event.message_id]: event,
      }));
      if (event.error) {
        setError(event.error);
      }
      setContextVersion((version) => version + 1);
      refreshMessages(selectedId).catch((err) => setError(errorMessage(err)));
    })
      .then((stop) => {
        if (cancelled) {
          stop();
        } else {
          unlisten = stop;
        }
      })
      .catch((err) => setError(errorMessage(err)));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refreshMessages, selectedId]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [messages, pendingReply]);

  useEffect(() => {
    const composer = composerRef.current;
    if (!composer) return;
    composer.style.height = "auto";
    composer.style.height = `${Math.min(composer.scrollHeight, 160)}px`;
  }, [draft, selectedId]);

  useEffect(() => {
    if (creatingChannel) channelNameRef.current?.focus();
  }, [creatingChannel]);

  useEffect(() => {
    if (creatingTask) taskDescriptionRef.current?.focus();
  }, [creatingTask]);

  const agentRows = useMemo(() => {
    const rows = agents
      .map((agent) => ({
        agent,
        channel:
          channels.find(
            (channel) => channel.kind === "direct" && channel.agent_id === agent.id,
          ) ?? null,
      }))
      .filter(
        (row): row is { agent: AgentProfile; channel: Channel } => row.channel !== null,
      );
    return rows.sort((left, right) => byRecentActivity(left, right, previews));
  }, [agents, channels, previews]);

  const conversationRows = useMemo(() => {
    return channels
      .filter((channel) => channel.kind === "conversation")
      .map((channel) => ({ channel }))
      .sort((left, right) => byRecentActivity(left, right, previews));
  }, [channels, previews]);

  const groupChannels = useMemo(() => {
    return channels
      .filter((channel) => channel.kind === "channel")
      .map((channel) => ({ channel }))
      .sort((left, right) => byRecentActivity(left, right, previews));
  }, [channels, previews]);

  const activeOverviewCount = useMemo(
    () => overview.filter((entry) => isActiveOverviewStatus(entry.status)).length,
    [overview],
  );

  const activeOverviewByChannel = useMemo(() => {
    const byChannel = new Map<string, TaskOverviewEntry[]>();
    for (const entry of overview) {
      if (!entry.channel_id || !isActiveOverviewStatus(entry.status)) continue;
      const entries = byChannel.get(entry.channel_id) ?? [];
      if (entries.length < 2) entries.push(entry);
      byChannel.set(entry.channel_id, entries);
    }
    return byChannel;
  }, [overview]);

  const normalizedQuery = query.trim().toLowerCase();

  const chatRows = useMemo(
    () =>
      [
        ...agentRows.map((row) => ({ ...row, kind: "agent" as const })),
        ...groupChannels.map((row) => ({ ...row, kind: "channel" as const })),
      ].sort((left, right) => byRecentActivity(left, right, previews)),
    [agentRows, groupChannels, previews],
  );

  const visibleChats = useMemo(
    () =>
      normalizedQuery
        ? chatRows.filter((row) =>
            (row.kind === "agent" ? row.agent.name : row.channel.name)
              .toLowerCase()
              .includes(normalizedQuery),
          )
        : chatRows,
    [chatRows, normalizedQuery],
  );

  const visibleConversations = useMemo(
    () =>
      normalizedQuery
        ? conversationRows.filter((row) =>
            row.channel.name.toLowerCase().includes(normalizedQuery),
          )
        : conversationRows,
    [conversationRows, normalizedQuery],
  );

  const showConversations =
    conversationsOpen ||
    creatingConversation ||
    selectedChannel?.kind === "conversation" ||
    (normalizedQuery.length > 0 && visibleConversations.length > 0);

  const channelMentionNames = useMemo(() => {
    const seen = new Set<string>();
    return agents.filter((agent) => {
      const key = agent.name.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [agents]);

  const conversationParticipants = conversationContext?.participants ?? [];

  // Tasks the selected chat is running, for `/stop`. Conversation turns are
  // tasks too, so they are cancelled alongside user tasks.
  const runningTaskIds = useMemo(
    () =>
      tasks
        .filter(
          (task) =>
            task.channel_id === selectedChannel?.id &&
            (task.status === "pending" || task.status === "in_progress"),
        )
        .map((task) => task.id),
    [selectedChannel?.id, tasks],
  );

  const mentionQuery = useMemo(() => {
    const match = draft.match(/(?:^|\s)@([\p{L}\p{N}_-]*)$/u);
    return match ? match[1] : null;
  }, [draft]);

  const commandQuery = useMemo(() => slashQuery(draft), [draft]);

  const commandCandidates = useMemo(() => {
    if (commandQuery === null || commandDismissed) return [];
    // `/help` expands the menu to the whole catalogue regardless of the query.
    if (commandExpanded) return commandCatalogue;
    return filterCommands(commandCatalogue, commandQuery);
  }, [commandCatalogue, commandDismissed, commandExpanded, commandQuery]);

  const activeCommandIndex = Math.min(
    commandIndex,
    Math.max(0, commandCandidates.length - 1),
  );

  const mentionCandidates = useMemo(() => {
    if (mentionQuery === null || mentionDismissed) return [];
    if (!selectedChannel || selectedChannel.kind === "direct") return [];

    const pool: { key: string; name: string; broadcast: boolean }[] = [];
    if (selectedChannel.kind === "conversation" && conversationParticipants.length > 0) {
      for (const participant of conversationParticipants) {
        pool.push({
          key: `agent-${participant.agent_id}`,
          name: participant.agent_name,
          broadcast: false,
        });
      }
    } else if (selectedChannel.kind === "conversation") {
      for (const agent of agents) {
        pool.push({ key: `agent-${agent.id}`, name: agent.name, broadcast: false });
      }
    } else {
      for (const agent of channelMentionNames) {
        pool.push({ key: `agent-${agent.id}`, name: agent.name, broadcast: false });
      }
    }

    const allowBroadcast =
      selectedChannel.kind === "channel" ||
      (selectedChannel.kind === "conversation" &&
        conversationParticipants.length > 1);
    if (allowBroadcast) {
      for (const name of broadcastNames) {
        pool.push({ key: `broadcast-${name}`, name, broadcast: true });
      }
    }

    const query = mentionQuery.toLowerCase();
    const seen = new Set<string>();
    return pool
      .filter((candidate) => {
        const key = candidate.name.toLowerCase();
        if (seen.has(key) || !key.startsWith(query)) return false;
        seen.add(key);
        return true;
      })
      .slice(0, 6);
  }, [
    agents,
    channelMentionNames,
    conversationParticipants,
    mentionDismissed,
    mentionQuery,
    selectedChannel,
  ]);

  const activeMentionIndex = Math.min(
    mentionIndex,
    Math.max(0, mentionCandidates.length - 1),
  );

  useEffect(() => {
    setMentionDismissed(false);
    setMentionIndex(0);
  }, [mentionQuery]);

  const replyingHere = pendingReply !== null && pendingReply.channelId === selectedId;
  const conversationParticipantNames = conversationParticipants.map(
    (participant) => participant.agent_name,
  );
  const defaultResponderName =
    conversationParticipants.find(
      (participant) =>
        participant.agent_id === conversationContext?.default_responder,
    )?.agent_name ?? null;
  const conversationFull =
    selectedChannel?.kind === "conversation" &&
    conversationParticipants.length <= 1 &&
    conversationContext?.state === "full";

  const inlineApprovals = useMemo(
    () =>
      approvals.filter(
        (approval) =>
          tasks.find((task) => task.id === approval.task_id)?.channel_id ===
          selectedId,
      ),
    [approvals, selectedId, tasks],
  );
  const inlineApprovalTool =
    inlineApprovals.length > 0
      ? toolCallDetails[inlineApprovals[0].tool_call_id]?.name
      : null;

  async function handleCreateChannel(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = channelName.trim();
    const topic = channelTopic.trim();
    if (!name) return;

    setError(null);
    try {
      const channel = await createChannel(name, topic);
      setCreatingChannel(false);
      setChannelName("");
      setChannelTopic("");
      setView("chat");
      await refreshChannels(channel.id);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  function openNewConversation() {
    setView("chat");
    setConversationsOpen(true);
    setCreatingConversation(true);
    setError(null);
    if (conversationIds.length === 0 && agents.length > 0) {
      setConversationIds([agents[0].id]);
    }
  }

  function toggleConversationParticipant(agentId: string) {
    setConversationIds((current) => {
      if (current.includes(agentId)) {
        return current.filter((id) => id !== agentId);
      }
      if (current.length >= 8) return current;
      return [...current, agentId];
    });
  }

  function moveConversationParticipant(agentId: string, delta: number) {
    setConversationIds((current) => {
      const index = current.indexOf(agentId);
      const next = index + delta;
      if (index < 0 || next < 0 || next >= current.length) return current;
      const reordered = [...current];
      [reordered[index], reordered[next]] = [reordered[next], reordered[index]];
      return reordered;
    });
  }

  async function handleCreateConversation(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const participants = conversationIds.filter((id) =>
      agents.some((agent) => agent.id === id),
    );
    if (participants.length === 0) return;

    setBusy("create-conversation");
    setError(null);
    try {
      const group = participants.length > 1;
      const channel = await createGroupConversation(
        conversationTitle.trim() || null,
        participants,
        group ? conversationRouter : false,
        group ? conversationDefaultResponder || null : null,
      );
      setCreatingConversation(false);
      setConversationTitle("");
      setConversationIds([]);
      setConversationRouter(false);
      setConversationDefaultResponder("");
      await refreshChannels(channel.id);
      setContextVersion((version) => version + 1);
      listConversationSummaries()
        .then((summaries) =>
          setConversationSummaries(
            Object.fromEntries(
              summaries.map((summary) => [summary.channel.id, summary]),
            ),
          ),
        )
        .catch(() => {
          // Sidebar avatars fall back to channel avatars.
        });
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function compactChannel(channel: Channel, agentId?: string) {
    setContextBusy(true);
    setError(null);
    try {
      const outcome = await compactConversation(channel.id, agentId);
      setNotice(
        outcome.compacted
          ? `Compacted ${outcome.summarized_entries} entries: ${outcome.tokens_before} → ${outcome.tokens_after} tokens.`
          : outcome.reason ??
              (outcome.in_progress
                ? "A compaction is already running for this branch."
                : "Nothing to compact yet."),
      );
      setContextVersion((version) => version + 1);
      await refreshMessages(channel.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setContextBusy(false);
    }
  }

  async function clearContextForChannel(channel: Channel, agentId?: string) {
    const target = agentId
      ? "this participant's context branch"
      : "this conversation's context";
    if (
      !window.confirm(
        `Clear ${target}? Messages stay visible, but the model will no longer see them.`,
      )
    ) {
      return;
    }

    setContextBusy(true);
    setError(null);
    try {
      const cleared = await clearConversationContext(channel.id, agentId);
      setNotice(
        cleared > 0
          ? `Cleared ${cleared} context entries.`
          : "The context was already empty.",
      );
      setContextVersion((version) => version + 1);
      await refreshMessages(channel.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setContextBusy(false);
    }
  }

  async function updateConversationOptions(
    channel: Channel,
    autoCompact: boolean,
    threshold: number,
  ) {
    setError(null);
    try {
      const context = await updateConversationSettings(
        channel.id,
        autoCompact,
        threshold,
      );
      setConversationContext(context);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function updateConversationRouting(
    channel: Channel,
    routerEnabled: boolean,
  ) {
    setError(null);
    try {
      const context = await updateConversationSettings(
        channel.id,
        conversationContext?.auto_compact ?? false,
        conversationContext?.compact_threshold ?? 80,
        routerEnabled,
      );
      setConversationContext(context);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function updateDefaultResponder(channel: Channel, agentId: string) {
    setError(null);
    try {
      const context = await updateConversationSettings(
        channel.id,
        conversationContext?.auto_compact ?? false,
        conversationContext?.compact_threshold ?? 80,
        undefined,
        agentId,
      );
      setConversationContext(context);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  function openMenu(
    event: ReactMouseEvent,
    kind: "agent" | "channel" | "conversation",
    channel: Channel,
    agent: AgentProfile | null = null,
  ) {
    event.preventDefault();
    event.stopPropagation();
    setMenu({ kind, channel, agent, task: null, x: event.clientX, y: event.clientY });
  }

  function openMenuFromButton(
    event: ReactMouseEvent<HTMLButtonElement>,
    kind: "agent" | "channel" | "conversation",
    channel: Channel,
    agent: AgentProfile | null = null,
  ) {
    event.preventDefault();
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    setMenu({ kind, channel, agent, task: null, x: rect.right - 4, y: rect.bottom + 4 });
  }

  function openMenuFromKey(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    kind: "agent" | "channel" | "conversation",
    channel: Channel,
    agent: AgentProfile | null = null,
  ) {
    if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    setMenu({ kind, channel, agent, task: null, x: rect.left + 24, y: rect.bottom + 4 });
  }

  async function togglePin(channel: Channel) {
    setError(null);
    try {
      await setChannelPinned(channel.id, !channel.pinned);
      await refreshChannels();
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function handleToggleArchive(task: Task) {
    setError(null);
    setBusy("archive-task");
    try {
      const updated = task.archived
        ? await unarchiveTask(task.id)
        : await archiveTask(task.id);
      await refreshTasks(updated.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  function openModelMenu(event: ReactMouseEvent<HTMLButtonElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    openModelMenuAt(rect.left, rect.top);
  }

  /// Open the model menu anchored to the composer, for `/model` with no name.
  function openModelMenuAboveComposer() {
    const rect = composerRef.current?.getBoundingClientRect();
    if (!rect) return;
    openModelMenuAt(rect.left, rect.top);
  }

  function openModelMenuAt(left: number, top: number) {
    const modelCount =
      modelSettings?.providers.find((provider) => provider.active)?.models
        .length ?? 1;
    setModelMenu({
      x: left,
      y: Math.max(8, top - 12 - (modelCount + 1) * 34),
    });
  }

  async function handleComposerModel(model: string) {
    const provider = modelSettings?.providers.find((candidate) => candidate.active);
    if (!provider) return;
    setError(null);
    try {
      setModelSettings(await setActiveModel(provider.id, model));
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  function openRenameDialog(channel: Channel) {
    setDialog({ kind: "rename-channel", channel });
    setDialogName(channel.name);
    setDialogTopic(channel.topic);
    setDialogError(null);
  }

  function openDeleteDialog(channel: Channel) {
    setDialog({ kind: "delete-channel", channel });
    setDialogError(null);
  }

  function openClearDialog(channel: Channel, agent: AgentProfile) {
    setDialog({ kind: "clear-history", channel, agent });
    setDialogError(null);
  }

  function closeDialog() {
    if (!dialogBusy) setDialog(null);
  }

  async function submitRename(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!dialog || dialog.kind !== "rename-channel") return;
    const target = dialog.channel;
    setDialogBusy(true);
    setDialogError(null);
    try {
      await renameChannel(target.id, dialogName, dialogTopic);
      setDialog(null);
      await refreshChannels(target.id);
    } catch (err) {
      setDialogError(errorMessage(err));
    } finally {
      setDialogBusy(false);
    }
  }

  async function submitDelete() {
    if (!dialog || dialog.kind !== "delete-channel") return;
    const target = dialog.channel;
    setDialogBusy(true);
    setDialogError(null);
    try {
      await deleteChannel(target.id);
      setDialog(null);
      await refreshChannels();
    } catch (err) {
      setDialogError(errorMessage(err));
    } finally {
      setDialogBusy(false);
    }
  }

  async function submitClear() {
    if (!dialog || dialog.kind !== "clear-history") return;
    const target = dialog.channel;
    setDialogBusy(true);
    setDialogError(null);
    try {
      await clearChannelMessages(target.id);
      setDialog(null);
      await refreshChannels(target.id);
      await refreshMessages(target.id);
    } catch (err) {
      setDialogError(errorMessage(err));
    } finally {
      setDialogBusy(false);
    }
  }

  function openEditAgentDialog(agent: AgentProfile) {
    setDialog({ kind: "edit-agent", agent });
    setDialogName(agent.name);
    setDialogRole(agent.role);
    setDialogTools([...agent.tools]);
    setDialogError(null);
  }

  function toggleTool(name: string) {
    setDialogTools((current) =>
      current.includes(name)
        ? current.filter((tool) => tool !== name)
        : [...current, name],
    );
  }

  async function submitEditAgent(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!dialog || dialog.kind !== "edit-agent") return;
    const target = dialog.agent;
    setDialogBusy(true);
    setDialogError(null);
    try {
      await updateAgent(target.id, dialogName, dialogRole, dialogTools);
      setDialog(null);
      await refreshAgents();
      await refreshChannels();
    } catch (err) {
      setDialogError(errorMessage(err));
    } finally {
      setDialogBusy(false);
    }
  }

  function handleEditAgent(agent: AgentProfile) {
    openEditAgentDialog(agent);
  }

  function acceptCommand(command: CommandInfo) {
    setCommandHint(null);
    setCommandExpanded(false);
    setCommandDismissed(true);
    setCommandIndex(0);
    const next = acceptedDraft(command);
    if (next) {
      setDraft(next);
      composerRef.current?.focus();
      return;
    }
    void runCommand(`/${command.name}`);
  }

  /// Dispatch a complete command line through the pure command planner, then
  /// carry out the action. #186 owns the menu and the parser; this owns what
  /// each command does.
  async function runCommand(line: string) {
    const parsed = await parseCommandDraft(line);
    switch (parsed.kind) {
      case "not_a_command":
        await sendMessage(line);
        return;
      case "unknown":
        setCommandHint(
          "Unknown command. Press Esc to send as a message, or type /help.",
        );
        setCommandDismissed(true);
        return;
      case "missing_argument":
        setCommandHint(`Usage: ${parsed.usage}`);
        return;
      case "invocation": {
        const action = planCommand(
          parsed,
          {
            channel: selectedChannel,
            participants: conversationParticipants.map((participant) => ({
              agentId: participant.agent_id,
              name: participant.agent_name,
            })),
            providers: (modelSettings?.providers ?? []).map((provider) => ({
              id: provider.id,
              name: provider.name,
              active: provider.active,
              model: provider.model,
              models: provider.models,
            })),
            approval: channelApproval,
          },
          runningTaskIds,
        );
        // A notice keeps the draft so the user can correct an argument; a
        // command that did something clears it.
        if (action.kind === "notice" || action.kind === "error") {
          setNotice(null);
          setError(action.kind === "error" ? action.message : null);
          setCommandHint(action.message);
          setCommandDismissed(true);
          return;
        }
        setCommandHint(null);
        setCommandDismissed(false);
        setCommandExpanded(false);
        setDraft("");
        await applyCommandAction(action);
        return;
      }
    }
  }

  async function applyCommandAction(action: Exclude<CommandAction, { kind: "notice" } | { kind: "error" }>) {
    switch (action.kind) {
      case "help":
        setCommandDismissed(false);
        setCommandExpanded(true);
        setCommandIndex(0);
        composerRef.current?.focus();
        return;

      case "open-new-conversation":
        openNewConversation();
        if (action.participants.length > 0) {
          setConversationIds(action.participants);
        }
        return;

      case "open-delete": {
        const channel = channels.find((candidate) => candidate.id === action.channelId);
        if (channel) openDeleteDialog(channel);
        return;
      }

      case "open-note":
        // The conversation note lives in the context popover; open it there
        // rather than duplicating the editor in the composer.
        setContextOpenSignal((value) => value + 1);
        return;

      case "open-settings":
        setSettingsSection(action.section);
        setView("settings");
        return;

      case "open-model-menu":
        openModelMenuAboveComposer();
        return;

      case "open-task":
        setTaskDescription(action.description);
        startTaskForCurrentChannel();
        return;

      case "set-approval": {
        if (
          action.mode === "trusted" &&
          !window.confirm(
            "Bypass permissions runs every tool call. Critical-risk calls follow the workspace policy in Settings → Approvals. Continue?",
          )
        ) {
          return;
        }
        setChannelApproval(action.mode);
        setNotice(`Approval mode set to ${approvalLevelLabel(action.mode)}.`);
        return;
      }

      case "compact": {
        const channel = channels.find((candidate) => candidate.id === action.channelId);
        if (channel) await compactChannel(channel, action.agentId ?? undefined);
        return;
      }

      case "clear": {
        const channel = channels.find((candidate) => candidate.id === action.channelId);
        if (channel) await clearContextForChannel(channel, action.agentId ?? undefined);
        return;
      }

      case "rename": {
        setError(null);
        try {
          await renameChannel(action.channelId, action.name, action.topic);
          await refreshChannels(action.channelId);
          setNotice(`Renamed to ${action.name}.`);
        } catch (err) {
          setError(errorMessage(err));
        }
        return;
      }

      case "pin": {
        setError(null);
        try {
          await setChannelPinned(action.channelId, action.pinned);
          await refreshChannels(action.channelId);
          setNotice(action.pinned ? "Chat pinned." : "Chat unpinned.");
        } catch (err) {
          setError(errorMessage(err));
        }
        return;
      }

      case "stop": {
        if (
          !window.confirm(
            `Cancel ${action.taskIds.length} running ${action.taskIds.length === 1 ? "task" : "tasks"} in this chat?`,
          )
        ) {
          return;
        }
        setError(null);
        let cancelled = 0;
        for (const taskId of action.taskIds) {
          try {
            await cancelTask(taskId, "Cancelled from the composer");
            cancelled += 1;
          } catch (err) {
            setError(errorMessage(err));
          }
        }
        await refreshTasks(selectedChannel?.id ?? undefined);
        setNotice(
          cancelled > 0
            ? `Cancelled ${cancelled} ${cancelled === 1 ? "task" : "tasks"}.`
            : "No tasks were cancelled.",
        );
        return;
      }

      case "remember": {
        setError(null);
        try {
          const saved = await addMemoryEntry(action.content, "note", {
            agentId: action.agentId ?? undefined,
            channelId: action.channelId ?? undefined,
          });
          const scope = action.channelId
            ? "this conversation"
            : action.agentId
              ? "this agent"
              : "the workspace";
          setContextVersion((version) => version + 1);
          setNotice(
            saved.redacted
              ? `Saved to ${scope} with credentials redacted.`
              : `Saved to ${scope}.`,
          );
        } catch (err) {
          setError(errorMessage(err));
        }
        return;
      }

      case "save-note": {
        setError(null);
        try {
          const saved = await saveMemoryNote(action.content, action.channelId);
          setContextVersion((version) => version + 1);
          setNotice(
            saved.redacted
              ? "Conversation note saved with credentials redacted."
              : "Conversation note saved.",
          );
        } catch (err) {
          setError(errorMessage(err));
        }
        return;
      }
    }
  }

  async function sendCommandDraft() {
    const line = draft.trim();
    if (!line) return;
    setSending(true);
    setError(null);
    try {
      await runCommand(line);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSending(false);
    }
  }

  async function sendMessage(text: string = draft) {
    if (sending) return false;
    const content = text.trim();
    if (!content || !selectedChannel || conversationFull) return false;

    setSending(true);
    setError(null);
    try {
      const posted = await postMessage(selectedChannel.id, content, channelApproval);
      const conversationAgents =
        selectedChannel.kind === "conversation" && conversationContext
          ? conversationContext.participants
              .map((participant) =>
                agents.find((agent) => agent.id === participant.agent_id),
              )
              .filter((agent): agent is AgentProfile => agent !== undefined)
          : [];
      const recipients =
        conversationAgents.length > 0
          ? conversationAgents
          : selectedAgent
            ? [selectedAgent]
            : addressedAgents(posted.content, agents);
      setPendingReply(
        recipients.length > 0
          ? { channelId: selectedChannel.id, messageId: posted.id, agents: recipients }
          : null,
      );
      setDraft("");
      // The sent message is now in the branch, so a captured draft must not be
      // appended a second time by the context preview.
      setContextTarget((current) =>
        current && current.kind === "conversation" && current.draft
          ? { ...current, draft: undefined }
          : current,
      );
      setPreviews((current) => ({ ...current, [selectedChannel.id]: posted }));
      await refreshMessages(selectedChannel.id);
      return true;
    } catch (err) {
      setError(errorMessage(err));
      return false;
    } finally {
      setSending(false);
    }
  }

  function acceptMention(name: string) {
    setDraft((current) =>
      current.replace(
        /(^|\s)@[\p{L}\p{N}_-]*$/u,
        (_match, prefix: string) => `${prefix}@${name} `,
      ),
    );
    setMentionDismissed(true);
    setMentionIndex(0);
    composerRef.current?.focus();
  }

  function handleComposerKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (commandCandidates.length > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setCommandIndex((current) => (current + 1) % commandCandidates.length);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setCommandIndex(
          (current) =>
            (current - 1 + commandCandidates.length) % commandCandidates.length,
        );
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        const candidate = commandCandidates[activeCommandIndex];
        if (candidate) acceptCommand(candidate);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setCommandDismissed(true);
        setCommandExpanded(false);
        setCommandHint(null);
        return;
      }
    }
    if (mentionCandidates.length > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setMentionIndex((current) => (current + 1) % mentionCandidates.length);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setMentionIndex(
          (current) =>
            (current - 1 + mentionCandidates.length) % mentionCandidates.length,
        );
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        const candidate = mentionCandidates[activeMentionIndex];
        if (candidate) acceptMention(candidate.name);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setMentionDismissed(true);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      if (conversationFull) return;
      // After Escape or any dispatch failure the draft is a slash line that is
      // not a catalogue command, and the user chose to send it as a message.
      if (commandCatalogue.length > 0 && isCommandDraft(draft)) {
        void sendCommandDraft();
        return;
      }
      void sendMessage();
    }
  }

  function openNewChannel() {
    setView("chat");
    setCreatingChannel(true);
    setError(null);
  }

  function openChannel(channelId: string) {
    setSelectedId(channelId);
    setView("chat");
  }

  function openTask(taskId: string) {
    setView("chat");
    setCreatingTask(false);
    setSelectedTaskId(taskId);
    setInspectorTab("task");
  }

  function toggleInspector(tab: InspectorTab) {
    setInspectorTab((current) => (current === tab ? null : tab));
  }

  function inspectConversationContext(agentId: string) {
    if (!selectedChannel) return;
    const participant = conversationParticipants.find(
      (candidate) => candidate.agent_id === agentId,
    );
    setContextTarget({
      kind: "conversation",
      channelId: selectedChannel.id,
      agentId,
      agentName: participant?.agent_name ?? "",
      title: selectedChannel.name,
      draft: draft.trim() ? draft.trim() : undefined,
      planMode: channelApproval === "plan_only",
    });
    setInspectorTab("context");
  }

  function inspectRunContext(runId: string) {
    setContextTarget({ kind: "run", runId });
    setInspectorTab("context");
  }

  function startCreatingTask() {
    setInspectorTab(null);
    setView("tasks");
    setCreatingTask(true);
    setError(null);
  }

  function startTaskForCurrentChannel() {
    if (!selectedChannel) return;
    setTaskChannel(selectedChannel.id);
    if (selectedAgent) setTaskAssignee(selectedAgent.id);
    startCreatingTask();
  }

  async function handleCreateTask(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const description = taskDescription.trim();
    const expectedOutput = taskOutput.trim();
    if (!description || !expectedOutput || !taskAssignee) return;

    setBusy("create-task");
    setError(null);
    try {
      const task = await createTask({
        description,
        expectedOutput,
        assignee: taskAssignee,
        channelId: taskChannel || null,
        approvalLevel: taskApproval,
        worktree:
          worktreeEnabled && taskProject && taskBaseBranch
            ? { projectId: taskProject, baseBranch: taskBaseBranch }
            : null,
      });
      setCreatingTask(false);
      setTaskDescription("");
      setTaskOutput("");
      setTaskChannel("");
      setWorktreeEnabled(false);
      setTaskProject("");
      setTaskBaseBranch("");
      setProjectBranches(null);
      await refreshTasks(task.id);
      setSelectedTaskId(task.id);
      setView("chat");
      setInspectorTab("task");
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function instantiateTemplate(templateId: string) {
    const added = await instantiateAgents(templateId);
    await refreshAgents();
    const nextChannels = await ensureAgentChannels();
    const preferred = added[0]
      ? nextChannels.find((channel) => channel.agent_id === added[0].id)?.id
      : undefined;
    applyChannels(nextChannels, preferred);
    if (added[0]) setTaskAssignee(added[0].id);
    return added;
  }

  async function handleAddAgents() {
    if (!templateId) return;
    setBusy("add-agents");
    setError(null);
    try {
      await instantiateTemplate(templateId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleAddProject(path: string) {
    setBusy("add-project");
    setError(null);
    try {
      setProjects(await addProject(path));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleRemoveProject(project: Project) {
    setBusy("remove-project");
    setError(null);
    try {
      setProjects(await removeProject(project.id));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleCompleteOnboarding() {
    const next = await completeOnboarding();
    setOnboarding(next);
  }

  async function handleCancelTask() {
    if (!selectedTaskId) return;
    if (!window.confirm("Cancel this task and its active assignment?")) return;

    setBusy("cancel-task");
    setError(null);
    try {
      await cancelTask(selectedTaskId, "Cancelled in desktop");
      await refreshTasks();
      await refreshTaskDetail(selectedTaskId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDiscardWorktree() {
    if (!selectedTaskId) return;

    setBusy("discard-worktree");
    setError(null);
    try {
      await discardTaskWorktree(selectedTaskId);
      setNotice("Worktree discarded.");
      await refreshTasks();
      await refreshTaskDetail(selectedTaskId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleRunTask(
    taskId: string,
    approvalLevel: ApprovalLevel = taskApproval,
  ) {
    setError(null);
    setRunningTasks((current) => new Set(current).add(taskId));
    try {
      await runTask(taskId, approvalLevel);
      await refreshTasks();
    } catch (err) {
      setRunningTasks((current) => {
        const next = new Set(current);
        next.delete(taskId);
        return next;
      });
      setError(errorMessage(err));
    }
  }

  async function handleApprovePlan(taskId: string) {
    setBusy("approve-plan");
    setError(null);
    try {
      await approveTaskPlan(taskId);
      setTaskApproval("ask");
      setChannelApproval("ask");
      await handleRunTask(taskId, "ask");
      await refreshTasks();
      if (selectedTaskId) await refreshTaskDetail(selectedTaskId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDecideApproval(
    approval: Approval,
    approved: boolean,
    forSession = false,
  ) {
    setBusy(approval.id);
    setError(null);
    try {
      await decideApproval(approval.id, approved, forSession);
      await refreshApprovals();
      await refreshTasks();
      if (selectedTaskId) await refreshTaskDetail(selectedTaskId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  const composerTarget = selectedAgent
    ? selectedAgent.name
    : selectedChannel
      ? `#${selectedChannel.name}`
      : "your team";

  function previewSnippet(message: ChannelMessage, channel: Channel): string {
    const text = plainTextPreview(message.content);
    const fromOwnAgent =
      channel.agent_id !== null &&
      message.sender.type === "Agent" &&
      message.sender.id === channel.agent_id;
    return fromOwnAgent
      ? text
      : `${senderLabel(message, session, agents)}: ${text}`;
  }

  function participantLabel(agentId: string): string {
    return (
      conversationParticipants.find(
        (participant) => participant.agent_id === agentId,
      )?.agent_name ?? agentName(agents, agentId)
    );
  }

  /**
   * Reload-safe Answered-by line and routing details for one user message.
   *
   * The persisted turn supplies history after a restart; the reply event adds
   * per-agent outcomes from this session.
   */
  function renderRouteLine(message: ChannelMessage) {
    if (conversationParticipants.length <= 1) return null;
    const turn = conversationTurns[message.id];
    const outcome = replyOutcomes[message.id];
    if (!turn && !outcome) return null;

    const source: RouteSource | null =
      turn?.source ?? ((outcome?.route as RouteSource | null) ?? null);
    const selected = outcome?.selected.length
      ? outcome.selected
      : turn?.selected_agents ?? [];
    const replied = outcome?.replied ?? [];
    const failed = outcome?.failed ?? [];
    const skipped = outcome?.skipped ?? [];
    const answered = replied.length > 0 ? replied : selected;
    const routeLabel =
      source === "mention"
        ? "by mention"
        : source === "router"
          ? "by routing"
          : source === "default"
            ? "by the default responder"
            : "";

    if (source === "silent") {
      return (
        <li className="chat-route-row">
          <span className="route-note">
            No one answered. Mention a participant or set a default responder.
          </span>
        </li>
      );
    }
    if (source === "unmatched") {
      const unknown = turn?.unknown_mentions ?? [];
      return (
        <li className="chat-route-row">
          <span className="route-note route-note-warning">
            No participant matched{" "}
            {unknown.length > 0
              ? unknown.map((name) => `@${name}`).join(", ")
              : "the mention"}
            .
          </span>
        </li>
      );
    }
    if (answered.length === 0 && failed.length === 0 && skipped.length === 0) {
      return null;
    }

    return (
      <li className="chat-route-row">
        <span className="route-answered">
          {answered.length > 0 ? (
            <>
              Answered {routeLabel} by{" "}
              <strong>{answered.map(participantLabel).join(", ")}</strong>
            </>
          ) : (
            "No reply was posted."
          )}
        </span>
        {failed.length > 0 ? (
          <span className="route-chip route-chip-failed">
            {failed.map(participantLabel).join(", ")} failed
          </span>
        ) : null}
        {skipped.length > 0 ? (
          <span className="route-chip">
            {skipped.map(participantLabel).join(", ")} skipped
          </span>
        ) : null}
        {turn ? (
          <details className="route-details">
            <summary>Routing</summary>
            <div className="route-details-body">
              <p>
                <span>Source</span>
                {source ?? "unknown"}
              </p>
              {turn.selected_agents.length > 0 ? (
                <p>
                  <span>Selected</span>
                  {turn.selected_agents.map(participantLabel).join(", ")}
                </p>
              ) : null}
              {turn.unknown_mentions.length > 0 ? (
                <p>
                  <span>Not matched</span>
                  {turn.unknown_mentions.map((name) => `@${name}`).join(", ")}
                </p>
              ) : null}
              {turn.reason ? (
                <p>
                  <span>Reason</span>
                  {turn.reason}
                </p>
              ) : null}
              {outcome ? (
                <p>
                  <span>Outcome</span>
                  {replied.length} replied · {failed.length} failed ·{" "}
                  {skipped.length} skipped
                </p>
              ) : null}
            </div>
          </details>
        ) : null}
      </li>
    );
  }

  const renderedMessages = useMemo(() => {
    let lastDay = "";
    return messages.map((message) => {
      const day = dayKey(message.created_at);
      const separator = day !== lastDay ? day : null;
      lastDay = day;
      return { message, separator };
    });
  }, [messages]);

  const menuItems: ContextMenuItem[] = (() => {
    if (!menu) return [];

    const channel = menu.channel;
    if (!channel) return [];
    const menuAgent = menu.agent;
    return menu.kind === "agent"
      ? [
          ...(menuAgent
            ? [
                {
                  label: "Edit agent…",
                  onSelect: () => openEditAgentDialog(menuAgent),
                },
              ]
            : []),
          {
            label: channel.pinned ? "Unpin" : "Pin to top",
            onSelect: () => void togglePin(channel),
          },
          ...(menuAgent
            ? [
                {
                  label: "Clear history…",
                  danger: true,
                  onSelect: () => openClearDialog(channel, menuAgent),
                },
              ]
            : []),
        ]
      : menu.kind === "conversation"
        ? [
            {
              label: "Rename conversation…",
              onSelect: () => openRenameDialog(channel),
            },
            {
              label: channel.pinned ? "Unpin" : "Pin to top",
              onSelect: () => void togglePin(channel),
            },
            {
              label: "Compact context",
              onSelect: () => void compactChannel(channel),
            },
            {
              label: "Clear context…",
              onSelect: () => void clearContextForChannel(channel),
            },
            {
              label: "Delete conversation…",
              danger: true,
              onSelect: () => openDeleteDialog(channel),
            },
          ]
        : [
            {
              label: "Rename channel…",
              onSelect: () => openRenameDialog(channel),
            },
            {
              label: channel.pinned ? "Unpin" : "Pin to top",
              onSelect: () => void togglePin(channel),
            },
            {
              label: "Delete channel…",
              danger: true,
              onSelect: () => openDeleteDialog(channel),
            },
          ];
  })();

  const activeModelProvider =
    modelSettings?.providers.find((provider) => provider.active) ?? null;
  const activeModelLabel = activeModelProvider?.model ?? "Mock model";
  const modelMenuItems: ContextMenuItem[] = modelMenu
    ? [
        ...(activeModelProvider
          ? activeModelProvider.models.map((model) => ({
              label: `${model === activeModelProvider.model ? "✓ " : ""}${model}`,
              onSelect: () => void handleComposerModel(model),
            }))
          : [
              {
                label: "Mock model (offline)",
                onSelect: () => undefined,
              },
            ]),
        {
          label: "Manage models…",
          onSelect: () => {
            setModelMenu(null);
            setSettingsSection("model");
            setView("settings");
          },
        },
      ]
    : [];

  const taskInspectorPanel = selectedTask ? (
    <TaskDetail
      task={selectedTask}
      assignments={taskAssignments}
      runs={taskRuns}
      runTasks={taskRunTasks}
      worktree={taskWorktree}
      discardingWorktree={busy === "discard-worktree"}
      onDiscardWorktree={() => void handleDiscardWorktree()}
      onOpenTask={openTask}
      onInspectRun={inspectRunContext}
      cancelling={busy === "cancel-task"}
      onCancel={handleCancelTask}
      running={runningTasks.has(selectedTask.id)}
      onRun={() => handleRunTask(selectedTask.id)}
      onApprovePlan={() => void handleApprovePlan(selectedTask.id)}
      approvingPlan={busy === "approve-plan"}
      onToggleArchive={() => void handleToggleArchive(selectedTask)}
      archiveBusy={busy === "archive-task"}
      onBack={() => setSelectedTaskId(null)}
    />
  ) : (
    <TaskList
      tasks={tasks}
      agents={agents}
      onOpenTask={openTask}
      onNewTask={startCreatingTask}
    />
  );

  const contextInspectorPanel = (
    <ContextInspector target={contextTarget} version={contextVersion} />
  );

  return (
    <div className={`app${inspectorTab ? " with-inspector" : ""}`}>
      <aside className="sidebar" inert={onboardingOpen}>
        <div className="sidebar-head">
          <img className="wordmark" src={wordmark} alt="CowBot" />
          <div className="sidebar-head-actions">
            <button
              type="button"
              className={`icon-button${view === "chat" ? " is-active" : ""}`}
              title="Chat"
              aria-label="Chat"
              aria-current={view === "chat" ? "page" : undefined}
              onClick={() => setView("chat")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.7"
                aria-hidden="true"
              >
                <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z" />
              </svg>
            </button>
            <button
              type="button"
              className={`icon-button${inspectorTab === "task" ? " is-active" : ""}`}
              title="Tasks"
              aria-label="Tasks"
              aria-pressed={inspectorTab === "task"}
              onClick={() =>
                setInspectorTab((current) => (current === "task" ? null : "task"))
              }
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.7"
                aria-hidden="true"
              >
                <path d="M9 6h11M9 12h11M9 18h11" />
                <path d="M4 6l1.5 1.5L8 5" />
                <path d="M4 12l1.5 1.5L8 11" />
                <path d="M4 18l1.5 1.5L8 17" />
              </svg>
            </button>
            <button
              type="button"
              className="icon-button"
              title="New channel"
              aria-label="New channel"
              onClick={openNewChannel}
            >
              +
            </button>
          </div>
        </div>

        {view !== "settings" ? (
          <label className="search-field">
            <svg
              className="search-icon"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.8"
              aria-hidden="true"
            >
              <circle cx="11" cy="11" r="6.5" />
              <path d="m16 16 4 4" />
            </svg>
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search chats and conversations"
              aria-label="Search chats and conversations"
            />
          </label>
        ) : null}

        <div className="sidebar-scroll">
          {view !== "settings" ? (
            <>
              <section className="sidebar-section" aria-label="Chats">
                <div className="nav-heading">
                  <span>Chats</span>
                  <button type="button" className="text-button" onClick={openNewChannel}>
                    New
                  </button>
                </div>
                {chatRows.length === 0 ? (
                  <p className="nav-note">
                    Add a team from a template in Settings to start a conversation.
                  </p>
                ) : visibleChats.length === 0 ? (
                  <p className="nav-note">No chats match.</p>
                ) : (
                  <ul className="contact-list">
                    {visibleChats.map((row) => {
                      const preview = previews[row.channel.id] ?? null;
                      const isAgent = row.kind === "agent";
                      const name = isAgent ? row.agent.name : row.channel.name;
                      return (
                        <li key={row.channel.id} className="contact-item">
                          <button
                            type="button"
                            className="contact-row"
                            aria-current={row.channel.id === selectedId ? "page" : undefined}
                            onClick={() => openChannel(row.channel.id)}
                            onContextMenu={(event) =>
                              isAgent
                                ? openMenu(event, "agent", row.channel, row.agent)
                                : openMenu(event, "channel", row.channel)
                            }
                            onKeyDown={(event) =>
                              isAgent
                                ? openMenuFromKey(event, "agent", row.channel, row.agent)
                                : openMenuFromKey(event, "channel", row.channel)
                            }
                          >
                            <Avatar name={name} kind={isAgent ? "agent" : "channel"} />
                            <span className="contact-text">
                              <span className="contact-top">
                                <span className="contact-name">
                                  {row.channel.pinned ? <PinMark /> : null}
                                  {name}
                                </span>
                                <time className="contact-time">
                                  {preview ? formatListTime(preview.created_at) : ""}
                                </time>
                              </span>
                              <span className="contact-snippet">
                                {preview
                                  ? previewSnippet(preview, row.channel)
                                  : isAgent
                                    ? row.agent.role
                                    : row.channel.topic || "No messages yet"}
                              </span>
                            </span>
                          </button>
                          <button
                            type="button"
                            className="contact-more"
                            title={isAgent ? "Conversation actions" : "Channel actions"}
                            aria-label={`Actions for ${name}`}
                            onClick={(event) =>
                              isAgent
                                ? openMenuFromButton(event, "agent", row.channel, row.agent)
                                : openMenuFromButton(event, "channel", row.channel)
                            }
                          >
                            ⋯
                          </button>
                          {!isAgent &&
                          (activeOverviewByChannel.get(row.channel.id) ?? []).length > 0 ? (
                            <div className="background-cards">
                              {(activeOverviewByChannel.get(row.channel.id) ?? []).map(
                                (entry) => (
                                  <BackgroundTaskCard
                                    key={entry.run_id}
                                    entry={entry}
                                    now={overviewNow}
                                    onOpen={() => openTask(entry.run_id)}
                                  />
                                ),
                              )}
                            </div>
                          ) : null}
                        </li>
                      );
                    })}
                  </ul>
                )}

                {creatingChannel ? (
                  <form className="new-channel" onSubmit={handleCreateChannel}>
                    <label className="field">
                      <span>Name</span>
                      <input
                        ref={channelNameRef}
                        value={channelName}
                        onChange={(event) => setChannelName(event.target.value)}
                        placeholder="engineering"
                        autoComplete="off"
                      />
                    </label>
                    <label className="field">
                      <span>Topic</span>
                      <input
                        value={channelTopic}
                        onChange={(event) => setChannelTopic(event.target.value)}
                        placeholder="What is this channel for?"
                        autoComplete="off"
                      />
                    </label>
                    <div className="form-actions">
                      <button
                        type="submit"
                        className="primary-button"
                        disabled={!channelName.trim()}
                      >
                        Create channel
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        onClick={() => setCreatingChannel(false)}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                ) : (
                  <button
                    type="button"
                    className="new-channel-button"
                    onClick={openNewChannel}
                  >
                    <span aria-hidden="true">+</span> New channel
                  </button>
                )}
              </section>

              <section className="sidebar-section" aria-label="Conversations">
                <div className="nav-heading">
                  <button
                    type="button"
                    className="nav-disclosure"
                    aria-expanded={showConversations}
                    aria-controls="sidebar-conversations"
                    onClick={() => setConversationsOpen((open) => !open)}
                  >
                    <span
                      className={`disclosure-mark${showConversations ? " is-open" : ""}`}
                      aria-hidden="true"
                    >
                      ▸
                    </span>
                    <span>Conversations</span>
                    {conversationRows.length > 0 ? (
                      <span className="nav-count-soft">{conversationRows.length}</span>
                    ) : null}
                  </button>
                  <button
                    type="button"
                    className="text-button"
                    onClick={openNewConversation}
                    disabled={agents.length === 0}
                  >
                    New
                  </button>
                </div>
                <div id="sidebar-conversations" hidden={!showConversations}>
                {visibleConversations.length > 0 ? (
                  <ul className="contact-list">
                    {visibleConversations.map(({ channel }) => {
                      const preview = previews[channel.id] ?? null;
                      return (
                        <li key={channel.id} className="contact-item">
                          <button
                            type="button"
                            className="contact-row"
                            aria-current={channel.id === selectedId ? "page" : undefined}
                            onClick={() => openChannel(channel.id)}
                            onContextMenu={(event) =>
                              openMenu(event, "conversation", channel)
                            }
                            onKeyDown={(event) =>
                              openMenuFromKey(event, "conversation", channel)
                            }
                          >
                            <GroupAvatar
                              names={(conversationSummaries[channel.id]?.participants ?? []).map(
                                (participant) => participant.name,
                              )}
                            />
                            <span className="contact-text">
                              <span className="contact-top">
                                <span className="contact-name">
                                  {channel.pinned ? <PinMark /> : null}
                                  {channel.name}
                                </span>
                                <time className="contact-time">
                                  {preview ? formatListTime(preview.created_at) : ""}
                                </time>
                              </span>
                              <span className="contact-snippet">
                                {preview
                                  ? previewSnippet(preview, channel)
                                  : channel.topic || "New conversation"}
                              </span>
                            </span>
                          </button>
                          <button
                            type="button"
                            className="contact-more"
                            title="Conversation actions"
                            aria-label={`Actions for ${channel.name}`}
                            onClick={(event) =>
                              openMenuFromButton(event, "conversation", channel)
                            }
                          >
                            ⋯
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                ) : normalizedQuery && conversationRows.length > 0 ? (
                  <p className="nav-note">No conversations match.</p>
                ) : null}

                {creatingConversation ? (
                  <form className="new-channel" onSubmit={handleCreateConversation}>
                    <label className="field">
                      <span>Title</span>
                      <input
                        value={conversationTitle}
                        onChange={(event) => setConversationTitle(event.target.value)}
                        placeholder="Design review"
                        autoComplete="off"
                      />
                    </label>
                    <fieldset className="participant-picker">
                      <legend>
                        Participants <span>{conversationIds.length}/8</span>
                      </legend>
                      <ul>
                        {agents.map((agent) => {
                          const order = conversationIds.indexOf(agent.id);
                          const selected = order >= 0;
                          return (
                            <li key={agent.id}>
                              <button
                                type="button"
                                className={`participant-option${
                                  selected ? " is-selected" : ""
                                }`}
                                aria-pressed={selected}
                                disabled={!selected && conversationIds.length >= 8}
                                onClick={() =>
                                  toggleConversationParticipant(agent.id)
                                }
                              >
                                <Avatar
                                  name={agent.name}
                                  kind="agent"
                                  size="small"
                                />
                                <span className="participant-name">
                                  {agent.name}
                                </span>
                                {selected ? (
                                  <span className="participant-order">
                                    {order + 1}
                                  </span>
                                ) : (
                                  <span className="participant-add">+</span>
                                )}
                              </button>
                              {selected ? (
                                <span className="participant-move">
                                  <button
                                    type="button"
                                    aria-label={`Move ${agent.name} earlier`}
                                    disabled={order === 0}
                                    onClick={() =>
                                      moveConversationParticipant(agent.id, -1)
                                    }
                                  >
                                    ↑
                                  </button>
                                  <button
                                    type="button"
                                    aria-label={`Move ${agent.name} later`}
                                    disabled={
                                      order === conversationIds.length - 1
                                    }
                                    onClick={() =>
                                      moveConversationParticipant(agent.id, 1)
                                    }
                                  >
                                    ↓
                                  </button>
                                </span>
                              ) : null}
                            </li>
                          );
                        })}
                      </ul>
                    </fieldset>
                    {conversationIds.length > 1 ? (
                      <>
                        <label className="field">
                          <span>Default responder</span>
                          <select
                            value={conversationDefaultResponder}
                            onChange={(event) =>
                              setConversationDefaultResponder(event.target.value)
                            }
                          >
                            {conversationIds.map((id) => {
                              const agent = agents.find(
                                (candidate) => candidate.id === id,
                              );
                              return agent ? (
                                <option key={id} value={id}>
                                  {agent.name}
                                </option>
                              ) : null;
                            })}
                          </select>
                        </label>
                        <label className="context-toggle">
                          <input
                            type="checkbox"
                            checked={conversationRouter}
                            onChange={(event) =>
                              setConversationRouter(event.target.checked)
                            }
                          />
                          <span>Route unaddressed messages with the model</span>
                        </label>
                      </>
                    ) : null}
                    <div className="form-actions">
                      <button
                        type="submit"
                        className="primary-button"
                        disabled={
                          busy === "create-conversation" ||
                          conversationIds.length === 0
                        }
                      >
                        {busy === "create-conversation" ? "Creating…" : "Create"}
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        onClick={() => setCreatingConversation(false)}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                ) : (
                  <button
                    type="button"
                    className="new-channel-button"
                    onClick={openNewConversation}
                    disabled={agents.length === 0}
                  >
                    <span aria-hidden="true">+</span> New conversation
                  </button>
                )}
                </div>
              </section>



            </>
          ) : (
            <section className="sidebar-section" aria-label="Settings sections">
              <div className="nav-heading">
                <span>Settings</span>
              </div>
              <ul className="contact-list">
                {settingsSections.map((item) => (
                  <li key={item.id}>
                    <button
                      type="button"
                      className="contact-row"
                      aria-current={
                        settingsSection === item.id ? "page" : undefined
                      }
                      onClick={() => setSettingsSection(item.id)}
                    >
                      <span className="contact-text">
                        <span className="contact-top">
                          <span className="contact-name">{item.label}</span>
                        </span>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>

        <div className="sidebar-footer">
          <div className="profile">
            <Avatar name={session?.user.name ?? "You"} kind="user" />
            <span className="profile-text">
              <span className="profile-name">{session?.user.name ?? "You"}</span>
              <span className="profile-workspace">
                {session?.workspace.name ?? "Local workspace"}
              </span>
            </span>
            <button
              type="button"
              className={`icon-button profile-settings${
                view === "settings" ? " is-active" : ""
              }`}
              title="Settings"
              aria-label="Settings"
              aria-current={view === "settings" ? "page" : undefined}
              onClick={() => setView("settings")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                aria-hidden="true"
              >
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
            </button>
          </div>
        </div>
      </aside>

      <main className="main" inert={onboardingOpen}>
        <UpdateBanner updater={updater} tasksRunning={runningTasks.size > 0 || pendingReply !== null} />

        {error ? (
          <div className="error-banner" role="alert">
            <span>{error}</span>
            <button type="button" className="text-button" onClick={() => setError(null)}>
              Dismiss
            </button>
          </div>
        ) : null}

        {notice ? (
          <div className="notice-banner" role="status">
            <span>{notice}</span>
            <button type="button" className="text-button" onClick={() => setNotice(null)}>
              Dismiss
            </button>
          </div>
        ) : null}

        {loading ? (
          <div className="loading-state">
            <p>Opening your workspace…</p>
          </div>
        ) : view === "chat" ? (
          selectedChannel ? (
            <>
              <header className="chat-header">
                <div className="chat-identity">
                  {selectedChannel.kind === "conversation" &&
                  conversationParticipants.length > 1 ? (
                    <GroupAvatar names={conversationParticipantNames} size="large" />
                  ) : (
                    <Avatar
                      name={selectedAgent?.name ?? selectedChannel.name}
                      kind={selectedAgent ? "agent" : "channel"}
                      size="large"
                    />
                  )}
                  <div className="chat-heading">
                    <h2 className="chat-title">
                      {selectedChannel.kind === "conversation" ? (
                        selectedChannel.name
                      ) : selectedAgent ? (
                        selectedAgent.name
                      ) : (
                        <>
                          <span className="channel-hash" aria-hidden="true">
                            #
                          </span>
                          {selectedChannel.name}
                        </>
                      )}
                    </h2>
                    <p className="chat-subtitle">
                      {selectedChannel.kind === "conversation"
                        ? conversationParticipants.length > 1
                          ? `${conversationParticipants.length} participants${
                              defaultResponderName
                                ? ` · ${defaultResponderName} answers unaddressed messages`
                                : " · unaddressed messages stay silent"
                            }`
                          : selectedAgent
                            ? `${selectedAgent.name} · ${selectedAgent.role}`
                            : "Private conversation"
                        : selectedAgent
                          ? selectedAgent.role
                          : selectedChannel.topic.trim() || "No topic yet."}
                    </p>
                    {selectedChannel.kind === "conversation" &&
                    conversationParticipants.length > 1 ? (
                      <div
                        className="chat-participants"
                        aria-label="Conversation participants"
                      >
                        {conversationParticipants.map((participant) => (
                          <span
                            key={participant.agent_id}
                            className="chat-participant-chip"
                          >
                            <Avatar
                              name={participant.agent_name}
                              kind="agent"
                              size="small"
                            />
                            {participant.agent_name}
                            {participant.agent_id ===
                            conversationContext?.default_responder ? (
                              <span className="chip-default">default</span>
                            ) : null}
                          </span>
                        ))}
                      </div>
                    ) : null}
                    {selectedChannel.kind === "channel" && channelMentionNames.length > 0 ? (
                      <p className="chat-members">
                        Mention{" "}
                        {channelMentionNames.map((agent) => `@${agent.name}`).join(" · ")}
                      </p>
                    ) : null}
                  </div>
                </div>
                <div className="chat-actions">
                  <div className="inspector-triggers">
                    <button
                      type="button"
                      className={`inspector-trigger${
                        inspectorTab === "approvals" ? " is-active" : ""
                      }`}
                      title="Approvals"
                      aria-label={`Approvals (${approvals.length} pending)`}
                      aria-pressed={inspectorTab === "approvals"}
                      onClick={() => toggleInspector("approvals")}
                    >
                      <svg
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.7"
                        aria-hidden="true"
                      >
                        <path d="M9 11l3 3L22 4" />
                        <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
                      </svg>
                      {approvals.length > 0 ? (
                        <span className="inspector-trigger-count">
                          {approvals.length}
                        </span>
                      ) : null}
                    </button>
                    <button
                      type="button"
                      className={`inspector-trigger${
                        inspectorTab === "background" ? " is-active" : ""
                      }`}
                      title="Background tasks"
                      aria-label={`Background tasks (${activeOverviewCount} active)`}
                      aria-pressed={inspectorTab === "background"}
                      onClick={() => toggleInspector("background")}
                    >
                      <svg
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.7"
                        aria-hidden="true"
                      >
                        <path d="M3 12h4l3-8 4 16 3-8h4" />
                      </svg>
                      {activeOverviewCount > 0 ? (
                        <span className="inspector-trigger-count">
                          {activeOverviewCount}
                        </span>
                      ) : null}
                    </button>
                  </div>
                  {selectedChannel.kind === "conversation" ? (
                    <>
                      {conversationContext ? (
                        <ContextMeter
                          context={conversationContext}
                          busy={contextBusy}
                          openSignal={contextOpenSignal}
                          onCompact={(agentId) =>
                            void compactChannel(selectedChannel, agentId)
                          }
                          onClear={(agentId) =>
                            void clearContextForChannel(selectedChannel, agentId)
                          }
                          onAutoCompactChange={(enabled) =>
                            void updateConversationOptions(
                              selectedChannel,
                              enabled,
                              conversationContext.compact_threshold,
                            )
                          }
                          onThresholdChange={(threshold) =>
                            void updateConversationOptions(
                              selectedChannel,
                              conversationContext.auto_compact,
                              threshold,
                            )
                          }
                          onRouterChange={(enabled) =>
                            void updateConversationRouting(selectedChannel, enabled)
                          }
                          onDefaultResponderChange={(agentId) =>
                            void updateDefaultResponder(selectedChannel, agentId)
                          }
                          onMemoryChanged={() =>
                            setContextVersion((version) => version + 1)
                          }
                          onNewConversation={openNewConversation}
                          onInspect={inspectConversationContext}
                        />
                      ) : null}
                      <span className="chat-kind">
                        {conversationParticipants.length > 1
                          ? "Group"
                          : "Conversation"}
                      </span>
                    </>
                  ) : selectedAgent ? (
                    <span className="chat-kind">Direct</span>
                  ) : null}
                </div>
              </header>

              <section
                className="chat-thread"
                aria-label={`Messages in ${selectedChannel.name}`}
                key={selectedChannel.id}
              >
                {messages.length === 0 ? (
                  <div className="chat-empty">
                    {selectedChannel.kind === "conversation" &&
                    conversationParticipants.length > 1 ? (
                      <GroupAvatar names={conversationParticipantNames} size="large" />
                    ) : (
                      <Avatar
                        name={selectedAgent?.name ?? selectedChannel.name}
                        kind={selectedAgent ? "agent" : "channel"}
                        size="large"
                      />
                    )}
                    <p className="empty-title">
                      {selectedChannel.kind === "conversation"
                        ? conversationParticipants.length > 1
                          ? `Start the conversation with ${conversationParticipantNames.join(", ")}.`
                          : selectedAgent
                            ? `Start a conversation with ${selectedAgent.name}.`
                            : "Start a conversation."
                        : selectedAgent
                          ? `Say hello to ${selectedAgent.name}.`
                          : "No messages yet."}
                    </p>
                    <p>
                      {selectedChannel.kind === "conversation"
                        ? conversationParticipants.length > 1
                          ? `Mention a participant, use @team, or send an unaddressed message${
                              defaultResponderName
                                ? ` for ${defaultResponderName} to answer`
                                : " (it stays silent while no default responder is set)"
                            }. Each participant keeps its own context branch.`
                          : "Each conversation keeps its own context branch; the header meter shows how full it is."
                        : selectedAgent
                          ? "Every message here goes straight to this agent."
                          : `Write the first message to #${selectedChannel.name}.`}
                    </p>
                  </div>
                ) : (
                  <ol className="chat-messages">
                    {renderedMessages.map(({ message, separator }) => (
                      <Fragment key={message.id}>
                        {separator ? (
                          <li className="chat-day">
                            {formatDayLabel(message.created_at)}
                          </li>
                        ) : null}
                        {message.sender.type === "System" ? (
                          <li className="chat-system">{message.content}</li>
                        ) : (
                          <li
                            className={`chat-row ${
                              message.sender.type === "User"
                                ? "chat-row-user"
                                : "chat-row-agent"
                            }`}
                          >
                            {message.sender.type === "User" ? null : (
                              <Avatar
                                name={senderLabel(message, session, agents)}
                                kind="agent"
                                size="small"
                              />
                            )}
                            <div className="chat-bubble-wrap">
                              {message.sender.type === "User" ? null : (
                                <span className="chat-sender">
                                  {senderLabel(message, session, agents)}
                                </span>
                              )}
                              <div
                                className={`chat-bubble ${
                                  message.sender.type === "User"
                                    ? "chat-bubble-user"
                                    : "chat-bubble-agent"
                                }`}
                              >
                                <RichText content={message.content} />
                              </div>
                              <span className="chat-meta">
                                <time dateTime={message.created_at}>
                                  {formatTime(message.created_at)}
                                </time>
                                {message.thread_id ? " · In a thread" : null}
                                {message.sender.type === "User" &&
                                selectedChannel.kind === "conversation" ? (
                                  <>
                                    {" · "}
                                    <button
                                      type="button"
                                      className="text-button chat-remember"
                                      onClick={() => setRememberMessage(message)}
                                    >
                                      Remember this message
                                    </button>
                                  </>
                                ) : null}
                              </span>
                            </div>
                          </li>
                        )}
                        {message.sender.type === "User" &&
                        selectedChannel.kind === "conversation"
                          ? renderRouteLine(message)
                          : null}
                      </Fragment>
                    ))}
                    {inlineApprovals.map((approval) => {
                      const task = tasks.find(
                        (candidate) => candidate.id === approval.task_id,
                      );
                      const agent = task?.assigned_agent
                        ? agents.find(
                            (candidate) => candidate.id === task.assigned_agent,
                          )
                        : undefined;
                      return (
                        <li
                          className="chat-approval-row"
                          key={`approval-${approval.id}`}
                        >
                          {agent ? (
                            <Avatar
                              name={agent.name}
                              kind="agent"
                              size="small"
                            />
                          ) : null}
                          <div className="chat-approval-body">
                            <span className="chat-sender">
                              {agent?.name ?? "Approval"}
                            </span>
                            <ApprovalCard
                              approval={approval}
                              task={task}
                              call={toolCallDetails[approval.tool_call_id]}
                              risk={approvalDetails[approval.id]}
                              deciding={busy === approval.id}
                              compact
                              onDecide={handleDecideApproval}
                            />
                          </div>
                        </li>
                      );
                    })}
                    {inlineApprovals.length > 0 ? (
                      <li className="chat-row chat-row-agent" key="approval-waiting">
                        <div className="chat-bubble-wrap">
                          <div className="chat-bubble chat-bubble-agent chat-bubble-waiting">
                            Waiting for approval
                            {inlineApprovalTool ? `: ${inlineApprovalTool}` : ""}…
                          </div>
                        </div>
                      </li>
                    ) : replyingHere ? (
                      pendingReply.agents.map((agent) => (
                        <li className="chat-row chat-row-agent" key={`typing-${agent.id}`}>
                          <Avatar name={agent.name} kind="agent" size="small" />
                          <div className="chat-bubble-wrap">
                            <span className="chat-sender">{agent.name}</span>
                            <div className="chat-bubble chat-bubble-agent">
                              <span className="typing-dots" aria-label="Working">
                                <span />
                                <span />
                                <span />
                              </span>
                            </div>
                          </div>
                        </li>
                      ))
                    ) : null}
                  </ol>
                )}
                <div ref={endRef} />
              </section>

              {conversationFull && selectedChannel.kind === "conversation" ? (
                <div className="context-full-banner" role="alert">
                  <span>
                    This conversation's context is full. Compact it, clear it, or start a
                    new conversation before sending.
                  </span>
                  <div className="context-full-actions">
                    <button
                      type="button"
                      className="primary-button"
                      disabled={contextBusy}
                      onClick={() => void compactChannel(selectedChannel)}
                    >
                      Compact
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      disabled={contextBusy}
                      onClick={() => void clearContextForChannel(selectedChannel)}
                    >
                      Clear context
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={openNewConversation}
                    >
                      New conversation
                    </button>
                  </div>
                </div>
              ) : null}

              <form
                className="composer-bar"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (commandCatalogue.length > 0 && isCommandDraft(draft)) {
                    void sendCommandDraft();
                    return;
                  }
                  void sendMessage();
                }}
              >
                <div className="composer-main">
                  {commandCandidates.length > 0 || commandHint ? (
                    <SlashCommandMenu
                      commands={commandCandidates}
                      activeIndex={activeCommandIndex}
                      onAccept={acceptCommand}
                      onHover={setCommandIndex}
                      hint={commandHint}
                    />
                  ) : null}
                  {mentionCandidates.length > 0 ? (
                    <div
                      className="mention-suggestions"
                      id="mention-suggestions"
                      role="listbox"
                      aria-label="Mention suggestions"
                    >
                      {mentionCandidates.map((candidate, index) => (
                        <button
                          key={candidate.key}
                          type="button"
                          role="option"
                          id={`mention-option-${index}`}
                          aria-selected={index === activeMentionIndex}
                          className={`mention-option${
                            index === activeMentionIndex ? " is-active" : ""
                          }`}
                          onMouseDown={(event) => {
                            event.preventDefault();
                            acceptMention(candidate.name);
                          }}
                        >
                          <span>@{candidate.name}</span>
                          {candidate.broadcast ? (
                            <span className="mention-broadcast">everyone</span>
                          ) : null}
                        </button>
                      ))}
                    </div>
                  ) : null}
                  <button
                    type="button"
                    className="composer-add"
                    title={selectedAgent ? `New task for ${selectedAgent.name}` : "New task"}
                    aria-label="New task"
                    onClick={startTaskForCurrentChannel}
                  >
                    +
                  </button>
                  <textarea
                    ref={composerRef}
                    className="composer-input"
                    value={draft}
                    onChange={(event) => {
                      setDraft(event.target.value);
                      // A new draft is a new question for the command menu.
                      setCommandDismissed(false);
                      setCommandIndex(0);
                      setCommandHint(null);
                      if (slashQuery(event.target.value) === null) {
                        setCommandExpanded(false);
                      }
                    }}
                    onKeyDown={handleComposerKeyDown}
                    placeholder={
                      conversationFull
                        ? "Context full — compact or start a new conversation"
                        : `Message ${composerTarget}`
                    }
                    aria-label={`Message ${composerTarget}`}
                    aria-autocomplete="list"
                    aria-expanded={
                      commandCandidates.length > 0 || mentionCandidates.length > 0
                    }
                    aria-controls={
                      commandCandidates.length > 0
                        ? "command-suggestions"
                        : mentionCandidates.length > 0
                          ? "mention-suggestions"
                          : undefined
                    }
                    aria-activedescendant={
                      commandCandidates.length > 0
                        ? `command-option-${activeCommandIndex}`
                        : mentionCandidates.length > 0
                          ? `mention-option-${activeMentionIndex}`
                          : undefined
                    }
                    disabled={conversationFull}
                    rows={1}
                  />
                  <button
                    type="submit"
                    className="composer-send"
                    disabled={!draft.trim() || sending || conversationFull}
                    aria-label="Send message"
                  >
                    <svg
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      aria-hidden="true"
                    >
                      <path d="M12 19V5" />
                      <path d="m6 11 6-6 6 6" />
                    </svg>
                  </button>
                </div>
                <div className="composer-footer">
                  <span className="composer-footer-spacer" />
                  {agents.length > 0 ? (
                    <ApprovalModeMenu
                      value={channelApproval}
                      onChange={setChannelApproval}
                    />
                  ) : null}
                  <button
                    type="button"
                    className="composer-model"
                    aria-haspopup="menu"
                    aria-expanded={modelMenu !== null}
                    title={
                      activeModelProvider
                        ? `${activeModelProvider.name} · ${activeModelProvider.model}`
                        : "Offline mock model"
                    }
                    onClick={openModelMenu}
                  >
                    {activeModelLabel}
                  </button>
                </div>
              </form>
            </>
          ) : (
            <div className="empty-workspace">
              <span className="empty-icon-tile">
                <img className="empty-icon" src={icon} alt="" />
              </span>
              <p className="empty-title">A quiet room, for now.</p>
              <p>Create a channel to start working with your agents.</p>
              <button type="button" className="primary-button" onClick={openNewChannel}>
                Create a channel
              </button>
            </div>
          )
        ) : view === "tasks" ? (
          creatingTask ? (
            <div className="task-form-view">
              <header className="view-header">
                <h2 className="view-title">New task</h2>
                <p className="view-subtitle">
                  Give an agent a clear outcome and an approval policy.
                </p>
              </header>

              {agents.length === 0 ? (
                <div className="agent-bootstrap">
                  <p className="empty-title">No agents yet.</p>
                  <p>Add a team from a built-in template to assign this task.</p>
                  <div className="inline-form">
                    <select
                      value={templateId}
                      onChange={(event) => setTemplateId(event.target.value)}
                      aria-label="Template"
                    >
                      {templates.map((template) => (
                        <option key={template.id} value={template.id}>
                          {template.name}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="primary-button"
                      onClick={handleAddAgents}
                      disabled={busy === "add-agents" || !templateId}
                    >
                      {busy === "add-agents" ? "Adding…" : "Add agents"}
                    </button>
                  </div>
                </div>
              ) : (
                <form className="task-form" onSubmit={handleCreateTask}>
                  <label className="field">
                    <span>Description</span>
                    <textarea
                      ref={taskDescriptionRef}
                      value={taskDescription}
                      onChange={(event) => setTaskDescription(event.target.value)}
                      placeholder="Implement the login flow and verify it in the sandbox."
                      rows={3}
                    />
                  </label>
                  <label className="field">
                    <span>Expected output</span>
                    <textarea
                      value={taskOutput}
                      onChange={(event) => setTaskOutput(event.target.value)}
                      placeholder="Working code, tests, and a short summary of changes."
                      rows={2}
                    />
                  </label>
                  <div className="form-row">
                    <label className="field">
                      <span>Assignee</span>
                      <select
                        value={taskAssignee}
                        onChange={(event) => setTaskAssignee(event.target.value)}
                      >
                        {agents.map((agent) => (
                          <option key={agent.id} value={agent.id}>
                            {agent.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="field">
                      <span>Channel</span>
                      <select
                        value={taskChannel}
                        onChange={(event) => setTaskChannel(event.target.value)}
                      >
                        <option value="">No channel</option>
                        {channels.map((channel) => (
                          <option key={channel.id} value={channel.id}>
                            {channel.agent_id ? channel.name : `#${channel.name}`}
                          </option>
                        ))}
                      </select>
                    </label>
                    <div className="field">
                      <span>Approval</span>
                      <ApprovalModeMenu
                        value={taskApproval}
                        onChange={setTaskApproval}
                      />
                    </div>
                  </div>
                  {availableProjects.length > 0 ? (
                    <section
                      className="task-worktree"
                      aria-label="Run in a git worktree"
                    >
                      <label className="worktree-toggle">
                        <input
                          type="checkbox"
                          checked={worktreeEnabled}
                          onChange={(event) =>
                            setWorktreeEnabled(event.target.checked)
                          }
                        />
                        <span>
                          <strong>Run in a git worktree</strong>
                          <small>
                            Work in an isolated checkout on its own branch. The
                            branch is not merged automatically.
                          </small>
                        </span>
                      </label>
                      {worktreeEnabled ? (
                        <div className="form-row">
                          <label className="field">
                            <span>Project</span>
                            <select
                              value={taskProject}
                              onChange={(event) =>
                                setTaskProject(event.target.value)
                              }
                            >
                              {availableProjects.map((project) => (
                                <option key={project.id} value={project.id}>
                                  {project.name}
                                </option>
                              ))}
                            </select>
                          </label>
                          <label className="field">
                            <span>Base branch</span>
                            <select
                              value={taskBaseBranch}
                              onChange={(event) =>
                                setTaskBaseBranch(event.target.value)
                              }
                              disabled={projectBranchesBusy || !projectBranches}
                            >
                              {projectBranches ? (
                                projectBranches.branches.map((branch) => (
                                  <option key={branch} value={branch}>
                                    {branch}
                                  </option>
                                ))
                              ) : (
                                <option value="">
                                  {projectBranchesBusy
                                    ? "Loading branches…"
                                    : "No local branches"}
                                </option>
                              )}
                            </select>
                          </label>
                          <div className="field">
                            <span>Worktree branch</span>
                            <code className="worktree-preview">
                              {worktreeBranchPreview(taskDescription)}
                            </code>
                          </div>
                        </div>
                      ) : null}
                    </section>
                  ) : null}
                  <div className="form-actions">
                    <button
                      type="submit"
                      className="primary-button"
                      disabled={
                        busy === "create-task" ||
                        !taskDescription.trim() ||
                        !taskOutput.trim() ||
                        !taskAssignee ||
                        (worktreeEnabled &&
                          (!taskProject ||
                            !taskBaseBranch ||
                            projectBranchesBusy))
                      }
                    >
                      {busy === "create-task" ? "Creating…" : "Create task"}
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => {
                        setCreatingTask(false);
                        setView("chat");
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                </form>
              )}
            </div>
          ) : (
            <div className="empty-workspace">
              <p className="empty-title">No task selected.</p>
              <p>Pick a task in the inspector, or create a new one.</p>
              <button type="button" className="primary-button" onClick={startCreatingTask}>
                New task
              </button>
            </div>
          )
        ) : (
          <SettingsView
            session={session}
            agents={agents}
            templates={templates}
            templateId={templateId}
            busy={busy}
            updater={updater}
            tasksRunning={runningTasks.size > 0 || pendingReply !== null}
            section={settingsSection}
            projects={projects}
            onSelectTemplate={setTemplateId}
            onAddAgents={handleAddAgents}
            onEditAgent={handleEditAgent}
            onSessionChange={setSession}
            onAddProject={handleAddProject}
            onRemoveProject={handleRemoveProject}
          />
        )}
      </main>

      {inspectorTab ? (
        <Inspector
          tab={inspectorTab}
          onTabChange={setInspectorTab}
          onClose={() => setInspectorTab(null)}
          taskPanel={taskInspectorPanel}
          contextPanel={contextInspectorPanel}
          overview={overview}
          agents={agents}
          now={overviewNow}
          inert={onboardingOpen}
          onOpenTask={openTask}
          approvals={approvals}
          tasks={tasks}
          toolCalls={toolCallDetails}
          risks={approvalDetails}
          decidingId={busy}
          onDecide={handleDecideApproval}
        />
      ) : null}

      {onboardingOpen ? (
        <Onboarding
          templates={templates}
          agents={agents}
          onAddAgents={instantiateTemplate}
          onComplete={handleCompleteOnboarding}
        />
      ) : null}

      {menu ? (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems}
          onClose={() => setMenu(null)}
        />
      ) : null}

      {modelMenu ? (
        <ContextMenu
          x={modelMenu.x}
          y={modelMenu.y}
          items={modelMenuItems}
          onClose={() => setModelMenu(null)}
        />
      ) : null}

      {dialog?.kind === "rename-channel" ? (
        <Modal
          title={
            dialog.channel.kind === "conversation"
              ? "Rename conversation"
              : "Rename channel"
          }
          onClose={closeDialog}
          footer={
            <>
              <button
                type="button"
                className="ghost-button"
                onClick={closeDialog}
                disabled={dialogBusy}
              >
                Cancel
              </button>
              <button
                type="submit"
                form="rename-channel-form"
                className="primary-button"
                disabled={dialogBusy || !dialogName.trim()}
              >
                {dialogBusy ? "Saving…" : "Save"}
              </button>
            </>
          }
        >
          <form id="rename-channel-form" className="dialog-form" onSubmit={submitRename}>
            <label className="field">
              <span>Name</span>
              <input
                value={dialogName}
                onChange={(event) => setDialogName(event.target.value)}
                autoComplete="off"
              />
            </label>
            <label className="field">
              <span>Topic</span>
              <input
                value={dialogTopic}
                onChange={(event) => setDialogTopic(event.target.value)}
                autoComplete="off"
              />
            </label>
            {dialogError ? (
              <p className="dialog-error" role="alert">
                {dialogError}
              </p>
            ) : null}
          </form>
        </Modal>
      ) : dialog?.kind === "delete-channel" ? (
        <Modal
          title={
            dialog.channel.kind === "conversation"
              ? "Delete conversation"
              : "Delete channel"
          }
          onClose={closeDialog}
          footer={
            <>
              <button
                type="button"
                className="ghost-button"
                onClick={closeDialog}
                disabled={dialogBusy}
              >
                Cancel
              </button>
              <button
                type="button"
                className="danger-button"
                onClick={() => void submitDelete()}
                disabled={dialogBusy}
              >
                {dialogBusy
                  ? "Deleting…"
                  : dialog.channel.kind === "conversation"
                    ? "Delete conversation"
                    : "Delete channel"}
              </button>
            </>
          }
        >
          {dialog.channel.kind === "conversation" ? (
            <p className="dialog-copy">
              Delete <strong>{dialog.channel.name}</strong>? Its messages and context
              will be deleted. This cannot be undone.
            </p>
          ) : (
            <p className="dialog-copy">
              Delete <strong>#{dialog.channel.name}</strong>? Its messages will be
              deleted. Tasks keep their history but are no longer linked to the
              channel.
            </p>
          )}
          {dialogError ? (
            <p className="dialog-error" role="alert">
              {dialogError}
            </p>
          ) : null}
        </Modal>
      ) : dialog?.kind === "clear-history" ? (
        <Modal
          title="Clear conversation"
          onClose={closeDialog}
          footer={
            <>
              <button
                type="button"
                className="ghost-button"
                onClick={closeDialog}
                disabled={dialogBusy}
              >
                Cancel
              </button>
              <button
                type="button"
                className="danger-button"
                onClick={() => void submitClear()}
                disabled={dialogBusy}
              >
                {dialogBusy ? "Clearing…" : "Clear history"}
              </button>
            </>
          }
        >
          <p className="dialog-copy">
            Clear the conversation with <strong>{dialog.agent.name}</strong>? All
            messages in this conversation will be deleted; the agent stays.
          </p>
          {dialogError ? (
            <p className="dialog-error" role="alert">
              {dialogError}
            </p>
          ) : null}
        </Modal>
      ) : dialog?.kind === "edit-agent" ? (
        <Modal
          title="Edit agent"
          onClose={closeDialog}
          footer={
            <>
              <button
                type="button"
                className="ghost-button"
                onClick={closeDialog}
                disabled={dialogBusy}
              >
                Cancel
              </button>
              <button
                type="submit"
                form="edit-agent-form"
                className="primary-button"
                disabled={dialogBusy || !dialogName.trim()}
              >
                {dialogBusy ? "Saving…" : "Save"}
              </button>
            </>
          }
        >
          <form id="edit-agent-form" className="dialog-form" onSubmit={submitEditAgent}>
            <label className="field">
              <span>Name</span>
              <input
                value={dialogName}
                onChange={(event) => setDialogName(event.target.value)}
                autoComplete="off"
              />
            </label>
            <label className="field">
              <span>Role</span>
              <input
                value={dialogRole}
                onChange={(event) => setDialogRole(event.target.value)}
                autoComplete="off"
              />
            </label>
            <fieldset className="tool-picker">
              <legend>Tools</legend>
              {availableTools.length === 0 ? (
                <p className="dialog-copy">Loading tools…</p>
              ) : (
                availableTools.map((tool) => (
                  <label key={tool.name} className="tool-option">
                    <input
                      type="checkbox"
                      checked={dialogTools.includes(tool.name)}
                      onChange={() => toggleTool(tool.name)}
                    />
                    <span>
                      <strong>{tool.name}</strong>
                      <small>{tool.description}</small>
                    </span>
                  </label>
                ))
              )}
            </fieldset>
            {dialogError ? (
              <p className="dialog-error" role="alert">
                {dialogError}
              </p>
            ) : null}
          </form>
        </Modal>
      ) : null}

      {rememberMessage ? (
        <RememberMessageModal
          channelId={rememberMessage.channel_id}
          initialContent={rememberMessage.content}
          onClose={() => setRememberMessage(null)}
          onSaved={() => {
            setNotice("Saved to memory.");
            setContextVersion((version) => version + 1);
          }}
        />
      ) : null}
    </div>
  );
}
