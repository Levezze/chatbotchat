-- Where an owner answer came from. The channel stays the trust class ('owner' is the user,
-- authenticated); the surface records which owner door carried it: NULL for the owner route,
-- 'dashboard' for the page. Only an owner answer can have one. A column rather than a new
-- channel value: widening the channel CHECK means rebuilding this table, which
-- candidate_request_events references, and with foreign keys on that rebuild cannot commit
-- inside a migration's transaction.
ALTER TABLE candidate_request_answers ADD COLUMN surface TEXT
    CHECK(surface IS NULL OR (surface = 'dashboard' AND channel = 'owner'));
