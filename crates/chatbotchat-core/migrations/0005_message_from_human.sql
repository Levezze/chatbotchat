-- Every message gains `from_human`: 1 when the sending agent folded its user's
-- input into this turn (the `--human` send), 0 for an autonomous agent turn.
-- This is the soft-cap reset boundary — the consecutive-`msg` counter restarts
-- at a `from_human = 1` row. Existing rows default to 0 (autonomous), preserving
-- historical soft-cap behavior. Independent of `type`: a `msg` can be human-fed.
ALTER TABLE messages ADD COLUMN from_human INTEGER NOT NULL DEFAULT 0;
