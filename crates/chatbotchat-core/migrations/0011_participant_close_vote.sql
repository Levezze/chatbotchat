-- A participant's vote to close the room. Closing is consensus-based: a room
-- transitions to `closed` only when a quorum of *live* participants (default:
-- all live, i.e. polled within GHOST_AFTER) have voted. `cbc_close` records a
-- vote here; the room closes once the quorum is met, otherwise the counterpart
-- learns of the pending proposal via a computed `close_proposed` wait status and
-- either votes too (agree) or sends a message (which clears all votes — "no, I
-- have more to say"). Nullable RFC3339 timestamp: NULL = no pending vote.
ALTER TABLE participants ADD COLUMN wants_close_at TEXT;
