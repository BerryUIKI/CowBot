-- Persisted, redacted prompt snapshots for LLM call audit (ADR-0037).
--
-- One row per LLM call. The payload is a sealed JSON document; the columns
-- beside it answer age, size, and integrity questions without parsing every
-- document. Rows cascade with their call, so deleting a run's history removes
-- its evidence with it.
--
-- `workspace_id` is denormalized so retention and channel purges stay scoped
-- even if a later build serves several workspaces from one database.
--
-- `payload` is NOT NULL: a snapshot that could not be bounded is still stored
-- as a sealed document carrying the truncation marker, never as a partial
-- string.

CREATE TABLE llm_prompt_snapshots (
    llm_call_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    token_count INTEGER NOT NULL,
    byte_size INTEGER NOT NULL,
    truncated INTEGER NOT NULL DEFAULT 0,
    redacted INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    payload TEXT NOT NULL,
    FOREIGN KEY (llm_call_id) REFERENCES llm_calls(id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);

CREATE INDEX idx_llm_prompt_snapshots_workspace_created
    ON llm_prompt_snapshots(workspace_id, created_at);

-- Workspace switch and retention window for prompt snapshots. A workspace
-- without a row uses the defaults: enabled, thirty days.
CREATE TABLE workspace_prompt_snapshot_settings (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    retention_days INTEGER NOT NULL DEFAULT 30,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);
