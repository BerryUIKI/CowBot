-- Whether a conversation follows the desktop's default model (ADR-0049).
--
-- Existing conversations keep the provider and model they recorded at
-- creation, so they backfill as pinned: their behaviour does not change on
-- upgrade. Conversations created after this migration write 1 and mirror the
-- default whenever it changes.
ALTER TABLE conversation_settings
    ADD COLUMN model_follows_default INTEGER NOT NULL DEFAULT 0;
