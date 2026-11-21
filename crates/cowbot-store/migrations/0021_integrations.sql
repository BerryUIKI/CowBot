-- IM integrations (ADR-0056).
--
-- Deployment state, not workspace content: none of these tables is exported in
-- a .cowpkg, because a package would carry a bot token and a dedupe history
-- that only mean something to the deployment that produced them. An operator
-- re-binds after an import, exactly as they re-register a git project.

CREATE TABLE integrations (
    id           TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    label        TEXT NOT NULL,
    -- Adapter configuration as JSON: endpoint, bot token, signing secret.
    -- Never returned by a read and never logged.
    config       TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    revoked_at   TEXT
);

CREATE INDEX idx_integrations_workspace ON integrations(workspace_id);

CREATE TABLE integration_channels (
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_id    TEXT NOT NULL,
    channel_id     TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    -- The agent a mention of this bot addresses in a group channel. A direct
    -- conversation needs none.
    agent_id       TEXT,
    PRIMARY KEY (integration_id, external_id)
);

CREATE INDEX idx_integration_channels_channel ON integration_channels(channel_id);

CREATE TABLE integration_principals (
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_id    TEXT NOT NULL,
    user_id        TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    display_name   TEXT NOT NULL,
    PRIMARY KEY (integration_id, external_id)
);

CREATE INDEX idx_integration_principals_user ON integration_principals(user_id);

-- One row per platform event already handled. The primary key is the dedupe:
-- a platform that retries a delivery inserts nothing the second time.
CREATE TABLE integration_events (
    integration_id    TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_event_id TEXT NOT NULL,
    received_at       TEXT NOT NULL,
    PRIMARY KEY (integration_id, external_event_id)
);
