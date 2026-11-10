const timeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
});

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

const dayFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
});

export function formatTime(value: string): string {
  return timeFormatter.format(new Date(value));
}

export function formatDateTime(value: string): string {
  return dateTimeFormatter.format(new Date(value));
}

/** Compact timestamp for conversation rows: a time today, a date otherwise. */
export function formatListTime(value: string): string {
  const date = new Date(value);
  const now = new Date();
  return date.toDateString() === now.toDateString()
    ? timeFormatter.format(date)
    : dayFormatter.format(date);
}

const fullDayFormatter = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
});

/** Stable local day key used to group timeline messages. */
export function dayKey(value: string): string {
  const date = new Date(value);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

/** Label for a timeline day separator: Today, Yesterday, or a date. */
export function formatDayLabel(value: string): string {
  const date = new Date(value);
  const now = new Date();
  if (dayKey(value) === dayKey(now.toISOString())) return "Today";
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (dayKey(value) === dayKey(yesterday.toISOString())) return "Yesterday";
  return fullDayFormatter.format(date);
}

/** Compact token count: 987, 12.3k, 1.2M. */
export function formatTokens(tokens: number): string {
  if (tokens < 1_000) return `${Math.round(tokens)}`;
  if (tokens < 1_000_000) {
    const value = tokens / 1_000;
    return `${value >= 100 ? Math.round(value) : value.toFixed(1).replace(/\.0$/, "")}k`;
  }
  const value = tokens / 1_000_000;
  return `${value >= 100 ? Math.round(value) : value.toFixed(1).replace(/\.0$/, "")}M`;
}

/** Elapsed time since an ISO timestamp, for running background tasks. */
export function formatElapsed(startedAt: string, now: number = Date.now()): string {
  const start = new Date(startedAt).getTime();
  if (Number.isNaN(start)) return "";
  const seconds = Math.max(0, Math.floor((now - start) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}
