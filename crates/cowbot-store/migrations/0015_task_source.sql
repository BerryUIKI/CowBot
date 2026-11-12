-- Distinguish user-created work items from conversation turns.
--
-- Channel and conversation turns reuse the task pipeline, but they are not
-- work items: the desktop lists user-created tasks by default and can reveal
-- turns on demand.

ALTER TABLE tasks ADD COLUMN source TEXT NOT NULL DEFAULT 'user';
