import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent } from "react";

import ApprovalCards from "./ApprovalCards";
import Avatar from "./Avatar";
import Onboarding from "./Onboarding";
import SettingsView from "./SettingsView";
import TaskDetail from "./TaskDetail";
import type { AssignmentWithEvents, RunWithCalls } from "./TaskDetail";
import UpdateBanner from "./UpdateBanner";
import {
  cancelTask,
  completeOnboarding,
  createChannel,
  createTask,
  decideApproval,
  ensureAgentChannels,
  getOnboardingState,
  getSession,
  instantiateAgents,
  listAgents,
  listAssignmentEvents,
  listAssignments,
  listChannels,
  listMessages,
  listRuns,
  listTasks,
  listTemplates,
  listToolCalls,
  onCowbotEvent,
  onChannelReply,
  onRunFinished,
  pendingApprovals,
  postMessage,
  runTask,
} from "./api";
import icon from "./assets/cowbot-icon.svg";
import wordmark from "./assets/cowbot-wordmark-dark.svg";
import { errorMessage } from "./errors";
import { formatListTime, formatTime } from "./format";
import type {
  AgentProfile,
  Approval,
  ApprovalLevel,
  Channel,
  ChannelMessage,
  ClientSession,
  OnboardingState,
  Task,
  TemplateDocument,
} from "./types";
import { approvalLevelLabel, taskStatusKind, taskStatusLabel } from "./types";
import { useUpdater } from "./updater";

type View = "chat" | "tasks" | "approvals" | "settings";

interface PendingReply {
  channelId: string;
  messageId: string;
  agents: AgentProfile[];
}

const approvalLevels: ApprovalLevel[] = ["ask", "safe_auto", "plan_only", "trusted"];

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
  const leftTime = previewTime(previews[left.channel.id]);
  const rightTime = previewTime(previews[right.channel.id]);
  if (leftTime !== rightTime) return leftTime < rightTime ? 1 : -1;
  const leftName = left.agent?.name ?? left.name ?? left.channel.name;
  const rightName = right.agent?.name ?? right.name ?? right.channel.name;
  return leftName.localeCompare(rightName);
}

export default function App() {
  const [view, setView] = useState<View>("chat");
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
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [templates, setTemplates] = useState<TemplateDocument[]>([]);
  const [onboarding, setOnboarding] = useState<OnboardingState | null>(null);
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [pendingReply, setPendingReply] = useState<PendingReply | null>(null);
  const [creatingChannel, setCreatingChannel] = useState(false);
  const [channelName, setChannelName] = useState("");
  const [channelTopic, setChannelTopic] = useState("");
  const [creatingTask, setCreatingTask] = useState(false);
  const [taskDescription, setTaskDescription] = useState("");
  const [taskOutput, setTaskOutput] = useState("");
  const [taskAssignee, setTaskAssignee] = useState("");
  const [taskChannel, setTaskChannel] = useState("");
  const [taskApproval, setTaskApproval] = useState<ApprovalLevel>("ask");
  const [channelApproval, setChannelApproval] = useState<ApprovalLevel>("ask");
  const [templateId, setTemplateId] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [runningTasks, setRunningTasks] = useState<Set<string>>(() => new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement | null>(null);
  const channelNameRef = useRef<HTMLInputElement | null>(null);
  const taskDescriptionRef = useRef<HTMLTextAreaElement | null>(null);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const updater = useUpdater();

  const selectedChannel = useMemo(
    () => channels.find((channel) => channel.id === selectedId) ?? null,
    [channels, selectedId],
  );

  const selectedAgent = useMemo(
    () =>
      selectedChannel?.agent_id
        ? agents.find((agent) => agent.id === selectedChannel.agent_id) ?? null
        : null,
    [agents, selectedChannel],
  );

  const selectedTask = useMemo(
    () => tasks.find((task) => task.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
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
        const firstDirect = next.find((channel) => channel.agent_id);
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
      return;
    }

    const [assignments, runs] = await Promise.all([
      listAssignments(taskId),
      listRuns(taskId),
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
  }, []);

  const refreshApprovals = useCallback(async () => {
    setApprovals(await pendingApprovals());
  }, []);

  const refreshAgents = useCallback(async () => {
    setAgents(await listAgents());
  }, []);

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
        ] = await Promise.all([
          getSession(),
          ensureAgentChannels(),
          listTasks(),
          pendingApprovals(),
          listAgents(),
          listTemplates(),
          getOnboardingState(),
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
    refreshTaskDetail(selectedTaskId).catch((err) => setError(errorMessage(err)));
  }, [refreshTaskDetail, selectedTaskId]);

  useEffect(() => {
    if (!taskAssignee && agents.length > 0) setTaskAssignee(agents[0].id);
  }, [agents, taskAssignee]);

  useEffect(() => {
    if (!templateId && templates.length > 0) setTemplateId(templates[0].id);
  }, [templates, templateId]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    onCowbotEvent((event) => {
      const type = event.payload.type;
      if (type === "ChannelCreated") {
        refreshChannels().catch((err) => setError(errorMessage(err)));
      }
      if (type === "MessagePosted") {
        const message = event.payload.payload as ChannelMessage | undefined;
        if (message && typeof message.channel_id === "string") {
          setPreviews((current) => ({ ...current, [message.channel_id]: message }));
        }
        refreshMessages(selectedId).catch((err) => setError(errorMessage(err)));
      }
      if (
        type === "TaskCreated" ||
        type === "TaskAssigned" ||
        type === "TaskCompleted" ||
        type === "TaskFailed"
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
    refreshApprovals,
    refreshChannels,
    refreshMessages,
    refreshTaskDetail,
    refreshTasks,
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
      if (event.error) {
        setError(event.error);
      }
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
        channel: channels.find((channel) => channel.agent_id === agent.id) ?? null,
      }))
      .filter(
        (row): row is { agent: AgentProfile; channel: Channel } => row.channel !== null,
      );
    return rows.sort((left, right) => byRecentActivity(left, right, previews));
  }, [agents, channels, previews]);

  const groupChannels = useMemo(() => {
    return channels
      .filter((channel) => !channel.agent_id)
      .map((channel) => ({ channel }))
      .sort((left, right) => byRecentActivity(left, right, previews));
  }, [channels, previews]);

  const normalizedQuery = query.trim().toLowerCase();

  const visibleAgentRows = useMemo(
    () =>
      normalizedQuery
        ? agentRows.filter((row) => row.agent.name.toLowerCase().includes(normalizedQuery))
        : agentRows,
    [agentRows, normalizedQuery],
  );

  const visibleChannels = useMemo(
    () =>
      normalizedQuery
        ? groupChannels.filter((row) => row.channel.name.toLowerCase().includes(normalizedQuery))
        : groupChannels,
    [groupChannels, normalizedQuery],
  );

  const visibleTasks = useMemo(
    () =>
      normalizedQuery
        ? tasks.filter(
            (task) =>
              task.description.toLowerCase().includes(normalizedQuery) ||
              agentName(agents, task.assigned_agent).toLowerCase().includes(normalizedQuery),
          )
        : tasks,
    [agents, normalizedQuery, tasks],
  );

  const channelMentionNames = useMemo(() => {
    const seen = new Set<string>();
    return agents.filter((agent) => {
      const key = agent.name.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [agents]);

  const replyingHere = pendingReply !== null && pendingReply.channelId === selectedId;

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

  async function sendMessage() {
    const content = draft.trim();
    if (!content || !selectedChannel || sending) return;

    setSending(true);
    setError(null);
    try {
      const posted = await postMessage(selectedChannel.id, content, channelApproval);
      const recipients = selectedAgent
        ? [selectedAgent]
        : addressedAgents(posted.content, agents);
      setPendingReply(
        recipients.length > 0
          ? { channelId: selectedChannel.id, messageId: posted.id, agents: recipients }
          : null,
      );
      setDraft("");
      setPreviews((current) => ({ ...current, [selectedChannel.id]: posted }));
      await refreshMessages(selectedChannel.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSending(false);
    }
  }

  function handleComposerKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void sendMessage();
    }
  }

  function openNewChannel() {
    setView("chat");
    setCreatingChannel(true);
    setError(null);
  }

  function startCreatingTask() {
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
      });
      setCreatingTask(false);
      setTaskDescription("");
      setTaskOutput("");
      setTaskChannel("");
      await refreshTasks(task.id);
      setSelectedTaskId(task.id);
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

  async function handleRunTask(taskId: string) {
    setError(null);
    setRunningTasks((current) => new Set(current).add(taskId));
    try {
      await runTask(taskId, taskApproval);
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

  async function handleDecideApproval(approval: Approval, approved: boolean) {
    setBusy(approval.id);
    setError(null);
    try {
      await decideApproval(approval.id, approved);
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

  return (
    <div className="app">
      <aside className="sidebar" inert={onboardingOpen}>
        <div className="sidebar-head">
          <img className="wordmark" src={wordmark} alt="CowBot" />
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

        {view === "chat" || view === "tasks" ? (
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
              placeholder={view === "tasks" ? "Search tasks" : "Search"}
              aria-label={view === "tasks" ? "Search tasks" : "Search conversations"}
            />
          </label>
        ) : null}

        <div className="sidebar-scroll">
          {view === "chat" ? (
            <>
              <section className="sidebar-section" aria-label="Agents">
                <div className="nav-heading">
                  <span>Agents</span>
                  <span className="nav-count-soft">{agentRows.length}</span>
                </div>
                {agentRows.length === 0 ? (
                  <p className="nav-note">
                    Add a team from a template in Settings to start a conversation.
                  </p>
                ) : visibleAgentRows.length === 0 ? (
                  <p className="nav-note">No agents match.</p>
                ) : (
                  <ul className="contact-list">
                    {visibleAgentRows.map(({ agent, channel }) => {
                      const preview = previews[channel.id] ?? null;
                      return (
                        <li key={agent.id}>
                          <button
                            type="button"
                            className="contact-row"
                            aria-current={channel.id === selectedId ? "page" : undefined}
                            onClick={() => setSelectedId(channel.id)}
                          >
                            <Avatar name={agent.name} kind="agent" />
                            <span className="contact-text">
                              <span className="contact-top">
                                <span className="contact-name">{agent.name}</span>
                                <time className="contact-time">
                                  {preview ? formatListTime(preview.created_at) : ""}
                                </time>
                              </span>
                              <span className="contact-snippet">
                                {preview
                                  ? `${senderLabel(preview, session, agents)}: ${preview.content}`
                                  : agent.role}
                              </span>
                            </span>
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </section>

              <section className="sidebar-section" aria-label="Channels">
                <div className="nav-heading">
                  <span>Channels</span>
                  <button type="button" className="text-button" onClick={openNewChannel}>
                    New
                  </button>
                </div>
                {groupChannels.length > 0 ? (
                  visibleChannels.length > 0 ? (
                    <ul className="contact-list">
                      {visibleChannels.map(({ channel }) => {
                        const preview = previews[channel.id] ?? null;
                        return (
                          <li key={channel.id}>
                            <button
                              type="button"
                              className="contact-row"
                              aria-current={channel.id === selectedId ? "page" : undefined}
                              onClick={() => setSelectedId(channel.id)}
                            >
                              <Avatar name={channel.name} kind="channel" />
                              <span className="contact-text">
                                <span className="contact-top">
                                  <span className="contact-name">{channel.name}</span>
                                  <time className="contact-time">
                                    {preview ? formatListTime(preview.created_at) : ""}
                                  </time>
                                </span>
                                <span className="contact-snippet">
                                  {preview
                                    ? `${senderLabel(preview, session, agents)}: ${preview.content}`
                                    : channel.topic || "No messages yet"}
                                </span>
                              </span>
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  ) : (
                    <p className="nav-note">No channels match.</p>
                  )
                ) : null}

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
            </>
          ) : view === "tasks" ? (
            <section className="sidebar-section" aria-label="Tasks">
              <div className="nav-heading">
                <span>Tasks</span>
                <button type="button" className="text-button" onClick={startCreatingTask}>
                  New
                </button>
              </div>
              <ul className="contact-list">
                {visibleTasks.map((task) => {
                  const kind = taskStatusKind(task.status);
                  return (
                    <li key={task.id}>
                      <button
                        type="button"
                        className="contact-row"
                        aria-current={
                          task.id === selectedTaskId && !creatingTask ? "page" : undefined
                        }
                        onClick={() => {
                          setSelectedTaskId(task.id);
                          setCreatingTask(false);
                        }}
                      >
                        <Avatar
                          name={agentName(agents, task.assigned_agent)}
                          kind="agent"
                          size="small"
                        />
                        <span className="contact-text">
                          <span className="contact-top">
                            <span className="contact-name">{task.description}</span>
                          </span>
                          <span className="contact-snippet">
                            <span className={`status-dot status-${kind}`} />{" "}
                            {taskStatusLabel(task.status)} ·{" "}
                            {agentName(agents, task.assigned_agent)}
                          </span>
                        </span>
                      </button>
                    </li>
                  );
                })}
                {visibleTasks.length === 0 ? (
                  <li className="nav-empty">
                    {tasks.length === 0 ? "No tasks yet." : "No tasks match."}
                  </li>
                ) : null}
              </ul>
              <button type="button" className="new-channel-button" onClick={startCreatingTask}>
                <span aria-hidden="true">+</span> New task
              </button>
            </section>
          ) : view === "approvals" ? (
            <section className="sidebar-section" aria-label="Approvals">
              <div className="nav-heading">
                <span>Approvals</span>
                <span className="nav-count-soft">{approvals.length}</span>
              </div>
              <p className="nav-note">
                Pending tool calls appear in the main pane. Agents request approval while a task
                runs.
              </p>
            </section>
          ) : null}
        </div>

        <div className="sidebar-footer">
          <nav className="footer-nav" aria-label="Views">
            <button
              type="button"
              className="footer-nav-item"
              aria-current={view === "tasks" ? "page" : undefined}
              onClick={() => setView("tasks")}
            >
              Tasks
            </button>
            <button
              type="button"
              className="footer-nav-item"
              aria-current={view === "approvals" ? "page" : undefined}
              onClick={() => setView("approvals")}
            >
              Approvals
              {approvals.length > 0 ? <span className="nav-count">{approvals.length}</span> : null}
            </button>
            <button
              type="button"
              className="footer-nav-item footer-nav-icon"
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
          </nav>
          <div className="profile">
            <Avatar name={session?.user.name ?? "You"} kind="user" />
            <span className="profile-text">
              <span className="profile-name">{session?.user.name ?? "You"}</span>
              <span className="profile-workspace">
                {session?.workspace.name ?? "Local workspace"}
              </span>
            </span>
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

        {loading ? (
          <div className="loading-state">
            <p>Opening your workspace…</p>
          </div>
        ) : view === "chat" ? (
          selectedChannel ? (
            <>
              <header className="chat-header">
                <div className="chat-identity">
                  <Avatar
                    name={selectedAgent?.name ?? selectedChannel.name}
                    kind={selectedAgent ? "agent" : "channel"}
                    size="large"
                  />
                  <div className="chat-heading">
                    <h2 className="chat-title">
                      {selectedAgent?.name ?? (
                        <>
                          <span className="channel-hash" aria-hidden="true">
                            #
                          </span>
                          {selectedChannel.name}
                        </>
                      )}
                    </h2>
                    <p className="chat-subtitle">
                      {selectedAgent
                        ? selectedAgent.role
                        : selectedChannel.topic.trim() || "No topic yet."}
                    </p>
                    {!selectedAgent && channelMentionNames.length > 0 ? (
                      <p className="chat-members">
                        Mention{" "}
                        {channelMentionNames.map((agent) => `@${agent.name}`).join(" · ")}
                      </p>
                    ) : null}
                  </div>
                </div>
                <div className="chat-actions">
                  {agents.length > 0 ? (
                    <label className="channel-approval">
                      <span>Approval</span>
                      <select
                        value={channelApproval}
                        onChange={(event) =>
                          setChannelApproval(event.target.value as ApprovalLevel)
                        }
                      >
                        {approvalLevels.map((level) => (
                          <option key={level} value={level}>
                            {approvalLevelLabel(level)}
                          </option>
                        ))}
                      </select>
                    </label>
                  ) : null}
                  {selectedAgent ? <span className="chat-kind">Direct</span> : null}
                </div>
              </header>

              <section
                className="chat-thread"
                aria-label={`Messages in ${selectedChannel.name}`}
                key={selectedChannel.id}
              >
                {messages.length === 0 ? (
                  <div className="chat-empty">
                    <Avatar
                      name={selectedAgent?.name ?? selectedChannel.name}
                      kind={selectedAgent ? "agent" : "channel"}
                      size="large"
                    />
                    <p className="empty-title">
                      {selectedAgent ? `Say hello to ${selectedAgent.name}.` : "No messages yet."}
                    </p>
                    <p>
                      {selectedAgent
                        ? "Every message here goes straight to this agent."
                        : `Write the first message to #${selectedChannel.name}.`}
                    </p>
                  </div>
                ) : (
                  <ol className="chat-messages">
                    {messages.map((message) => {
                      if (message.sender.type === "System") {
                        return (
                          <li className="chat-system" key={message.id}>
                            {message.content}
                          </li>
                        );
                      }

                      const fromUser = message.sender.type === "User";
                      const name = senderLabel(message, session, agents);
                      return (
                        <li
                          className={`chat-row ${fromUser ? "chat-row-user" : "chat-row-agent"}`}
                          key={message.id}
                        >
                          {fromUser ? null : <Avatar name={name} kind="agent" size="small" />}
                          <div className="chat-bubble-wrap">
                            {fromUser ? null : <span className="chat-sender">{name}</span>}
                            <div
                              className={`chat-bubble ${
                                fromUser ? "chat-bubble-user" : "chat-bubble-agent"
                              }`}
                            >
                              {message.content}
                            </div>
                            <span className="chat-meta">
                              <time dateTime={message.created_at}>
                                {formatTime(message.created_at)}
                              </time>
                              {message.thread_id ? " · In a thread" : null}
                            </span>
                          </div>
                        </li>
                      );
                    })}
                    {replyingHere
                      ? pendingReply.agents.map((agent) => (
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
                      : null}
                  </ol>
                )}
                <div ref={endRef} />
              </section>

              <form
                className="composer-bar"
                onSubmit={(event) => {
                  event.preventDefault();
                  void sendMessage();
                }}
              >
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
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={handleComposerKeyDown}
                  placeholder={`Message ${composerTarget}`}
                  aria-label={`Message ${composerTarget}`}
                  rows={1}
                />
                <button
                  type="submit"
                  className="composer-send"
                  disabled={!draft.trim() || sending}
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
                    <label className="field">
                      <span>Approval</span>
                      <select
                        value={taskApproval}
                        onChange={(event) =>
                          setTaskApproval(event.target.value as ApprovalLevel)
                        }
                      >
                        {approvalLevels.map((level) => (
                          <option key={level} value={level}>
                            {approvalLevelLabel(level)}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  <div className="form-actions">
                    <button
                      type="submit"
                      className="primary-button"
                      disabled={
                        busy === "create-task" ||
                        !taskDescription.trim() ||
                        !taskOutput.trim() ||
                        !taskAssignee
                      }
                    >
                      {busy === "create-task" ? "Creating…" : "Create task"}
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => setCreatingTask(false)}
                    >
                      Cancel
                    </button>
                  </div>
                </form>
              )}
            </div>
          ) : selectedTask ? (
            <TaskDetail
              task={selectedTask}
              assignments={taskAssignments}
              runs={taskRuns}
              cancelling={busy === "cancel-task"}
              onCancel={handleCancelTask}
              running={runningTasks.has(selectedTask.id)}
              onRun={() => handleRunTask(selectedTask.id)}
            />
          ) : (
            <div className="empty-workspace">
              <p className="empty-title">No task selected.</p>
              <p>Create a task and choose an agent to own it.</p>
              <button type="button" className="primary-button" onClick={startCreatingTask}>
                New task
              </button>
            </div>
          )
        ) : view === "approvals" ? (
          <ApprovalCards
            approvals={approvals}
            tasks={tasks}
            decidingId={busy}
            onDecide={handleDecideApproval}
          />
        ) : (
          <SettingsView
            session={session}
            agents={agents}
            templates={templates}
            templateId={templateId}
            busy={busy}
            updater={updater}
            tasksRunning={runningTasks.size > 0 || pendingReply !== null}
            onSelectTemplate={setTemplateId}
            onAddAgents={handleAddAgents}
          />
        )}
      </main>

      {onboardingOpen ? (
        <Onboarding
          templates={templates}
          agents={agents}
          onAddAgents={instantiateTemplate}
          onComplete={handleCompleteOnboarding}
        />
      ) : null}
    </div>
  );
}
