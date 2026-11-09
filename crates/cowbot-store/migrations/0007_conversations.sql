-- Conversation sessions and per-agent context branches (ADR-0030).
--
-- `channels.kind` distinguishes group channels, agent-owned direct
-- conversations, and user-created conversation sessions. Existing rows are
-- backfilled before the column is read by application code.

ALTER TABLE channels ADD COLUMN kind TEXT NOT NULL DEFAULT 'channel';
UPDATE channels SET kind = 'direct' WHERE agent_id IS NOT NULL;

CREATE INDEX idx_channels_kind ON channels(workspace_id, kind);

CREATE TABLE channel_participants (
    channel_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    joined_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, agent_id),
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE INDEX idx_channel_participants_agent ON channel_participants(agent_id);

CREATE TABLE conversation_settings (
    channel_id TEXT PRIMARY KEY NOT NULL,
    mode TEXT NOT NULL,
    provider_id TEXT,
    model TEXT NOT NULL,
    router_enabled INTEGER NOT NULL DEFAULT 0,
    auto_compact INTEGER NOT NULL DEFAULT 0,
    compact_threshold INTEGER NOT NULL DEFAULT 80,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE context_entries (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT,
    excluded INTEGER NOT NULL DEFAULT 0,
    tokens INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE,
    UNIQUE (channel_id, agent_id, seq)
);

CREATE INDEX idx_context_entries_branch
    ON context_entries(channel_id, agent_id, seq);

CREATE TABLE context_events (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT,
    tokens_before INTEGER NOT NULL,
    tokens_after INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE INDEX idx_context_events_branch
    ON context_events(channel_id, agent_id, created_at);
