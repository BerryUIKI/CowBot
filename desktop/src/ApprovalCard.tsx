import type { Approval, ApprovalRisk, Task, ToolCallDetail } from "./types";
import { formatDateTime } from "./format";
import { useT } from "./i18n";
import type { TranslationKey } from "./i18n/catalog";

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

const riskKeys: Record<string, TranslationKey> = {
  low: "approvals.risk.low",
  medium: "approvals.risk.medium",
  high: "approvals.risk.high",
  critical: "approvals.risk.critical",
};

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
  const t = useT();
  const destructive = call?.name === "delete_channel";
  const channel = channelArgument(call);
  // Without live event metadata we cannot prove the call is non-critical, so
  // a missing risk is treated like Critical for session approval.
  const canRemember = risk !== undefined && risk.risk !== "critical";

  return (
    <div
      className={`approval-card${compact ? " approval-card-inline" : ""}${
        destructive ? " approval-card-destructive" : ""
      }${risk?.risk === "critical" ? " approval-card-critical" : ""}`}
    >
      <div className="approval-head">
        <span className="status-badge status-in_progress">
          {t("approvals.pending")}
        </span>
        <time>{formatDateTime(approval.created_at)}</time>
      </div>
      <p className="approval-task">
        {task?.title ||
          task?.description ||
          t("approvals.taskRef", { id: approval.task_id.slice(0, 8) })}
      </p>
      {destructive ? (
        <p className="approval-destructive">
          {t("approvals.deleteChannel", {
            name: channel ?? t("approvals.unnamed"),
          })}
        </p>
      ) : null}
      <p className="approval-meta">
        {call ? (
          <>
            {t("approvals.tool")} <code>{call.name}</code>
          </>
        ) : (
          <>
            {t("approvals.toolCall")} <code>{approval.tool_call_id}</code>
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
          {riskKeys[risk.risk] ? t(riskKeys[risk.risk]) : risk.risk.replaceAll("_", " ")}
          {risk.reason ? ` — ${risk.reason}` : ""}
        </p>
      ) : null}
      {risk?.rule ? (
        <p className="approval-rule">
          {t("approvals.workspaceRule", {
            description: risk.rule.description,
            decision: risk.rule.decision,
          })}
        </p>
      ) : null}
      <div className="approval-actions">
        <button
          type="button"
          className="primary-button"
          disabled={deciding}
          onClick={() => onDecide(approval, true)}
        >
          {deciding ? t("approvals.saving") : t("approvals.approveOnce")}
        </button>
        {canRemember ? (
          <button
            type="button"
            className="ghost-button"
            disabled={deciding}
            onClick={() => onDecide(approval, true, true)}
          >
            {t("approvals.approveSession")}
          </button>
        ) : null}
        <button
          type="button"
          className="ghost-button"
          disabled={deciding}
          onClick={() => onDecide(approval, false)}
        >
          {t("approvals.reject")}
        </button>
      </div>
    </div>
  );
}
