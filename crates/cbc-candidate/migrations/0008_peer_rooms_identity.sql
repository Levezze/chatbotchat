-- Core-bound identity (T-25) and one active room per pair (T-26), #113 record layer R1.

-- A pair now owns a chain of rooms. Each row is one generation; at most one is active.
-- The old primary key (one room per pair, kind and scope) is replaced by the room id.
-- The old table moves aside first, so the new one and its self-reference carry their final
-- name whatever the foreign_keys and legacy_alter_table settings are.
ALTER TABLE candidate_connections RENAME TO candidate_connections_v7;
CREATE TABLE candidate_connections (
    context_id TEXT NOT NULL,
    first_member TEXT NOT NULL,
    second_member TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    room_id TEXT NOT NULL,
    introduction_sequence INTEGER NOT NULL REFERENCES candidate_items(sequence),
    generation INTEGER NOT NULL CHECK(typeof(generation) = 'integer' AND generation >= 1),
    supersedes_room TEXT,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    closed_by TEXT,
    close_reason TEXT,
    closed_wall_ms INTEGER,
    PRIMARY KEY(context_id, room_id),
    FOREIGN KEY(context_id,first_member,second_member,kind,scope_ref)
      REFERENCES candidate_route_grants(context_id,first_member,second_member,kind,scope_ref),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id),
    FOREIGN KEY(context_id,supersedes_room) REFERENCES candidate_connections(context_id,room_id),
    CHECK((closed_by IS NULL) = (close_reason IS NULL) AND (closed_by IS NULL) = (closed_wall_ms IS NULL)),
    CHECK(active = 0 OR closed_by IS NULL),
    CHECK((generation = 1) = (supersedes_room IS NULL))
);

-- Existing rows become generation 1 of their own chain. Before this migration a pair could
-- hold one open room per scope; only the newest open one stays the pair's active room. The
-- others stay open and readable, but a new open no longer returns them.
INSERT INTO candidate_connections(context_id,first_member,second_member,kind,scope_ref,room_id,introduction_sequence,generation,supersedes_room,active)
SELECT c.context_id, c.first_member, c.second_member, c.kind, c.scope_ref, c.room_id, c.introduction_sequence, 1, NULL,
       CASE WHEN r.closed = 0 AND c.introduction_sequence = (
           SELECT max(o.introduction_sequence) FROM candidate_connections_v7 o
           JOIN candidate_rooms orr ON orr.context_id = o.context_id AND orr.room_id = o.room_id
           WHERE o.context_id = c.context_id AND orr.closed = 0
             AND min(o.first_member, o.second_member) = min(c.first_member, c.second_member)
             AND max(o.first_member, o.second_member) = max(c.first_member, c.second_member))
       THEN 1 ELSE 0 END
FROM candidate_connections_v7 c
JOIN candidate_rooms r ON r.context_id = c.context_id AND r.room_id = c.room_id;

DROP TABLE candidate_connections_v7;

-- T-26: one active room per unordered pair, whatever the kind or scope. Concurrent opens are
-- serialized by BEGIN IMMEDIATE; this index is the backstop.
CREATE UNIQUE INDEX candidate_connections_active_pair ON candidate_connections(
    context_id, min(first_member, second_member), max(first_member, second_member)
) WHERE active = 1;
CREATE INDEX candidate_connections_pair ON candidate_connections(context_id, first_member, second_member, generation);

-- T-25: a retired identity never comes back, through registration, binding or enrollment.
ALTER TABLE candidate_members ADD COLUMN retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1));

CREATE TABLE candidate_identity_events (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('enrolled','rebound','retired')),
    epoch INTEGER NOT NULL CHECK(typeof(epoch) = 'integer' AND epoch >= 0),
    reason TEXT,
    at_wall_ms INTEGER NOT NULL CHECK(typeof(at_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id),
    CHECK((kind = 'retired') = (reason IS NOT NULL))
);
CREATE INDEX candidate_identity_events_member ON candidate_identity_events(context_id, member_id, event_id);
