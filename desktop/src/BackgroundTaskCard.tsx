import { formatElapsed } from "./format";
import { useT } from "./i18n";
import { t as translate } from "./i18n/catalog";
import type { TaskOverviewEntry, ToolCallRecord } from "./types";

function stringArgument(call: ToolCallRecord, key: string): string | null {
  if (!call.arguments || typeof call.arguments !== "object") return null;
  const value = (call.arguments as Record<string, unknown>)[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** One-line activity for a background card, derived from the latest tool call. */
export function taskActivity(call: ToolCallRecord | null): string {
  if (!call) return translate("background.activityWaiting");
  const path = stringArgument(call, "path");
  const command = stringArgument(call, "command");
  switch (call.name) {
    case "read_file":
      return path
        ? translate("background.activityReadingPath", { path })
        : translate("background.activityReadingFile");
    case "write_file":
      return path
        ? translate("background.activityWritingPath", { path })
        : translate("background.activityWritingFile");
    case "execute_code":
      return command
        ? translate("background.activityRunningCommand", { command })
        : translate("background.activityRunningACommand");
    default:
      return call.name.replaceAll("_", " ");
  }
}

interface BackgroundTaskCardProps {
  entry: TaskOverviewEntry;
  now: number;
  onOpen: () => void;
}

export default function BackgroundTaskCard({
  entry,
  now,
  onOpen,
}: BackgroundTaskCardProps) {
  const t = useT();
  return (
    <button
      type="button"
      className="background-card"
      onClick={onOpen}
      aria-label={t("background.openTaskAria", { title: entry.title })}
    >
      <span className={`status-dot status-${entry.status}`} aria-hidden="true" />
      <span className="background-card-body">
        <span className="background-card-title">{entry.title}</span>
        <span className="background-card-activity">
          {taskActivity(entry.latest_tool_call)}
        </span>
        <span className="background-card-meta">
          {entry.worktree?.branch ? (
            <span
              className="background-card-branch"
              title={entry.worktree.branch}
            >
              {entry.worktree.branch}
            </span>
          ) : null}
          <span>{formatElapsed(entry.started_at, now)}</span>
        </span>
      </span>
      <span className="background-card-chevron" aria-hidden="true">
        ›
      </span>
    </button>
  );
}
