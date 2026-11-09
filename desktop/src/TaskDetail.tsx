import { approvalLevelLabel, taskStatusKind, taskStatusLabel } from "./types";
import type {
  Assignment,
  AssignmentEvent,
  AssignmentOutcome,
  Run,
  Task,
  ToolCallRecord,
} from "./types";
import { formatDateTime, formatTime } from "./format";
import RichText from "./RichText";

export interface AssignmentWithEvents {
  assignment: Assignment;
  events: AssignmentEvent[];
}

export interface RunWithCalls {
  run: Run;
  calls: ToolCallRecord[];
}

interface TaskDetailProps {
  task: Task;
  assignments: AssignmentWithEvents[];
  runs: RunWithCalls[];
  cancelling: boolean;
  onCancel: () => void;
  running: boolean;
  onRun: () => void;
  onToggleArchive: () => void;
  archiveBusy: boolean;
}

function outcomeText(outcome: AssignmentOutcome): string {
  if (outcome.status === "completed") return outcome.detail.output;
  if (outcome.status === "failed") return outcome.detail.error;
  return outcome.detail.reason;
}

function toolResultText(result: unknown): string {
  if (result && typeof result === "object") {
    const record = result as Record<string, unknown>;
    if (typeof record.output === "string" && record.output.trim().length > 0) {
      return record.output;
    }
    if (record.success === false && typeof record.error === "string") {
      return record.error;
    }
  }
  return JSON.stringify(result ?? null) ?? "null";
}

export default function TaskDetail({
  task,
  assignments,
  runs,
  cancelling,
  onCancel,
  running,
  onRun,
  onToggleArchive,
  archiveBusy,
}: TaskDetailProps) {
  const kind = taskStatusKind(task.status);
  const canCancel = (kind === "pending" || kind === "in_progress") && !running;
  const canRun = kind === "pending" && !running;
  const canArchive =
    kind === "completed" || kind === "failed" || kind === "cancelled";
  const approvalLevel = assignments[0]
    ? approvalLevelLabel(assignments[0].assignment.approval_level)
    : "—";

  return (
    <article className="task-detail">
      <header className="task-header">
        <div className="task-heading">
          <div className="task-badges">
            <span className={`status-badge status-${kind}`}>{taskStatusLabel(task.status)}</span>
            {task.archived ? <span className="settings-badge">Archived</span> : null}
            <span className="task-id">Task {task.id.slice(0, 8)}</span>
          </div>
          <h2 className="task-title">{task.title || task.description}</h2>
          <p className="task-output">
            <span className="task-output-label">Expected output</span>
            {task.expected_output}
          </p>
        </div>
        <div className="task-actions">
          {canRun || running ? (
            <button
              type="button"
              className="primary-button"
              onClick={onRun}
              disabled={running}
            >
              {running ? "Running…" : "Run task"}
            </button>
          ) : null}
          {canArchive ? (
            <button
              type="button"
              className="ghost-button"
              onClick={onToggleArchive}
              disabled={archiveBusy}
            >
              {archiveBusy
                ? "Saving…"
                : task.archived
                  ? "Unarchive task"
                  : "Archive task"}
            </button>
          ) : null}
          {canCancel || kind === "pending" ? (
            <button
              type="button"
              className="danger-button"
              onClick={onCancel}
              disabled={cancelling || running}
            >
              {cancelling ? "Cancelling…" : "Cancel task"}
            </button>
          ) : null}
        </div>
      </header>

      <section className="task-prompt" aria-label="Task prompt">
        <h3 className="task-prompt-label">Task prompt</h3>
        <p className="task-prompt-text">{task.description}</p>
      </section>

      <dl className="task-meta">
        <div>
          <dt>Assignee</dt>
          <dd>{task.assigned_agent ?? "Unassigned"}</dd>
        </div>
        <div>
          <dt>Approval</dt>
          <dd>{approvalLevel}</dd>
        </div>
        <div>
          <dt>Assignments</dt>
          <dd>{assignments.length}</dd>
        </div>
        <div>
          <dt>Runs</dt>
          <dd>{runs.length}</dd>
        </div>
      </dl>

      {typeof task.status === "object" && "completed" in task.status ? (
        <section className="task-result">
          <h3>Deliverable</h3>
          <RichText content={task.status.completed.output} />
        </section>
      ) : null}
      {typeof task.status === "object" && "failed" in task.status ? (
        <section className="task-result task-result-failed">
          <h3>Failure</h3>
          <pre>{task.status.failed.error}</pre>
        </section>
      ) : null}

      <section className="task-section">
        <h3>Assignments</h3>
        {assignments.length === 0 ? (
          <p className="muted">No assignments yet.</p>
        ) : (
          assignments.map(({ assignment, events }) => (
            <details className="assignment" key={assignment.id}>
              <summary>
                <span className={`status-dot status-${assignment.status}`} />
                <span className="summary-label">{assignment.status.replaceAll("_", " ")}</span>
                <span className="muted">
                  {approvalLevelLabel(assignment.approval_level)} · {assignment.executions} run
                  {assignment.executions === 1 ? "" : "s"}
                </span>
                <time className="summary-time">{formatDateTime(assignment.updated_at)}</time>
              </summary>
              <p className="assignment-id">Assignment {assignment.id.slice(0, 8)}</p>
              {assignment.outcome ? (
                <p className="assignment-outcome">{outcomeText(assignment.outcome)}</p>
              ) : null}
              {assignment.last_error ? (
                <p className="assignment-error">{assignment.last_error}</p>
              ) : null}
              <ol className="event-log">
                {events.map((event) => (
                  <li key={event.id}>
                    <span className="event-kind">{event.kind.replaceAll("_", " ")}</span>
                    {event.detail ? <span className="event-detail">{event.detail}</span> : null}
                    <time className="event-time">{formatTime(event.created_at)}</time>
                  </li>
                ))}
              </ol>
            </details>
          ))
        )}
      </section>

      <section className="task-section">
        <h3>Runs</h3>
        {runs.length === 0 ? (
          <p className="muted">No runs recorded yet.</p>
        ) : (
          runs.map(({ run, calls }) => (
            <details className="run" key={run.id}>
              <summary>
                <span className={`status-dot status-${run.status}`} />
                <span className="summary-label">{run.status.replaceAll("_", " ")}</span>
                <span className="muted">Agent {run.agent_id.slice(0, 8)}</span>
                <time className="summary-time">
                  {formatTime(run.started_at)} — {run.finished_at ? formatTime(run.finished_at) : "running"}
                </time>
              </summary>
              {calls.length === 0 ? (
                <p className="muted">No tool calls.</p>
              ) : (
                <ul className="tool-calls">
                  {calls.map((call) => (
                    <li key={call.id}>
                      <div className="tool-call-head">
                        <span className="tool-name">{call.name}</span>
                        <span className="muted">{call.id}</span>
                      </div>
                      <code className="tool-arguments">{JSON.stringify(call.arguments)}</code>
                      <pre className="tool-result">{toolResultText(call.result)}</pre>
                      {call.approved_by ? (
                        <p className="muted">Approved by {call.approved_by}</p>
                      ) : null}
                    </li>
                  ))}
                </ul>
              )}
            </details>
          ))
        )}
      </section>
    </article>
  );
}
