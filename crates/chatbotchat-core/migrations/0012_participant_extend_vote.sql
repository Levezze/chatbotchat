-- A participant's vote to extend the room's message cap. Extending is
-- consensus-based, mirroring close: the hard cap rises by +10 only when a quorum
-- of *live* participants (default: all live, i.e. polled within GHOST_AFTER) have
-- voted. `cbc_extend` records a vote here; the cap bumps once the quorum is met
-- (and all extend votes clear), otherwise the counterpart learns of the pending
-- proposal via a computed `extend_proposed` wait status and either votes too
-- (agree) or declines by ignoring it. Unlike `wants_close_at`, a conversational
-- message does NOT clear this — wanting to extend and continuing to talk are not
-- opposites. Nullable RFC3339 timestamp: NULL = no pending vote.
ALTER TABLE participants ADD COLUMN wants_extend_at TEXT;
