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
import type {
  LlmCallKind,
  PromptSnapshotDetail,
  PromptSnapshotSettings,
  PromptSnapshotSummary,
} from "./types";
import Modal from "./Modal";

const callKindLabels: Record<LlmCallKind, string> = {
  agent: "Agent",
  router: "Router",
  summary: "Summary",
  compact: "Compaction",
  memory_extract: "Memory extraction",
  side_question: "Side question",
};

const roleLabels: Record<string, string> = {
  system: "System",
  user: "User",
  assistant: "Assistant",
  tool: "Tool",
};

/** One-line label for a stored snapshot list row. */
function summaryLabel(summary: PromptSnapshotSummary): string {
  return `${callKindLabels[summary.call_kind]} · ${summary.model} · ${formatTokens(
    summary.token_count,
  )}`;
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
      title="Recorded prompt"
      onClose={onClose}
      footer={
        <button type="button" className="ghost-button" onClick={onClose}>
          Close
        </button>
      }
    >
      {loading ? <p className="muted">Loading the snapshot…</p> : null}
      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}
      {!loading && !error && !snapshot ? (
        <p className="muted">
          This call has no stored snapshot. Prompt snapshots may be turned off
          for the workspace, or the call happened before they were recorded.
        </p>
      ) : null}

      {snapshot && detail ? (
        <div className="snapshot-viewer">
          <dl className="snapshot-meta">
            <div>
              <dt>Kind</dt>
              <dd>{callKindLabels[snapshot.call_kind]}</dd>
            </div>
            <div>
              <dt>Model</dt>
              <dd>{snapshot.model}</dd>
            </div>
            <div>
              <dt>Tokens</dt>
              <dd>{formatTokens(snapshot.token_count)}</dd>
            </div>
            <div>
              <dt>Recorded</dt>
              <dd>{formatDateTime(snapshot.created_at)}</dd>
            </div>
            <div>
              <dt>Stored size</dt>
              <dd>{formatTokens(detail.summary.byte_size)}</dd>
            </div>
          </dl>

          {detail.hash_mismatch ? (
            <p className="dialog-error" role="alert">
              The stored document no longer matches its content hash. Treat it
              as untrusted evidence.
            </p>
          ) : null}
          {snapshot.redacted ? (
            <p className="snapshot-note">
              Credential-shaped content was removed before storage.
            </p>
          ) : null}
          {snapshot.truncated ? (
            <p className="snapshot-note">
              The payload was shortened to fit the size bound.
              {snapshot.omitted_messages > 0
                ? ` ${snapshot.omitted_messages} older message${
                    snapshot.omitted_messages === 1 ? "" : "s"
                  } were omitted.`
                : ""}
            </p>
          ) : null}

          {(snapshot.slots?.length ?? 0) > 0 ? (
            <section className="snapshot-section">
              <h4>System prompt slots</h4>
              <ul className="prompt-slots">
                {(snapshot.slots ?? []).map((slot) => (
                  <li key={slot.name}>
                    <details>
                      <summary>
                        <span>{slotLabels[slot.name] ?? slot.name}</span>
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
            <h4>Messages</h4>
            <ol className="prompt-messages">
              {(snapshot.messages ?? []).map((message, index) => (
                <li key={`${index}-${message.role}`}>
                  <details>
                    <summary>
                      <span className={`prompt-role prompt-role-${message.role}`}>
                        {roleLabels[message.role] ?? message.role}
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
                      {message.content || "(no text)"}
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
                      <p className="muted">Answers call {message.tool_call_id}</p>
                    ) : null}
                  </details>
                </li>
              ))}
            </ol>
          </section>

          <section className="snapshot-section">
            <h4>Tools</h4>
            {snapshot.tools.length === 0 ? (
              <p className="muted">This request sent no tool schemas.</p>
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
              <h4>Memory</h4>
              <pre className="context-inspector-content">
                {snapshot.memory.block}
              </pre>
              {(snapshot.selected_memory_ids?.length ?? 0) > 0 ? (
                <p className="muted">
                  Selected entries: {(snapshot.selected_memory_ids ?? []).join(", ")}
                </p>
              ) : null}
            </section>
          ) : null}

          <p className="snapshot-hash">
            <span className="muted">Content hash</span>{" "}
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
    return <p className="muted">Loading recorded prompts…</p>;
  }
  if (summaries.length === 0) {
    return (
      <p className="muted">
        No prompt snapshots recorded for this run. They may be turned off in
        Settings → Audit, or the run predates them.
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
                {callKindLabels[summary.call_kind]}
              </span>
              <span className="muted">{summary.model}</span>
              <span className="muted">{formatTokens(summary.token_count)}</span>
              {summary.redacted ? (
                <span className="settings-badge">Redacted</span>
              ) : null}
              {summary.truncated ? (
                <span className="settings-badge">Truncated</span>
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
        {shown ? "Hide recorded prompts" : "Recorded prompts"}
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
        Every model call can store the request it sent — messages, tool
        schemas, prompt slots, and the memory it selected — redacted before
        storage. Snapshots are device-local evidence and never leave this
        workspace.
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
          <span>Store prompt snapshots</span>
        </label>
        <label className="field">
          <span>Keep for (days)</span>
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
            {busy ? "Saving…" : "Save"}
          </button>
          <button
            type="button"
            className="ghost-button"
            onClick={() => void purge()}
            disabled={busy}
          >
            Purge expired now
          </button>
        </div>
        {settings ? (
          <p className="muted">
            Stored snapshots still expire when recording is off. Usage rows are
            never deleted with them.
          </p>
        ) : null}
        {purged !== null ? (
          <p className="muted">
            Removed {purged} expired snapshot{purged === 1 ? "" : "s"}.
          </p>
        ) : null}
      </div>

      <section className="snapshot-section">
        <h4>Recent recorded prompts</h4>
        {summaries.length === 0 ? (
          <p className="muted">No prompt snapshots recorded yet.</p>
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
                    {callKindLabels[summary.call_kind]}
                  </span>
                  <span className="muted">{summaryLabel(summary)}</span>
                  <span className="muted">
                    {summary.task_id
                      ? `Task ${summary.task_id.slice(0, 8)}`
                      : summary.channel_id
                        ? `Channel ${summary.channel_id.slice(0, 8)}`
                        : "Workspace"}
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
