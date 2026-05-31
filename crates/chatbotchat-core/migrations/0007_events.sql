-- Append-only lifecycle event log. Records every state transition and, on
-- archival, an `archive` row. This table IS the `on_archive` hook seam: a future
-- v2 vector-indexer subscribes by tailing `events` (kind = 'archive') without
-- modifying daemon code. Never updated or deleted.
CREATE TABLE events (
    id         INTEGER PRIMARY KEY,
    room_id    TEXT NOT NULL REFERENCES rooms(id),
    kind       TEXT NOT NULL,        -- 'transition' | 'archive'
    from_state TEXT,                 -- prior room state (NULL for non-transition kinds)
    to_state   TEXT,                 -- new room state
    detail     TEXT,                 -- optional free text (e.g. pause reason)
    at         TEXT NOT NULL         -- RFC3339 timestamp
);

CREATE INDEX events_room ON events (room_id, id);
