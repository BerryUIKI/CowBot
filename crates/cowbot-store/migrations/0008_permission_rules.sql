-- Workspace permission rules and audit decision sources (ADR-0003).
--
-- Rules are evaluated before the model evaluator. Audit rows carry the
-- decision source so the audit view can filter by rule, model, session,
-- human, policy, or system.

ALTER TABLE audit_events ADD COLUMN source TEXT;

CREATE INDEX idx_audit_events_source ON audit_events(workspace_id, source, created_at);

CREATE TABLE permission_rules (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    command TEXT,
    decision TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    source TEXT NOT NULL DEFAULT 'local',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);

CREATE INDEX idx_permission_rules_workspace ON permission_rules(workspace_id);
