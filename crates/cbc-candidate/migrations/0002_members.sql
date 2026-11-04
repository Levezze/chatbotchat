CREATE TABLE candidate_members (
    context_id TEXT NOT NULL REFERENCES candidate_contexts(context_id) ON DELETE RESTRICT,
    member_id TEXT NOT NULL CHECK(length(member_id) BETWEEN 1 AND 128),
    role TEXT NOT NULL CHECK(role IN ('project_lead', 'repo_lead', 'feature_lead', 'task_agent', 'participant')),
    active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1)),
    epoch INTEGER NOT NULL DEFAULT 0 CHECK(typeof(epoch) = 'integer' AND epoch >= 0),
    device_id TEXT,
    native_session_id TEXT,
    credential_digest BLOB CHECK(credential_digest IS NULL OR length(credential_digest) = 32),
    PRIMARY KEY(context_id, member_id)
);
