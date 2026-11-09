-- Channel management: pinned state for channels and conversations.

ALTER TABLE channels ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
