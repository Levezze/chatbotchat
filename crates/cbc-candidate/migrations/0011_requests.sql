-- Canonical requests and their answers (T-10, T-12), #113 record layer R3.

DROP VIEW candidate_unread_items;
DROP VIEW candidate_delivery_items;

-- A request event is a sixth item class. Rebuilt in place, as in 0009.
ALTER TABLE candidate_item_facts RENAME TO candidate_item_facts_v10;
CREATE TABLE candidate_item_facts (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    item_class TEXT NOT NULL CHECK(item_class IN ('participant','introduction','notice','closure','work_event','request_event')),
    committed_instance TEXT NOT NULL REFERENCES candidate_core_instances(instance_id),
    committed_mono_us INTEGER NOT NULL CHECK(typeof(committed_mono_us) = 'integer' AND committed_mono_us >= 0),
    committed_wall_ms INTEGER NOT NULL CHECK(typeof(committed_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);
INSERT INTO candidate_item_facts SELECT * FROM candidate_item_facts_v10;
DROP TABLE candidate_item_facts_v10;

-- One canonical row per question. The callback is the member that asked and is woken by the
-- answer. The owner answers: the user, or one member.
CREATE TABLE candidate_requests (
    context_id TEXT NOT NULL REFERENCES candidate_contexts(context_id) ON DELETE RESTRICT,
    request_id TEXT NOT NULL CHECK(length(request_id) BETWEEN 1 AND 128),
    callback_member TEXT NOT NULL,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('user','member')),
    owner_member TEXT,
    item_id TEXT,
    question TEXT NOT NULL CHECK(length(question) BETWEEN 1 AND 4000),
    state TEXT NOT NULL CHECK(state IN ('open','relayed','answered','withdrawn')),
    withdraw_reason TEXT,
    request_key TEXT NOT NULL CHECK(length(request_key) BETWEEN 1 AND 128),
    fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
    created_wall_ms INTEGER NOT NULL CHECK(typeof(created_wall_ms) = 'integer'),
    PRIMARY KEY(context_id, request_id),
    FOREIGN KEY(context_id,callback_member) REFERENCES candidate_members(context_id,member_id),
    FOREIGN KEY(context_id,owner_member) REFERENCES candidate_members(context_id,member_id),
    FOREIGN KEY(context_id,item_id) REFERENCES candidate_work_items(context_id,item_id),
    CHECK((owner_kind = 'member') = (owner_member IS NOT NULL)),
    CHECK(owner_member IS NULL OR owner_member <> callback_member),
    CHECK((state = 'withdrawn') = (withdraw_reason IS NOT NULL))
);
CREATE UNIQUE INDEX candidate_request_keys ON candidate_requests(context_id, callback_member, request_key);
CREATE INDEX candidate_requests_open ON candidate_requests(context_id, state);

-- Append-only. The channel comes from the route that carried the answer, never from the
-- client: 'owner' is the owner-authenticated route, 'member' the owner member's own binding,
-- 'relayed' any other member passing on what the user said in a chat.
CREATE TABLE candidate_request_answers (
    answer_id INTEGER PRIMARY KEY,
    context_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    channel TEXT NOT NULL CHECK(channel IN ('owner','member','relayed')),
    actor_member TEXT,
    body TEXT NOT NULL CHECK(length(body) BETWEEN 1 AND 4000),
    request_key TEXT NOT NULL CHECK(length(request_key) BETWEEN 1 AND 128),
    fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
    at_wall_ms INTEGER NOT NULL CHECK(typeof(at_wall_ms) = 'integer'),
    -- Set once, by the callback, when it acts on the answer. Answered, delivered and applied
    -- are three different facts.
    applied_wall_ms INTEGER CHECK(applied_wall_ms IS NULL OR typeof(applied_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,request_id) REFERENCES candidate_requests(context_id,request_id),
    FOREIGN KEY(context_id,actor_member) REFERENCES candidate_members(context_id,member_id),
    CHECK((channel = 'owner') = (actor_member IS NULL))
);
CREATE UNIQUE INDEX candidate_request_answer_keys
    ON candidate_request_answers(context_id, channel, coalesce(actor_member, ''), request_key);
CREATE INDEX candidate_request_answers_request ON candidate_request_answers(context_id, request_id);

-- Request events go through the inbox like work events, with an explicit audience.
CREATE TABLE candidate_request_events (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    event TEXT NOT NULL CHECK(event IN ('opened','answered','withdrawn')),
    answer_id INTEGER REFERENCES candidate_request_answers(answer_id),
    FOREIGN KEY(context_id,request_id) REFERENCES candidate_requests(context_id,request_id),
    CHECK((event = 'answered') = (answer_id IS NOT NULL))
);
CREATE INDEX candidate_request_events_request ON candidate_request_events(context_id, request_id);
CREATE TABLE candidate_request_event_recipients (
    sequence INTEGER NOT NULL REFERENCES candidate_request_events(sequence),
    member_id TEXT NOT NULL,
    PRIMARY KEY(sequence, member_id)
);

-- As in 0009, plus request events.
CREATE VIEW candidate_delivery_items AS
SELECT i.sequence,i.context_id,i.room_id,m.member_id AS recipient,
       CASE WHEN n.sequence IS NULL AND w.sequence IS NULL AND q.sequence IS NULL THEN i.sender ELSE 'cbc' END AS sender,
       CASE WHEN n.sequence IS NOT NULL THEN 'room_notice'
            WHEN w.sequence IS NOT NULL THEN 'work_event'
            WHEN q.sequence IS NOT NULL THEN 'request_event'
            ELSE i.kind END AS kind,
       i.body
FROM candidate_items i
JOIN candidate_room_members m ON m.context_id=i.context_id AND m.room_id=i.room_id AND m.active=1
LEFT JOIN candidate_room_notices n ON n.sequence=i.sequence
LEFT JOIN candidate_work_events w ON w.sequence=i.sequence
LEFT JOIN candidate_request_events q ON q.sequence=i.sequence
WHERE (n.sequence IS NULL AND w.sequence IS NULL AND q.sequence IS NULL AND i.sender<>m.member_id)
   OR (n.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_notice_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
   ))
   OR (w.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_work_event_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
   ))
   OR (q.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_request_event_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
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
