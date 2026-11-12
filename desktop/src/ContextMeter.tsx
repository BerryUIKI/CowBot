import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import NotesEditor from "./NotesEditor";
import { formatTokens } from "./format";
import type {
  CacheUsageSummary,
  ConversationContext,
  ConversationParticipantView,
} from "./types";

interface ContextMeterProps {
  context: ConversationContext;
  busy: boolean;
  /** Increment to open the popover from outside, such as from \`/note\`. */
  openSignal?: number;
  onCompact: (agentId?: string) => void;
  onClear: (agentId?: string) => void;
  onAutoCompactChange: (enabled: boolean) => void;
  onThresholdChange: (threshold: number) => void;
  onRouterChange: (enabled: boolean) => void;
  onDefaultResponderChange: (agentId: string) => void;
  onMemoryChanged: () => void;
  onNewConversation: () => void;
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
  if (rate === null) return "No usage recorded";
  return `${Math.round(rate * 100)}% cached · ${formatTokens(
    summary.cached_input_tokens,
  )} of ${formatTokens(summary.total_input_tokens)} input`;
}

/** How the meter's token numbers were counted. */
function tokenSourceLabel(context: ConversationContext): string {
  if (context.token_source === "estimate") return "Estimated from characters";
  const counter = context.token_counter ?? context.token_source;
  return `Exact · ${counter}`;
}

/** View values for a backend that predates per-participant contexts. */
function fallbackParticipant(context: ConversationContext): ConversationParticipantView {
  return {
    agent_id: context.agent_id,
    agent_name: "",
    system_tokens: context.system_tokens,
    tool_tokens: context.tool_tokens,
    history_tokens: context.history_tokens,
    latest_tokens: context.latest_tokens,
    total_tokens: context.total_tokens,
    memory_tokens: context.memory_tokens,
    active_entries: context.active_entries,
    excluded_entries: context.excluded_entries,
    percent_used: context.percent_used,
    state: context.state,
    cache_last_run: context.cache_last_turn,
    cache_conversation: context.cache_conversation,
  };
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
      label: "Messages",
      tokens: current.history_tokens + current.latest_tokens,
    },
    { id: "memory", label: "Memory", tokens: current.memory_tokens },
    { id: "system", label: "System prompt", tokens: current.system_tokens },
    { id: "tools", label: "Tools", tokens: current.tool_tokens },
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
  onCompact,
  onClear,
  onAutoCompactChange,
  onThresholdChange,
  onRouterChange,
  onDefaultResponderChange,
  onMemoryChanged,
  onNewConversation,
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

  const participants =
    context.participants.length > 0
      ? context.participants
      : [fallbackParticipant(context)];
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
    ? `exact count${context.token_counter ? ` (${context.token_counter})` : ""}`
    : "estimated count";
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
    <div className="context-meter-wrap" ref={rootRef}>
      <button
        type="button"
        className={`context-meter context-${state}`}
        aria-label="Conversation context usage"
        aria-expanded={open}
        title={
          group
            ? percent === null
              ? `${formatTokens(worst.total_tokens)} tokens in the fullest branch (${current.agent_name}) · ${sourceNote}`
              : `${Math.round(percent)}% used in the fullest participant branch · ${sourceNote}`
            : percent === null
              ? `${formatTokens(worst.total_tokens)} tokens in context · ${sourceNote}`
              : `${Math.round(percent)}% of the usable context window · ${sourceNote}`
        }
        onClick={() => setOpen((currentOpen) => !currentOpen)}
      >
        {percent === null ? (
          <span className="context-absolute">
            <strong>{formatTokens(worst.total_tokens)}</strong> tokens
          </span>
        ) : (
          <>
            <svg className="context-ring" viewBox="0 0 32 32" aria-hidden="true">
              <circle className="context-ring-track" cx="16" cy="16" r={RADIUS} />
              <circle
                className="context-ring-fill"
                cx="16"
                cy="16"
                r={RADIUS}
                strokeDasharray={CIRCUMFERENCE}
                strokeDashoffset={CIRCUMFERENCE * (1 - fraction)}
              />
            </svg>
            <span className="context-percent">{Math.round(percent)}%</span>
          </>
        )}
      </button>

      {open ? (
        <div
          className="context-popover"
          role="dialog"
          aria-label="Context usage"
        >
          <div className="context-popover-head">
            <h3>Context</h3>
            <span className={`context-state context-${state}`}>
              {percent === null
                ? "Window unknown"
                : `${Math.round(percent)}% used · ${Math.max(
                    0,
                    Math.round(100 - percent),
                  )}% left`}
            </span>
          </div>

          {group ? (
            <div className="context-tabs" role="tablist" aria-label="Participants">
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
                  {participant.agent_name || "Participant"}
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
              <>
                <div className="context-summary">
                  <span>
                    {formatTokens(current.total_tokens)} /{" "}
                    {formatTokens(bar.usable)}
                    {current.percent_used === null
                      ? ""
                      : ` (${Math.round(current.percent_used)}%)`}
                  </span>
                  <span className="context-until">
                    {bar.untilAutoCompact === null
                      ? "Automatic compaction off"
                      : `${formatTokens(bar.untilAutoCompact)} until auto-compact`}
                  </span>
                  <button
                    type="button"
                    className="ghost-button"
                    disabled={busy}
                    onClick={() => onCompact(group ? current.agent_id : undefined)}
                  >
                    {busy ? "Compacting…" : "Compact now"}
                  </button>
                </div>

                <div
                  className="context-bar"
                  role="img"
                  aria-label={`Context used: ${formatTokens(
                    bar.used,
                  )} of ${formatTokens(bar.usable)} tokens`}
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

                <ul className="context-legend">
                  {bar.segments.map((segment) => (
                    <li key={segment.id}>
                      <span
                        className={`context-swatch ${segmentClasses[segment.id]}`}
                        aria-hidden="true"
                      />
                      <span className="context-legend-label">
                        {segment.id === "memory" && exact
                          ? "Memory (estimated)"
                          : segment.label}
                      </span>
                      <span className="context-legend-value">
                        {formatTokens(segment.tokens)} ·{" "}
                        {Math.round(segment.percent)}%
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
                      {formatTokens(bar.freeTokens)} ·{" "}
                      {Math.round((bar.freeTokens / bar.usable) * 100)}%
                    </span>
                  </li>
                  {context.auto_compact ? (
                    <li>
                      <span
                        className="context-swatch context-bar-buffer"
                        aria-hidden="true"
                      />
                      <span className="context-legend-label">
                        Auto-compact buffer
                      </span>
                      <span className="context-legend-value">
                        {formatTokens(bar.bufferTokens)} ·{" "}
                        {Math.round((bar.bufferTokens / bar.usable) * 100)}%
                      </span>
                    </li>
                  ) : null}
                </ul>
              </>
            ) : (
              <p className="context-ratio">
                {formatTokens(current.total_tokens)} tokens · set the model
                context window in Settings → Models to see a percentage
              </p>
            )}

            {full ? (
              <div className="context-full-warning" role="alert">
                <strong>Context full.</strong>{" "}
                {group
                  ? `${current.agent_name || "This participant"} is skipped until this branch is compacted or cleared.`
                  : "Sending is blocked until you compact, clear, or start a new conversation."}
                <div className="context-actions">
                  <button
                    type="button"
                    className="primary-button"
                    disabled={busy}
                    onClick={() => onCompact(group ? current.agent_id : undefined)}
                  >
                    Compact now
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={onNewConversation}
                  >
                    New conversation
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
              {showDetails ? "Hide detailed breakdown" : "See detailed breakdown"}
            </button>

            {showDetails ? (
              <>
                <dl className="context-breakdown">
                  <div>
                    <dt>System prompt</dt>
                    <dd>{formatTokens(current.system_tokens)}</dd>
                  </div>
                  <div>
                    <dt>Tools</dt>
                    <dd>{formatTokens(current.tool_tokens)}</dd>
                  </div>
                  <div>
                    <dt>History</dt>
                    <dd>{formatTokens(current.history_tokens)}</dd>
                  </div>
                  <div>
                    <dt>Latest message</dt>
                    <dd>{formatTokens(current.latest_tokens)}</dd>
                  </div>
                  <div>
                    <dt>
                      {exact
                        ? "Memory (next turn, estimated)"
                        : "Memory (next turn)"}
                    </dt>
                    <dd>{formatTokens(current.memory_tokens)}</dd>
                  </div>
                </dl>

                <div className="context-cache">
                  <h4>Cache</h4>
                  <p>
                    <span>{group ? "Latest run" : "Last turn"}</span>
                    {cacheLabel(current.cache_last_run)}
                  </p>
                  <p>
                    <span>{group ? "Conversation total" : "Conversation"}</span>
                    {cacheLabel(current.cache_conversation)}
                  </p>
                </div>
              </>
            ) : null}

            {group ? (
              <div className="context-routing">
                <h4>Routing</h4>
                <label className="context-toggle">
                  <input
                    type="checkbox"
                    checked={context.router_enabled}
                    disabled={busy}
                    onChange={(event) => onRouterChange(event.target.checked)}
                  />
                  <span>Route unaddressed messages with the model</span>
                </label>
                <label className="context-default-responder">
                  <span>Default responder</span>
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
                        None (unaddressed messages stay silent)
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
                  Participants are fixed at creation. Start a new conversation
                  for a different team.
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
                <span>Compact automatically</span>
              </label>
              <label className="context-threshold">
                <span>at</span>
                <select
                  value={context.compact_threshold}
                  disabled={busy || !context.auto_compact}
                  onChange={(event) => onThresholdChange(Number(event.target.value))}
                  aria-label="Automatic compaction threshold percent"
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
              label="Conversation note"
              hint="Shared with every participant and injected before this branch."
              onChanged={onMemoryChanged}
            />

            <p className="context-meta">
              {context.model}
              {context.provider_id ? ` · ${context.provider_id}` : ""} ·{" "}
              {current.active_entries} active
              {current.excluded_entries > 0
                ? `, ${current.excluded_entries} excluded`
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
                Inspect
              </button>
              <button
                type="button"
                className="ghost-button"
                disabled={busy}
                onClick={() => onClear(group ? current.agent_id : undefined)}
              >
                {group ? "Clear this branch" : "Clear context"}
              </button>
              {group ? (
                <button
                  type="button"
                  className="ghost-button"
                  disabled={busy}
                  onClick={() => onClear()}
                >
                  Clear all
                </button>
              ) : null}
              <button
                type="button"
                className="ghost-button"
                onClick={onNewConversation}
              >
                New conversation
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
