import type { Approval, ApprovalRisk, Task, ToolCallDetail } from "./types";
import { formatDateTime } from "./format";

interface ApprovalCardProps {
  approval: Approval;
  task: Task | undefined;
  call: ToolCallDetail | undefined;
  risk: ApprovalRisk | undefined;
  deciding: boolean;
  compact?: boolean;
  onDecide: (
    approval: Approval,
    approved: boolean,
    forSession?: boolean,
  ) => void;
}

function channelArgument(call: ToolCallDetail | undefined): string | null {
  if (!call || typeof call.arguments !== "object" || call.arguments === null) {
    return null;
  }
  const value = (call.arguments as Record<string, unknown>).channel;
  return typeof value === "string" && value.trim() ? value : null;
}

export default function ApprovalCard({
  approval,
  task,
  call,
  risk,
  deciding,
  compact = false,
  onDecide,
}: ApprovalCardProps) {
  const destructive = call?.name === "delete_channel";
  const channel = channelArgument(call);
  const canRemember = risk?.risk !== "critical";

  return (
    <div
      className={`approval-card${compact ? " approval-card-inline" : ""}${
        destructive ? " approval-card-destructive" : ""
      }${risk?.risk === "critical" ? " approval-card-critical" : ""}`}
    >
      <div className="approval-head">
        <span className="status-badge status-in_progress">Pending</span>
        <time>{formatDateTime(approval.created_at)}</time>
      </div>
      <p className="approval-task">
        {task?.title || task?.description || `Task ${approval.task_id.slice(0, 8)}`}
      </p>
      {destructive ? (
        <p className="approval-destructive">
          Delete channel <strong>{channel ?? "(unnamed)"}</strong>
        </p>
      ) : null}
      <p className="approval-meta">
        {call ? (
          <>
            Tool <code>{call.name}</code>
          </>
        ) : (
          <>
            Tool call <code>{approval.tool_call_id}</code>
          </>
        )}
      </p>
      {call ? (
        <pre className="approval-args">
          {JSON.stringify(call.arguments ?? null, null, 2)}
        </pre>
      ) : null}
      {risk ? (
        <p className={`approval-risk risk-${risk.risk}`}>
          {risk.risk.replaceAll("_", " ")}
          {risk.reason ? ` — ${risk.reason}` : ""}
        </p>
      ) : null}
      {risk?.rule ? (
        <p className="approval-rule">
          Workspace rule <code>{risk.rule.description}</code> —{" "}
          {risk.rule.decision}
        </p>
      ) : null}
      <div className="approval-actions">
        <button
          type="button"
          className="primary-button"
          disabled={deciding}
          onClick={() => onDecide(approval, true)}
        >
          {deciding ? "Saving…" : "Approve once"}
        </button>
        {canRemember ? (
          <button
            type="button"
            className="ghost-button"
            disabled={deciding}
            onClick={() => onDecide(approval, true, true)}
          >
            Approve for session
          </button>
        ) : null}
        <button
          type="button"
          className="ghost-button"
          disabled={deciding}
          onClick={() => onDecide(approval, false)}
        >
          Reject
        </button>
      </div>
    </div>
  );
}
