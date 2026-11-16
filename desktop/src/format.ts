import { currentIntlLocale, t } from "./i18n/catalog";

interface Formatters {
  locale: string;
  time: Intl.DateTimeFormat;
  dateTime: Intl.DateTimeFormat;
  day: Intl.DateTimeFormat;
  fullDay: Intl.DateTimeFormat;
}

let formatters: Formatters | null = null;

/** Build (or rebuild) formatters when the interface language changes. */
function activeFormatters(): Formatters {
  const locale = currentIntlLocale();
  if (!formatters || formatters.locale !== locale) {
    formatters = {
      locale,
      time: new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }),
      dateTime: new Intl.DateTimeFormat(locale, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      }),
      day: new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" }),
      fullDay: new Intl.DateTimeFormat(locale, {
        year: "numeric",
        month: "short",
        day: "numeric",
      }),
    };
  }
  return formatters;
}

export function formatTime(value: string): string {
  return activeFormatters().time.format(new Date(value));
}

export function formatDateTime(value: string): string {
  return activeFormatters().dateTime.format(new Date(value));
}

/** Compact timestamp for conversation rows: a time today, a date otherwise. */
export function formatListTime(value: string): string {
  const date = new Date(value);
  const now = new Date();
  return date.toDateString() === now.toDateString()
    ? activeFormatters().time.format(date)
    : activeFormatters().day.format(date);
}

/** Stable local day key used to group timeline messages. */
export function dayKey(value: string): string {
  const date = new Date(value);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

/** Label for a timeline day separator: Today, Yesterday, or a date. */
export function formatDayLabel(value: string): string {
  const date = new Date(value);
  const now = new Date();
  if (dayKey(value) === dayKey(now.toISOString())) return t("time.today");
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (dayKey(value) === dayKey(yesterday.toISOString())) return t("time.yesterday");
  return activeFormatters().fullDay.format(date);
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
