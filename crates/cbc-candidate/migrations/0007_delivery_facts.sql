-- Durable first-occurrence delivery facts (#113 PR-A). Only stages the core observes itself.
-- No message body and no batch reference (a receipt capability) is ever stored here.

-- One row per Store::open. Monotonic offsets are comparable only within one instance;
-- intervals that cross instances are estimates from wall time.
CREATE TABLE candidate_core_instances (
    instance_id TEXT PRIMARY KEY NOT NULL CHECK(length(instance_id) = 32),
    started_wall_ms INTEGER NOT NULL CHECK(typeof(started_wall_ms) = 'integer')
);

CREATE TABLE candidate_item_facts (
    sequence INTEGER PRIMARY KEY REFERENCES candidate_items(sequence),
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    item_class TEXT NOT NULL CHECK(item_class IN ('participant','introduction','notice','closure')),
    committed_instance TEXT NOT NULL REFERENCES candidate_core_instances(instance_id),
    committed_mono_us INTEGER NOT NULL CHECK(typeof(committed_mono_us) = 'integer' AND committed_mono_us >= 0),
    committed_wall_ms INTEGER NOT NULL CHECK(typeof(committed_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);

-- expected=1: the recipient was in the commit-time delivery snapshot, so later revocation never
-- shrinks the denominator. expected=0: first seen at offer (an item older than this migration).
CREATE TABLE candidate_recipient_facts (
    sequence INTEGER NOT NULL REFERENCES candidate_items(sequence),
    member_id TEXT NOT NULL,
    expected INTEGER NOT NULL CHECK(expected IN (0,1)),
    offer_count INTEGER NOT NULL DEFAULT 0 CHECK(typeof(offer_count) = 'integer' AND offer_count >= 0),
    first_batch_tag TEXT CHECK(first_batch_tag IS NULL OR length(first_batch_tag) = 16),
    offered_instance TEXT REFERENCES candidate_core_instances(instance_id),
    offered_mono_us INTEGER,
    offered_wall_ms INTEGER,
    receipted_instance TEXT REFERENCES candidate_core_instances(instance_id),
    receipted_mono_us INTEGER,
    receipted_wall_ms INTEGER,
    PRIMARY KEY(sequence, member_id),
    CHECK((offered_instance IS NULL) = (offered_mono_us IS NULL) AND (offered_instance IS NULL) = (offered_wall_ms IS NULL)),
    CHECK((receipted_instance IS NULL) = (receipted_mono_us IS NULL) AND (receipted_instance IS NULL) = (receipted_wall_ms IS NULL))
);
CREATE INDEX candidate_recipient_facts_member ON candidate_recipient_facts(member_id, sequence);
