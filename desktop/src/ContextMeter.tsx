import { useEffect, useRef, useState } from "react";

import { formatTokens } from "./format";
import type { CacheUsageSummary, ConversationContext } from "./types";

interface ContextMeterProps {
  context: ConversationContext;
  busy: boolean;
  onCompact: () => void;
  onClear: () => void;
  onAutoCompactChange: (enabled: boolean) => void;
  onThresholdChange: (threshold: number) => void;
  onNewConversation: () => void;
}

const RADIUS = 13;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

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

/**
 * Context ring and popover for a user-created conversation.
 *
 * Known windows show percent used with neutral/warning/danger colours at the
 * 70/90 thresholds. Unknown windows show absolute tokens only.
 */
export default function ContextMeter({
  context,
  busy,
  onCompact,
  onClear,
  onAutoCompactChange,
  onThresholdChange,
  onNewConversation,
}: ContextMeterProps) {
  const [open, setOpen] = useState(false);
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

  const percent = context.percent_used;
  const fraction = percent === null ? 0 : Math.min(percent, 100) / 100;
  const state = context.state;
  const full = state === "full";

  return (
    <div className="context-meter-wrap" ref={rootRef}>
      <button
        type="button"
        className={`context-meter context-${state}`}
        aria-label="Conversation context usage"
        aria-expanded={open}
        title={
          percent === null
            ? `${formatTokens(context.total_tokens)} tokens in context`
            : `${Math.round(percent)}% of the usable context window`
        }
        onClick={() => setOpen((current) => !current)}
      >
        {percent === null ? (
          <span className="context-absolute">
            <strong>{formatTokens(context.total_tokens)}</strong> tokens
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
        <div className="context-popover" role="dialog" aria-label="Context usage">
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

          <p className="context-ratio">
            {context.usable_tokens !== null
              ? `${formatTokens(context.total_tokens)} / ${formatTokens(
                  context.usable_tokens,
                )} usable tokens`
              : `${formatTokens(context.total_tokens)} tokens · set the model context window in Settings → Models to see a percentage`}
          </p>

          {full ? (
            <div className="context-full-warning" role="alert">
              <strong>Context full.</strong> Sending is blocked until you compact,
              clear, or start a new conversation.
              <div className="context-actions">
                <button
                  type="button"
                  className="primary-button"
                  disabled={busy}
                  onClick={onCompact}
                >
                  Compact now
                </button>
                <button type="button" className="ghost-button" onClick={onNewConversation}>
                  New conversation
                </button>
              </div>
            </div>
          ) : null}

          <dl className="context-breakdown">
            <div>
              <dt>System prompt</dt>
              <dd>{formatTokens(context.system_tokens)}</dd>
            </div>
            <div>
              <dt>Tools</dt>
              <dd>{formatTokens(context.tool_tokens)}</dd>
            </div>
            <div>
              <dt>History</dt>
              <dd>{formatTokens(context.history_tokens)}</dd>
            </div>
            <div>
              <dt>Latest message</dt>
              <dd>{formatTokens(context.latest_tokens)}</dd>
            </div>
          </dl>

          <div className="context-cache">
            <h4>Cache</h4>
            <p>
              <span>Last turn</span>
              {cacheLabel(context.cache_last_turn)}
            </p>
            <p>
              <span>Conversation</span>
              {cacheLabel(context.cache_conversation)}
            </p>
          </div>

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
            {context.active_entries} active
            {context.excluded_entries > 0
              ? `, ${context.excluded_entries} excluded`
              : ""}
          </p>

          <div className="context-actions">
            <button
              type="button"
              className="primary-button"
              disabled={busy}
              onClick={onCompact}
            >
              {busy ? "Compacting…" : "Compact now"}
            </button>
            <button
              type="button"
              className="ghost-button"
              disabled={busy}
              onClick={onClear}
            >
              Clear context
            </button>
            <button type="button" className="ghost-button" onClick={onNewConversation}>
              New conversation
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
