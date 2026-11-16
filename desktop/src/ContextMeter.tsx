import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import NotesEditor from "./NotesEditor";
import { formatTokens } from "./format";
import { t } from "./i18n/catalog";
import type {
  CacheUsageSummary,
  ConversationContext,
  ConversationParticipantView,
  RoutingMode,
} from "./types";

interface ContextMeterProps {
  context: ConversationContext;
  busy: boolean;
  /** Increment to open the popover from outside, such as from \`/note\`. */
  openSignal?: number;
  placement?: "header" | "composer";
  onCompact: (agentId?: string) => void;
  onClear: (agentId?: string) => void;  onAutoCompactChange: (enabled: boolean) => void;  onThresholdChange: (threshold: number) => void;
  onRouterChange: (enabled: boolean) => void;
  onDefaultResponderChange: (agentId: string) => void;
  onRoutingModeChange: (mode: RoutingMode) => void;
  onMutedChange: (agentIds: string[]) => void;
  onReplyCleanupChange: (enabled: boolean) => void;
  onMemoryChanged: () => void;
  onNewConversation: () => void;
  /** Open Settings → Models, where a model's context window is set. */
  onOpenModelSettings: () => void;
  /** Open the read-only prompt and recall inspector for one participant. */
  onInspect: (agentId: string) => void;
}

const RADIUS = 13;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

const stateRank: Record<ConversationParticipantView["state"], number> = {
  unknown: 0,
  neutral: 1,
  warning: 2,
  danger: 3,
  full: 4,
};

const segmentClasses: Record<string, string> = {
  messages: "context-segment-messages",
  memory: "context-segment-memory",
  system: "context-segment-system",
  tools: "context-segment-tools",
};

function cacheRate(summary: CacheUsageSummary): number | null {
  if (summary.total_input_tokens <= 0) return null;
  return summary.cached_input_tokens / summary.total_input_tokens;
}

function cacheLabel(summary: CacheUsageSummary): string {
  const rate = cacheRate(summary);
  if (rate === null) return t("context.meter.cacheNone");
  return t("context.meter.cacheValue", {
    percent: Math.round(rate * 100),
    cached: formatTokens(summary.cached_input_tokens),
    total: formatTokens(summary.total_input_tokens),
  });
}

/** How the meter's token numbers were counted. */
function tokenSourceLabel(context: ConversationContext): string {
  if (context.token_source === "estimate") {
    return t("context.meter.sourceEstimated");
  }
  const counter = context.token_counter ?? context.token_source;
  return t("context.meter.sourceExactWithCounter", { counter });
}

interface ContextSegment {
  id: string;
  label: string;
  tokens: number;
  percent: number;
}

interface ContextBar {
  segments: ContextSegment[];
  used: number;
  usable: number;
  freeTokens: number;
  bufferTokens: number;
  untilAutoCompact: number | null;
}

/** The colored stacked bar values for one participant's next request. */
function buildContextBar(
  current: ConversationParticipantView,
  context: ConversationContext,
): ContextBar | null {
  const usable = context.usable_tokens;
  if (usable === null || usable <= 0) return null;
  const segments: ContextSegment[] = [
    {
      id: "messages",
      label: t("context.meter.messages"),
      tokens: current.history_tokens + current.latest_tokens,
    },
    { id: "memory", label: t("context.meter.memoryNext"), tokens: current.memory_tokens },
    {
      id: "system",
      label: t("context.meter.systemPrompt"),
      tokens: current.system_tokens,
    },
    { id: "tools", label: t("context.meter.tools"), tokens: current.tool_tokens },
  ].map((segment) => ({
    ...segment,
    percent: (segment.tokens / usable) * 100,
  }));
  const used = segments.reduce((total, segment) => total + segment.tokens, 0);
  const threshold = context.auto_compact
    ? Math.round((usable * context.compact_threshold) / 100)
    : null;
  const freeTokens =
    threshold === null
      ? Math.max(0, usable - used)
      : Math.max(0, threshold - used);
  const bufferTokens =
    threshold === null ? 0 : Math.max(0, usable - Math.max(used, threshold));
  const untilAutoCompact =
    threshold === null ? null : Math.max(0, threshold - used);
  return {
    segments,
    used,
    usable,
    freeTokens,
    bufferTokens,
    untilAutoCompact,
  };
}

/**
 * Context ring and popover for a user-created conversation.
 *
 * The ring shows the highest occupancy among participants. The popover lists
 * one tab per participant with a colored stacked bar, a labeled legend, the
 * remaining tokens until auto-compaction, the conversation note, routing
 * settings, and per-branch compaction actions.
 */
export default function ContextMeter({
  context,
  busy,
  openSignal = 0,
  placement = "header",
  onCompact,
  onClear,
  onAutoCompactChange,
  onThresholdChange,
  onRouterChange,
  onDefaultResponderChange,
  onRoutingModeChange,
  onMutedChange,
  onReplyCleanupChange,
  onMemoryChanged,
  onNewConversation,
  onOpenModelSettings,
  onInspect,
}: ContextMeterProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [showDetails, setShowDetails] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  // An outside request, such as `/note`, opens the popover for the editor.
  useEffect(() => {
    if (openSignal > 0) setOpen(true);
  }, [openSignal]);

  const participants = context.participants;
  const group = participants.length > 1;
  const index = Math.min(activeIndex, participants.length - 1);
  const current = participants[index];
  const worst = participants.reduce(
    (selected, participant) =>
      stateRank[participant.state] > stateRank[selected.state] ? participant : selected,
    participants[0],
  );

  const percent = worst.percent_used;
  const fraction = percent === null ? 0 : Math.min(percent, 100) / 100;
  const state = worst.state;
  const full = current.state === "full";
  const bar = buildContextBar(current, context);
  // Memory is always counted with the character estimator, so an exact
  // context meter labels that segment separately (ADR-0035).
  const exact = context.token_source === "tiktoken" || context.token_source === "provider";
  const sourceNote = exact
    ? context.token_counter
      ? t("context.meter.sourceExactWithCounter", {
          counter: context.token_counter,
        })
      : t("context.meter.sourceExact")
    : t("context.meter.sourceEstimated");
  const tabId = (position: number) => `context-tab-${context.channel_id}-${position}`;
  const panelId = `context-panel-${context.channel_id}`;

  function focusTab(position: number) {
    window.requestAnimationFrame(() => {
      document.getElementById(tabId(position))?.focus();
    });
  }

  function handleTabKey(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    position: number,
  ) {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next = (position + delta + participants.length) % participants.length;
    setActiveIndex(next);
    focusTab(next);
  }

  return (
    <div
      className={`context-meter-wrap${
        placement === "composer" ? " composer-context" : ""
      }`}
      ref={rootRef}
    >
      <button
        type="button"
        className={`context-meter context-${state}`}
        aria-label={t("context.meter.ringAria")}
        aria-expanded={open}
        title={
          group
            ? percent === null
              ? t("context.meter.groupTitleUnknown", {
                  tokens: formatTokens(worst.total_tokens),
                  name: current.agent_name,
                  source: sourceNote,
                })
              : t("context.meter.groupTitle", {
                  percent: Math.round(percent),
                  source: sourceNote,
                })
            : percent === null
              ? t("context.meter.singleTitleUnknown", {
                  tokens: formatTokens(worst.total_tokens),
                  source: sourceNote,
                })
              : t("context.meter.singleTitle", {
                  percent: Math.round(percent),
                  source: sourceNote,
                })
        }
        onClick={() => setOpen((currentOpen) => !currentOpen)}
      >
        <svg
          className={`context-ring${percent === null ? " context-ring-unknown" : ""}`}
          viewBox="0 0 32 32"
          aria-hidden="true"
        >
          <circle className="context-ring-track" cx="16" cy="16" r={RADIUS} />
          {percent === null ? null : (
            <circle
              className="context-ring-fill"
              cx="16"
              cy="16"
              r={RADIUS}
              strokeDasharray={CIRCUMFERENCE}
              strokeDashoffset={CIRCUMFERENCE * (1 - fraction)}
            />
          )}
        </svg>
      </button>

      {open ? (
        <div
          className="context-popover"
          role="dialog"
          aria-label={t("context.meter.popoverAria")}
        >
          <div className="context-popover-head">
            <h3>{t("context.meter.windowTitle")}</h3>
            <span className="context-head-usage">
              {bar && current.percent_used !== null
                ? t("context.meter.headUsage", {
                    used: formatTokens(bar.used),
                    usable: formatTokens(bar.usable),
                    percent: Math.round(current.percent_used),
                  })
                : t("context.meter.headTokens", {
                    tokens: formatTokens(current.total_tokens),
                  })}
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                aria-hidden="true"
              >
                <path d="m9 6 6 6-6 6" />
              </svg>
            </span>
          </div>

          {group ? (
            <div className="context-tabs" role="tablist" aria-label={t("context.meter.participantsAria")}>
              {participants.map((participant, position) => (
                <button
                  key={participant.agent_id}
                  id={tabId(position)}
                  type="button"
                  role="tab"
                  aria-selected={position === index}
                  aria-controls={panelId}
                  tabIndex={position === index ? 0 : -1}
                  className={`context-tab context-${participant.state}${
                    position === index ? " is-active" : ""
                  }`}
                  onClick={() => setActiveIndex(position)}
                  onKeyDown={(event) => handleTabKey(event, position)}
                >
                  {participant.agent_name || t("context.meter.participantFallback")}
                </button>
              ))}
            </div>
          ) : null}

          <div
            id={panelId}
            role={group ? "tabpanel" : undefined}
            aria-labelledby={group ? tabId(index) : undefined}
          >
            {bar ? (
              <div
                className="context-bar"
                role="img"
                aria-label={t("context.meter.barAria", {
                  used: formatTokens(bar.used),
                  usable: formatTokens(bar.usable),
                })}
              >
                {bar.segments
                  .filter((segment) => segment.tokens > 0)
                    .map((segment) => (
                      <span
                        key={segment.id}
                        className={`context-bar-segment ${segmentClasses[segment.id]}`}
                        style={{ width: `${segment.percent}%` }}
                      />
                    ))}
                  {bar.freeTokens > 0 ? (
                    <span
                      className="context-bar-segment context-bar-free"
                      style={{ width: `${(bar.freeTokens / bar.usable) * 100}%` }}
                    />
                  ) : null}
                  {bar.bufferTokens > 0 ? (
                    <span
                      className="context-bar-segment context-bar-buffer"
                      style={{
                        width: `${(bar.bufferTokens / bar.usable) * 100}%`,
                      }}
                    />
                  ) : null}
                </div>
            ) : (
              <div className="context-unknown">
                <p className="context-ratio">
                  {t("context.meter.unknownText", {
                    tokens: formatTokens(current.total_tokens),
                  })}
                </p>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => {
                    setOpen(false);
                    onOpenModelSettings();
                  }}
                >
                  {t("context.meter.setWindow")}
                </button>
              </div>
            )}

            {full ? (
              <div className="context-full-warning" role="alert">
                <strong>{t("context.meter.fullTitle")}</strong>{" "}
                {group
                  ? t("context.meter.fullGroup", {
                      name:
                        current.agent_name ||
                        t("context.meter.participantFallback"),
                    })
                  : t("context.meter.fullSingle")}
                <div className="context-actions">
                  <button
                    type="button"
                    className="primary-button"
                    disabled={busy}
                    onClick={() => onCompact(group ? current.agent_id : undefined)}
                  >
                    {t("context.meter.compactNow")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={onNewConversation}
                  >
                    {t("context.meter.newConversation")}
                  </button>
                </div>
              </div>
            ) : null}

            <button
              type="button"
              className="text-button context-details-toggle"
              aria-expanded={showDetails}
              onClick={() => setShowDetails((shown) => !shown)}
            >
              {showDetails
                ? t("context.meter.hideDetails")
                : t("context.meter.showDetails")}
            </button>

            {showDetails ? (
              <>
                <dl className="context-breakdown">
                  <div>
                    <dt>{t("context.meter.systemPrompt")}</dt>
                    <dd>{formatTokens(current.system_tokens)}</dd>
                  </div>
                  <div>
                    <dt>{t("context.meter.tools")}</dt>
                    <dd>{formatTokens(current.tool_tokens)}</dd>
                  </div>
                  <div>
                    <dt>{t("context.meter.history")}</dt>
                    <dd>{formatTokens(current.history_tokens)}</dd>
                  </div>
                  <div>
                    <dt>{t("context.meter.latestMessage")}</dt>
                    <dd>{formatTokens(current.latest_tokens)}</dd>
                  </div>
                  <div>
                    <dt>
                      {exact
                        ? t("context.meter.memoryNextEstimated")
                        : t("context.meter.memoryNext")}
                    </dt>
                    <dd>{formatTokens(current.memory_tokens)}</dd>
                  </div>
                </dl>

                <div className="context-cache">
                  <h4>{t("context.meter.cacheTitle")}</h4>
                  <p>
                    <span>
                      {group
                        ? t("context.meter.latestRun")
                        : t("context.meter.lastTurn")}
                    </span>
                    {cacheLabel(current.cache_last_run)}
                  </p>
                  <p>
                    <span>
                      {group
                        ? t("context.meter.conversationTotal")
                        : t("context.meter.conversation")}
                    </span>
                    {cacheLabel(current.cache_conversation)}
                  </p>
                </div>
                            {group ? (
                              <div className="context-routing">
                                <h4>{t("context.meter.routingTitle")}</h4>
                                <label className="context-default-responder">
                                  <span>{t("context.meter.unaddressedQuestion")}</span>
                                  <select
                                    value={context.routing_mode}
                                    disabled={busy}
                                    onChange={(event) =>
                                      onRoutingModeChange(event.target.value as RoutingMode)
                                    }
                                  >
                                    <option value="mention">
                                      {t("context.meter.routingMention")}
                                    </option>
                                    <option value="rotation">
                                      {t("context.meter.routingRotation")}
                                    </option>
                                    <option value="default">
                                      {t("context.meter.routingDefault")}
                                    </option>
                                  </select>
                                </label>
                                {context.routing_mode === "mention" ? (
                                  <label className="context-toggle">
                                    <input
                                      type="checkbox"
                                      checked={context.router_enabled}
                                      disabled={busy}
                                      onChange={(event) => onRouterChange(event.target.checked)}
                                    />
                                    <span>{t("context.meter.routeWithModel")}</span>
                                  </label>
                                ) : null}
                                <div className="context-mute">
                                  <span>{t("context.meter.mute")}</span>
                                  <div className="context-mute-list">
                                    {participants.map((participant) => {
                                      const muted = context.muted_participants.includes(
                                        participant.agent_id,
                                      );
                                      return (
                                        <label key={participant.agent_id} className="context-mute-item">
                                          <input
                                            type="checkbox"
                                            checked={muted}
                                            disabled={busy}
                                            onChange={() =>
                                              onMutedChange(
                                                muted
                                                  ? context.muted_participants.filter(
                                                      (id) => id !== participant.agent_id,
                                                    )
                                                  : [...context.muted_participants, participant.agent_id],
                                              )
                                            }
                                          />
                                          <span>{participant.agent_name || participant.agent_id}</span>
                                        </label>
                                      );
                                    })}
                                  </div>
                                  <p className="context-hint">
                                    {t("context.meter.muteHint")}
                                  </p>
                                </div>
                                <label className="context-toggle">
                                  <input
                                    type="checkbox"
                                    checked={context.reply_cleanup}
                                    disabled={busy}
                                    onChange={(event) => onReplyCleanupChange(event.target.checked)}
                                  />
                                  <span>{t("context.meter.stripLabels")}</span>
                                </label>
                                <label className="context-default-responder">
                                  <span>{t("context.meter.defaultResponder")}</span>
                                  <select
                                    value={context.default_responder ?? ""}
                                    disabled={busy}
                                    onChange={(event) => {
                                      if (event.target.value) {
                                        onDefaultResponderChange(event.target.value);
                                      }
                                    }}
                                  >
                                    {context.default_responder === null ? (
                                      <option value="" disabled>
                                        {t("context.meter.noneSilent")}
                                      </option>
                                    ) : null}
                                    {participants.map((participant) => (
                                      <option
                                        key={participant.agent_id}
                                        value={participant.agent_id}
                                      >
                                        {participant.agent_name || participant.agent_id}
                                      </option>
                                    ))}
                                  </select>
                                </label>
                                <p className="context-hint">
                                  {t("context.meter.participantsFixed")}
                                </p>
                              </div>
                            ) : null}

                            <div className="context-settings">
                              <label className="context-toggle">
                                <input
                                  type="checkbox"
                                  checked={context.auto_compact}
                                  disabled={busy}
                                  onChange={(event) => onAutoCompactChange(event.target.checked)}
                                />
                                <span>{t("context.meter.autoCompact")}</span>
                              </label>
                              <label className="context-threshold">
                                <span>{t("context.meter.at")}</span>
                                <select
                                  value={context.compact_threshold}
                                  disabled={busy || !context.auto_compact}
                                  onChange={(event) => onThresholdChange(Number(event.target.value))}
                                  aria-label={t("context.meter.thresholdAria")}
                                >
                                  {[60, 70, 80, 90].map((threshold) => (
                                    <option key={threshold} value={threshold}>
                                      {threshold}%
                                    </option>
                                  ))}
                                  {[60, 70, 80, 90].includes(context.compact_threshold) ? null : (
                                    <option value={context.compact_threshold}>
                                      {context.compact_threshold}%
                                    </option>
                                  )}
                                </select>
                              </label>
                            </div>

                            <NotesEditor
                              channelId={context.channel_id}
                              label={t("context.meter.conversationNote")}
                              hint={t("context.meter.conversationNoteHint")}
                              onChanged={onMemoryChanged}
                            />

                            <p className="context-meta">
                              {context.provider_id
                                ? context.model
                                : t("context.meter.offlineModel")}
                              {context.provider_name
                                ? ` · ${context.provider_name}`
                                : ""}{" "}
                              ·{" "}
                              {context.model_follows_default
                                ? t("context.meter.followsDefault")
                                : t("context.meter.pinned")}
                              {" · "}
                              {t("context.meter.activeEntries", {
                                count: current.active_entries,
                              })}
                              {current.excluded_entries > 0
                                ? t("context.meter.excludedEntries", {
                                    count: current.excluded_entries,
                                  })
                                : ""}
                              {" · "}
                              {tokenSourceLabel(context)}
                            </p>

                            <div className="context-actions">
                              <button
                                type="button"
                                className="ghost-button"
                                onClick={() => {
                                  setOpen(false);
                                  onInspect(current.agent_id);
                                }}
                              >
                                {t("context.meter.inspect")}
                              </button>
                              <button
                                type="button"
                                className="ghost-button"
                                disabled={busy}
                                onClick={() => onClear(group ? current.agent_id : undefined)}
                              >
                                {group
                                  ? t("context.meter.clearBranch")
                                  : t("context.meter.clearContext")}
                              </button>
                              {group ? (
                                <button
                                  type="button"
                                  className="ghost-button"
                                  disabled={busy}
                                  onClick={() => onClear()}
                                >
                                  {t("context.meter.clearAll")}
                                </button>
                              ) : null}
                              <button
                                type="button"
                                className="ghost-button"
                                onClick={onNewConversation}
                              >
                                {t("context.meter.newConversation")}
                              </button>
                            </div>
              </>
            ) : null}

          </div>
        </div>
      ) : null}
    </div>
  );
}
