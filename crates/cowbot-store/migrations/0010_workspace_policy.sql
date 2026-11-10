-- Per-workspace approval policy (ADR-0003).
--
-- `bypass_critical` controls Critical-risk tool calls under Bypass
-- permissions: allow (default), ask, or block. Other modes always block
-- Critical calls.

CREATE TABLE workspace_policies (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    bypass_critical TEXT NOT NULL DEFAULT 'allow',
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);
