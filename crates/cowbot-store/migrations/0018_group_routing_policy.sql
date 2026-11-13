-- Group routing policy, participant mute, and reply cleanup (ADR-0042).

-- How an unmentioned group message picks its responder. `mention` keeps the
-- pre-existing behaviour: mentions first, then the opt-in model router, then
-- the default responder.
ALTER TABLE conversation_settings
    ADD COLUMN routing_mode TEXT NOT NULL DEFAULT 'mention';

-- Participants kept in the roster and history but never selected for a turn.
-- Stored as a JSON array of agent ids; empty means nobody is muted.
ALTER TABLE conversation_settings
    ADD COLUMN muted_participants TEXT NOT NULL DEFAULT '[]';

-- Strip a leading speaker label and truncate at another participant's turn.
-- On by default; a conversation can keep raw model output instead.
ALTER TABLE conversation_settings
    ADD COLUMN reply_cleanup INTEGER NOT NULL DEFAULT 1;
