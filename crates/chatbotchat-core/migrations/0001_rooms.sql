CREATE TABLE rooms (
    id               TEXT PRIMARY KEY,
    subject          TEXT NOT NULL,
    started_at       TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    state            TEXT NOT NULL,
    config           TEXT NOT NULL,
    prev_room_id     TEXT
);
