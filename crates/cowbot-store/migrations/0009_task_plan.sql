-- Plan-mode artifacts stored with a task (ADR-0003).
--
-- A Plan-mode run writes its final output to `plan`. The user approves it
-- from the task detail, which flips `plan_approved` and allows continuing in
-- Manual mode.

ALTER TABLE tasks ADD COLUMN plan TEXT;
ALTER TABLE tasks ADD COLUMN plan_approved INTEGER NOT NULL DEFAULT 0;
