-- Group conversations and routing (ADR-0032).
--
-- Additive. Participant order, the persisted default responder, peer
-- attribution for mirrored replies, and the routing audit. Existing rows
-- backfill deterministically and older packages import unchanged.

ALTER TABLE channel_participants ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

-- Backfill in creation order. `joined_at` ties break by agent id so the
-- result is stable across databases.
UPDATE channel_participants
SET position = (
    SELECT COUNT(*)
    FROM channel_participants AS earlier
    WHERE earlier.channel_id = channel_participants.channel_id
      AND (
          earlier.joined_at < channel_participants.joined_at
          OR (
              earlier.joined_at = channel_participants.joined_at
              AND earlier.agent_id < channel_participants.agent_id
          )
      )
);

CREATE INDEX idx_channel_participants_position
    ON channel_participants(channel_id, position);

ALTER TABLE conversation_settings ADD COLUMN default_responder TEXT;

ALTER TABLE context_entries ADD COLUMN author_agent_id TEXT;

CREATE TABLE conversation_turns (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL,
    selected_agents TEXT NOT NULL DEFAULT '[]',
    unknown_mentions TEXT NOT NULL DEFAULT '[]',
    reason TEXT,
    router_call_id TEXT,
    task_ids TEXT NOT NULL DEFAULT '[]',
    mirrored_at TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
);

CREATE INDEX idx_conversation_turns_channel
    ON conversation_turns(channel_id, created_at);
