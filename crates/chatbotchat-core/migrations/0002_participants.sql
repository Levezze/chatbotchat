CREATE TABLE participants (
    handle        TEXT PRIMARY KEY,
    room_id       TEXT NOT NULL REFERENCES rooms(id),
    repo          TEXT NOT NULL,
    model         TEXT NOT NULL,
    cwd           TEXT NOT NULL,
    joined_at     TEXT NOT NULL,
    last_poll_at  TEXT NOT NULL,
    UNIQUE (room_id, repo, model, cwd)
);

CREATE INDEX participants_room_id ON participants (room_id);
