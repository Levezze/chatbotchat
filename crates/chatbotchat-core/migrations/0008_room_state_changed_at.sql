-- When the room last entered its current state. The sweeper anchors the
-- `stale`/`closed` -> `archived` window (14d in-state) to this column, not to
-- `last_activity_at`. Denormalized for cheap per-tick sweeping (user story 28);
-- the `events` table remains the durable audit log. Set on every transition.
ALTER TABLE rooms ADD COLUMN state_changed_at TEXT NOT NULL DEFAULT '';

-- Backfill any pre-existing rows so the column always holds a valid timestamp.
UPDATE rooms SET state_changed_at = started_at WHERE state_changed_at = '';
