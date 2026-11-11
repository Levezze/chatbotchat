-- Work items, atomic revisions and outbox events (T-07, T-09), #113 record layer R2.

-- Views read the tables rebuilt below; drop them first and recreate them at the end.
DROP VIEW candidate_unread_items;
DROP VIEW candidate_delivery_items;

-- A work event is a fifth item class. Nothing references this table, so it is rebuilt in place.
ALTER TABLE candidate_item_facts RENAME TO candidate_item_facts_v8;
CREATE TABLE candidate_item_facts (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    item_class TEXT NOT NULL CHECK(item_class IN ('participant','introduction','notice','closure','work_event')),
    committed_instance TEXT NOT NULL REFERENCES candidate_core_instances(instance_id),
    committed_mono_us INTEGER NOT NULL CHECK(typeof(committed_mono_us) = 'integer' AND committed_mono_us >= 0),
    committed_wall_ms INTEGER NOT NULL CHECK(typeof(committed_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);
INSERT INTO candidate_item_facts SELECT * FROM candidate_item_facts_v8;
DROP TABLE candidate_item_facts_v8;

-- One owner, fixed at registration. `revision` is the latest committed revision.
CREATE TABLE candidate_work_items (
    context_id TEXT NOT NULL REFERENCES candidate_contexts(context_id) ON DELETE RESTRICT,
    item_id TEXT NOT NULL CHECK(length(item_id) BETWEEN 1 AND 128),
    owner_member TEXT NOT NULL,
    summary TEXT NOT NULL CHECK(length(summary) BETWEEN 1 AND 2000),
    stage TEXT NOT NULL CHECK(stage IN ('planning','implementing','in_review','ready_to_merge','merged','abandoned')),
    next_action TEXT NOT NULL DEFAULT '',
    blocked_operation TEXT,
    branch TEXT,
    hitl INTEGER NOT NULL DEFAULT 0 CHECK(hitl IN (0,1)),
    revision INTEGER NOT NULL CHECK(typeof(revision) = 'integer' AND revision >= 1),
    created_wall_ms INTEGER NOT NULL CHECK(typeof(created_wall_ms) = 'integer'),
    updated_wall_ms INTEGER NOT NULL CHECK(typeof(updated_wall_ms) = 'integer'),
    PRIMARY KEY(context_id, item_id),
    FOREIGN KEY(context_id,owner_member) REFERENCES candidate_members(context_id,member_id)
);
CREATE INDEX candidate_work_items_owner ON candidate_work_items(context_id, owner_member);

-- Append-only. The owner-authenticated route is the user, who is not a member.
CREATE TABLE candidate_work_revisions (
    context_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(typeof(revision) = 'integer' AND revision >= 1),
    actor_kind TEXT NOT NULL CHECK(actor_kind IN ('member','owner')),
    actor_member TEXT,
    request_key TEXT NOT NULL CHECK(length(request_key) BETWEEN 1 AND 128),
    fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
    changed TEXT NOT NULL,
    at_wall_ms INTEGER NOT NULL CHECK(typeof(at_wall_ms) = 'integer'),
    PRIMARY KEY(context_id, item_id, revision),
    FOREIGN KEY(context_id,item_id) REFERENCES candidate_work_items(context_id,item_id),
    FOREIGN KEY(context_id,actor_member) REFERENCES candidate_members(context_id,member_id),
    CHECK((actor_kind = 'member') = (actor_member IS NOT NULL))
);
CREATE UNIQUE INDEX candidate_work_revision_keys
    ON candidate_work_revisions(context_id, actor_kind, coalesce(actor_member, ''), request_key);

-- The outbox: one inbox item per revision per report room of the owner, tagged rather than
-- stored as a new kind, so the item table and its immutable batches stay as they are. The
-- stored sender is the acting member, or the item owner for a change made by the user.
CREATE TABLE candidate_work_events (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    FOREIGN KEY(context_id,item_id,revision) REFERENCES candidate_work_revisions(context_id,item_id,revision)
);
CREATE INDEX candidate_work_events_revision ON candidate_work_events(context_id, item_id, revision);
CREATE TABLE candidate_work_event_recipients (
    sequence INTEGER NOT NULL REFERENCES candidate_work_events(sequence),
    member_id TEXT NOT NULL,
    PRIMARY KEY(sequence, member_id)
);

-- As in 0005, plus work events: server information with an explicit audience.
CREATE VIEW candidate_delivery_items AS
SELECT i.sequence,i.context_id,i.room_id,m.member_id AS recipient,
       CASE WHEN n.sequence IS NULL AND w.sequence IS NULL THEN i.sender ELSE 'cbc' END AS sender,
       CASE WHEN n.sequence IS NOT NULL THEN 'room_notice'
            WHEN w.sequence IS NOT NULL THEN 'work_event'
            ELSE i.kind END AS kind,
       i.body
FROM candidate_items i
JOIN candidate_room_members m ON m.context_id=i.context_id AND m.room_id=i.room_id AND m.active=1
LEFT JOIN candidate_room_notices n ON n.sequence=i.sequence
LEFT JOIN candidate_work_events w ON w.sequence=i.sequence
WHERE (n.sequence IS NULL AND w.sequence IS NULL AND i.sender<>m.member_id)
   OR (n.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_notice_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
   ))
   OR (w.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_work_event_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
   ));

-- Unchanged from 0005.
CREATE VIEW candidate_unread_items AS
SELECT d.* FROM candidate_delivery_items d
LEFT JOIN candidate_room_notices n ON n.sequence=d.sequence
WHERE NOT EXISTS (
    SELECT 1 FROM candidate_receipts r
    WHERE r.context_id=d.context_id AND r.member_id=d.recipient AND r.sequence=d.sequence
) AND (
    n.sequence IS NULL
    OR NOT EXISTS (
        SELECT 1 FROM candidate_room_notices newer
        JOIN candidate_notice_recipients nr ON nr.sequence=newer.sequence AND nr.member_id=d.recipient
        WHERE newer.context_id=n.context_id AND newer.room_id=n.room_id AND newer.message_count>n.message_count
    )
    OR EXISTS (
        SELECT 1 FROM candidate_batch_items bi
        JOIN candidate_batches b ON b.reference=bi.reference
        JOIN candidate_members a ON a.context_id=b.context_id AND a.member_id=b.member_id AND a.epoch=b.epoch
        WHERE bi.sequence=d.sequence AND b.context_id=d.context_id AND b.member_id=d.recipient AND b.state='pending'
    )
);
