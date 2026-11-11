# Work records and external observations

Framework requirements clarified 2026-11-09 for PRD #112 and stages #115–#118.
These capabilities follow the observable working-hierarchy gate in #113.
Updated 2026-11-10: a thin slice (registration, atomic work-item revisions,
requests with callbacks, outbox) and a dashboard skeleton move into #113 ahead
of the hierarchy trial. See [decisions T-07 to T-12](working-trial-protocol.md#2-decision-record-2026-11-10).
Stage #115 owns work records, PR observations and attention. It retains typed
deployment references and their separate ownership contract; actual provider
and environment observation integrations belong to #116 alongside their views.

## Shared coordination records

CBC is an opinionated agent-coordination framework. Project/repository/feature
views aggregate this CBC workspace's assignments, questions and evidence, not
every contributor's work or the product's authoritative specification. Link to
GitHub, repository documentation, Jira or Notion rather than replacing them.

A shared feature reference can group contributions from several repositories.
Each repository contribution retains its own finite Chapter Master and Primarch.
The repo view lists active contributions and concise current summaries; the
feature view groups owners, relevant rooms, PR links, decisions and milestones.
Agents read the same canonical service records through authorized role views as
the human GUI. Higher tiers do not gain access to private internal maps.

Chapter Masters submit small meaningful semantic changes: coarse stage, next
action, a specifically blocked operation, findings and evidence. Primarch notes,
priorities, deadlines and sequencing constraints have separate ownership. CBC
maintains timestamps, room/read state, routing and projections deterministically.
Transport activity does not reset the age of meaningful progress. Routine CI or
receipt bookkeeping does not require another model-authored summary.

A work-item update is one validated multi-field patch carrying an expected
revision and idempotency key. The service commits it as one work revision and one
outbox event; an equal or no-op patch produces no wake. Relevance and batching
apply on top. Status hints may coalesce; action and control records never do.

A coordinator-owned action survives a read receipt, an unrelated conversation
or completion of a status report. Preserve its owner, ready/request time, next
action, disposition and outcome evidence; a last-poll timestamp cannot reset its
age or resolve it. Classify actual native pending/refusal separately from agent
interpretation, product decisions, resource conflict and routine coordination.
An unattempted native action has an unknown verdict. The
[release follow-through case](../case-studies/2026-11-09-release-follow-through.md)
supplies concrete qualification scenarios without adding another approval tier.

The optional scope-supervisor [draft](../proposals/optional-scope-supervisor.md)
remains for discussion outside the execution/approval chain. It is not a
prerequisite for this store, the trial or ordinary work.

## Record layer plan for the trial (#113, 2026-11-10)

This is the thin slice of #115 that decision T-09 moves ahead of the trial. Much
of the identity and room machinery already exists in the candidate core, so the
plan below lists, for each decision, what exists, what is missing, and which PR
builds it. Only the missing part is built.

| Decision | Already in the candidate core | Missing | PR |
|---|---|---|---|
| T-25 identity | The owner assigns a member ID. Enrollment binds it to a device and native session after adapter evidence, and the adapter injects the binding token, so the model never types an identity. A new enrollment against the expected epoch rebinds the same identity to a new session, which is the handoff operation. `unbind_member`, `revoke_binding` and `remove_member` end a binding. | A retired state, distinct from unbound, that a later enrollment can't revive. Handoff and retirement provenance: actor, reason, time. Tests that a new native session can't take over an identity except through the rebind path. | R1 |
| T-26 peer rooms | Opening is idempotent per pair, kind and `scope_ref`, with a deterministic room ID. Any member can close a room. A closed room refuses to reopen. | Uniqueness per unordered pair, whatever the scope. Close records actor and reason. A replacement room after a close, with a new ID and a link to the room it supersedes. A member's peer rooms go inactive when its identity retires, but not on a handoff. | R1 |
| T-07, T-09 work items | Nothing. | Work items owned by one member. Revisions as one validated multi-field patch with an expected revision and an idempotency key, committed with one outbox event in the same transaction. A no-op patch produces no event. The HITL flag can be turned on by the user or a repo lead and turned off only on the owner-authenticated route. A feature lead can't change it. | R2 |
| Outbox | Nothing beyond the inbox. | Outbox events are delivered through the existing inbox as a new item class, so the #113 PR-A delivery facts and waits cover them. No second delivery path. | R2 |
| Work-item routes | Nothing. | Connector member routes for register, patch, HITL on and read; the owner route for HITL on and off and read; waking watchers when a work event commits. HITL scoped to the owner's own repo lead. | R2b |
| Work-item follow-ups | R2 and R2b. | A no-op binds its idempotency key; a replayed registration is labelled; an item registered before its report room exists is delivered as a snapshot when the room connects. | R2c |
| Inactive rooms | R1. | A room set aside by the 0008 upgrade could still be written to, so a pair could keep several writable rooms. It is now drain-only: read and receipt, no send. | R1b |
| T-10, T-12 requests | Nothing. | One canonical request with an owner (who answers) and a callback (who is woken). The answer records its channel. An answer an agent relays from chat is stored as relayed, with the relaying member, and never counts as owner-authenticated (proposed default, for the user's review). The answer wakes the asker through the outbox. | R3 |
| T-27 waits | First-occurrence commit, offer and receipt facts (#113 PR-A). | A mapping from each wait cause in the trial protocol §6 to the facts it is derived from. CI waits (CM-A's observer), holds (#114) and acted-on (#113 PR-B) can't be observed yet and report `unknown`. The interval calculator itself belongs to the report in #113 C. | R3 |
| Request routes | R3. | Connector member routes to open, answer, withdraw, mark applied and read; the owner route to answer and read; waking watchers when a request event commits. | R3b |

New routes for work items, requests and answers go in new modules, so the
connector stays a thin wiring layer.

### R1 as built: identity and peer rooms

Migration 0008 (schema 8). Tests are in `tests/peer_rooms.rs`.

- **One active room per pair.** An open returns the pair's active room whatever
  the kind or scope, with `reused` set. The open still needs a grant for the
  scope it names. A pair is only ever eligible for one kind, so report rooms
  follow the same rule as peer rooms.
- **Close.** A close needs a reason: one line, at most 200 characters. The
  reason is the body of the closure item the other member receives. The room's
  connection record keeps the actor, reason and time. A repeated close returns
  the first closure unchanged.
- **Replacement.** After a close, the next open creates one replacement room
  with a new ID. It records the generation and the room it supersedes.
  Concurrent opens get the same replacement. Replaying an old request key still
  returns the old, closed room. A closed room is never reopened.
- **Revocation and removal.** Revoking a grant, removing a member or revoking a
  room membership takes the room out of the pair's active slot. A later
  authorized open gets a replacement, never the revoked room or its history.
- **Retirement.** `retire_member` is an owner operation, also available on the
  owner connector route and as the owner CLI command `cbc-candidate retire`,
  which takes `--context`, `--member` and `--reason` beside the usual `--root`
  and `--protected-db`. It closes the member's active rooms in its name with
  the note `retired: <reason>`, which also fits the 200-character limit. It
  ends the binding and marks the identity retired. Registration, binding and
  enrollment all refuse a retired identity, and retiring twice fails.
  Detecting that a session has ended is not built; that stays with #117.
- **Identity events.** Enrollment through an invitation records `enrolled` the
  first time and `rebound` on every later handoff; retirement records
  `retired` with its reason. `bind_member` is fixture administration and
  records nothing. A member enrolled before migration 0008 has no earlier
  event, so its first handoff after the upgrade is recorded as `enrolled`.
- **Upgrade.** Before 0008 a pair could hold one open room per scope. The
  newest usable one becomes the active room: open, under an active grant, with
  both members still active in it. A revoked room never takes the slot. The
  others are drain-only (R1b): their history and unread items are still read
  and receipted, and a member can close them, but a send is refused, so the
  pair has exactly one writable room. An open no longer returns them. Each row
  records whether it ever held the pair's active slot (`held_slot`), and a
  replacement supersedes the last room that held it, never an older room the
  upgrade set aside.

Two existing tests changed contract under T-26, and the PR names both. A
reopen after a close used to fail; it now creates a replacement. An open after
a revoke and a regrant used to fail; it now creates a replacement that
supersedes the revoked room. Neither change restores a closed or revoked room.

### R2 as built: work items and the outbox

Migration 0009 (schema 9). The store operations are in `src/work.rs` and the
tests in `tests/work_items.rs`. The connector routes are R2b, below.

- **Ownership.** Only a feature lead registers a work item, and it becomes the
  owner. Only the owner patches it. An item ID is unique in its context, so a
  second registration under a new key fails with `already registered`.
- **Revisions.** Registration is revision 1, recorded as `["registered"]`. A
  patch names any of `stage`, `summary`, `next_action`, `blocked_operation` and
  `branch`, carries the revision it was read at, and commits as one revision
  that lists the fields that actually changed, sorted. A stale revision fails
  with `revision conflict` and changes nothing. A patch that changes nothing
  returns `NoOp` and records nothing, and an empty patch is refused.
- **Stages.** `planning`, `implementing`, `in_review`, `ready_to_merge`,
  `merged`, `abandoned`. Any stage can follow any other: the record keeps what
  the owner reports, and checking it against the PR is #115's observation
  work.
- **Idempotency.** A key belongs to its actor. Retrying the same request with
  the same key returns the first result: the revision for a patch, the item as
  it is now for a registration. The same key with a different request fails
  with `idempotency key conflicts`. A `NoOp` records no revision. Since R2c it
  still binds its key (see below).
- **HITL (T-07).** A repo lead can turn it on. Only the user turns it off,
  through the owner route. The owner can't change it either way, and a content
  patch never touches it. Any active repo lead in the context qualifies; tying
  that to the item's own report line is left for the hierarchy work.
- **Text bounds.** The summary is trimmed and must be 1 to 500 characters.
  `next_action` is at most 500 characters and may be empty; both may hold line
  breaks but no other control characters. `branch` and `blocked_operation` are
  one line of 1 to 200 characters, or cleared. Every error names its field.
- **Outbox.** Each revision writes one inbox item to each open report room of
  the owner, in the same transaction. Readers see sender `cbc` and kind
  `work_event`, and the body is JSON: `item`, `revision`, `actor` (the member,
  or `user`), `changed`, `stage` and `hitl`. The acting member is not woken by
  its own change; a change by the user reaches every member of the room. A
  work event is a new delivery-fact class, `work_event`, so the PR-A commit,
  offer and receipt facts cover it with no second delivery path.
- **No room, no wait.** With no open report room the revision still commits,
  and `work_revisions` shows an empty `delivered_to`. The record never waits on
  transport.
- **Upgrade.** 0009 rebuilds the delivery-facts table to admit the new class
  and copies every existing fact. A test builds a schema-8 database with a
  recorded fact and checks it survives unchanged.

### R2b as built: work-item routes

The routes are in `src/connector/work.rs`; the connector only adds one `work`
operation to the member and owner protocols. Tests are in
`tests/work_routes.rs`, over a real socket.

- **Member route.** A bound member registers, patches and turns HITL on through
  its binding; the model never supplies a member ID. The store's rules apply
  unchanged, and errors reach the client in the connector's usual opaque form.
- **Reading.** A member reads an item it owns, or any item if it is a repo
  lead. Anyone else gets the same answer as for an item that does not exist.
- **Owner route.** The user turns HITL on or off and reads an item on the
  owner-authenticated route. History is a separate read in pages of at most
  100 revisions, so a long-lived item never outgrows the connector's reply
  limit. Revisions are append-only, so pages taken at different times never
  contradict each other or the item. (Codex review of #135: a single reply
  could exceed the limit, and reading the item and history on separate
  connections could disagree under a concurrent update.)
- **HITL scope.** A repo lead turns HITL on only for an item whose owner holds
  an active owner-issued report grant to that lead. The grant decides, not an
  open room, a poll or a connected socket: replacing a room leaves the
  authority in place, and revoking the grant removes it. Another repo lead in
  the same context can read the item but can't put a hold on it. (Codex
  supervision: T-07's Primarch is the owner's own repo lead.)
- **Wire patch.** An absent field is left alone, `null` clears it, and a value
  sets it. Unknown fields are refused, so HITL can't ride in a patch.
- **Waking.** A registration or an applied revision wakes parked watchers, the
  same signal a sent message gives. A no-op or a read wakes nobody. A test
  parks a lead's watch, registers an item, and fails if the watch isn't woken.

Mutation checks:
- Removing the wake fails the parked-watch test.
- Removing the page cap fails the paging test.
- Removing the grant check fails the two-lead test.
- Making the hidden-item error differ fails the store-level read test. The
  connector flattens member errors, so that test runs against the store.

Three findings from the same review are left for R2c, which needs migration
0010:
- A no-op must still bind its idempotency key. Otherwise a delayed retry of an
  old "HITL off" can clear a newer hold.
- A replayed registration should say it is a replay.
- An item registered before its owner's report room exists should reach the
  lead as a current snapshot once the room is connected.

### R2c as built: work-item follow-ups

Migration 0010 (schema 10). The changes are in `src/work.rs` and one call in
`src/enrollment.rs`; tests are in `tests/work_followups.rs`.

- **Operation ledger.** Each keyed request is recorded with its first outcome,
  `applied` or `noop`, and the revision it saw. A retry with the same key
  returns that outcome, even if the item has changed since. Revisions remain
  the record of change; the ledger is the record of requests. Without it, a
  delayed retry of an old "HITL off", sent while HITL was already off, would
  clear a hold the lead set in between. A no-op patch binds its key the same
  way, and the key can't be reused for a different change.
- **Replayed registration.** A registration returns the item and a `replayed`
  flag. A replay returns the item as it is now. It records nothing, and over
  the connector it wakes nobody.
- **Catch-up on connect.** When a report room is created, in the same
  transaction, the parent receives one work event per open item of the child
  (any stage but `merged` and `abandoned`). The event carries the item's
  current state: `item`, `revision`, `snapshot: true`, `stage` and `hitl`, with
  no intermediate history. The child receives nothing. A reused room gets no
  catch-up, since its parent received each change as it happened. A
  replacement room after a close gets one. Catch-up events are tagged
  `snapshot` and are not counted as a delivery of the revision they carry, so
  an item's history still shows that a change made with no room reached nobody
  at the time.
- **Upgrade.** 0010 copies every key a revision used into the ledger as
  `applied`. A test builds a schema-9 database with two revisions and checks
  that a pre-upgrade key is still refused for a different request.

Mutation checks:
- Not binding a no-op HITL key fails the stale-retry test.
- Not binding a no-op patch key fails the patch-key test.
- Skipping the backfill fails the upgrade test.
- Labelling a replay as new fails the replay test.
- Skipping the catch-up, including finished items in it, or counting it as a
  delivery each fail the catch-up test.

### R3 as built: requests and the waits mapping

Migration 0011 (schema 11). The code is in `src/requests.rs` and
`src/waits.rs`, plus one call in `retire_member`; tests are in
`tests/requests.rs` and `tests/waits.rs`. Routes come in R3b, so this layer is
reached only through the store.

- **One row per question.** A request has an ID, a callback (the member that
  asked and is woken by the answer), an owner (the user or one member), an
  optional work item, and the question. The work item must be one the asker
  can read under the R2b rule (its owner, or a repo lead); any other gets the
  same refusal as a missing item, so a request can't be used to probe for
  another tree's items. Its state is `open`,
  `relayed`, `answered` or `withdrawn`. Opening is idempotent per asker and
  key; the same key for a different question is refused.
- **The server picks the ID.** An ID is `req-` and 128 random bits. With
  caller-chosen IDs, two members picking the same one would collide, and the
  refusal would tell one about the other's request. For the same reason a
  member who is neither the asker nor the owner gets "unknown request" for a
  member's request, exactly as for one that doesn't exist, and so does anyone
  but the asker who tries to withdraw a request or mark an answer applied.
- **The route decides the channel.** The owner route records `owner`. The
  owner member's own binding records `member`. Any other member records
  `relayed`, with the relaying member. Nothing the caller sends can choose the
  channel.
- **Relays (proposed default, for the user's review).** The policy is one
  function, `relay_allowed`. Only a question for the user can be relayed, and
  only until the user has answered it on the owner route. Any active member
  may relay, the asker included. A relay moves the request to `relayed`, never
  to `answered`, and the user's own answer is still accepted afterwards. A
  question for a member is answered by that member only; nobody relays for a
  member.
- **Who hears what.** A question for a member needs the pair's active room,
  where the owner receives an `opened` event; with no such room the request is
  refused and nothing is stored. A question for the user needs no room and
  sends no event. An answer wakes the asker, unless the asker is the one
  relaying: in the room shared with the owner member if it is still active,
  otherwise in the asker's first active room, report rooms first. With no room
  the answer is recorded and undelivered. Each event goes to one named
  recipient and has its own delivery class, `request_event`, so the #113 PR-A
  facts cover it.
- **Answers are append-only.** Once a request is `answered`, every further
  answer is refused, whatever its channel, and so is any answer to a withdrawn
  request, with the withdrawal reason in the error. Answers already recorded
  are never changed. A retried key returns the first answer
  before any state check, so a retry still succeeds after the request has
  moved on.
- **Applied is separate.** Answered, delivered and applied are three facts.
  Only the asker marks an answer applied, and the first time is kept.
- **Withdrawal and retirement.** Only the asker withdraws, with a reason, and
  only while the request is open or relayed; the owner of a member's request
  is told if their room is still active.
  Retirement withdraws every open request the member asked or owns, with
  `retired: <reason>`, in the retirement's transaction. It sends no request
  events, because the member's rooms close in the same transaction and say
  why.
- **HITL is untouched.** A request never sets or clears the HITL flag.
- **Waits mapping (T-27).** `WaitCause` lists the eight §6 causes in the
  protocol's own wording, and a test checks them against the §6 table, so the
  two can't drift apart. Each cause states how much the records carry:
  - known: waiting on the user, on another Chapter Master, on delivery;
  - partial: human review (the HITL flag exists, the PR observation doesn't);
    the Primarch (open requests are derived, holds wait for #114);
  - unknown: a resource (#114), CI (PR observations), ready but idle (acted-on
    facts, #113 PR-B).

  `waits_now` lists a member's current waits: each open request it asked,
  classified by its owner (the user; a repo lead is the Primarch; a feature
  lead is another Chapter Master; any other role has no cause and reports as
  `unknown`), and each item not yet receipted, as waiting on delivery. A
  relayed request is not a wait, because the asker has an answer to act on.
  `open_requests`, by contrast, lists it, since the user has not answered it.
  The interval calculator stays in #113 C.
- **Upgrade.** 0011 rebuilds the delivery-facts table to admit the new class.
  A test builds a schema-10 database with a work-event fact and checks it
  survives. Another checks that, after every migration, no schema object
  still refers to a renamed-and-dropped table and no foreign key is violated.

Mutation checks:
- A relay that sets `answered`, or a relay allowed after the user's answer,
  fails the relay test.
- Accepting a second owner answer, or a second member answer, fails the
  owner-route or member-owner test.
- A distinguishable refusal for a request the member can't see, on answer or
  withdraw, fails the member-owner or withdrawal test.
- A fixed request ID fails the idempotency test and two others.
- A non-owner member answering a member's request, or the owner route
  answering one, fails the member-owner test.
- Storing a member's request with no room fails the room test.
- Waking the relaying asker fails the relay test.
- Letting anyone withdraw, withdrawing an answered request, or answering a
  withdrawn request fails the withdrawal test.
- Skipping the retirement hook, or withdrawing only the asker's side, fails the
  retirement test.
- Letting anyone mark an answer applied, or overwriting the first applied
  time, fails the applied test.
- Checking state before the key, so a retry fails, fails the idempotency test.
- Skipping the work-item check, or letting any member name any item, fails
  the item test.
- Recording the wrong delivery class fails the delivery-facts test.
- Counting relayed requests, swapping the Primarch and Chapter Master, dropping
  delivery waits, changing a label or claiming CI is known each fail a waits
  test.

### R3b as built: request routes

`connector/requests.rs` wires the R3 requests into the local connector. As with
work items, it holds no rules: every authority check stays in
`crate::requests`, and the connector only adds one variant to each of the
member operation, owner operation and reply enums.

- **Member routes**, through the member's binding:
  - `open`, `answer`, `withdraw` and `applied`: the R3 writes. None of them
    takes a channel or an actor; both come from the binding. Unknown fields
    are refused, so a caller can't pass either one.
  - `get`: one request.
  - `answers`: a page of a request's answers.
  - `pending`: a page of the member's own waiting requests.
- **Owner routes:** `answer`, `get`, `answers`, and `pending` across the
  context. The owner answer is the only route that records the `owner`
  channel.
- **Read scope.** A member reads a request if it asked it, if it owns it, or if
  it is a repo lead, which is the work-item rule. Anyone else gets "unknown
  request", the same refusal as for a missing one.
  - A relayer who is none of these can't read the request, and needs nothing
    more than its unguessable ID to relay. That ID reaches the relayer out of
    band, normally from the user in chat.
  - This is part of the relay policy awaiting the user's review.
  - The `pending` list is the member's own, a repo lead's included. A repo
    lead reads other members' requests one at a time.
- **Bounded pages.** The connector refuses the whole call if a reply exceeds
  its limit (128 KiB plus 8 KiB); it does not truncate.
  - A question or answer is at most 4000 characters, about 16 KB on the wire
    at four bytes a character. `MAX_REQUEST_PAGE` is therefore 7, and both
    lists and answer histories come in pages of at most that.
  - A test sends a full page of the largest questions, and of the largest
    answers, over both routes. A cap of 10 fails it.
  - Requests page by ID and answers by answer ID, so a page never repeats or
    skips a row.
  - The unbounded `open_requests` stays a trusted in-process read and is not
    exposed.
- **Waking.**
  - An `open`, `answer` or `withdraw` reply bumps the watch revision, as a
    committed work event does. A replayed open bumps it too; the watcher finds
    nothing new and parks again.
  - `applied` commits no event and wakes nobody.
  - The owner route already bumps the revision on every success.
  - Each event path has a parked-watcher test: an open wakes the member owner,
    a member answer and a relay wake the asker, the user's answer wakes the
    asker, and a withdrawal wakes the member owner.
- **Wire refusals are uniform.** Every member error reaches the client as
  "candidate operation rejected". Wire tests therefore assert only success or
  failure, and the store tests in `tests/request_reads.rs` assert the reasons.

Mutation checks:
- Dropping the request arm from the wake condition fails three wake tests.
  Dropping `open`, `answer` or `withdraw` from `committed` fails the matching
  one.
- Each part of the read rule is checked. Letting the asker, the owner or a
  repo lead not read fails a read test, and so does letting anyone read.
- Listing others' requests, or listing answered and withdrawn ones, fails the
  pending test.
- Repeating the page boundary, or dropping either page cap, fails a paging
  test.
- Reading answers of a missing request on the owner route fails the answers
  test.
- A page cap of 10 fails the full-page wire test.
- Accepting unknown fields fails the wire-form unit test.

### D1 as built: dashboard sign-in and the pending page

D1 is the first slice of the dashboard skeleton (T-11, T-12). It serves a plain,
read-only page from the core, reached only through a browser session that the
user's own command starts. Answering from the page is D2; work items and the
outbox timeline are D1b.

- **Where it runs.** `cbc-candidate serve --dashboard` also listens on
  `127.0.0.1` at the candidate port. The dashboard is opt-in, so a core started
  without the flag opens no TCP port at all.
  - The listener must be on the IPv4 loopback address; any other address is
    refused before the core starts.
  - The page reads through the same trusted owner reads as the owner route, on
    the core's own `Store`, so no second process opens the database.
- **Sign-in (T-12).**
  - `cbc-candidate dashboard` asks the running core, over the owner route, for
    a single-use 128-bit login code that lasts 30 seconds. It then opens
    `http://127.0.0.1:<port>/login?code=…` in the default browser.
  - If no browser opens, the command fails and does not print the link, since
    anyone who reads the link can sign in as the user. The code then expires
    unused.
  - `/login` spends the code for a 256-bit session and answers with a redirect
    to `/` that sets the cookie `HttpOnly; SameSite=Strict; Path=/`, so the
    code leaves the address bar. A spent, expired or unknown code gets a 403.
  - The cookie is named `cbc_dashboard_<port>`. Browsers scope cookies by
    host, not port, so with one shared name two cores on one machine would
    overwrite each other's session. A core accepts only the cookie for its own
    port.
  - Codes and sessions exist only in the core's memory, and only as SHA-256
    digests. Nothing is written to SQLite or any file, and a restart signs
    every browser out. A test signs in, then scans every file in the state
    directory, database bytes included, for both secrets.
  - Sessions last 12 hours. That is a proposed default awaiting the user's
    review. At most 8 codes and 16 sessions are kept; when full, the one
    nearest expiry makes room.
  - The code does reach the browser through the opener's command line, which
    another process of the same user could see for the moment it runs. That
    user can already read the owner key, so this widens nothing.
  - Only the user runs `dashboard`; an agent running it would be minting the
    user's consent. It goes on the launch sheet, and its success path is never
    run by a test, because it opens the user's real browser.
- **Hostile text.** Every question, member ID and context ID is agent text.
  - The page has no JavaScript, and every value goes through one escape
    function for `& < > " '`.
  - Every response carries `Content-Security-Policy: default-src 'none';
    style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none';
    base-uri 'none'`, plus `Referrer-Policy: no-referrer`,
    `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. That
    includes errors, redirects and the 404.
  - Links carry IDs URL-encoded and then escaped.
- **Host check.** A request whose `Host` is anything but `127.0.0.1:<port>` is
  refused with 421 before any route runs, signed in or not, so a rebound DNS
  name can't read the page. A request with two `Host` headers, or an
  absolute-form target naming another authority, is refused the same way. The
  `Origin` check for posted answers comes with D2.
- **The page.** Each context gets a panel with one page of its waiting
  requests: open or relayed, at most `MAX_REQUEST_PAGE`. A full page links to
  the next one (`/?context=…&after=…`). A page lists at most 20 contexts, in
  ID order; a full one links to the next (`/?contexts_after=…`), so no context
  is out of reach.
- **Connections.** The dashboard runs its own accept loop rather than
  `axum::serve`.
  - Each connection is a task the loop owns. On shutdown the loop stops
    accepting and aborts every connection, so a browser that stopped reading
    can't hold the store open. The core stops the dashboard with its socket
    listener and still allows 5 seconds as a backstop.
  - At most 16 connections are open at once; past that a new one is closed
    unanswered. A connection that doesn't finish its request headers within
    10 seconds is closed.
  - If the dashboard stops early, the core logs it at once and returns the
    error when it exits.

Mutation checks: each of the following fails at least one test.
- Dropping the escape.
- Dropping the CSP header or the `nosniff` header.
- Skipping the Host check, the authority check, or the check for a second
  `Host` header.
- Letting a code be spent twice, or ignoring a code's or a session's expiry.
- Dropping `HttpOnly`.
- Treating every request as signed in, or deriving the session from the code.
  The first run missed the latter, because the test only checked that the
  two differed; it now checks that the session doesn't contain the code.
- Dropping the next-page link or the more-contexts link.
- Accepting the cookie of another port.
- Waiting for connections at shutdown instead of aborting them.
- Skipping the loopback check.
- Ignoring `--dashboard` in `serve`.
- Lifting the code cap.

Not covered by a test: the 16-connection cap and the 10-second header
timeout. Both are plain settings of the accept loop.

## Registration, controls and eligible merge

A Chapter Master registers its work item (assignment summary, branch/worktree,
declared surfaces and source links), receives the current constraint snapshot and
begins ordinary granted work without a startup GO or acknowledgement. A clear
snapshot means no declared hold at that revision, not proof of no conflict.
Advisory overlap metadata combines declared paths and contracts with bounded
changed-path/base/head facts collected on the execution device or by the PR
observer. The core does not clone repositories, read all code or impose blanket
file locks, and disjoint paths do not prove safety.

Primarch controls are structured records: continue, note, hold or stop, each with
scope, actor, revision, reason and release condition. An advisory note does not
become a hold. A hold affects only its operation, not independent work, and a
timeout never becomes approval. Record requested, received and applied
separately; delivery to a busy host is not immediate cancellation. The Chapter
Master checks current controls before affected effects such as a shared push or a
merge. This adds no per-tool parent permission.

A merge is eligible when repository-configured gates pass for the current head
and base, declared dependencies are met, no applicable hold exists and the work
item's HITL flag is off. With the flag on, a ready PR waits for the user's
review. The user or the Primarch may turn the flag on; only the user turns it
off; the Chapter Master cannot change it (T-07). A user instruction to promote
against the project's policy is recorded as a fact and the Primarch is notified,
not asked (T-08). Build,
test and review run outside a short conditional integration step; a moved base
revalidates only affected evidence. There is no global feature queue. Target
branches, environments and review policy are repository configuration. The
Chapter Master executes eligible merges. Semantic, cross-repo, migration and
resource exceptions within the Primarch's repository mandate go to the Primarch as targeted decisions. Questions the user owns go to the user as requests, not through the Primarch (T-10). Merge is not
deployment: an assigned promotion has a named release owner through its observed
outcome. Native permissions and grants stay separate from any role.

Controls belong to #114; work items and PR observations to #115. Until those are
implemented, Chapter Masters perform these eligibility checks themselves through
their existing tools and records; no CBC service enforces them yet.

## PR observations

Register a PR reference once, with optional agent notes. A shared authorized
GitHub integration maintains source-owned observations such as open/closed/merged,
draft state, current head revision and checks associated with that revision.
Retain provider IDs, source/evidence, observation time and unavailable/stale state.
An old successful check must not qualify a changed head. Provider observations
do not overwrite agent findings or Primarch annotations.

Use webhooks where reachable and suitable; otherwise use one bounded shared
conditional refresh for tracked references, with caching and rate-limit handling.
This is deterministic external synchronization, not an AI automation or per-agent
terminal poll. See GitHub's [pull-request API](https://docs.github.com/en/rest/pulls/pulls)
and [integration guidance](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api).

## Optional deployment observations

PR state and deployment state are separate fields/records. A merged PR does not
implicitly mean deployed, production-ready or enabled. Deployment tracking is
optional and configured per repository/environment/service. Environments are
configured names, not a hard-coded dev/staging/production enum.

A deployment observation needs an environment reference, provider/service,
release revision or artifact, observed state, observation time and source/evidence.
Verification is separate from deployment completion. Preserve multiple services
or attempts instead of collapsing them into one boolean. Distinguish absent
integration, unknown/stale data, in-progress, failure and observed success.

Delivery profiles describe the repository's branch/tag/workflow mapping and
configured observation adapter. Some projects deploy main to production; others
promote a production branch or tag. A mapping is an expectation, not runtime
evidence. Match observed releases/artifacts to work through explicit provenance,
including squash/cherry-pick differences, rather than blindly comparing a PR head.

For GCP, a configured observer may use authorized read-only provider APIs or an
existing authenticated gcloud context on the configured device. CBC does not
assume that login exists, copy credentials into chat, grant cloud permissions or
run a deployment to discover its state. Missing access remains explicit and does
not block messaging or projects without deployment integration. Provider-specific
queries and credentials require separate implementation qualification.

The same contract allows other providers and GitHub-backed deployment evidence.
Feature activation and user-visible outcomes may need their own evidence: a
release being deployed does not prove a flag is ON, an email was sent or a user
received it. The [activation case](../case-studies/2026-11-06-notification-activation-coordination.md)
records why these distinctions matter.

## Views and attention

The dashboard reflects canonical changes without asking models to rewrite boards.
Granular PR/provider observations can update the GUI without a Primarch or Emperor
wake. Role-scoped relevance determines meaningful notifications; a single core
scheduler supplies due/unchanged-responsibility attention later in #115. Questions
retain one request and its attributed review notes rather than copied hop-by-hop
versions. Requests are visible and answerable by their authorized human owner
from creation; Primarch review is not a prerequisite. Native callback expiry
is explicit, and a late dashboard answer cannot pretend to release an expired
native invocation. Both apps remain qualification targets; an unsupported lane
requires an explicit product decision, never a silent reduction to one app.
An Emperor can discuss shared project status without becoming the
mandatory path for answers or progress.

An empty inbox does not imply no responsibilities. Active managed work retains
an expected oversight checkpoint, so the hourly reminder can restore unchanged
parallel responsibilities when due. No managed work or open/due obligation means
no model wake. Core due policy is shared with the bounded #113 recovery path;
the connector does not maintain a competing business schedule.

Implementation belongs to the subsequent work-map/dashboard stages after the
working messaging trial. The trial's dashboard skeleton (T-11, T-12) is the
exception, built from D1 onward (see "D1 as built" above). No provider
integration, skill or service is implemented or activated by this
requirements update.
