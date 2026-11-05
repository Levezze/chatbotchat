PRAGMA application_id = 1128416050;
CREATE TABLE candidate_contexts (
    context_id TEXT PRIMARY KEY NOT NULL CHECK(length(context_id) BETWEEN 1 AND 128)
);
