-- Work-item follow-ups, R2c: an operation ledger so a no-op binds its key, and catch-up
-- snapshots on report connect.

-- Every keyed request and its first outcome, applied or no-op. A key answers with that outcome
-- for ever, so a delayed retry can't turn into a new decision. Revisions stay the record of
-- change; this table is the record of requests.
CREATE TABLE candidate_work_operations (
    context_id TEXT NOT NULL,
    actor_kind TEXT NOT NULL CHECK(actor_kind IN ('member','owner')),
    actor_member TEXT,
    request_key TEXT NOT NULL CHECK(length(request_key) BETWEEN 1 AND 128),
    fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
    item_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('applied','noop')),
    revision INTEGER NOT NULL CHECK(typeof(revision) = 'integer' AND revision >= 1),
    at_wall_ms INTEGER NOT NULL CHECK(typeof(at_wall_ms) = 'integer'),
    FOREIGN KEY(context_id,item_id) REFERENCES candidate_work_items(context_id,item_id),
    FOREIGN KEY(context_id,actor_member) REFERENCES candidate_members(context_id,member_id),
    CHECK((actor_kind = 'member') = (actor_member IS NOT NULL))
);
CREATE UNIQUE INDEX candidate_work_operation_keys
    ON candidate_work_operations(context_id, actor_kind, coalesce(actor_member, ''), request_key);

-- Every key a revision used was an applied request.
INSERT INTO candidate_work_operations(context_id,actor_kind,actor_member,request_key,fingerprint,item_id,outcome,revision,at_wall_ms)
SELECT context_id,actor_kind,actor_member,request_key,fingerprint,item_id,'applied',revision,at_wall_ms
FROM candidate_work_revisions;

-- A snapshot event carries an item's current state to a report room that connected after the
-- item changed. It is not part of any revision's delivery, so history leaves it out.
ALTER TABLE candidate_work_events ADD COLUMN snapshot INTEGER NOT NULL DEFAULT 0 CHECK(snapshot IN (0,1));
