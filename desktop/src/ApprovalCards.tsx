import type { Approval, ApprovalRisk, Task, ToolCallDetail } from "./types";
import { formatDateTime } from "./format";

interface ApprovalCardsProps {
  approvals: Approval[];
  tasks: Task[];
  toolCalls: Record<string, ToolCallDetail>;
  risks: Record<string, ApprovalRisk>;
  decidingId: string | null;
  onDecide: (approval: Approval, approved: boolean) => void;
}

export default function ApprovalCards({
  approvals,
  tasks,
  toolCalls,
  risks,
  decidingId,
  onDecide,
}: ApprovalCardsProps) {
  return (
    <div className="approvals-view">
      <header className="view-header">
        <h2 className="view-title">Approvals</h2>
        <p className="view-subtitle">Tool calls waiting for your decision.</p>
      </header>

      {approvals.length === 0 ? (
        <div className="empty-timeline">
          <p className="empty-title">Nothing waiting.</p>
          <p>Approvals appear here when a running agent asks to use a tool.</p>
        </div>
      ) : (
        <ul className="approval-list">
          {approvals.map((approval) => {
            const task = tasks.find((candidate) => candidate.id === approval.task_id);
            const deciding = decidingId === approval.id;
            const call = toolCalls[approval.tool_call_id];
            const risk = risks[approval.id];
            return (
              <li className="approval-card" key={approval.id}>
                <div className="approval-head">
                  <span className="status-badge status-in_progress">Pending</span>
                  <time>{formatDateTime(approval.created_at)}</time>
                </div>
                <p className="approval-task">
                  {task?.description ?? `Task ${approval.task_id.slice(0, 8)}`}
                </p>
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
                <div className="approval-actions">
                  <button
                    type="button"
                    className="primary-button"
                    disabled={deciding}
                    onClick={() => onDecide(approval, true)}
                  >
                    {deciding ? "Saving…" : "Approve"}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    disabled={deciding}
                    onClick={() => onDecide(approval, false)}
                  >
                    Reject
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
