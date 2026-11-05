-- Counts are participant sends, never deterministic introductions, closure or server notices.
ALTER TABLE candidate_rooms ADD COLUMN participant_message_count INTEGER NOT NULL DEFAULT 0
  CHECK(typeof(participant_message_count)='integer' AND participant_message_count>=0);
UPDATE candidate_rooms SET participant_message_count=(
  SELECT COUNT(*) FROM candidate_items i
  WHERE i.context_id=candidate_rooms.context_id AND i.room_id=candidate_rooms.room_id
    AND i.kind='message' AND i.request_key IS NOT NULL
);

-- Tag an existing item rather than replacing the item table referenced by immutable batches.
-- The stored sender retains triggering provenance; adapters render these as server information.
CREATE TABLE candidate_room_notices (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    message_count INTEGER NOT NULL CHECK(typeof(message_count)='integer' AND message_count>0),
    UNIQUE(context_id,room_id,message_count),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);
CREATE TABLE candidate_notice_recipients (
    sequence INTEGER NOT NULL REFERENCES candidate_room_notices(sequence),
    member_id TEXT NOT NULL,
    PRIMARY KEY(sequence,member_id)
);

-- One authorization rule for checks, batches, activity and feeds. Raw room membership does
-- not authorize a child to read a parent-only milestone, including a milestone it triggered.
CREATE VIEW candidate_delivery_items AS
SELECT i.sequence,i.context_id,i.room_id,m.member_id AS recipient,
       CASE WHEN n.sequence IS NULL THEN i.sender ELSE 'cbc' END AS sender,
       CASE WHEN n.sequence IS NULL THEN i.kind ELSE 'room_notice' END AS kind,
       i.body
FROM candidate_items i
JOIN candidate_room_members m ON m.context_id=i.context_id AND m.room_id=i.room_id AND m.active=1
LEFT JOIN candidate_room_notices n ON n.sequence=i.sequence
WHERE (n.sequence IS NULL AND i.sender<>m.member_id)
   OR (n.sequence IS NOT NULL AND EXISTS (
       SELECT 1 FROM candidate_notice_recipients r WHERE r.sequence=i.sequence AND r.member_id=m.member_id
   ));

-- Coalesce old unfrozen informational notices. An already-offered immutable batch keeps
-- its exact notice until receipt/rebind; newer messages never rewrite that transport batch.
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
