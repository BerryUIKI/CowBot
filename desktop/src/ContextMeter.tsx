import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import { formatTokens } from "./format";
import type {
  CacheUsageSummary,
  ConversationContext,
  ConversationParticipantView,
} from "./types";

interface ContextMeterProps {
  context: ConversationContext;
  busy: boolean;
  onCompact: (agentId?: string) => void;
  onClear: (agentId?: string) => void;
  onAutoCompactChange: (enabled: boolean) => void;
  onThresholdChange: (threshold: number) => void;
  onRouterChange: (enabled: boolean) => void;
  onDefaultResponderChange: (agentId: string) => void;
  onNewConversation: () => void;
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
    active_entries: context.active_entries,
    excluded_entries: context.excluded_entries,
    percent_used: context.percent_used,
    state: context.state,
    cache_last_run: context.cache_last_turn,
    cache_conversation: context.cache_conversation,
  };
}

/**
 * Context ring and popover for a user-created conversation.
 *
 * The ring shows the highest occupancy among participants. The popover lists
 * one tab per participant with that branch's token breakdown, cache figures,
 * routing settings, and per-branch compaction actions.
 */
export default function ContextMeter({
  context,
  busy,
  onCompact,
  onClear,
  onAutoCompactChange,
  onThresholdChange,
  onRouterChange,
  onDefaultResponderChange,
  onNewConversation,
}: ContextMeterProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
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
              ? `${formatTokens(worst.total_tokens)} tokens in the fullest branch (${current.agent_name})`
              : `${Math.round(percent)}% used in the fullest participant branch`
            : percent === null
              ? `${formatTokens(worst.total_tokens)} tokens in context`
              : `${Math.round(percent)}% of the usable context window`
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
            <p className="context-ratio">
              {context.usable_tokens !== null
                ? `${formatTokens(current.total_tokens)} / ${formatTokens(
                    context.usable_tokens,
                  )} usable tokens`
                : `${formatTokens(current.total_tokens)} tokens · set the model context window in Settings → Models to see a percentage`}
            </p>

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

            <p className="context-meta">
              {context.model}
              {context.provider_id ? ` · ${context.provider_id}` : ""} ·{" "}
              {current.active_entries} active
              {current.excluded_entries > 0
                ? `, ${current.excluded_entries} excluded`
                : ""}
            </p>

            <div className="context-actions">
              <button
                type="button"
                className="primary-button"
                disabled={busy}
                onClick={() => onCompact(group ? current.agent_id : undefined)}
              >
                {busy ? "Compacting…" : "Compact now"}
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
