-- Re-key participant identity from the `(room_id, repo, model, cwd)` tuple to a
-- per-agent `instance` token. Two agents in the same repo+model+cwd (e.g. two
-- Claude Code sessions launched from the same project dir) previously collapsed
-- onto one handle and went invisible to each other; `instance` tells them apart.
-- `repo`/`model`/`cwd` stay as descriptive attributes (and the handle prefix).
--
-- SQLite cannot drop a table-level UNIQUE via ALTER, so recreate the table to
-- swap `UNIQUE (room_id, repo, model, cwd)` for `UNIQUE (room_id, instance)`.
-- Existing rows backfill `instance` from the tuple (newline-joined) — the same
-- expression the server synthesizes for a legacy caller that sends no instance,
-- so already-open rooms keep their current identities unchanged.

CREATE TABLE participants_new (
    handle        TEXT PRIMARY KEY,
    room_id       TEXT NOT NULL REFERENCES rooms(id),
    repo          TEXT NOT NULL,
    model         TEXT NOT NULL,
    cwd           TEXT NOT NULL,
    instance      TEXT NOT NULL DEFAULT '',
    joined_at     TEXT NOT NULL,
    last_poll_at  TEXT NOT NULL,
    last_read_seq INTEGER NOT NULL DEFAULT 0,
    UNIQUE (room_id, instance)
);

INSERT INTO participants_new
    (handle, room_id, repo, model, cwd, instance, joined_at, last_poll_at, last_read_seq)
SELECT handle, room_id, repo, model, cwd,
       repo || char(10) || model || char(10) || cwd,
       joined_at, last_poll_at, last_read_seq
FROM participants;

DROP TABLE participants;
ALTER TABLE participants_new RENAME TO participants;

CREATE INDEX participants_room_id ON participants (room_id);
