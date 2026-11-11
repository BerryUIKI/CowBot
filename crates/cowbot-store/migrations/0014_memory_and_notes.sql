-- Agent memory and shared notes (ADR-0033).
--
-- Notes are workspace or conversation Markdown shared with every participant.
-- Memory entries carry a scope through optional agent and channel ids, an
-- active/pending/archived status, and recall statistics. The FTS5 index is
-- maintained by the store in the same transaction as each entry write.

CREATE TABLE notes (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    channel_id TEXT,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX idx_notes_workspace
    ON notes(workspace_id) WHERE channel_id IS NULL;

CREATE UNIQUE INDEX idx_notes_channel
    ON notes(channel_id) WHERE channel_id IS NOT NULL;

CREATE TABLE workspace_memory_settings (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    extraction_enabled INTEGER NOT NULL DEFAULT 0,
    recall_enabled INTEGER NOT NULL DEFAULT 1,
    notes_budget_tokens INTEGER NOT NULL DEFAULT 800,
    recall_budget_tokens INTEGER NOT NULL DEFAULT 1200,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);

CREATE TABLE memory_entries (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    agent_id TEXT,
    channel_id TEXT,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    source TEXT NOT NULL,
    source_ref TEXT,
    status TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    tokens INTEGER NOT NULL,
    recall_count INTEGER NOT NULL DEFAULT 0,
    last_recalled_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id) REFERENCES agent_profiles(id) ON DELETE CASCADE,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE INDEX idx_memory_entries_scope
    ON memory_entries(workspace_id, channel_id, agent_id, status, pinned, updated_at);

CREATE VIRTUAL TABLE memory_entries_fts USING fts5(
    id UNINDEXED,
    content,
    tokenize = 'trigram'
);

-- Triggers keep the index in the same transaction as every write, including
-- foreign-key cascades that remove entries without going through the store.
CREATE TRIGGER memory_entries_ai AFTER INSERT ON memory_entries BEGIN
    INSERT INTO memory_entries_fts (id, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER memory_entries_au AFTER UPDATE OF content ON memory_entries BEGIN
    DELETE FROM memory_entries_fts WHERE id = old.id;
    INSERT INTO memory_entries_fts (id, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER memory_entries_ad AFTER DELETE ON memory_entries BEGIN
    DELETE FROM memory_entries_fts WHERE id = old.id;
END;

