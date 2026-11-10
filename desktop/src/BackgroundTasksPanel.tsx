import Avatar from "./Avatar";
import { formatElapsed } from "./format";
import type { AgentProfile, TaskOverviewEntry } from "./types";
import { isActiveOverviewStatus } from "./types";

function agentName(agents: AgentProfile[], id: string): string {
  return agents.find((agent) => agent.id === id)?.name ?? `Agent ${id.slice(0, 8)}`;
}

function statusLabel(status: string): string {
  return status.replaceAll("_", " ");
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
            {statusLabel(entry.status)} · {formatElapsed(entry.started_at, now)}
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
          aria-label={`${entry.progress.completed} of ${entry.progress.total} assignments complete`}
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
            Open task
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
          <p className="muted">No assignments yet.</p>
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
                    {statusLabel(assignment.status)}
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
            Up next: {upNext.map((assignment) => agentName(agents, assignment.assignee)).join(", ")}
          </p>
        ) : null}
      </div>
    </details>
  );
}

interface BackgroundTasksPanelProps {
  overview: TaskOverviewEntry[];
  agents: AgentProfile[];
  now: number;
  inert?: boolean;
  onClose: () => void;
  onOpenTask: (runId: string) => void;
}

export default function BackgroundTasksPanel({
  overview,
  agents,
  now,
  inert = false,
  onClose,
  onOpenTask,
}: BackgroundTasksPanelProps) {
  const active = overview.filter((entry) =>
    isActiveOverviewStatus(entry.status),
  );
  const finished = overview
    .filter((entry) => !isActiveOverviewStatus(entry.status))
    .slice(0, 20);

  return (
    <aside className="background-panel" aria-label="Background tasks" inert={inert}>
      <header className="background-panel-head">
        <h2 className="background-panel-title">Background tasks</h2>
        <button
          type="button"
          className="icon-button"
          aria-label="Close background tasks"
          onClick={onClose}
        >
          ×
        </button>
      </header>
      <div className="background-panel-scroll">
        <section aria-label="Running tasks">
          <h3 className="background-panel-heading">Running {active.length}</h3>
          {active.length === 0 ? (
            <p className="background-panel-empty">
              Nothing running. Start a task and it will appear here.
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

        <section aria-label="Finished tasks">
          <details className="background-finished">
            <summary>
              <span>Finished {finished.length}</span>
              <span className="muted">Last 20</span>
            </summary>
            {finished.length === 0 ? (
              <p className="background-panel-empty">No finished runs yet.</p>
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
    </aside>
  );
}
