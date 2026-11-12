import { useEffect, useState } from "react";

import { getConversationContextPreview, getTaskRunContextPreview } from "./api";
import { errorMessage } from "./errors";
import { formatTokens } from "./format";
import type {
  ContextPreview,
  PromptMessage,
  PromptSlotName,
  RecallCandidate,
  RecallSkipReason,
  TokenSource,
} from "./types";

/** What the inspector should preview. */
export interface ContextTarget {
  kind: "conversation" | "run";
  channelId?: string;
  agentId?: string;
  agentName?: string;
  runId?: string;
  title?: string;
  /** Composer draft captured when the inspector opened. */
  draft?: string;
  /** Whether the turn runs in Plan mode, which adds an instruction message. */
  planMode?: boolean;
}

interface ContextInspectorProps {
  target: ContextTarget | null;
  /** Bumped whenever the conversation context changes. */
  version: number;
}

type ContextTab = "prompt" | "recall" | "budget";

const tabLabels: { id: ContextTab; label: string }[] = [
  { id: "prompt", label: "Prompt" },
  { id: "recall", label: "Recall" },
  { id: "budget", label: "Budget" },
];

const slotLabels: Record<PromptSlotName, string> = {
  agent: "Agent profile",
  platform: "Platform context",
  roster: "Participant roster",
  default_responder: "Default responder",
  peer_convention: "Peer convention",
  conversation_convention: "Conversation convention",
  mode_instruction: "Plan mode instruction",
};

const roleLabels: Record<PromptMessage["role"], string> = {
  system: "System",
  user: "User",
  assistant: "Assistant",
  tool: "Tool",
};

const scopeLabels: Record<RecallCandidate["scope"], string> = {
  workspace: "Workspace",
  agent: "Agent",
  conversation: "Conversation",
  agent_in_conversation: "Agent in conversation",
};

const segmentClasses: Record<string, string> = {
  messages: "context-segment-messages",
  memory: "context-segment-memory",
  system: "context-segment-system",
  tools: "context-segment-tools",
};

function sourceLabel(source: TokenSource, counter: string | null): string {
  if (source === "estimate") return "Estimated from characters";
  return `Exact · ${counter ?? source}`;
}

function skipReasonLabel(reason: RecallSkipReason): string {
  if (reason.kind === "inactive") {
    return "Not active: pending and archived entries never enter a prompt.";
  }
  return `Did not fit: only ${formatTokens(reason.remaining_tokens)} recall tokens remained.`;
}

function messageLabel(message: PromptMessage): string {
  const firstLine = message.content.split("\n").find((line) => line.trim()) ?? "";
  return firstLine.length > 90 ? `${firstLine.slice(0, 90)}…` : firstLine;
}

/**
 * Read-only prompt, recall, and budget inspector for one turn or task run.
 *
 * It renders the service's preview directly and never assembles a prompt on
 * the client (ADR-0036).
 */
export default function ContextInspector({
  target,
  version,
}: ContextInspectorProps) {
  const [preview, setPreview] = useState<ContextPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<ContextTab>("prompt");
  const [reload, setReload] = useState(0);

  // Changing the inspected participant or run starts fresh; a draft change
  // within the same target keeps the panel and its active sub-tab.
  const targetKey = target
    ? `${target.kind}:${target.channelId ?? ""}:${target.runId ?? ""}:${
        target.agentId ?? ""
      }`
    : "";

  useEffect(() => {
    setTab("prompt");
    setPreview(null);
    setError(null);
  }, [targetKey]);

  useEffect(() => {
    if (!target) {
      setPreview(null);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    const request =
      target.kind === "run"
        ? getTaskRunContextPreview(target.runId ?? "")
        : getConversationContextPreview(
            target.channelId ?? "",
            target.agentId ?? "",
            target.draft,
            target.planMode ?? false,
          );
    request
      .then((next) => {
        if (!cancelled) setPreview(next);
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
  }, [target, version, reload]);

  if (!target) {
    return (
      <p className="inspector-hint">
        Open a conversation's context meter and choose <strong>Inspect</strong>,
        or open a task run and inspect its context.
      </p>
    );
  }

  const title =
    preview?.title ||
    target.title ||
    (target.kind === "run" ? "Task run" : "Conversation");
  const meta = preview
    ? `${preview.agent_name} · ${preview.plan.model} · ${sourceLabel(
        preview.plan.projection.token_source,
        preview.plan.projection.token_counter,
      )}`
    : target.agentName;

  return (
    <div className="context-inspector">
      <header className="context-inspector-head">
        <div>
          <h3>{title}</h3>
          <p className="context-inspector-meta">{meta}</p>
        </div>
        <button
          type="button"
          className="ghost-button"
          disabled={loading}
          onClick={() => setReload((count) => count + 1)}
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </header>

      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}

      {preview ? (
        <>
          <div className="context-inspector-tabs" role="tablist">
            {tabLabels.map((item) => (
              <button
                key={item.id}
                type="button"
                role="tab"
                aria-selected={tab === item.id}
                className={`context-inspector-tab${tab === item.id ? " is-active" : ""}`}
                onClick={() => setTab(item.id)}
              >
                {item.label}
              </button>
            ))}
          </div>

          {target.kind === "run" ? (
            <p className="context-inspector-note">
              Rebuilt from the agent profile recorded with the run and the
              task's current plan and memory settings. Historical iterations
              are not persisted yet, so the request can differ from what was
              actually sent.
            </p>
          ) : null}

          {tab === "prompt" ? <PromptTab preview={preview} /> : null}
          {tab === "recall" ? <RecallTab preview={preview} /> : null}
          {tab === "budget" ? <BudgetTab preview={preview} /> : null}
        </>
      ) : loading ? (
        <p className="muted">Building the preview…</p>
      ) : null}
    </div>
  );
}

function PromptTab({ preview }: { preview: ContextPreview }) {
  const plan = preview.plan;
  return (
    <div className="context-inspector-body">
      <section className="context-inspector-section">
        <h4>
          System prompt{" "}
          <span>{formatTokens(plan.projection.system_tokens)}</span>
        </h4>
        <ul className="prompt-slots">
          {plan.slots.map((slot) => (
            <li key={slot.name}>
              <details>
                <summary>
                  <span>{slotLabels[slot.name]}</span>
                  <span className="muted">{formatTokens(slot.tokens)}</span>
                </summary>
                <pre className="context-inspector-content">{slot.content}</pre>
              </details>
            </li>
          ))}
        </ul>
      </section>

      <section className="context-inspector-section">
        <h4>
          Messages{" "}
          <span>
            {formatTokens(
              plan.projection.history_tokens + plan.projection.latest_tokens,
            )}
          </span>
        </h4>
        <ol className="prompt-messages">
          {plan.messages.map((message, index) => (
            <li key={`${index}-${message.entry_id ?? message.role}`}>
              <details>
                <summary>
                  <span className={`prompt-role prompt-role-${message.role}`}>
                    {roleLabels[message.role]}
                  </span>
                  {message.latest ? (
                    <span className="prompt-latest">latest</span>
                  ) : null}
                  <span className="prompt-message-label">
                    {messageLabel(message)}
                  </span>
                  <span className="muted">{formatTokens(message.tokens)}</span>
                </summary>
                <pre className="context-inspector-content">
                  {message.content || "(no text)"}
                </pre>
                {message.tool_calls.length > 0 ? (
                  <p className="muted">
                    Tool calls: {message.tool_calls.join(", ")}
                  </p>
                ) : null}
                {message.tool_call_id ? (
                  <p className="muted">Answers call {message.tool_call_id}</p>
                ) : null}
              </details>
            </li>
          ))}
        </ol>
      </section>

      <section className="context-inspector-section">
        <h4>
          Tools <span>{formatTokens(plan.projection.tool_tokens)}</span>
        </h4>
        {plan.tools.length === 0 ? (
          <p className="muted">This request sends no tool schemas.</p>
        ) : (
          <ul className="prompt-slots">
            {plan.tools.map((tool) => (
              <li key={tool.name}>
                <details>
                  <summary>
                    <span>{tool.name}</span>
                    <span className="muted">{formatTokens(tool.tokens)}</span>
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
    </div>
  );
}

function RecallTab({ preview }: { preview: ContextPreview }) {
  const recall = preview.plan.recall;
  if (!recall.enabled) {
    return (
      <div className="context-inspector-body">
        <p className="muted">
          Memory recall is disabled for this workspace. Enable it in Settings →
          Memory to inject notes and recalled entries.
        </p>
      </div>
    );
  }
  const selected = recall.candidates.filter((candidate) => candidate.selected);
  return (
    <div className="context-inspector-body">
      <dl className="context-breakdown">
        <div>
          <dt>Search query</dt>
          <dd>
            {recall.query
              ? `"${recall.query}"`
              : "None (notes and pinned entries only)"}
          </dd>
        </div>
        <div>
          <dt>Selected</dt>
          <dd>
            {selected.length} of {recall.candidates.length} candidates
          </dd>
        </div>
        <div>
          <dt>Selected entries</dt>
          <dd>
            {formatTokens(
              selected.reduce((total, candidate) => total + candidate.tokens, 0),
            )}{" "}
            of {formatTokens(recall.budget_tokens)} recall budget
          </dd>
        </div>
        <div>
          <dt>Notes</dt>
          <dd>
            {formatTokens(recall.notes_tokens)} of{" "}
            {formatTokens(recall.notes_budget_tokens)}
          </dd>
        </div>
        <div>
          <dt>Block as a message</dt>
          <dd>
            {formatTokens(preview.plan.memory?.tokens ?? 0)}
          </dd>
        </div>
      </dl>

      {recall.block ? (
        <section className="context-inspector-section">
          <h4>Injected block</h4>
          <pre className="context-inspector-content">{recall.block}</pre>
        </section>
      ) : (
        <p className="muted">
          Nothing would be injected: there are no notes and no selected entries.
        </p>
      )}

      <section className="context-inspector-section">
        <h4>Candidates</h4>
        {recall.candidates.length === 0 ? (
          <p className="muted">
            No pinned entries or search matches were considered.
          </p>
        ) : (
          <ul className="recall-candidates">
            {recall.candidates.map((candidate) => (
              <li
                key={candidate.entry_id}
                className={candidate.selected ? "is-selected" : "is-skipped"}
              >
                <div className="recall-head">
                  <span
                    className={`recall-status ${
                      candidate.selected ? "is-selected" : "is-skipped"
                    }`}
                  >
                    {candidate.selected ? "Selected" : "Skipped"}
                  </span>
                  <span className="muted">
                    {candidate.kind} · {scopeLabels[candidate.scope]} ·{" "}
                    {formatTokens(candidate.tokens)}
                  </span>
                  {candidate.pinned ? (
                    <span className="muted">pinned</span>
                  ) : null}
                  {candidate.score != null ? (
                    <span className="muted">
                      score {candidate.score.toFixed(2)}
                    </span>
                  ) : null}
                </div>
                <p className="recall-content">{candidate.content}</p>
                {candidate.skip_reason ? (
                  <p className="recall-reason">
                    {skipReasonLabel(candidate.skip_reason)}
                  </p>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function BudgetTab({ preview }: { preview: ContextPreview }) {
  const plan = preview.plan;
  const projection = plan.projection;
  const memoryTokens = plan.memory?.tokens ?? 0;
  const segments = [
    {
      id: "messages",
      label: "Messages",
      tokens: projection.history_tokens + projection.latest_tokens,
    },
    { id: "memory", label: "Memory", tokens: memoryTokens },
    { id: "system", label: "System prompt", tokens: projection.system_tokens },
    { id: "tools", label: "Tools", tokens: projection.tool_tokens },
  ];
  const usable = preview.usable_tokens;
  const compactAt =
    plan.auto_compact && usable !== null
      ? Math.round((usable * plan.compact_threshold) / 100)
      : null;
  const remaining = usable === null ? null : Math.max(0, usable - preview.request_tokens);
  const untilCompact =
    compactAt === null ? null : Math.max(0, compactAt - preview.request_tokens);

  return (
    <div className="context-inspector-body">
      {preview.over_budget ? (
        <div className="context-full-warning" role="alert">
          <strong>Over budget.</strong> This request is{" "}
          {formatTokens(preview.request_tokens)} tokens against a usable window
          of {formatTokens(usable ?? 0)}. Sending would exceed the model's
          context; compact or clear the branch first.
        </div>
      ) : null}

      {usable !== null ? (
        <>
          <div
            className="context-bar"
            role="img"
            aria-label={`Request uses ${formatTokens(
              preview.request_tokens,
            )} of ${formatTokens(usable)} tokens`}
          >
            {segments
              .filter((segment) => segment.tokens > 0)
              .map((segment) => (
                <span
                  key={segment.id}
                  className={`context-bar-segment ${segmentClasses[segment.id]}`}
                  style={{ width: `${(segment.tokens / usable) * 100}%` }}
                />
              ))}
            {!preview.over_budget ? (
              <span
                className="context-bar-segment context-bar-free"
                style={{ width: `${((remaining ?? 0) / usable) * 100}%` }}
              />
            ) : null}
          </div>
          <ul className="context-legend">
            {segments.map((segment) => (
              <li key={segment.id}>
                <span
                  className={`context-swatch ${segmentClasses[segment.id]}`}
                  aria-hidden="true"
                />
                <span className="context-legend-label">{segment.label}</span>
                <span className="context-legend-value">
                  {formatTokens(segment.tokens)} ·{" "}
                  {Math.round((segment.tokens / usable) * 100)}%
                </span>
              </li>
            ))}
            <li>
              <span
                className="context-swatch context-bar-free"
                aria-hidden="true"
              />
              <span className="context-legend-label">Free space</span>
              <span className="context-legend-value">
                {formatTokens(remaining ?? 0)}
              </span>
            </li>
          </ul>
        </>
      ) : null}

      <dl className="context-breakdown">
        <div>
          <dt>System prompt</dt>
          <dd>{formatTokens(projection.system_tokens)}</dd>
        </div>
        <div>
          <dt>Tools</dt>
          <dd>{formatTokens(projection.tool_tokens)}</dd>
        </div>
        <div>
          <dt>History</dt>
          <dd>{formatTokens(projection.history_tokens)}</dd>
        </div>
        <div>
          <dt>Latest message</dt>
          <dd>{formatTokens(projection.latest_tokens)}</dd>
        </div>
        <div>
          <dt>Memory</dt>
          <dd>{formatTokens(memoryTokens)}</dd>
        </div>
        <div>
          <dt>Request total</dt>
          <dd>{formatTokens(preview.request_tokens)}</dd>
        </div>
        <div>
          <dt>Usable window</dt>
          <dd>
            {usable === null ? "Unknown" : formatTokens(usable)}
          </dd>
        </div>
        <div>
          <dt>Used</dt>
          <dd>
            {preview.percent_used === null
              ? "Unknown"
              : `${Math.round(preview.percent_used)}% (${preview.state})`}
          </dd>
        </div>
        <div>
          <dt>Automatic compaction</dt>
          <dd>
            {plan.auto_compact
              ? `at ${plan.compact_threshold}%${
                  untilCompact === null
                    ? ""
                    : ` · ${formatTokens(untilCompact)} left`
                }`
              : "Off"}
          </dd>
        </div>
        <div>
          <dt>Counting</dt>
          <dd>
            {sourceLabel(projection.token_source, projection.token_counter)}
          </dd>
        </div>
      </dl>
    </div>
  );
}
