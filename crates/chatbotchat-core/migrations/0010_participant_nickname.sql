-- Add an optional human-friendly display label for a participant, distinct from
-- its identity. The nickname never affects handle derivation, routing, or the
-- `sender` of a message — it is purely a display alias (e.g. "concierge-agent")
-- so humans can tell rows apart in `cbc list` / `cbc status`, especially when a
-- room has accrued dead "ghost" rows from identity churn. Nullable: a participant
-- that joins without one renders by its handle.

ALTER TABLE participants ADD COLUMN nickname TEXT;
