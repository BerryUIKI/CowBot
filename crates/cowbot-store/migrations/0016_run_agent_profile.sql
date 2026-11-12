-- Snapshot the executing agent profile with each run.
--
-- Standard crew roles are transient profiles that are never written to the
-- agent table, so a run could not otherwise name its own agent. The context
-- inspector rebuilds a run's request from this snapshot (ADR-0036).

ALTER TABLE runs ADD COLUMN agent_profile TEXT;
