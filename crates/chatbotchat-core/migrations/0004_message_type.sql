-- Every message gains a `type`. `msg` is a conversation turn (counts toward the
-- caps); the other variants are sentinels (signals that do not count). Existing
-- rows default to `msg`, so historical cap counts do not shift.
ALTER TABLE messages ADD COLUMN type TEXT NOT NULL DEFAULT 'msg';
