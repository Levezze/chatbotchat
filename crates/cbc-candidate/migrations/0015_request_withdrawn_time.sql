-- C2 review: when a request was withdrawn. A withdrawal emits an inbox event only when the
-- owner is a member with an open room, so the event cannot date the user's requests or the
-- ones retirement withdraws. Every withdrawal now records its own wall time.
ALTER TABLE candidate_requests ADD COLUMN withdrawn_wall_ms INTEGER
    CHECK(withdrawn_wall_ms IS NULL
          OR (typeof(withdrawn_wall_ms) = 'integer' AND state = 'withdrawn'));

-- Earlier withdrawals are dated by their event where there is one. The rest stay NULL: the
-- report leaves them out and counts them rather than guess.
UPDATE candidate_requests AS q
SET withdrawn_wall_ms = (
    SELECT min(f.committed_wall_ms)
    FROM candidate_request_events e
    JOIN candidate_item_facts f ON f.sequence = e.sequence
    WHERE e.context_id = q.context_id AND e.request_id = q.request_id AND e.event = 'withdrawn'
)
WHERE q.state = 'withdrawn';
