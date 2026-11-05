-- Nullable invitation targets preserve all pre-target invitations as legacy evidence.
ALTER TABLE candidate_enrollment_invites ADD COLUMN target_json TEXT;
ALTER TABLE candidate_enrollment_invites ADD COLUMN target_digest BLOB
    CHECK(target_digest IS NULL OR length(target_digest)=32);

-- Immutable per-epoch history, never reinterpreted as a target for a later binding.
CREATE TABLE candidate_native_targets (
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch > 0),
    host TEXT NOT NULL CHECK(host IN ('codex','claude')),
    device_id TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    target_json TEXT NOT NULL,
    target_digest BLOB NOT NULL CHECK(length(target_digest)=32),
    PRIMARY KEY(context_id,member_id,epoch),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);
