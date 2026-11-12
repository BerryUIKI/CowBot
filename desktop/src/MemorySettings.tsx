import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import NotesEditor from "./NotesEditor";
import {
  addMemoryEntry,
  approveMemoryEntry,
  archiveMemoryEntry,
  deleteMemoryEntry,
  dismissMemoryEntry,
  getMemorySettings,
  listConversationSummaries,
  listMemoryEntries,
  updateMemoryEntry,
  updateMemorySettings,
} from "./api";
import { errorMessage } from "./errors";
import { formatDateTime } from "./format";
import type {
  AgentProfile,
  ConversationSummary,
  MemoryEntry,
  MemoryKind,
  MemorySettings,
  MemoryStatus,
} from "./types";

interface MemorySettingsProps {
  agents: AgentProfile[];
}

type ScopeFilter =
  | "all"
  | "workspace"
  | "agent"
  | "conversation"
  | "agent-in-conversation";

const kinds: MemoryKind[] = [
  "fact",
  "preference",
  "decision",
  "summary",
  "note",
];

const scopeOptions: { id: ScopeFilter; label: string }[] = [
  { id: "all", label: "All scopes" },
  { id: "workspace", label: "Workspace-shared" },
  { id: "agent", label: "Agent" },
  { id: "conversation", label: "Conversation-shared" },
  { id: "agent-in-conversation", label: "Agent in a conversation" },
];

function scopeLabel(
  entry: MemoryEntry,
  agents: AgentProfile[],
  conversations: ConversationSummary[],
): string {
  const agent = entry.agent_id
    ? agents.find((candidate) => candidate.id === entry.agent_id)?.name ??
      entry.agent_id
    : null;
  const conversation = entry.channel_id
    ? conversations.find(
        (candidate) => candidate.channel.id === entry.channel_id,
      )?.channel.name ?? entry.channel_id
    : null;
  if (agent && conversation) return `${agent} in ${conversation}`;
  if (agent) return agent;
  if (conversation) return conversation;
  return "Workspace";
}

function matchesScope(entry: MemoryEntry, scope: ScopeFilter): boolean {
  switch (scope) {
    case "workspace":
      return !entry.agent_id && !entry.channel_id;
    case "agent":
      return Boolean(entry.agent_id) && !entry.channel_id;
    case "conversation":
      return !entry.agent_id && Boolean(entry.channel_id);
    case "agent-in-conversation":
      return Boolean(entry.agent_id) && Boolean(entry.channel_id);
    default:
      return true;
  }
}

/**
 * Settings → Memory: notes, extraction and recall settings, entry management,
 * and the pending-candidate review.
 */
export default function MemorySettings({ agents }: MemorySettingsProps) {
  const [settings, setSettings] = useState<MemorySettings | null>(null);
  const [extraction, setExtraction] = useState(false);
  const [recall, setRecall] = useState(true);
  const [notesBudget, setNotesBudget] = useState(800);
  const [recallBudget, setRecallBudget] = useState(1200);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsNotice, setSettingsNotice] = useState<string | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);

  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [statusFilter, setStatusFilter] = useState<MemoryStatus | "">("");
  const [scopeFilter, setScopeFilter] = useState<ScopeFilter>("all");
  const [agentFilter, setAgentFilter] = useState("");
  const [channelFilter, setChannelFilter] = useState("");
  const [listBusy, setListBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [newContent, setNewContent] = useState("");
  const [newKind, setNewKind] = useState<MemoryKind>("fact");
  const [newAgent, setNewAgent] = useState("");
  const [newChannel, setNewChannel] = useState("");
  const [newPinned, setNewPinned] = useState(false);
  const [addBusy, setAddBusy] = useState(false);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editContent, setEditContent] = useState("");
  const [editKind, setEditKind] = useState<MemoryKind>("fact");
  const [editPinned, setEditPinned] = useState(false);
  const [rowBusy, setRowBusy] = useState<string | null>(null);

  useEffect(() => {
    getMemorySettings()
      .then((loaded) => {
        setSettings(loaded);
        setExtraction(loaded.extraction_enabled);
        setRecall(loaded.recall_enabled);
        setNotesBudget(loaded.notes_budget_tokens);
        setRecallBudget(loaded.recall_budget_tokens);
      })
      .catch((err) => setSettingsError(errorMessage(err)));
    listConversationSummaries()
      .then(setConversations)
      .catch((err) => setError(errorMessage(err)));
  }, []);

  const refresh = useCallback(async () => {
    setListBusy(true);
    try {
      const listed = await listMemoryEntries({
        status: statusFilter || undefined,
        agentId: agentFilter || undefined,
        channelId: channelFilter || undefined,
      });
      setEntries(listed);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setListBusy(false);
    }
  }, [statusFilter, agentFilter, channelFilter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const visibleEntries = useMemo(
    () => entries.filter((entry) => matchesScope(entry, scopeFilter)),
    [entries, scopeFilter],
  );
  const pending = visibleEntries.filter((entry) => entry.status === "pending");
  const listed = visibleEntries.filter((entry) => entry.status !== "pending");

  async function handleSettingsSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSettingsBusy(true);
    setSettingsError(null);
    setSettingsNotice(null);
    try {
      const saved = await updateMemorySettings({
        extractionEnabled: extraction,
        recallEnabled: recall,
        notesBudgetTokens: notesBudget,
        recallBudgetTokens: recallBudget,
      });
      setSettings(saved);
      setSettingsNotice("Memory settings saved.");
    } catch (err) {
      setSettingsError(errorMessage(err));
    } finally {
      setSettingsBusy(false);
    }
  }

  async function handleAdd(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setAddBusy(true);
    setError(null);
    setNotice(null);
    try {
      const saved = await addMemoryEntry(newContent, newKind, {
        agentId: newAgent || undefined,
        channelId: newChannel || undefined,
        pinned: newPinned,
      });
      setNewContent("");
      setNotice(
        saved.redacted
          ? `Added a memory entry; credentials were redacted.`
          : "Added a memory entry.",
      );
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setAddBusy(false);
    }
  }

  function beginEdit(entry: MemoryEntry) {
    setEditingId(entry.id);
    setEditContent(entry.content);
    setEditKind(entry.kind);
    setEditPinned(entry.pinned);
  }

  async function saveEdit(id: string) {
    setRowBusy(id);
    setError(null);
    setNotice(null);
    try {
      const saved = await updateMemoryEntry(id, {
        content: editContent,
        kind: editKind,
        pinned: editPinned,
      });
      setEditingId(null);
      setNotice(
        saved.redacted
          ? "Memory entry saved; credentials were redacted."
          : "Memory entry saved.",
      );
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setRowBusy(null);
    }
  }

  async function runEntryAction(
    id: string,
    action: () => Promise<unknown>,
    message: string,
  ) {
    setRowBusy(id);
    setError(null);
    setNotice(null);
    try {
      await action();
      setNotice(message);
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setRowBusy(null);
    }
  }

  function renderEntry(entry: MemoryEntry) {
    const busy = rowBusy === entry.id;
    const editing = editingId === entry.id;
    return (
      <li key={entry.id} className="memory-entry">
        <div className="memory-entry-head">
          <span className="settings-badge">{entry.kind}</span>
          <span className={`settings-badge memory-status-${entry.status}`}>
            {entry.status}
          </span>
          {entry.pinned ? <span className="settings-badge">pinned</span> : null}
          <span className="settings-meta-inline">
            {scopeLabel(entry, agents, conversations)}
          </span>
        </div>
        {editing ? (
          <div className="memory-entry-edit">
            <textarea
              value={editContent}
              rows={3}
              disabled={busy}
              onChange={(event) => setEditContent(event.target.value)}
            />
            <div className="memory-entry-controls">
              <select
                value={editKind}
                disabled={busy}
                aria-label="Memory kind"
                onChange={(event) =>
                  setEditKind(event.target.value as MemoryKind)
                }
              >
                {kinds.map((kind) => (
                  <option key={kind} value={kind}>
                    {kind}
                  </option>
                ))}
              </select>
              <label className="context-toggle">
                <input
                  type="checkbox"
                  checked={editPinned}
                  disabled={busy}
                  onChange={(event) => setEditPinned(event.target.checked)}
                />
                <span>Pinned</span>
              </label>
              <button
                type="button"
                className="primary-button"
                disabled={busy || !editContent.trim()}
                onClick={() => void saveEdit(entry.id)}
              >
                {busy ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                className="ghost-button"
                disabled={busy}
                onClick={() => setEditingId(null)}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <p className="memory-entry-content">{entry.content}</p>
        )}
        <div className="memory-entry-foot">
          <span className="settings-meta-inline">
            {entry.source} · recalled {entry.recall_count}× · updated{" "}
            {formatDateTime(entry.updated_at)}
          </span>
          <div className="memory-entry-actions">
            {entry.status === "pending" ? (
              <>
                <button
                  type="button"
                  className="primary-button"
                  disabled={busy}
                  onClick={() =>
                    void runEntryAction(
                      entry.id,
                      () => approveMemoryEntry(entry.id),
                      "Candidate approved.",
                    )
                  }
                >
                  Approve
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  disabled={busy}
                  onClick={() =>
                    void runEntryAction(
                      entry.id,
                      () => dismissMemoryEntry(entry.id),
                      "Candidate dismissed.",
                    )
                  }
                >
                  Dismiss
                </button>
              </>
            ) : null}
            <button
              type="button"
              className="text-button"
              disabled={busy}
              onClick={() => beginEdit(entry)}
            >
              Edit
            </button>
            <button
              type="button"
              className="text-button"
              disabled={busy}
              onClick={() =>
                void runEntryAction(
                  entry.id,
                  () =>
                    updateMemoryEntry(entry.id, { pinned: !entry.pinned }),
                  entry.pinned ? "Entry unpinned." : "Entry pinned.",
                )
              }
            >
              {entry.pinned ? "Unpin" : "Pin"}
            </button>
            {entry.status !== "archived" ? (
              <button
                type="button"
                className="text-button"
                disabled={busy}
                onClick={() =>
                  void runEntryAction(
                    entry.id,
                    () => archiveMemoryEntry(entry.id),
                    "Entry archived.",
                  )
                }
              >
                Archive
              </button>
            ) : null}
            <button
              type="button"
              className="text-button danger-text"
              disabled={busy}
              onClick={() =>
                void runEntryAction(
                  entry.id,
                  () => deleteMemoryEntry(entry.id),
                  "Entry deleted.",
                )
              }
            >
              Delete
            </button>
          </div>
        </div>
      </li>
    );
  }

  return (
    <>
      <section className="settings-section" aria-labelledby="settings-memory-note">
        <div className="settings-section-head">
          <h3 id="settings-memory-note">Notes</h3>
          <p className="settings-hint">
            Notes are shared reference text injected into every prompt in their
            scope. Credential-shaped text is redacted on save.
          </p>
        </div>
        <NotesEditor label="Workspace note" />
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-behavior">
        <div className="settings-section-head">
          <h3 id="settings-memory-behavior">Recall and extraction</h3>
          <p className="settings-hint">
            Recall injects matching notes and entries into prompts. Extraction
            asks the conversation model for up to three pending candidates after
            a turn; nothing extracted is injected until you approve it.
          </p>
        </div>
        <form className="settings-form" onSubmit={(event) => void handleSettingsSave(event)}>
          <label className="context-toggle">
            <input
              type="checkbox"
              checked={recall}
              disabled={settingsBusy}
              onChange={(event) => setRecall(event.target.checked)}
            />
            <span>Recall memory in prompts</span>
          </label>
          <label className="context-toggle">
            <input
              type="checkbox"
              checked={extraction}
              disabled={settingsBusy}
              onChange={(event) => setExtraction(event.target.checked)}
            />
            <span>Extract pending candidates after conversation turns</span>
          </label>
          <div className="memory-budgets">
            <label className="field">
              <span>Notes budget (tokens)</span>
              <input
                type="number"
                min={100}
                max={8000}
                step={100}
                value={notesBudget}
                disabled={settingsBusy}
                onChange={(event) => setNotesBudget(Number(event.target.value))}
              />
            </label>
            <label className="field">
              <span>Recall budget (tokens)</span>
              <input
                type="number"
                min={100}
                max={8000}
                step={100}
                value={recallBudget}
                disabled={settingsBusy}
                onChange={(event) => setRecallBudget(Number(event.target.value))}
              />
            </label>
          </div>
          <div className="settings-form-actions">
            <button type="submit" className="primary-button" disabled={settingsBusy}>
              {settingsBusy ? "Saving…" : "Save memory settings"}
            </button>
            {settings ? (
              <span className="settings-meta-inline">
                Updated {formatDateTime(settings.updated_at)}
              </span>
            ) : null}
          </div>
          {settingsNotice ? (
            <p className="settings-notice" role="status">
              {settingsNotice}
            </p>
          ) : null}
          {settingsError ? (
            <p className="dialog-error" role="alert">
              {settingsError}
            </p>
          ) : null}
        </form>
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-add">
        <div className="settings-section-head">
          <h3 id="settings-memory-add">Add a memory</h3>
          <p className="settings-hint">
            Leave agent and conversation empty for a workspace-shared entry.
            Pick both to scope it to one agent in one conversation.
          </p>
        </div>
        <form className="settings-form" onSubmit={(event) => void handleAdd(event)}>
          <label className="field">
            <span>Content</span>
            <textarea
              value={newContent}
              rows={3}
              required
              disabled={addBusy}
              onChange={(event) => setNewContent(event.target.value)}
            />
          </label>
          <div className="memory-budgets">
            <label className="field">
              <span>Kind</span>
              <select
                value={newKind}
                disabled={addBusy}
                onChange={(event) => setNewKind(event.target.value as MemoryKind)}
              >
                {kinds.map((kind) => (
                  <option key={kind} value={kind}>
                    {kind}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>Agent</span>
              <select
                value={newAgent}
                disabled={addBusy}
                onChange={(event) => setNewAgent(event.target.value)}
              >
                <option value="">Shared (no agent)</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>Conversation</span>
              <select
                value={newChannel}
                disabled={addBusy}
                onChange={(event) => setNewChannel(event.target.value)}
              >
                <option value="">Workspace-wide</option>
                {conversations.map((conversation) => (
                  <option
                    key={conversation.channel.id}
                    value={conversation.channel.id}
                  >
                    {conversation.channel.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label className="context-toggle">
            <input
              type="checkbox"
              checked={newPinned}
              disabled={addBusy}
              onChange={(event) => setNewPinned(event.target.checked)}
            />
            <span>Pin this entry</span>
          </label>
          <div className="settings-form-actions">
            <button
              type="submit"
              className="primary-button"
              disabled={addBusy || !newContent.trim()}
            >
              {addBusy ? "Adding…" : "Add memory"}
            </button>
          </div>
        </form>
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-entries">
        <div className="settings-section-head">
          <h3 id="settings-memory-entries">Memory entries</h3>
          <p className="settings-hint">
            Archived entries stay visible but are never injected. Pinned entries
            are recalled before scored entries.
          </p>
        </div>
        <div className="memory-filters">
          <label className="field">
            <span>Status</span>
            <select
              value={statusFilter}
              onChange={(event) =>
                setStatusFilter(event.target.value as MemoryStatus | "")
              }
            >
              <option value="">All statuses</option>
              <option value="active">Active</option>
              <option value="pending">Pending</option>
              <option value="archived">Archived</option>
            </select>
          </label>
          <label className="field">
            <span>Scope</span>
            <select
              value={scopeFilter}
              onChange={(event) =>
                setScopeFilter(event.target.value as ScopeFilter)
              }
            >
              {scopeOptions.map((option) => (
                <option key={option.id} value={option.id}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Agent</span>
            <select
              value={agentFilter}
              onChange={(event) => setAgentFilter(event.target.value)}
            >
              <option value="">All agents</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Conversation</span>
            <select
              value={channelFilter}
              onChange={(event) => setChannelFilter(event.target.value)}
            >
              <option value="">All conversations</option>
              {conversations.map((conversation) => (
                <option
                  key={conversation.channel.id}
                  value={conversation.channel.id}
                >
                  {conversation.channel.name}
                </option>
              ))}
            </select>
          </label>
        </div>

        {notice ? (
          <p className="settings-notice" role="status">
            {notice}
          </p>
        ) : null}
        {error ? (
          <p className="dialog-error" role="alert">
            {error}
          </p>
        ) : null}

        {listBusy && visibleEntries.length === 0 ? (
          <p className="settings-empty">Loading memory…</p>
        ) : (
          <>
            <h4 className="memory-group-title">Pending review</h4>
            {pending.length === 0 ? (
              <p className="settings-empty">No pending candidates.</p>
            ) : (
              <ul className="memory-list">{pending.map(renderEntry)}</ul>
            )}

            <h4 className="memory-group-title">Entries</h4>
            {listed.length === 0 ? (
              <p className="settings-empty">No entries match these filters.</p>
            ) : (
              <ul className="memory-list">{listed.map(renderEntry)}</ul>
            )}
          </>
        )}
      </section>
    </>
  );
}
