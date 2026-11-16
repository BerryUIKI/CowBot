import { useCallback, useEffect, useState } from "react";

import {
  getLlmPromptSnapshot,
  getPromptSnapshotSettings,
  listRecentPromptSnapshots,
  listRunPromptSnapshots,
  purgePromptSnapshots,
  updatePromptSnapshotSettings,
} from "./api";
import { slotLabels } from "./ContextInspector";
import { errorMessage } from "./errors";
import { formatDateTime, formatTime, formatTokens } from "./format";
import { t, type TranslationKey } from "./i18n/catalog";
import type {
  LlmCallKind,
  PromptSnapshotDetail,
  PromptSnapshotSettings,
  PromptSnapshotSummary,
} from "./types";
import Modal from "./Modal";

const callKindLabels: Record<LlmCallKind, TranslationKey> = {
  agent: "snapshot.callKind.agent",
  router: "snapshot.callKind.router",
  summary: "snapshot.callKind.summary",
  compact: "snapshot.callKind.compact",
  memory_extract: "snapshot.callKind.memoryExtraction",
  side_question: "snapshot.callKind.sideQuestion",
};

const roleLabels: Record<string, TranslationKey> = {
  system: "context.inspector.roleSystem",
  user: "context.inspector.roleUser",
  assistant: "context.inspector.roleAssistant",
  tool: "context.inspector.roleTool",
};

/** One-line label for a stored snapshot list row. */
function summaryLabel(summary: PromptSnapshotSummary): string {
  return t("snapshot.summaryLine", {
    kind: t(callKindLabels[summary.call_kind]),
    model: summary.model,
    tokens: formatTokens(summary.token_count),
  });
}

/**
 * Read-only viewer for one recorded request (ADR-0037).
 *
 * It renders the stored document directly. A snapshot is the request that was
 * actually sent; it is never rebuilt from current settings.
 */
export function PromptSnapshotViewer({
  callId,
  onClose,
}: {
  callId: string;
  onClose: () => void;
}) {
  const [detail, setDetail] = useState<PromptSnapshotDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    getLlmPromptSnapshot(callId)
      .then((next) => {
        if (!cancelled) setDetail(next);
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [callId]);

  const snapshot = detail?.snapshot;

  return (
    <Modal
      title={t("snapshot.title")}
      onClose={onClose}
      footer={
        <button type="button" className="ghost-button" onClick={onClose}>
          {t("snapshot.close")}
        </button>
      }
    >
      {loading ? <p className="muted">{t("snapshot.loading")}</p> : null}
      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}
      {!loading && !error && !snapshot ? (
        <p className="muted">
          {t("snapshot.none")}
        </p>
      ) : null}

      {snapshot && detail ? (
        <div className="snapshot-viewer">
          <dl className="snapshot-meta">
            <div>
              <dt>{t("snapshot.kind")}</dt>
              <dd>{t(callKindLabels[snapshot.call_kind])}</dd>
            </div>
            <div>
              <dt>{t("snapshot.model")}</dt>
              <dd>{snapshot.model}</dd>
            </div>
            <div>
              <dt>{t("snapshot.tokens")}</dt>
              <dd>{formatTokens(snapshot.token_count)}</dd>
            </div>
            <div>
              <dt>{t("snapshot.recordedAt")}</dt>
              <dd>{formatDateTime(snapshot.created_at)}</dd>
            </div>
            <div>
              <dt>{t("snapshot.storedSize")}</dt>
              <dd>{formatTokens(detail.summary.byte_size)}</dd>
            </div>
          </dl>

          {detail.hash_mismatch ? (
            <p className="dialog-error" role="alert">
              {t("snapshot.hashMismatch")}
            </p>
          ) : null}
          {snapshot.redacted ? (
            <p className="snapshot-note">
              {t("snapshot.redactedNote")}
            </p>
          ) : null}
          {snapshot.truncated ? (
            <p className="snapshot-note">
              {t("snapshot.truncatedNote", {
                count: snapshot.omitted_messages,
              })}
            </p>
          ) : null}

          {(snapshot.slots?.length ?? 0) > 0 ? (
            <section className="snapshot-section">
              <h4>{t("snapshot.slotsTitle")}</h4>
              <ul className="prompt-slots">
                {(snapshot.slots ?? []).map((slot) => (
                  <li key={slot.name}>
                    <details>
                      <summary>
                        <span>{t(slotLabels[slot.name])}</span>
                        <span className="muted">{formatTokens(slot.tokens)}</span>
                      </summary>
                      <pre className="context-inspector-content">
                        {slot.content}
                      </pre>
                    </details>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <section className="snapshot-section">
            <h4>{t("snapshot.messagesTitle")}</h4>
            <ol className="prompt-messages">
              {(snapshot.messages ?? []).map((message, index) => (
                <li key={`${index}-${message.role}`}>
                  <details>
                    <summary>
                      <span className={`prompt-role prompt-role-${message.role}`}>
                        {roleLabels[message.role]
                          ? t(roleLabels[message.role])
                          : message.role}
                      </span>
                      {message.name ? (
                        <span className="muted">{message.name}</span>
                      ) : null}
                      <span className="prompt-message-label">
                        {(message.content.split("\n").find((line) => line.trim()) ??
                          "").slice(0, 90)}
                      </span>
                    </summary>
                    <pre className="context-inspector-content">
                      {message.content || t("snapshot.noText")}
                    </pre>
                    {(message.tool_calls?.length ?? 0) > 0 ? (
                      <ul className="snapshot-tool-calls">
                        {(message.tool_calls ?? []).map((call) => (
                          <li key={call.id}>
                            <span className="tool-name">{call.name}</span>
                            <pre className="context-inspector-content">
                              {JSON.stringify(call.arguments, null, 2)}
                            </pre>
                          </li>
                        ))}
                      </ul>
                    ) : null}
                    {message.tool_call_id ? (
                      <p className="muted">
                        {t("snapshot.answersCall", {
                          id: message.tool_call_id,
                        })}
                      </p>
                    ) : null}
                  </details>
                </li>
              ))}
            </ol>
          </section>

          <section className="snapshot-section">
            <h4>{t("snapshot.toolsTitle")}</h4>
            {snapshot.tools.length === 0 ? (
              <p className="muted">{t("snapshot.noTools")}</p>
            ) : (
              <ul className="prompt-slots">
                {snapshot.tools.map((tool) => (
                  <li key={tool.name}>
                    <details>
                      <summary>
                        <span>{tool.name}</span>
                      </summary>
                      <p className="muted">{tool.description}</p>
                      <pre className="context-inspector-content">
                        {JSON.stringify(tool.parameters, null, 2)}
                      </pre>
                    </details>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {snapshot.memory ? (
            <section className="snapshot-section">
              <h4>{t("snapshot.memoryTitle")}</h4>
              <pre className="context-inspector-content">
                {snapshot.memory.block}
              </pre>
              {(snapshot.selected_memory_ids?.length ?? 0) > 0 ? (
                <p className="muted">
                  {t("snapshot.selectedEntries", {
                    ids: (snapshot.selected_memory_ids ?? []).join(", "),
                  })}
                </p>
              ) : null}
            </section>
          ) : null}

          <p className="snapshot-hash">
            <span className="muted">{t("snapshot.contentHash")}</span>{" "}
            <code>{snapshot.content_hash}</code>
          </p>
        </div>
      ) : null}
    </Modal>
  );
}

/** The recorded requests of one run, opened one at a time. */
export function RunPromptSnapshots({ runId }: { runId: string }) {
  const [summaries, setSummaries] = useState<PromptSnapshotSummary[] | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [openCall, setOpenCall] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSummaries(null);
    setError(null);
    listRunPromptSnapshots(runId)
      .then((next) => {
        if (!cancelled) setSummaries(next);
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, [runId]);

  if (error) {
    return (
      <p className="muted" role="alert">
        {error}
      </p>
    );
  }
  if (!summaries) {
    return <p className="muted">{t("snapshot.loadingRun")}</p>;
  }
  if (summaries.length === 0) {
    return (
      <p className="muted">
        {t("snapshot.noneForRun")}
      </p>
    );
  }

  return (
    <>
      <ul className="snapshot-list">
        {summaries.map((summary) => (
          <li key={summary.llm_call_id}>
            <button
              type="button"
              className="snapshot-row"
              onClick={() => setOpenCall(summary.llm_call_id)}
            >
              <span className="snapshot-kind">
                {t(callKindLabels[summary.call_kind])}
              </span>
              <span className="muted">{summary.model}</span>
              <span className="muted">{formatTokens(summary.token_count)}</span>
              {summary.redacted ? (
                <span className="settings-badge">
                  {t("snapshot.redactedBadge")}
                </span>
              ) : null}
              {summary.truncated ? (
                <span className="settings-badge">
                  {t("snapshot.truncatedBadge")}
                </span>
              ) : null}
              <time className="summary-time">
                {formatTime(summary.created_at)}
              </time>
            </button>
          </li>
        ))}
      </ul>
      {openCall ? (
        <PromptSnapshotViewer
          callId={openCall}
          onClose={() => setOpenCall(null)}
        />
      ) : null}
    </>
  );
}

/** A run's recorded requests, loaded only when the user asks for them. */
export function RunPromptSnapshotsToggle({ runId }: { runId: string }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="run-snapshots">
      <button
        type="button"
        className="ghost-button"
        aria-expanded={shown}
        onClick={() => setShown((value) => !value)}
      >
        {shown
          ? t("snapshot.hideRecorded")
          : t("snapshot.recorded")}
      </button>
      {shown ? <RunPromptSnapshots runId={runId} /> : null}
    </div>
  );
}

/** Settings → Audit: retention controls and the newest recorded requests. */
export function PromptSnapshotAuditPanel() {
  const [settings, setSettings] = useState<PromptSnapshotSettings | null>(null);
  const [summaries, setSummaries] = useState<PromptSnapshotSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [purged, setPurged] = useState<number | null>(null);
  const [openCall, setOpenCall] = useState<string | null>(null);
  const [enabledDraft, setEnabledDraft] = useState(true);
  const [daysDraft, setDaysDraft] = useState(30);

  const load = useCallback(async () => {
    setError(null);
    const [nextSettings, nextSummaries] = await Promise.all([
      getPromptSnapshotSettings(),
      listRecentPromptSnapshots(50),
    ]);
    setSettings(nextSettings);
    setEnabledDraft(nextSettings.enabled);
    setDaysDraft(nextSettings.retention_days);
    setSummaries(nextSummaries);
  }, []);

  useEffect(() => {
    load().catch((err) => setError(errorMessage(err)));
  }, [load]);

  async function save() {
    setBusy(true);
    setError(null);
    setPurged(null);
    try {
      const next = await updatePromptSnapshotSettings(
        enabledDraft,
        daysDraft,
      );
      setSettings(next);
      setDaysDraft(next.retention_days);
      setSummaries(await listRecentPromptSnapshots(50));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function purge() {
    setBusy(true);
    setError(null);
    try {
      const removed = await purgePromptSnapshots();
      setPurged(removed);
      setSummaries(await listRecentPromptSnapshots(50));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="snapshot-audit">
      <p className="muted">
        {t("snapshot.auditIntro")}
      </p>

      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}

      <div className="snapshot-settings">
        <label className="context-toggle">
          <input
            type="checkbox"
            checked={enabledDraft}
            onChange={(event) => setEnabledDraft(event.target.checked)}
          />
          <span>{t("snapshot.store")}</span>
        </label>
        <label className="field">
          <span>{t("snapshot.keepFor")}</span>
          <input
            type="number"
            min={1}
            max={3650}
            value={daysDraft}
            onChange={(event) => setDaysDraft(Number(event.target.value))}
          />
        </label>
        <div className="form-actions">
          <button
            type="button"
            className="primary-button"
            onClick={() => void save()}
            disabled={busy}
          >
            {busy ? t("snapshot.saving") : t("snapshot.save")}
          </button>
          <button
            type="button"
            className="ghost-button"
            onClick={() => void purge()}
            disabled={busy}
          >
            {t("snapshot.purgeExpired")}
          </button>
        </div>
        {settings ? (
          <p className="muted">
            {t("snapshot.expiryNote")}
          </p>
        ) : null}
        {purged !== null ? (
          <p className="muted">
            {t(
              purged === 1 ? "snapshot.purgedOne" : "snapshot.purgedOther",
              { count: purged },
            )}
          </p>
        ) : null}
      </div>

      <section className="snapshot-section">
        <h4>{t("snapshot.recentTitle")}</h4>
        {summaries.length === 0 ? (
          <p className="muted">{t("snapshot.noneYet")}</p>
        ) : (
          <ul className="snapshot-list">
            {summaries.map((summary) => (
              <li key={summary.llm_call_id}>
                <button
                  type="button"
                  className="snapshot-row"
                  onClick={() => setOpenCall(summary.llm_call_id)}
                >
                  <span className="snapshot-kind">
                    {t(callKindLabels[summary.call_kind])}
                  </span>
                  <span className="muted">{summaryLabel(summary)}</span>
                  <span className="muted">
                    {summary.task_id
                      ? t("snapshot.taskRef", {
                          id: summary.task_id.slice(0, 8),
                        })
                      : summary.channel_id
                        ? t("snapshot.channelRef", {
                            id: summary.channel_id.slice(0, 8),
                          })
                        : t("snapshot.workspaceRef")}
                  </span>
                  <time className="summary-time">
                    {formatDateTime(summary.created_at)}
                  </time>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {openCall ? (
        <PromptSnapshotViewer
          callId={openCall}
          onClose={() => setOpenCall(null)}
        />
      ) : null}
    </div>
  );
}
