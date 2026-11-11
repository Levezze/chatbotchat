-- A context's outbox page (work and request events, newest first) reads through this index rather
-- than walking the whole facts history backwards. The WHERE must match the query's class filter
-- exactly, or SQLite will not use a partial index.
CREATE INDEX candidate_outbox_facts
    ON candidate_item_facts(context_id, sequence)
    WHERE item_class IN ('work_event', 'request_event');
