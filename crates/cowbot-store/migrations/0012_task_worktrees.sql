-- Per-task git worktrees (ADR-0031).
--
-- Records are local to this machine: they are not part of .cowpkg exports.
-- The row is keyed by the owning task; a crew run uses the submitted task.

CREATE TABLE task_worktrees (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    project_path TEXT NOT NULL,
    base_branch TEXT NOT NULL,
    branch TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
