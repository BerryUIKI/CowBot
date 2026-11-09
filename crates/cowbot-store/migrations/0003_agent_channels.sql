-- Agent direct conversations: a channel can belong to exactly one agent.

ALTER TABLE channels ADD COLUMN agent_id TEXT;

CREATE UNIQUE INDEX idx_channels_agent
    ON channels(workspace_id, agent_id)
    WHERE agent_id IS NOT NULL;
