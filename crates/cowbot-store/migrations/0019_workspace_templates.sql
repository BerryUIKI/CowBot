-- Workspace-scoped templates (Phase 2 milestone 2.5).
--
-- The `templates` table has existed since 0001 without a workspace, because
-- local mode has exactly one workspace and sharing was harmless. A server is
-- multi-tenant: the same table would make every user template visible to, and
-- editable by, every other workspace. Give the column a default so rows written
-- before this migration keep working, then backfill them from the `agents` a
-- template most closely belongs to — which is no relation at all, so the
-- honest backfill is the single workspace a local database has.
ALTER TABLE templates ADD COLUMN workspace_id TEXT;

-- Exactly one workspace exists in a database written before this migration
-- (the local bootstrap ensures it), so an unscoped row can be attributed to it
-- without guessing. A database with no workspace yet has no such rows.
UPDATE templates
SET workspace_id = (SELECT id FROM workspaces ORDER BY created_at, id LIMIT 1)
WHERE workspace_id IS NULL;

CREATE INDEX idx_templates_workspace ON templates(workspace_id);
