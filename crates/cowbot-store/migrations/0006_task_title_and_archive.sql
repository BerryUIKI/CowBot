ALTER TABLE tasks ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE tasks ADD COLUMN archived_at TEXT;

UPDATE tasks
SET title = substr(trim(description), 1, 80)
WHERE title = '';

CREATE INDEX idx_tasks_workspace_archived ON tasks(workspace_id, archived_at);
