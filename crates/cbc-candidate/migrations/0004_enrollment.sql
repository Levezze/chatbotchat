CREATE TABLE candidate_directory (
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    repository TEXT NOT NULL,
    label TEXT NOT NULL,
    summary TEXT NOT NULL,
    PRIMARY KEY(context_id,member_id),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);

CREATE TABLE candidate_enrollment_invites (
    token_digest BLOB PRIMARY KEY NOT NULL CHECK(length(token_digest)=32),
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    expected_epoch INTEGER NOT NULL,
    host TEXT NOT NULL CHECK(host IN ('codex','claude')),
    device_id TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    evidence_ref TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    consumed_epoch INTEGER,
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id)
);

CREATE TABLE candidate_route_grants (
    context_id TEXT NOT NULL,
    first_member TEXT NOT NULL,
    second_member TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('report','peer')),
    scope_ref TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
    PRIMARY KEY(context_id,first_member,second_member,kind,scope_ref),
    FOREIGN KEY(context_id,first_member) REFERENCES candidate_members(context_id,member_id),
    FOREIGN KEY(context_id,second_member) REFERENCES candidate_members(context_id,member_id),
    CHECK(first_member<>second_member)
);

CREATE TABLE candidate_connections (
    context_id TEXT NOT NULL,
    first_member TEXT NOT NULL,
    second_member TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    room_id TEXT NOT NULL,
    introduction_sequence INTEGER NOT NULL REFERENCES candidate_items(sequence),
    PRIMARY KEY(context_id,first_member,second_member,kind,scope_ref),
    UNIQUE(context_id,room_id),
    FOREIGN KEY(context_id,first_member,second_member,kind,scope_ref)
      REFERENCES candidate_route_grants(context_id,first_member,second_member,kind,scope_ref),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);

CREATE TABLE candidate_connection_requests (
    context_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
    room_id TEXT NOT NULL,
    PRIMARY KEY(context_id,member_id,request_key),
    FOREIGN KEY(context_id,member_id) REFERENCES candidate_members(context_id,member_id),
    FOREIGN KEY(context_id,room_id) REFERENCES candidate_rooms(context_id,room_id)
);
