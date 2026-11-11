-- #113 B1: wake attempts reported by the device driver, and the first CBC activity of a
-- recipient after its receipt. No body, prompt or capability is stored.

-- One row per wake attempt the driver reports with the member's own binding. A hint covers a
-- member's unread items up to through_sequence, so one attempt can stand for many items.
CREATE TABLE candidate_wake_attempts (
    attempt_id INTEGER PRIMARY KEY,
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    through_sequence INTEGER NOT NULL CHECK(typeof(through_sequence) = 'integer' AND through_sequence > 0),
    kind TEXT NOT NULL CHECK(kind IN ('claude_channel','codex_queue','claude_mod')),
    outcome TEXT NOT NULL CHECK(outcome IN ('accepted','failed')),
    failure TEXT CHECK((outcome = 'failed') = (failure IS NOT NULL) AND (failure IS NULL OR length(failure) BETWEEN 1 AND 64)),
    recovery INTEGER NOT NULL CHECK(recovery IN (0,1)),
    reported_instance TEXT NOT NULL REFERENCES candidate_core_instances(instance_id),
    reported_mono_us INTEGER NOT NULL CHECK(typeof(reported_mono_us) = 'integer' AND reported_mono_us >= 0),
    reported_wall_ms INTEGER NOT NULL CHECK(typeof(reported_wall_ms) = 'integer'),
    attempt_wall_us INTEGER NOT NULL CHECK(typeof(attempt_wall_us) = 'integer'),
    FOREIGN KEY(context_id, member_id) REFERENCES candidate_members(context_id, member_id)
);
CREATE INDEX candidate_wake_attempts_member ON candidate_wake_attempts(context_id, member_id, attempt_id);

-- First dispatch, first accepted wake and first activity per recipient fact. Each group is set
-- once (first occurrence wins), from the core clock at the moment the core learns of it.
ALTER TABLE candidate_recipient_facts ADD COLUMN dispatched_instance TEXT REFERENCES candidate_core_instances(instance_id);
ALTER TABLE candidate_recipient_facts ADD COLUMN dispatched_mono_us INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN dispatched_wall_ms INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN woken_instance TEXT REFERENCES candidate_core_instances(instance_id);
ALTER TABLE candidate_recipient_facts ADD COLUMN woken_mono_us INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN woken_wall_ms INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN active_instance TEXT REFERENCES candidate_core_instances(instance_id);
ALTER TABLE candidate_recipient_facts ADD COLUMN active_mono_us INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN active_wall_ms INTEGER;
ALTER TABLE candidate_recipient_facts ADD COLUMN active_operation TEXT;

-- The receipt's wall clock in microseconds, so a wake attempt in the same millisecond can be
-- ordered against it. NULL for receipts stamped before this migration.
ALTER TABLE candidate_recipient_facts ADD COLUMN receipted_wall_us INTEGER;

-- Only rows still waiting for activity: receipted, no activity yet. Stays small.
CREATE INDEX candidate_recipient_facts_awaiting_activity ON candidate_recipient_facts(member_id)
    WHERE receipted_instance IS NOT NULL AND active_instance IS NULL;
