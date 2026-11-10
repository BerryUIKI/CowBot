import ApprovalCards from "./ApprovalCards";
import BackgroundTasksContent from "./BackgroundTasksContent";
import type {
  AgentProfile,
  Approval,
  ApprovalRisk,
  Task,
  TaskOverviewEntry,
  ToolCallDetail,
} from "./types";
import { isActiveOverviewStatus } from "./types";

export type InspectorTab = "approvals" | "background";

interface InspectorProps {
  tab: InspectorTab;
  onTabChange: (tab: InspectorTab) => void;
  onClose: () => void;
  inert?: boolean;
  overview: TaskOverviewEntry[];
  agents: AgentProfile[];
  now: number;
  onOpenTask: (runId: string) => void;
  approvals: Approval[];
  tasks: Task[];
  toolCalls: Record<string, ToolCallDetail>;
  risks: Record<string, ApprovalRisk>;
  decidingId: string | null;
  onDecide: (
    approval: Approval,
    approved: boolean,
    forSession?: boolean,
  ) => void;
}

const tabs: { id: InspectorTab; label: string }[] = [
  { id: "approvals", label: "Approvals" },
  { id: "background", label: "Background" },
];

export default function Inspector({
  tab,
  onTabChange,
  onClose,
  inert = false,
  overview,
  agents,
  now,
  onOpenTask,
  approvals,
  tasks,
  toolCalls,
  risks,
  decidingId,
  onDecide,
}: InspectorProps) {
  const activeCount = overview.filter((entry) =>
    isActiveOverviewStatus(entry.status),
  ).length;

  return (
    <aside className="inspector" aria-label="Inspector" inert={inert}>
      <header className="inspector-head">
        <div className="inspector-tabs" role="tablist" aria-label="Inspector">
          {tabs.map((item) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              id={`inspector-tab-${item.id}`}
              aria-selected={tab === item.id}
              aria-controls={`inspector-panel-${item.id}`}
              className={`inspector-tab${tab === item.id ? " is-active" : ""}`}
              onClick={() => onTabChange(item.id)}
            >
              {item.label}
              {item.id === "approvals" && approvals.length > 0 ? (
                <span className="nav-count">{approvals.length}</span>
              ) : null}
              {item.id === "background" && activeCount > 0 ? (
                <span className="nav-count">{activeCount}</span>
              ) : null}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="icon-button"
          aria-label="Close inspector"
          title="Close"
          onClick={onClose}
        >
          ×
        </button>
      </header>

      <div className="inspector-body">
        <div
          className="inspector-pane"
          role="tabpanel"
          id="inspector-panel-approvals"
          aria-labelledby="inspector-tab-approvals"
          hidden={tab !== "approvals"}
        >
          <ApprovalCards
            embedded
            approvals={approvals}
            tasks={tasks}
            toolCalls={toolCalls}
            risks={risks}
            decidingId={decidingId}
            onDecide={onDecide}
          />
        </div>
        <div
          className="inspector-pane"
          role="tabpanel"
          id="inspector-panel-background"
          aria-labelledby="inspector-tab-background"
          hidden={tab !== "background"}
        >
          <BackgroundTasksContent
            overview={overview}
            agents={agents}
            now={now}
            onOpenTask={onOpenTask}
          />
        </div>
      </div>
    </aside>
  );
}
