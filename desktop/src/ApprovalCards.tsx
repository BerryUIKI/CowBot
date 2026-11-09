import ApprovalCard from "./ApprovalCard";
import type { Approval, ApprovalRisk, Task, ToolCallDetail } from "./types";

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
            return (
              <li key={approval.id}>
                <ApprovalCard
                  approval={approval}
                  task={task}
                  call={toolCalls[approval.tool_call_id]}
                  risk={risks[approval.id]}
                  deciding={decidingId === approval.id}
                  onDecide={onDecide}
                />
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
