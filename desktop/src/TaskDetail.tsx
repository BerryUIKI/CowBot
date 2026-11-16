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
import { t, type TranslationKey } from "./i18n/catalog";
import Modal from "./Modal";
import { RunPromptSnapshotsToggle } from "./PromptSnapshotViewer";
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
  /** Open the read-only prompt inspector for one run. */
  onInspectRun: (runId: string) => void;
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

const worktreeStatusKeys: Record<WorktreeStatus, TranslationKey> = {
  requested: "tasks.status.worktree.requested",
  ready: "tasks.status.worktree.ready",
  missing: "tasks.status.worktree.missing",
  removed: "tasks.status.worktree.removed",
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

const runStatusKeys: Record<string, TranslationKey> = {
  running: "tasks.status.run.running",
  waiting_approval: "tasks.status.run.waitingApproval",
  completed: "tasks.status.run.completed",
  failed: "tasks.status.run.failed",
  cancelled: "tasks.status.run.cancelled",
};

const eventKindKeys: Record<string, TranslationKey> = {
  enqueued: "tasks.status.event.enqueued",
  started: "tasks.status.event.started",
  waiting: "tasks.status.event.waiting",
  resumed: "tasks.status.event.resumed",
  needs_input: "tasks.status.event.needsInput",
  completed: "tasks.status.event.completed",
  failed: "tasks.status.event.failed",
  cancelled: "tasks.status.event.cancelled",
  restarted: "tasks.status.event.restarted",
  stalled: "tasks.status.event.stalled",
  budget_exhausted: "tasks.status.event.budgetExhausted",
};

function statusLabel(
  keys: Record<string, TranslationKey>,
  value: string,
): string {
  const key = keys[value];
  return key ? t(key) : value.replaceAll("_", " ");
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
  return JSON.stringify(result ?? null);
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
  onInspectRun,
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
    if (window.confirm(t("tasks.detail.confirmDiscard"))) {
      onDiscardWorktree();
    }
  }

  return (
    <article className="task-detail">
      {onBack ? (
        <button type="button" className="text-button task-back" onClick={onBack}>
          {t("tasks.detail.allTasks")}
        </button>
      ) : null}
      <header className="task-header">
        <div className="task-heading">
          <div className="task-badges">
            <span className={`status-badge status-${kind}`}>{taskStatusLabel(task.status)}</span>
            {task.archived ? (
              <span className="settings-badge">{t("tasks.detail.archived")}</span>
            ) : null}
            <span className="task-id">
              {t("tasks.detail.taskId", { id: task.id.slice(0, 8) })}
            </span>
          </div>
          <h2 className="task-title">{task.title || task.description}</h2>
          <div className="task-output">
            <span className="task-output-label">
              {t("tasks.detail.expectedOutput")}
            </span>
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
                {outputExpanded
                  ? t("tasks.detail.showLess")
                  : t("tasks.detail.showMore", { count: outputLines })}
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
              {running
                ? t("tasks.detail.running")
                : t("tasks.detail.run")}
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
                ? t("tasks.detail.saving")
                : task.archived
                  ? t("tasks.detail.unarchive")
                  : t("tasks.detail.archive")}
            </button>
          ) : null}
          {canCancel || kind === "pending" ? (
            <button
              type="button"
              className="danger-button"
              onClick={onCancel}
              disabled={cancelling || running}
            >
              {cancelling
                ? t("tasks.detail.cancelling")
                : t("tasks.detail.cancel")}
            </button>
          ) : null}
        </div>
      </header>

      <section className="task-prompt" aria-label={t("tasks.detail.promptAria")}>
        <div className="task-section-head">
          <h3 className="task-prompt-label">{t("tasks.detail.prompt")}</h3>
          {promptLong ? (
            <button
              type="button"
              className="text-button task-toggle"
              aria-expanded={promptExpanded}
              aria-controls="task-prompt-text"
              onClick={() => setPromptExpanded((value) => !value)}
            >
              {promptExpanded
                ? t("tasks.detail.showLess")
                : t("tasks.detail.showMore", { count: promptLines })}
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
        <section className="task-plan" aria-label={t("tasks.detail.planAria")}>
          <div className="task-plan-head">
            <h3>{t("tasks.detail.plan")}</h3>
            {task.plan_approved ? (
              <span className="settings-badge settings-badge-active">
                {t("tasks.detail.approved")}
              </span>
            ) : (
              <span className="status-badge status-pending">
                {t("tasks.detail.awaitingApproval")}
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
                ? t("tasks.detail.starting")
                : task.plan_approved
                  ? t("tasks.detail.continueTask")
                  : t("tasks.detail.approvePlan")}
            </button>
          </div>
        </section>
      ) : null}

      <dl className="task-meta">
        <div>
          <dt>{t("tasks.detail.assignee")}</dt>
          <dd>{task.assigned_agent ?? t("tasks.detail.unassigned")}</dd>
        </div>
        <div>
          <dt>{t("tasks.detail.approval")}</dt>
          <dd>{approvalLevel}</dd>
        </div>
        <div>
          <dt>{t("tasks.detail.assignments")}</dt>
          <dd>{assignments.length}</dd>
        </div>
        <div>
          <dt>{t("tasks.detail.runs")}</dt>
          <dd>{runs.length}</dd>
        </div>
      </dl>

      {worktree ? (
        <section className="task-section" aria-label={t("tasks.detail.worktreeAria")}>
          <div className="worktree-card">
            <div className="worktree-card-head">
              <h3>{t("tasks.detail.worktree")}</h3>
              <span
                className={`settings-badge worktree-status worktree-status-${worktree.status}`}
              >
                {t(worktreeStatusKeys[worktree.status])}
              </span>
            </div>
            <dl className="worktree-meta">
              <div>
                <dt>{t("tasks.detail.branch")}</dt>
                <dd>
                  <code>
                    {worktree.branch || t("tasks.detail.createdWhenRun")}
                  </code>
                  {worktree.branch ? (
                    <button
                      type="button"
                      className="text-button worktree-copy"
                      onClick={() => void copyWorktreeBranch()}
                    >
                      {copiedBranch
                        ? t("tasks.detail.copied")
                        : t("tasks.detail.copy")}
                    </button>
                  ) : null}
                </dd>
              </div>
              <div>
                <dt>{t("tasks.detail.baseBranch")}</dt>
                <dd>{worktree.base_branch}</dd>
              </div>
              <div>
                <dt>{t("tasks.detail.project")}</dt>
                <dd>{worktree.project_path}</dd>
              </div>
              <div>
                <dt>{t("tasks.detail.path")}</dt>
                <dd>
                  {worktree.worktree_path || t("tasks.detail.createdWhenRun")}
                </dd>
              </div>
              <div>
                <dt>{t("tasks.detail.created")}</dt>
                <dd>{formatDateTime(worktree.created_at)}</dd>
              </div>
            </dl>
            {worktree.status === "requested" ? (
              <p className="worktree-note">
                {t("tasks.detail.worktreeRequested")}
              </p>
            ) : null}
            {worktree.status === "missing" ? (
              <p className="worktree-note">
                {t("tasks.detail.worktreeMissing")}
              </p>
            ) : null}
            {worktree.status === "removed" ? (
              <p className="worktree-note">
                {t("tasks.detail.worktreeRemoved")}
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
                    {t("tasks.detail.openFolder")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={openWorktreeDiff}
                  >
                    {t("tasks.detail.viewDiff")}
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
                  {discardingWorktree
                    ? t("tasks.detail.discarding")
                    : t("tasks.detail.discard")}
                </button>
              ) : null}
            </div>
          </div>
        </section>
      ) : null}

      {typeof task.status === "object" && "completed" in task.status ? (
        <section className="task-result">
          <h3>{t("tasks.detail.deliverable")}</h3>
          <RichText content={task.status.completed.output} />
        </section>
      ) : null}
      {typeof task.status === "object" && "failed" in task.status ? (
        <section className="task-result task-result-failed">
          <h3>{t("tasks.detail.failure")}</h3>
          <pre>{task.status.failed.error}</pre>
        </section>
      ) : null}

      {runTasks.length > 0 ? (
        <section className="task-section" aria-label={t("tasks.detail.runTasksAria")}>
          <h3>{t("tasks.detail.runTasks")}</h3>
          <p className="muted">
            {t("tasks.detail.runTasksHint", { count: runTasks.length })}
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
                      {runTask.assigned_agent ?? t("tasks.detail.unassigned")}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      <section className="task-section">
        <h3>{t("tasks.detail.assignments")}</h3>
        {assignments.length === 0 ? (
          <p className="muted">{t("tasks.detail.noAssignments")}</p>
        ) : (
          assignments.map(({ assignment, events }) => (
            <details className="assignment" key={assignment.id}>
              <summary>
                <span className={`status-dot status-${assignment.status}`} />
                <span className="summary-label">
                  {statusLabel(assignmentStatusKeys, assignment.status)}
                </span>
                <span className="muted">
                  {approvalLevelLabel(assignment.approval_level)} ·{" "}
                  {t(
                    assignment.executions === 1
                      ? "tasks.detail.executionsOne"
                      : "tasks.detail.executionsOther",
                    { count: assignment.executions },
                  )}
                </span>
                <time className="summary-time">{formatDateTime(assignment.updated_at)}</time>
              </summary>
              <p className="assignment-id">
                {t("tasks.detail.assignmentId", {
                  id: assignment.id.slice(0, 8),
                })}
              </p>
              {assignment.outcome ? (
                <p className="assignment-outcome">{outcomeText(assignment.outcome)}</p>
              ) : null}
              {assignment.last_error ? (
                <p className="assignment-error">{assignment.last_error}</p>
              ) : null}
              <ol className="event-log">
                {events.map((event) => (
                  <li key={event.id}>
                    <span className="event-kind">
                      {statusLabel(eventKindKeys, event.kind)}
                    </span>
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
        <h3>{t("tasks.detail.runs")}</h3>
        {runs.length === 0 ? (
          <p className="muted">{t("tasks.detail.noRuns")}</p>
        ) : (
          runs.map(({ run, calls }) => (
            <details className="run" key={run.id}>
              <summary>
                <span className={`status-dot status-${run.status}`} />
                <span className="summary-label">
                  {statusLabel(runStatusKeys, run.status)}
                </span>
                <span className="muted">
                  {t("tasks.detail.agent", { id: run.agent_id.slice(0, 8) })}
                </span>
                <time className="summary-time">
                  {formatTime(run.started_at)} —{" "}
                  {run.finished_at
                    ? formatTime(run.finished_at)
                    : t("tasks.detail.runningStatus")}
                </time>
              </summary>
              {task.source === "user" ? (
                <div className="run-actions">
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => onInspectRun(run.id)}
                  >
                    {t("tasks.detail.inspectContext")}
                  </button>
                </div>
              ) : (
                <p className="muted">
                  {t("tasks.detail.turnRunHint")}
                </p>
              )}
              <RunPromptSnapshotsToggle runId={run.id} />
              {calls.length === 0 ? (
                <p className="muted">{t("tasks.detail.noToolCalls")}</p>
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
                        <p className="muted">
                          {t("tasks.detail.approvedBy", {
                            name: call.approved_by,
                          })}
                        </p>
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
          title={t("tasks.detail.diffTitle")}
          onClose={() => setDiffOpen(false)}
          footer={
            <button
              type="button"
              className="ghost-button"
              onClick={() => setDiffOpen(false)}
            >
              {t("tasks.detail.close")}
            </button>
          }
        >
          {diffBusy ? (
            <p className="muted">{t("tasks.detail.loadingDiff")}</p>
          ) : diffError ? (
            <p className="dialog-error" role="alert">
              {diffError}
            </p>
          ) : diff ? (
            <div className="worktree-diff">
              {diff.status ? (
                <>
                  <h4>{t("tasks.detail.status")}</h4>
                  <pre className="worktree-patch">{diff.status}</pre>
                </>
              ) : (
                <p className="muted">{t("tasks.detail.clean")}</p>
              )}
              {diff.stat ? (
                <>
                  <h4>{t("tasks.detail.diffstat")}</h4>
                  <pre className="worktree-patch">{diff.stat}</pre>
                </>
              ) : null}
              {diff.truncated ? (
                <p className="worktree-note">
                  {t("tasks.detail.truncated")}
                </p>
              ) : null}
              <h4>{t("tasks.detail.patch")}</h4>
              <pre className="worktree-patch">
                {diff.patch || t("tasks.detail.noTrackedChanges")}
              </pre>
            </div>
          ) : null}
        </Modal>
      ) : null}
    </article>
  );
}
