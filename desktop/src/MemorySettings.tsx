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
  previewMemoryRecall,
  updateMemoryEntry,
  updateMemorySettings,
} from "./api";
import { errorMessage } from "./errors";
import { formatDateTime } from "./format";
import { t, type TranslationKey } from "./i18n/catalog";
import type {
  AgentProfile,
  ConversationSummary,
  MemoryEntry,
  MemoryKind,
  MemorySettings,
  MemoryStatus,
  RecalledMemory,
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

const kindKeys: Record<MemoryKind, TranslationKey> = {
  fact: "memory.kind.fact",
  preference: "memory.kind.preference",
  decision: "memory.kind.decision",
  summary: "memory.kind.summary",
  note: "memory.kind.note",
};

const statusKeys: Record<MemoryStatus, TranslationKey> = {
  active: "memory.status.active",
  pending: "memory.status.pending",
  archived: "memory.status.archived",
};

const sourceKeys: Record<string, TranslationKey> = {
  user: "memory.source.user",
  agent: "memory.source.agent",
  extraction: "memory.source.extraction",
  compaction: "memory.source.compaction",
};

const scopeOptions: { id: ScopeFilter; labelKey: TranslationKey }[] = [
  { id: "all", labelKey: "memory.scope.all" },
  { id: "workspace", labelKey: "memory.scope.workspace" },
  { id: "agent", labelKey: "memory.scope.agent" },
  { id: "conversation", labelKey: "memory.scope.conversation" },
  {
    id: "agent-in-conversation",
    labelKey: "memory.scope.agentInConversation",
  },
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
  if (agent && conversation) {
    return t("memory.scope.agentIn", { agent, conversation });
  }
  if (agent) return agent;
  if (conversation) return conversation;
  return t("memory.scope.workspaceLabel");
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

  const [recallAgent, setRecallAgent] = useState("");
  const [recallChannel, setRecallChannel] = useState("");
  const [recallQuery, setRecallQuery] = useState("");
  const [recallPreview, setRecallPreview] = useState<RecalledMemory | null>(null);
  const [recallBusy, setRecallBusy] = useState(false);
  const [recallNotice, setRecallNotice] = useState<string | null>(null);

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
      setSettingsNotice(t("memory.savedSettings"));
    } catch (err) {
      setSettingsError(errorMessage(err));
    } finally {
      setSettingsBusy(false);
    }
  }

  /**
   * Show what the next turn would inject, without writing a recall statistic.
   *
   * This is the only way to see the ranking before it matters: the entries a
   * turn would receive depend on the budget, the scope, and the query, and
   * until now none of that was visible outside a real turn.
   */
  async function handleRecallPreview(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!recallAgent.trim()) {
      setRecallNotice(t("memory.chooseAgentNotice"));
      return;
    }
    setRecallBusy(true);
    setRecallNotice(null);
    try {
      const preview = await previewMemoryRecall(
        recallAgent,
        recallQuery,
        recallChannel || null,
      );
      setRecallPreview(preview);
      if (!preview) setRecallNotice(t("memory.nothingRecalled"));
    } catch (err) {
      setRecallNotice(errorMessage(err));
    } finally {
      setRecallBusy(false);
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
          ? t("memory.addedRedacted")
          : t("memory.added"),
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
          ? t("memory.savedRedacted")
          : t("memory.savedEntry"),
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
          <span className="settings-badge">{t(kindKeys[entry.kind])}</span>
          <span className={`settings-badge memory-status-${entry.status}`}>
            {t(statusKeys[entry.status])}
          </span>
          {entry.pinned ? (
            <span className="settings-badge">{t("memory.pinned")}</span>
          ) : null}
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
                aria-label={t("memory.memoryKindAria")}
                onChange={(event) =>
                  setEditKind(event.target.value as MemoryKind)
                }
              >
                {kinds.map((kind) => (
                  <option key={kind} value={kind}>
                    {t(kindKeys[kind])}
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
                <span>{t("memory.pinned")}</span>
              </label>
              <button
                type="button"
                className="primary-button"
                disabled={busy || !editContent.trim()}
                onClick={() => void saveEdit(entry.id)}
              >
                {busy ? t("memory.saving") : t("memory.save")}
              </button>
              <button
                type="button"
                className="ghost-button"
                disabled={busy}
                onClick={() => setEditingId(null)}
              >
                {t("memory.cancel")}
              </button>
            </div>
          </div>
        ) : (
          <p className="memory-entry-content">{entry.content}</p>
        )}
        <div className="memory-entry-foot">
          <span className="settings-meta-inline">
            {t("memory.stats", {
              source: sourceKeys[entry.source]
                ? t(sourceKeys[entry.source])
                : entry.source,
              count: entry.recall_count,
              date: formatDateTime(entry.updated_at),
            })}
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
                      t("memory.candidateApproved"),
                    )
                  }
                >
                  {t("memory.approve")}
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  disabled={busy}
                  onClick={() =>
                    void runEntryAction(
                      entry.id,
                      () => dismissMemoryEntry(entry.id),
                      t("memory.candidateDismissed"),
                    )
                  }
                >
                  {t("memory.dismiss")}
                </button>
              </>
            ) : null}
            <button
              type="button"
              className="text-button"
              disabled={busy}
              onClick={() => beginEdit(entry)}
            >
              {t("memory.edit")}
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
                  entry.pinned
                    ? t("memory.entryUnpinned")
                    : t("memory.entryPinned"),
                )
              }
            >
              {entry.pinned ? t("memory.unpin") : t("memory.pin")}
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
                    t("memory.entryArchived"),
                  )
                }
              >
                {t("memory.archive")}
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
                  t("memory.entryDeleted"),
                )
              }
            >
              {t("memory.delete")}
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
          <h3 id="settings-memory-note">{t("memory.notesTitle")}</h3>
          <p className="settings-hint">
            {t("memory.notesHint")}
          </p>
        </div>
        <NotesEditor label={t("memory.workspaceNote")} />
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-behavior">
        <div className="settings-section-head">
          <h3 id="settings-memory-behavior">{t("memory.behaviorTitle")}</h3>
          <p className="settings-hint">
            {t("memory.behaviorHint")}
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
            <span>{t("memory.recallInPrompts")}</span>
          </label>
          <label className="context-toggle">
            <input
              type="checkbox"
              checked={extraction}
              disabled={settingsBusy}
              onChange={(event) => setExtraction(event.target.checked)}
            />
            <span>{t("memory.extractCandidates")}</span>
          </label>
          <div className="memory-budgets">
            <label className="field">
              <span>{t("memory.notesBudget")}</span>
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
              <span>{t("memory.recallBudget")}</span>
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
              {settingsBusy
                ? t("memory.saving")
                : t("memory.saveSettings")}
            </button>
            {settings ? (
              <span className="settings-meta-inline">
                {t("memory.updated", {
                  date: formatDateTime(settings.updated_at),
                })}
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

      <section className="settings-section" aria-labelledby="settings-memory-recall">
        <div className="settings-section-head">
          <h3 id="settings-memory-recall">{t("memory.previewTitle")}</h3>
          <p className="settings-hint">
            {t("memory.previewHint")}
          </p>
        </div>
        <form className="settings-form" onSubmit={(event) => void handleRecallPreview(event)}>
          <div className="memory-budgets">
            <label className="field">
              <span>{t("memory.agent")}</span>
              <select
                value={recallAgent}
                disabled={recallBusy}
                onChange={(event) => setRecallAgent(event.target.value)}
              >
                <option value="">{t("memory.chooseAgent")}</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>{t("memory.conversationOptional")}</span>
              <select
                value={recallChannel}
                disabled={recallBusy}
                onChange={(event) => setRecallChannel(event.target.value)}
              >
                <option value="">{t("memory.workspaceOnly")}</option>
                {conversations.map((summary) => (
                  <option key={summary.channel.id} value={summary.channel.id}>
                    {summary.channel.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label className="field">
            <span>{t("memory.query")}</span>
            <input
              value={recallQuery}
              placeholder={t("memory.queryPlaceholder")}
              disabled={recallBusy}
              onChange={(event) => setRecallQuery(event.target.value)}
            />
          </label>
          <div className="settings-form-actions">
            <button type="submit" className="primary-button" disabled={recallBusy}>
              {recallBusy
                ? t("memory.ranking")
                : t("memory.previewRecall")}
            </button>
          </div>
          {recallNotice ? (
            <p className="settings-notice" role="status">
              {recallNotice}
            </p>
          ) : null}
          {recallPreview ? (
            <div className="settings-subsection">
              <p className="settings-hint">
                {t("memory.previewSummary", {
                  tokens: recallPreview.tokens,
                  count: recallPreview.entry_ids.length,
                })}
              </p>
              <pre className="memory-recall-block">{recallPreview.block}</pre>
            </div>
          ) : null}
        </form>
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-add">
        <div className="settings-section-head">
          <h3 id="settings-memory-add">{t("memory.addTitle")}</h3>
          <p className="settings-hint">
            {t("memory.addHint")}
          </p>
        </div>
        <form className="settings-form" onSubmit={(event) => void handleAdd(event)}>
          <label className="field">
            <span>{t("memory.content")}</span>
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
              <span>{t("memory.kindLabel")}</span>
              <select
                value={newKind}
                disabled={addBusy}
                onChange={(event) => setNewKind(event.target.value as MemoryKind)}
              >
                {kinds.map((kind) => (
                  <option key={kind} value={kind}>
                    {t(kindKeys[kind])}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>{t("memory.agent")}</span>
              <select
                value={newAgent}
                disabled={addBusy}
                onChange={(event) => setNewAgent(event.target.value)}
              >
                <option value="">{t("memory.sharedNoAgent")}</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>{t("memory.conversation")}</span>
              <select
                value={newChannel}
                disabled={addBusy}
                onChange={(event) => setNewChannel(event.target.value)}
              >
                <option value="">{t("memory.workspaceWide")}</option>
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
            <span>{t("memory.pinEntry")}</span>
          </label>
          <div className="settings-form-actions">
            <button
              type="submit"
              className="primary-button"
              disabled={addBusy || !newContent.trim()}
            >
              {addBusy ? t("memory.adding") : t("memory.add")}
            </button>
          </div>
        </form>
      </section>

      <section className="settings-section" aria-labelledby="settings-memory-entries">
        <div className="settings-section-head">
          <h3 id="settings-memory-entries">{t("memory.entriesTitle")}</h3>
          <p className="settings-hint">
            {t("memory.entriesHint")}
          </p>
        </div>
        <div className="memory-filters">
          <label className="field">
            <span>{t("memory.statusLabel")}</span>
            <select
              value={statusFilter}
              onChange={(event) =>
                setStatusFilter(event.target.value as MemoryStatus | "")
              }
            >
              <option value="">{t("memory.allStatuses")}</option>
              <option value="active">{t("memory.status.active")}</option>
              <option value="pending">{t("memory.status.pending")}</option>
              <option value="archived">{t("memory.status.archived")}</option>
            </select>
          </label>
          <label className="field">
            <span>{t("memory.scopeLabel")}</span>
            <select
              value={scopeFilter}
              onChange={(event) =>
                setScopeFilter(event.target.value as ScopeFilter)
              }
            >
              {scopeOptions.map((option) => (
                <option key={option.id} value={option.id}>
                  {t(option.labelKey)}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>{t("memory.agent")}</span>
            <select
              value={agentFilter}
              onChange={(event) => setAgentFilter(event.target.value)}
            >
              <option value="">{t("memory.allAgents")}</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>{t("memory.conversation")}</span>
            <select
              value={channelFilter}
              onChange={(event) => setChannelFilter(event.target.value)}
            >
              <option value="">{t("memory.allConversations")}</option>
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
          <p className="settings-empty">{t("memory.loading")}</p>
        ) : (
          <>
            <h4 className="memory-group-title">{t("memory.pendingReview")}</h4>
            {pending.length === 0 ? (
              <p className="settings-empty">{t("memory.noPending")}</p>
            ) : (
              <ul className="memory-list">{pending.map(renderEntry)}</ul>
            )}

            <h4 className="memory-group-title">{t("memory.entries")}</h4>
            {listed.length === 0 ? (
              <p className="settings-empty">{t("memory.noMatch")}</p>
            ) : (
              <ul className="memory-list">{listed.map(renderEntry)}</ul>
            )}
          </>
        )}
      </section>
    </>
  );
}
