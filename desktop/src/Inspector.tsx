import type { ReactNode } from "react";

import ApprovalCards from "./ApprovalCards";
import BackgroundTasksContent from "./BackgroundTasksContent";
import { useT } from "./i18n";
import type { TranslationKey } from "./i18n/catalog";
import type {
  AgentProfile,
  Approval,
  ApprovalRisk,
  Task,
  TaskOverviewEntry,
  ToolCallDetail,
} from "./types";
import { isActiveOverviewStatus } from "./types";

export type InspectorTab = "approvals" | "background" | "task" | "context";

interface InspectorProps {
  tab: InspectorTab;
  onTabChange: (tab: InspectorTab) => void;
  onClose: () => void;
  inert?: boolean;
  taskPanel?: ReactNode | null;
  contextPanel?: ReactNode | null;
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

const baseTabs: { id: InspectorTab; labelKey: TranslationKey }[] = [
  { id: "approvals", labelKey: "inspector.approvals" },
  { id: "background", labelKey: "inspector.background" },
];

const middleTabs: { id: InspectorTab; labelKey: TranslationKey }[] = [
  { id: "context", labelKey: "inspector.context" },
];

export default function Inspector({
  tab,
  onTabChange,
  onClose,
  inert = false,
  taskPanel = null,
  contextPanel = null,
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
  const t = useT();
  const activeCount = overview.filter((entry) =>
    isActiveOverviewStatus(entry.status),
  ).length;

  const tabs = [
    { id: "task" as const, labelKey: "inspector.task" as const },
    ...middleTabs,
    ...baseTabs,
  ];
  const activeTab: InspectorTab = tab;

  return (
    <aside className="inspector" aria-label={t("inspector.aria")} inert={inert}>
      <header className="inspector-head">
        <div className="inspector-tabs" role="tablist" aria-label={t("inspector.tabsAria")}>
          {tabs.map((item) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              id={`inspector-tab-${item.id}`}
              aria-selected={activeTab === item.id}
              aria-controls={`inspector-panel-${item.id}`}
              className={`inspector-tab${activeTab === item.id ? " is-active" : ""}`}
              onClick={() => onTabChange(item.id)}
            >
              {t(item.labelKey)}
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
          aria-label={t("inspector.close")}
          title={t("inspector.closeTitle")}
          onClick={onClose}
        >
          ×
        </button>
      </header>

      <div className="inspector-body">
        <div
          className="inspector-pane"
          role="tabpanel"
          id="inspector-panel-task"
          aria-labelledby="inspector-tab-task"
          hidden={activeTab !== "task"}
        >
          {taskPanel}
        </div>
        <div
          className="inspector-pane"
          role="tabpanel"
          id="inspector-panel-context"
          aria-labelledby="inspector-tab-context"
          hidden={activeTab !== "context"}
        >
          {contextPanel}
        </div>
        <div
          className="inspector-pane"
          role="tabpanel"
          id="inspector-panel-approvals"
          aria-labelledby="inspector-tab-approvals"
          hidden={activeTab !== "approvals"}
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
          hidden={activeTab !== "background"}
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
