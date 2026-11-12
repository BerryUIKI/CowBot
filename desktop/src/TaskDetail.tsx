import { useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { approvalLevelLabel, taskStatusKind, taskStatusLabel } from "./types";
import type {
  Assignment,
  AssignmentEvent,
  AssignmentOutcome,
  Run,
  Task,
  TaskWorktree,
  TaskWorktreeDiff,
  ToolCallRecord,
  WorktreeStatus,
} from "./types";
import { taskWorktreeDiff } from "./api";
import { errorMessage } from "./errors";
import { formatDateTime, formatTime } from "./format";
import Modal from "./Modal";
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
  runTasks: Task[];
  worktree: TaskWorktree | null;
  discardingWorktree: boolean;
  onDiscardWorktree: () => void;
  onOpenTask: (taskId: string) => void;
  cancelling: boolean;
  onCancel: () => void;
  running: boolean;
  onRun: () => void;
  onApprovePlan: () => void;
  approvingPlan: boolean;
  onToggleArchive: () => void;
  archiveBusy: boolean;
  onBack?: () => void;
}

function outcomeText(outcome: AssignmentOutcome): string {
  if (outcome.status === "completed") return outcome.detail.output;
  if (outcome.status === "failed") return outcome.detail.error;
  return outcome.detail.reason;
}

const worktreeStatusLabels: Record<WorktreeStatus, string> = {
  requested: "Requested",
  ready: "Ready",
  missing: "Missing",
  removed: "Removed",
};

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
  runTasks,
  worktree,
  discardingWorktree,
  onDiscardWorktree,
  onOpenTask,
  cancelling,
  onCancel,
  running,
  onRun,
  onApprovePlan,
  approvingPlan,
  onToggleArchive,
  archiveBusy,
  onBack,
}: TaskDetailProps) {
  const kind = taskStatusKind(task.status);
  const canCancel = (kind === "pending" || kind === "in_progress") && !running;
  const canRun = kind === "pending" && !running;
  const [promptExpanded, setPromptExpanded] = useState(false);
  const [outputExpanded, setOutputExpanded] = useState(false);
  const promptLines = task.description ? task.description.split("\n").length : 0;
  const promptLong = task.description.length > 400 || promptLines > 6;
  const outputLines = task.expected_output
    ? task.expected_output.split("\n").length
    : 0;
  const outputLong = task.expected_output.length > 240 || outputLines > 3;
  const canArchive =
    kind === "completed" || kind === "failed" || kind === "cancelled";
  const approvalLevel = assignments[0]
    ? approvalLevelLabel(assignments[0].assignment.approval_level)
    : "—";

  const [diffOpen, setDiffOpen] = useState(false);
  const [diff, setDiff] = useState<TaskWorktreeDiff | null>(null);
  const [diffBusy, setDiffBusy] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [copiedBranch, setCopiedBranch] = useState(false);
  const [worktreeError, setWorktreeError] = useState<string | null>(null);

  async function revealWorktree() {
    if (!worktree) return;
    setWorktreeError(null);
    try {
      await revealItemInDir(worktree.worktree_path);
    } catch (error) {
      setWorktreeError(errorMessage(error));
    }
  }

  async function copyWorktreeBranch() {
    if (!worktree) return;
    try {
      await navigator.clipboard.writeText(worktree.branch);
      setCopiedBranch(true);
      window.setTimeout(() => setCopiedBranch(false), 1500);
    } catch (error) {
      setWorktreeError(errorMessage(error));
    }
  }

  function openWorktreeDiff() {
    setDiffOpen(true);
    setDiff(null);
    setDiffError(null);
    setDiffBusy(true);
    taskWorktreeDiff(task.id)
      .then(setDiff)
      .catch((error) => setDiffError(errorMessage(error)))
      .finally(() => setDiffBusy(false));
  }

  function discardWorktree() {
    if (
      window.confirm(
        "Discard this worktree? The checkout folder and its branch are removed, and uncommitted changes are lost.",
      )
    ) {
      onDiscardWorktree();
    }
  }

  return (
    <article className="task-detail">
      {onBack ? (
        <button type="button" className="text-button task-back" onClick={onBack}>
          ← All tasks
        </button>
      ) : null}
      <header className="task-header">
        <div className="task-heading">
          <div className="task-badges">
            <span className={`status-badge status-${kind}`}>{taskStatusLabel(task.status)}</span>
            {task.archived ? <span className="settings-badge">Archived</span> : null}
            <span className="task-id">Task {task.id.slice(0, 8)}</span>
          </div>
          <h2 className="task-title">{task.title || task.description}</h2>
          <div className="task-output">
            <span className="task-output-label">Expected output</span>
            <p
              id="task-output-text"
              className={`task-output-text${
                outputLong && !outputExpanded ? " is-collapsed" : ""
              }`}
            >
              {task.expected_output}
            </p>
            {outputLong ? (
              <button
                type="button"
                className="text-button task-toggle"
                aria-expanded={outputExpanded}
                aria-controls="task-output-text"
                onClick={() => setOutputExpanded((value) => !value)}
              >
                {outputExpanded ? "Show less" : `Show more (${outputLines} lines)`}
              </button>
            ) : null}
          </div>
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
        <div className="task-section-head">
          <h3 className="task-prompt-label">Task prompt</h3>
          {promptLong ? (
            <button
              type="button"
              className="text-button task-toggle"
              aria-expanded={promptExpanded}
              aria-controls="task-prompt-text"
              onClick={() => setPromptExpanded((value) => !value)}
            >
              {promptExpanded
                ? "Show less"
                : `Show more (${promptLines} lines)`}
            </button>
          ) : null}
        </div>
        <p
          id="task-prompt-text"
          className={`task-prompt-text${
            promptLong && !promptExpanded ? " is-collapsed" : ""
          }`}
        >
          {task.description}
        </p>
      </section>

      {task.plan ? (
        <section className="task-plan" aria-label="Task plan">
          <div className="task-plan-head">
            <h3>Plan</h3>
            {task.plan_approved ? (
              <span className="settings-badge settings-badge-active">
                Approved
              </span>
            ) : (
              <span className="status-badge status-pending">
                Awaiting approval
              </span>
            )}
          </div>
          <div className="task-plan-body">
            <RichText content={task.plan} />
          </div>
          <div className="form-actions">
            <button
              type="button"
              className="primary-button"
              onClick={onApprovePlan}
              disabled={approvingPlan || running}
            >
              {approvingPlan || running
                ? "Starting…"
                : task.plan_approved
                  ? "Continue task"
                  : "Approve plan and continue"}
            </button>
          </div>
        </section>
      ) : null}

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

      {worktree ? (
        <section className="task-section" aria-label="Task worktree">
          <div className="worktree-card">
            <div className="worktree-card-head">
              <h3>Worktree</h3>
              <span
                className={`settings-badge worktree-status worktree-status-${worktree.status}`}
              >
                {worktreeStatusLabels[worktree.status]}
              </span>
            </div>
            <dl className="worktree-meta">
              <div>
                <dt>Branch</dt>
                <dd>
                  <code>{worktree.branch || "Created when the task runs"}</code>
                  {worktree.branch ? (
                    <button
                      type="button"
                      className="text-button worktree-copy"
                      onClick={() => void copyWorktreeBranch()}
                    >
                      {copiedBranch ? "Copied" : "Copy"}
                    </button>
                  ) : null}
                </dd>
              </div>
              <div>
                <dt>Base branch</dt>
                <dd>{worktree.base_branch}</dd>
              </div>
              <div>
                <dt>Project</dt>
                <dd>{worktree.project_path}</dd>
              </div>
              <div>
                <dt>Path</dt>
                <dd>{worktree.worktree_path || "Created when the task runs"}</dd>
              </div>
              <div>
                <dt>Created</dt>
                <dd>{formatDateTime(worktree.created_at)}</dd>
              </div>
            </dl>
            {worktree.status === "requested" ? (
              <p className="worktree-note">
                The checkout and branch are created when you run this task.
              </p>
            ) : null}
            {worktree.status === "missing" ? (
              <p className="worktree-note">
                The checkout folder no longer exists. Discard the record and
                create a new task to run in a worktree.
              </p>
            ) : null}
            {worktree.status === "removed" ? (
              <p className="worktree-note">
                This worktree was discarded. Its branch and folder are gone.
              </p>
            ) : null}
            {worktreeError ? (
              <p className="worktree-note worktree-note-error" role="alert">
                {worktreeError}
              </p>
            ) : null}
            <div className="form-actions">
              {worktree.status === "ready" ? (
                <>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void revealWorktree()}
                  >
                    Open folder
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={openWorktreeDiff}
                  >
                    View diff
                  </button>
                </>
              ) : null}
              {worktree.status !== "removed" ? (
                <button
                  type="button"
                  className="danger-button"
                  onClick={discardWorktree}
                  disabled={discardingWorktree || running}
                >
                  {discardingWorktree ? "Discarding…" : "Discard worktree"}
                </button>
              ) : null}
            </div>
          </div>
        </section>
      ) : null}

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

      {runTasks.length > 0 ? (
        <section className="task-section" aria-label="Run tasks">
          <h3>Run tasks</h3>
          <p className="muted">
            This run is split across {runTasks.length} crew tasks. Open one to see
            its assignments and tool calls.
          </p>
          <ul className="run-task-list">
            {runTasks.map((runTask) => {
              const runKind = taskStatusKind(runTask.status);
              return (
                <li key={runTask.id}>
                  <button
                    type="button"
                    className="run-task-row"
                    onClick={() => onOpenTask(runTask.id)}
                  >
                    <span className={`status-dot status-${runKind}`} />
                    <span className="run-task-title">
                      {runTask.title || runTask.description}
                    </span>
                    <span className="muted">{taskStatusLabel(runTask.status)}</span>
                    <span className="run-task-agent">
                      {runTask.assigned_agent ?? "Unassigned"}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
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

      {diffOpen ? (
        <Modal
          title="Worktree diff"
          onClose={() => setDiffOpen(false)}
          footer={
            <button
              type="button"
              className="ghost-button"
              onClick={() => setDiffOpen(false)}
            >
              Close
            </button>
          }
        >
          {diffBusy ? (
            <p className="muted">Loading the diff…</p>
          ) : diffError ? (
            <p className="dialog-error" role="alert">
              {diffError}
            </p>
          ) : diff ? (
            <div className="worktree-diff">
              {diff.status ? (
                <>
                  <h4>Status</h4>
                  <pre className="worktree-patch">{diff.status}</pre>
                </>
              ) : (
                <p className="muted">The working tree is clean.</p>
              )}
              {diff.stat ? (
                <>
                  <h4>Diffstat</h4>
                  <pre className="worktree-patch">{diff.stat}</pre>
                </>
              ) : null}
              {diff.truncated ? (
                <p className="worktree-note">
                  The patch is truncated at 256 KiB.
                </p>
              ) : null}
              <h4>Patch</h4>
              <pre className="worktree-patch">
                {diff.patch || "No tracked changes."}
              </pre>
            </div>
          ) : null}
        </Modal>
      ) : null}
    </article>
  );
}
