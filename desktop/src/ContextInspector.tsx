import { useEffect, useState } from "react";

import { getConversationContextPreview, getTaskRunContextPreview } from "./api";
import { errorMessage } from "./errors";
import { formatTokens } from "./format";
import { t, type TranslationKey } from "./i18n/catalog";
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

const tabLabels: { id: ContextTab; labelKey: TranslationKey }[] = [
  { id: "prompt", labelKey: "context.inspector.tabPrompt" },
  { id: "recall", labelKey: "context.inspector.tabRecall" },
  { id: "budget", labelKey: "context.inspector.tabBudget" },
];

export const slotLabels: Record<PromptSlotName, TranslationKey> = {
  agent: "context.inspector.slotAgent",
  platform: "context.inspector.slotPlatform",
  roster: "context.inspector.slotRoster",
  default_responder: "context.inspector.slotDefaultResponder",
  peer_convention: "context.inspector.slotPeerConvention",
  conversation_convention: "context.inspector.slotConversationConvention",
  mode_instruction: "context.inspector.slotModeInstruction",
  instruction: "context.inspector.slotInstruction",
};

const roleLabels: Record<PromptMessage["role"], TranslationKey> = {
  system: "context.inspector.roleSystem",
  user: "context.inspector.roleUser",
  assistant: "context.inspector.roleAssistant",
  tool: "context.inspector.roleTool",
};

const scopeLabels: Record<RecallCandidate["scope"], TranslationKey> = {
  workspace: "memory.scope.workspaceLabel",
  agent: "memory.scope.agent",
  conversation: "memory.scope.conversation",
  agent_in_conversation: "memory.scope.agentInConversation",
};

const kindLabels: Record<string, TranslationKey> = {
  fact: "memory.kind.fact",
  preference: "memory.kind.preference",
  decision: "memory.kind.decision",
  summary: "memory.kind.summary",
  note: "memory.kind.note",
};

const segmentClasses: Record<string, string> = {
  messages: "context-segment-messages",
  memory: "context-segment-memory",
  system: "context-segment-system",
  tools: "context-segment-tools",
};

function sourceLabel(source: TokenSource, counter: string | null | undefined): string {
  if (source === "estimate") return t("context.inspector.sourceEstimated");
  return t("context.inspector.sourceExact", { counter: counter ?? source });
}

function stateLabel(state: string): string {
  switch (state) {
    case "neutral":
      return t("context.state.neutral");
    case "warning":
      return t("context.state.warning");
    case "danger":
      return t("context.state.danger");
    case "full":
      return t("context.state.full");
    default:
      return t("context.state.unknown");
  }
}

function skipReasonLabel(reason: RecallSkipReason): string {
  if (reason.kind === "inactive") {
    return t("context.inspector.skipInactive");
  }
  return t("context.inspector.skipBudget", {
    tokens: formatTokens(reason.remaining_tokens),
  });
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
        {t("context.inspector.emptyHint")}
      </p>
    );
  }

  const title =
    preview?.title ||
    target.title ||
    (target.kind === "run"
      ? t("context.inspector.taskRun")
      : t("context.inspector.conversation"));
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
          {loading
            ? t("context.inspector.loading")
            : t("context.inspector.refresh")}
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
                {t(item.labelKey)}
              </button>
            ))}
          </div>

          {target.kind === "run" ? (
            <p className="context-inspector-note">
              {t("context.inspector.runNote")}
            </p>
          ) : null}

          {tab === "prompt" ? <PromptTab preview={preview} /> : null}
          {tab === "recall" ? <RecallTab preview={preview} /> : null}
          {tab === "budget" ? <BudgetTab preview={preview} /> : null}
        </>
      ) : loading ? (
        <p className="muted">{t("context.inspector.building")}</p>
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
          {t("context.inspector.systemPrompt")}{" "}
          <span>{formatTokens(plan.projection.system_tokens)}</span>
        </h4>
        <ul className="prompt-slots">
          {plan.slots.map((slot) => (
            <li key={slot.name}>
              <details>
                <summary>
                  <span>{t(slotLabels[slot.name])}</span>
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
          {t("context.inspector.messages")}{" "}
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
                    {t(roleLabels[message.role])}
                  </span>
                  {message.latest ? (
                    <span className="prompt-latest">
                      {t("context.inspector.latest")}
                    </span>
                  ) : null}
                  <span className="prompt-message-label">
                    {messageLabel(message)}
                  </span>
                  <span className="muted">{formatTokens(message.tokens)}</span>
                </summary>
                <pre className="context-inspector-content">
                  {message.content || t("context.inspector.noText")}
                </pre>
                {message.tool_calls.length > 0 ? (
                  <p className="muted">
                    {t("context.inspector.toolCalls", {
                      names: message.tool_calls.join(", "),
                    })}
                  </p>
                ) : null}
                {message.tool_call_id ? (
                  <p className="muted">
                    {t("context.inspector.answersCall", {
                      id: message.tool_call_id,
                    })}
                  </p>
                ) : null}
              </details>
            </li>
          ))}
        </ol>
      </section>

      <section className="context-inspector-section">
        <h4>
          {t("context.inspector.tools")}{" "}
          <span>{formatTokens(plan.projection.tool_tokens)}</span>
        </h4>
        {plan.tools.length === 0 ? (
          <p className="muted">{t("context.inspector.noTools")}</p>
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
          {t("context.inspector.recallDisabled")}
        </p>
      </div>
    );
  }
  const selected = recall.candidates.filter((candidate) => candidate.selected);
  return (
    <div className="context-inspector-body">
      <dl className="context-breakdown">
        <div>
          <dt>{t("context.inspector.searchQuery")}</dt>
          <dd>
            {recall.query
              ? `"${recall.query}"`
              : t("context.inspector.noQuery")}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.selected")}</dt>
          <dd>
            {t("context.inspector.candidatesCount", {
              selected: selected.length,
              total: recall.candidates.length,
            })}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.selectedEntries")}</dt>
          <dd>
            {t("context.inspector.selectedEntriesValue", {
              tokens: formatTokens(
                selected.reduce(
                  (total, candidate) => total + candidate.tokens,
                  0,
                ),
              ),
              budget: formatTokens(recall.budget_tokens),
            })}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.notes")}</dt>
          <dd>
            {t("context.inspector.notesValue", {
              tokens: formatTokens(recall.notes_tokens),
              budget: formatTokens(recall.notes_budget_tokens),
            })}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.blockAsMessage")}</dt>
          <dd>
            {formatTokens(preview.plan.memory?.tokens ?? 0)}
          </dd>
        </div>
      </dl>

      {recall.block ? (
        <section className="context-inspector-section">
          <h4>{t("context.inspector.injectedBlock")}</h4>
          <pre className="context-inspector-content">{recall.block}</pre>
        </section>
      ) : (
        <p className="muted">
          {t("context.inspector.nothingInjected")}
        </p>
      )}

      <section className="context-inspector-section">
        <h4>{t("context.inspector.candidates")}</h4>
        {recall.candidates.length === 0 ? (
          <p className="muted">
            {t("context.inspector.noCandidates")}
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
                    {candidate.selected
                      ? t("context.inspector.selectedChip")
                      : t("context.inspector.skippedChip")}
                  </span>
                  <span className="muted">
                    {t("context.inspector.candidateMeta", {
                      kind: kindLabels[candidate.kind]
                        ? t(kindLabels[candidate.kind])
                        : candidate.kind,
                      scope: t(scopeLabels[candidate.scope]),
                      tokens: formatTokens(candidate.tokens),
                    })}
                  </span>
                  {candidate.pinned ? (
                    <span className="muted">
                      {t("context.inspector.pinned")}
                    </span>
                  ) : null}
                  {candidate.score != null ? (
                    <span className="muted">
                      {t("context.inspector.score", {
                        score: candidate.score.toFixed(2),
                      })}
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
      label: t("context.inspector.messages"),
      tokens: projection.history_tokens + projection.latest_tokens,
    },
    { id: "memory", label: t("context.inspector.memory"), tokens: memoryTokens },
    {
      id: "system",
      label: t("context.inspector.systemPrompt"),
      tokens: projection.system_tokens,
    },
    { id: "tools", label: t("context.inspector.tools"), tokens: projection.tool_tokens },
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
          <strong>{t("context.inspector.overBudgetTitle")}</strong>{" "}
          {t("context.inspector.overBudgetBody", {
            tokens: formatTokens(preview.request_tokens),
            usable: formatTokens(usable ?? 0),
          })}
        </div>
      ) : null}

      {usable !== null ? (
        <>
          <div
            className="context-bar"
            role="img"
            aria-label={t("context.inspector.requestUses", {
              tokens: formatTokens(preview.request_tokens),
              usable: formatTokens(usable),
            })}
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
              <span className="context-legend-label">
                {t("context.inspector.freeSpace")}
              </span>
              <span className="context-legend-value">
                {formatTokens(remaining ?? 0)}
              </span>
            </li>
          </ul>
        </>
      ) : null}

      <dl className="context-breakdown">
        <div>
          <dt>{t("context.inspector.systemPrompt")}</dt>
          <dd>{formatTokens(projection.system_tokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.tools")}</dt>
          <dd>{formatTokens(projection.tool_tokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.history")}</dt>
          <dd>{formatTokens(projection.history_tokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.latestMessage")}</dt>
          <dd>{formatTokens(projection.latest_tokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.memory")}</dt>
          <dd>{formatTokens(memoryTokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.requestTotal")}</dt>
          <dd>{formatTokens(preview.request_tokens)}</dd>
        </div>
        <div>
          <dt>{t("context.inspector.usableWindow")}</dt>
          <dd>
            {usable === null
              ? t("context.inspector.unknown")
              : formatTokens(usable)}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.used")}</dt>
          <dd>
            {preview.percent_used === null
              ? t("context.inspector.unknown")
              : t("context.inspector.usedValue", {
                  percent: Math.round(preview.percent_used),
                  state: stateLabel(preview.state),
                })}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.autoCompaction")}</dt>
          <dd>
            {plan.auto_compact
              ? t("context.inspector.autoCompactionValue", {
                  threshold: plan.compact_threshold,
                  left:
                    untilCompact === null
                      ? ""
                      : t("context.inspector.autoCompactionLeft", {
                          tokens: formatTokens(untilCompact),
                        }),
                })
              : t("context.inspector.off")}
          </dd>
        </div>
        <div>
          <dt>{t("context.inspector.counting")}</dt>
          <dd>
            {sourceLabel(projection.token_source, projection.token_counter)}
          </dd>
        </div>
      </dl>
    </div>
  );
}
