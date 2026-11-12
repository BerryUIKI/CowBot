import { useMemo, useState } from "react";

import Avatar from "./Avatar";
import type { AgentProfile, Task } from "./types";
import { taskStatusKind, taskStatusLabel } from "./types";

type TaskFilter = "active" | "archived" | "all";

interface TaskListProps {
  tasks: Task[];
  agents: AgentProfile[];
  onOpenTask: (taskId: string) => void;
  onNewTask: () => void;
}

function agentName(agents: AgentProfile[], id: string | null): string {
  if (!id) return "Unassigned";
  return agents.find((agent) => agent.id === id)?.name ?? "Unknown agent";
}

/**
 * The task list shown in the inspector when no task is selected. Conversation
 * turns are hidden by default so the list holds work items, not chat replies.
 */
export default function TaskList({
  tasks,
  agents,
  onOpenTask,
  onNewTask,
}: TaskListProps) {
  const [filter, setFilter] = useState<TaskFilter>("active");
  const [showTurns, setShowTurns] = useState(false);

  const rows = useMemo(
    () =>
      tasks
        .filter((task) => showTurns || task.source === "user")
        .filter((task) =>
          filter === "all"
            ? true
            : filter === "archived"
              ? task.archived
              : !task.archived,
        ),
    [filter, showTurns, tasks],
  );

  return (
    <section className="task-panel" aria-label="Tasks">
      <header className="task-panel-head">
        <div className="task-filter" role="group" aria-label="Task filter">
          {(["active", "archived", "all"] as const).map((item) => (
            <button
              key={item}
              type="button"
              className="task-filter-item"
              aria-pressed={filter === item}
              onClick={() => setFilter(item)}
            >
              {item === "active"
                ? "Active"
                : item === "archived"
                  ? "Archived"
                  : "All"}
            </button>
          ))}
        </div>
        <button type="button" className="text-button" onClick={onNewTask}>
          New task
        </button>
      </header>

      {rows.length === 0 ? (
        <p className="task-panel-empty">
          {tasks.length === 0 ? "No tasks yet." : "No tasks match."}
        </p>
      ) : (
        <ul className="contact-list">
          {rows.map((task) => {
            const kind = taskStatusKind(task.status);
            const assignee = agentName(agents, task.assigned_agent);
            return (
              <li key={task.id} className="contact-item">
                <button
                  type="button"
                  className="contact-row"
                  onClick={() => onOpenTask(task.id)}
                >
                  <Avatar name={assignee} kind="agent" size="small" />
                  <span className="contact-text">
                    <span className="contact-top">
                      <span className="contact-name">
                        {task.title || task.description}
                      </span>
                    </span>
                    <span className="contact-snippet">
                      <span className={`status-dot status-${kind}`} />{" "}
                      {taskStatusLabel(task.status)} · {assignee}
                      {task.archived ? " · Archived" : ""}
                      {task.source !== "user" ? " · Turn" : ""}
                    </span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <label className="context-toggle task-panel-toggle">
        <input
          type="checkbox"
          checked={showTurns}
          onChange={(event) => setShowTurns(event.target.checked)}
        />
        <span>Show conversation turns</span>
      </label>
    </section>
  );
}
