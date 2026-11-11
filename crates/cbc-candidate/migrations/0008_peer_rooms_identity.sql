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
    -- 1 once this room has held the pair's active slot. A replacement supersedes the last
    -- room that held it, never an older room that was set aside by this upgrade.
    held_slot INTEGER NOT NULL CHECK(held_slot IN (0,1)),
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
    CHECK(active = 0 OR held_slot = 1),
    CHECK((generation = 1) = (supersedes_room IS NULL))
);

-- Existing rows become generation 1 of their own chain. Before this migration a pair could
-- hold one open room per scope; only the newest usable one becomes the pair's active room.
-- Usable means open, with an active grant and both members still active in the room: a
-- revoked room stays open in candidate_rooms but must never take the slot. The others stay
-- readable, but a new open no longer returns them.
CREATE TEMP TABLE usable_v7 AS
SELECT c.context_id, c.room_id, c.introduction_sequence,
       min(c.first_member, c.second_member) AS low, max(c.first_member, c.second_member) AS high
FROM candidate_connections_v7 c
JOIN candidate_rooms r ON r.context_id = c.context_id AND r.room_id = c.room_id AND r.closed = 0
JOIN candidate_route_grants g ON g.context_id = c.context_id AND g.first_member = c.first_member
  AND g.second_member = c.second_member AND g.kind = c.kind AND g.scope_ref = c.scope_ref AND g.active = 1
WHERE (SELECT count(*) FROM candidate_room_members m
       WHERE m.context_id = c.context_id AND m.room_id = c.room_id AND m.active = 1
         AND m.member_id IN (c.first_member, c.second_member)) = 2;

INSERT INTO candidate_connections(context_id,first_member,second_member,kind,scope_ref,room_id,introduction_sequence,generation,supersedes_room,active,held_slot)
SELECT c.context_id, c.first_member, c.second_member, c.kind, c.scope_ref, c.room_id, c.introduction_sequence, 1, NULL,
       a.flag, a.flag
FROM candidate_connections_v7 c
JOIN (SELECT v.context_id, v.room_id,
             CASE WHEN EXISTS (
                 SELECT 1 FROM usable_v7 u WHERE u.context_id = v.context_id AND u.room_id = v.room_id
                   AND u.introduction_sequence = (
                       SELECT max(o.introduction_sequence) FROM usable_v7 o
                       WHERE o.context_id = u.context_id AND o.low = u.low AND o.high = u.high))
             THEN 1 ELSE 0 END AS flag
      FROM candidate_connections_v7 v) a ON a.context_id = c.context_id AND a.room_id = c.room_id;

DROP TABLE usable_v7;
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
