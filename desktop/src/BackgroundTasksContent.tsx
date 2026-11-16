import Avatar from "./Avatar";
import { formatElapsed } from "./format";
import { useT } from "./i18n";
import { t as translate, type TranslationKey } from "./i18n/catalog";
import type { AgentProfile, TaskOverviewEntry } from "./types";
import { isActiveOverviewStatus } from "./types";

const overviewStatusKeys: Record<string, TranslationKey> = {
  pending: "taskStatus.pending",
  in_progress: "taskStatus.inProgress",
  completed: "taskStatus.completed",
  failed: "taskStatus.failed",
  cancelled: "taskStatus.cancelled",
};

const assignmentStatusKeys: Record<string, TranslationKey> = {
  queued: "tasks.status.assignment.queued",
  running: "tasks.status.assignment.running",
  waiting: "tasks.status.assignment.waiting",
  resume: "tasks.status.assignment.resume",
  needs_input: "tasks.status.assignment.needsInput",
  completed: "tasks.status.assignment.completed",
  failed: "tasks.status.assignment.failed",
  cancelled: "tasks.status.assignment.cancelled",
};

function agentName(agents: AgentProfile[], id: string): string {
  return (
    agents.find((agent) => agent.id === id)?.name ??
    translate("background.agentFallback", { id: id.slice(0, 8) })
  );
}

function statusLabel(
  keys: Record<string, TranslationKey>,
  status: string,
): string {
  const key = keys[status];
  return key ? translate(key) : status.replaceAll("_", " ");
}

function outcomeSnippet(entry: TaskOverviewEntry): string | null {
  const last = entry.assignments[entry.assignments.length - 1];
  return last?.detail?.trim() || null;
}

interface ActiveRunProps {
  entry: TaskOverviewEntry;
  agents: AgentProfile[];
  now: number;
  onOpenTask: (runId: string) => void;
}

function ActiveRun({ entry, agents, now, onOpenTask }: ActiveRunProps) {
  const t = useT();
  const percent =
    entry.progress.total > 0
      ? Math.round((entry.progress.completed / entry.progress.total) * 100)
      : 0;
  const upNext = entry.assignments.filter(
    (assignment) =>
      assignment.status === "queued" || assignment.status === "resume",
  );

  return (
    <details className="background-run">
      <summary>
        <span className={`status-dot status-${entry.status}`} aria-hidden="true" />
        <span className="background-run-heading">
          <span className="background-run-title">{entry.title}</span>
          <span className="background-run-meta">
            {statusLabel(overviewStatusKeys, entry.status)} ·{" "}
            {formatElapsed(entry.started_at, now)}
          </span>
        </span>
        <span className="background-run-count">
          {entry.progress.completed}/{entry.progress.total}
        </span>
      </summary>
      <div className="background-run-body">
        <div
          className="background-progress"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={entry.progress.total}
          aria-valuenow={entry.progress.completed}
          aria-label={t("background.progressAria", {
            completed: entry.progress.completed,
            total: entry.progress.total,
          })}
        >
          <div
            className="background-progress-fill"
            style={{ width: `${percent}%` }}
          />
        </div>
        <div className="background-run-actions">
          <button
            type="button"
            className="text-button"
            onClick={() => onOpenTask(entry.run_id)}
          >
            {t("background.openTask")}
          </button>
          {entry.worktree?.branch ? (
            <span
              className="background-card-branch"
              title={entry.worktree.branch}
            >
              {entry.worktree.branch}
            </span>
          ) : null}
        </div>
        {entry.assignments.length === 0 ? (
          <p className="muted">{t("background.noAssignments")}</p>
        ) : (
          <ul className="background-assignment-list">
            {entry.assignments.map((assignment) => (
              <li key={assignment.id} className="background-assignment">
                <Avatar
                  name={agentName(agents, assignment.assignee)}
                  kind="agent"
                  size="small"
                />
                <span className="background-assignment-text">
                  <span className="background-assignment-name">
                    {agentName(agents, assignment.assignee)} ·{" "}
                    {statusLabel(assignmentStatusKeys, assignment.status)}
                  </span>
                  {assignment.detail ? (
                    <span className="background-assignment-detail">
                      {assignment.detail}
                    </span>
                  ) : null}
                </span>
              </li>
            ))}
          </ul>
        )}
        {upNext.length > 0 ? (
          <p className="background-upnext">
            {t("background.upNext", {
              names: upNext
                .map((assignment) => agentName(agents, assignment.assignee))
                .join(", "),
            })}
          </p>
        ) : null}
      </div>
    </details>
  );
}

interface BackgroundTasksContentProps {
  overview: TaskOverviewEntry[];
  agents: AgentProfile[];
  now: number;
  onOpenTask: (runId: string) => void;
}

export default function BackgroundTasksContent({
  overview,
  agents,
  now,
  onOpenTask,
}: BackgroundTasksContentProps) {
  const t = useT();
  const active = overview.filter((entry) =>
    isActiveOverviewStatus(entry.status),
  );
  const finished = overview
    .filter((entry) => !isActiveOverviewStatus(entry.status))
    .slice(0, 20);

  return (
    <div className="background-panel-scroll">
      <section aria-label={t("background.runningAria")}>
        <h3 className="background-panel-heading">
          {t("background.runningHeading", { count: active.length })}
        </h3>
        {active.length === 0 ? (
          <p className="background-panel-empty">
            {t("background.nothingRunning")}
          </p>
        ) : (
          active.map((entry) => (
            <ActiveRun
              key={entry.run_id}
              entry={entry}
              agents={agents}
              now={now}
              onOpenTask={onOpenTask}
            />
          ))
        )}
      </section>

      <section aria-label={t("background.finishedAria")}>
        <details className="background-finished">
          <summary>
            <span>
              {t("background.finishedHeading", { count: finished.length })}
            </span>
            <span className="muted">{t("background.last20")}</span>
          </summary>
          {finished.length === 0 ? (
            <p className="background-panel-empty">{t("background.noFinished")}</p>
          ) : (
            <ul className="background-finished-list">
              {finished.map((entry) => {
                const snippet = outcomeSnippet(entry);
                return (
                  <li key={entry.run_id}>
                    <button
                      type="button"
                      className="background-finished-row"
                      onClick={() => onOpenTask(entry.run_id)}
                    >
                      <span
                        className={`status-dot status-${entry.status}`}
                        aria-hidden="true"
                      />
                      <span className="background-finished-text">
                        <span className="background-finished-title">
                          {entry.title}
                        </span>
                        {snippet ? (
                          <span className="background-finished-snippet">
                            {snippet}
                          </span>
                        ) : null}
                      </span>
                      <span className="background-finished-meta">
                        {formatElapsed(
                          entry.started_at,
                          new Date(entry.updated_at).getTime(),
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </details>
      </section>
    </div>
  );
}
