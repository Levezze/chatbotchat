-- Preserve v2 epochs and credentials while adding the optional domain role.
ALTER TABLE candidate_members RENAME TO candidate_members_v2;
CREATE TABLE candidate_members (
    context_id TEXT NOT NULL REFERENCES candidate_contexts(context_id) ON DELETE RESTRICT,
    member_id TEXT NOT NULL CHECK(length(member_id) BETWEEN 1 AND 128),
    role TEXT NOT NULL CHECK(role IN ('project_lead','repo_lead','domain_lead','feature_lead','task_agent','participant')),
    active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
    epoch INTEGER NOT NULL DEFAULT 0 CHECK(typeof(epoch) = 'integer' AND epoch >= 0),
    device_id TEXT,
    native_session_id TEXT,
    credential_digest BLOB CHECK(credential_digest IS NULL OR length(credential_digest) = 32),
    PRIMARY KEY(context_id, member_id)
);
INSERT INTO candidate_members(context_id,member_id,role,active,epoch,device_id,native_session_id,credential_digest)
SELECT context_id,member_id,role,active,epoch,device_id,native_session_id,credential_digest FROM candidate_members_v2;
DROP TABLE candidate_members_v2;

CREATE TABLE candidate_rooms (
    context_id TEXT NOT NULL REFERENCES candidate_contexts(context_id) ON DELETE RESTRICT,
    room_id TEXT NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0 CHECK(closed IN (0,1)),
    PRIMARY KEY(context_id, room_id)
);
CREATE TABLE candidate_room_members (
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
    PRIMARY KEY(context_id, room_id, member_id),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);
CREATE INDEX candidate_members_rooms ON candidate_room_members(context_id,member_id,active,room_id);
CREATE TABLE candidate_items (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    context_id TEXT NOT NULL,
    room_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('message','closed')),
    body TEXT NOT NULL,
    request_key TEXT,
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id),
    FOREIGN KEY(context_id,sender) REFERENCES candidate_members(context_id,member_id),
    UNIQUE(context_id,sender,request_key)
);
CREATE INDEX candidate_room_sequence ON candidate_items(context_id,room_id,sequence);
CREATE UNIQUE INDEX candidate_one_close ON candidate_items(context_id,room_id) WHERE kind = 'closed';

CREATE TABLE candidate_batches (
    reference TEXT PRIMARY KEY NOT NULL,
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','receipted','cancelled','superseded')),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);
CREATE UNIQUE INDEX candidate_one_pending ON candidate_batches(context_id,member_id,epoch) WHERE state='pending';
CREATE TABLE candidate_batch_items (
    reference TEXT NOT NULL REFERENCES candidate_batches(reference),
    ordinal INTEGER NOT NULL,
    sequence INTEGER NOT NULL REFERENCES candidate_items(sequence),
    PRIMARY KEY(reference,ordinal),
    UNIQUE(reference,sequence)
);
CREATE TABLE candidate_receipts (
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    sequence INTEGER NOT NULL REFERENCES candidate_items(sequence),
    PRIMARY KEY(context_id,member_id,sequence),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);
CREATE TABLE candidate_inbox_state (
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    last_room TEXT NOT NULL,
    PRIMARY KEY(context_id,member_id),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);
