-- Link crew tasks to the task that started the run.
--
-- build_standard_crew sets run_group to the submitted task id on the
-- Planner/Coder/Reviewer tasks, so background cards and per-run worktrees have
-- one owning task.

ALTER TABLE tasks ADD COLUMN run_group TEXT;

CREATE INDEX idx_tasks_run_group ON tasks(run_group);
