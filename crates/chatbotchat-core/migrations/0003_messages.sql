CREATE TABLE messages (
    seq        INTEGER PRIMARY KEY,
    room_id    TEXT NOT NULL REFERENCES rooms(id),
    sender     TEXT NOT NULL,
    recipient  TEXT,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX messages_room_seq ON messages (room_id, seq);

ALTER TABLE participants ADD COLUMN last_read_seq INTEGER NOT NULL DEFAULT 0;
