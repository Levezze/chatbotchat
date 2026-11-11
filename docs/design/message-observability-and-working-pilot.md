# Message observability and working hierarchy trial

Requirements clarified 2026-11-09 for PRD #112 and stage #113. This is a design
and acceptance update, not implemented logging or a completed trial.

Updated 2026-11-10: the [working trial protocol](working-trial-protocol.md) holds
the decision record (T-01 to T-27), the trial setup, the measurements and the run
log. Where it differs from this document, the protocol is newer and wins.

## First framework acceptance gate

The existing native runs are preliminary transport probes. Marker exchanges,
short sleeps and interrupted generation establish specific capabilities; they
do not qualify a hierarchy implementing features under realistic load.

Before dashboard/work-map implementation, run a fresh isolated local setup with
one Repo Lead/Primarch and at least two implementation-owning Feature Leads/
Chapter Masters. Use actual bounded coding, testing and review assignments in
candidate-only workspaces. Include Codex and Claude native apps; qualify CLI
profiles separately. Native helpers remain Chapter Master subagents, not another
mandatory CBC reporting tier. Trial 1 seats both Chapter Masters in Claude Code
and the Primarch in Codex (T-13).

Chapter Masters own their assignments through eligible merge: worktree,
implementation, native helpers, tests, CI, reviews, PR and merge under
repository-configured gates and their existing grants. Each registers its work
and starts without a startup GO; the Primarch sees registrations asynchronously.
The Primarch maintains context, constraints, priorities, dependencies, shared
resources and release coordination, and intervenes on exceptions. It does not
approve routine PRs or merges. See
[eligible merge](work-records-and-external-observations.md#registration-controls-and-eligible-merge). A held test port blocks that test operation, not unrelated source work.
Test direct peer and parent-report communication while the recipients are busy
implementing, running tools and discussing another assignment.

Record the assignments, versions, native identities, actual role instructions,
configuration revision and isolated resources before the trial. Do not migrate
daily agents or use another product's work as a test. Standard one-time native
setup consent is distinct from repeated coordination approvals. No Emperor, new
supervisor role or full work-map service is a prerequisite. By decision T-09 and
T-11, the record layer (registration, work-item revisions, requests, outbox) and
a dashboard skeleton are built before the hierarchy phase, so that the trial
measures the intended topology rather than status passed by chat.

## Delivery facts and the meaning of seen

Track each logical message and authorized recipient separately. A coalesced wake
or batch links to all covered messages; it is not one delivery receipt for every
participant. Retries retain the logical message identity and add attempt IDs.

| Stage | Evidence required |
| --- | --- |
| Committed | The message transaction succeeded in CBC |
| Dispatch started | The shared connector began a recipient attempt |
| Native wake accepted | The supported host operation accepted the hint; not model uptake |
| Inbox offered | A check selected an immutable batch; not yet delivered |
| Tool result written | The adapter completed its defined result-write boundary |
| Transport receipted | CBC durably recorded the exact adapter receipt |
| Acted on | Separate, attributable work/response evidence when available |

The current core implements exact receipts but not a complete timed event model.
Its receipt follows adapter result writing. It does not prove that the host
consumed the bytes, the model understood them, or work resumed. A future UI may
call this `seen` only with that transport meaning explicitly documented; otherwise
use a precise received/receipted label. Unsupported host-observed stages stay
unknown. Do not create an LLM acknowledgement merely to manufacture a timestamp.

An optional `first_observed_post_receipt_activity` records the next observable
request started by the bound session after that receipt/result boundary. Record
what is observable: a CBC adapter sees CBC calls, not every other MCP server.
Exclude requests already in flight. This is an activity proxy, not proof that the
message caused the call. Native turn/phase identifiers or boundary events may
corroborate mid-turn versus next-turn timing where supported; without them the
classification remains unknown. Do not add a global tool interceptor or require
an extra model acknowledgement just to fill a metric.

Keep canonical send/receipt facts and first-occurrence times durable in SQLite,
independent of debug mode. An absent or dropped diagnostic log cannot mark a
message seen. Failed receipt persistence leaves delivery uncertain and work
recoverable, not falsely completed. Exact timestamp/schema changes belong to
implementation and migration review.

### As built in #113 PR-A

Migration 0007 adds:

- `candidate_core_instances`: one row per store open.
- `candidate_item_facts`: one row per committed item, classed `participant`, `introduction`, `notice` or `closure`.
- `candidate_recipient_facts`: one row per item and recipient.

Every fact is written inside the existing send, close, connect, check and receipt transactions. So a fact exists exactly when its transaction committed, whatever the log level.

- **Committed.** The commit records the item fact and a snapshot of its recipients from `candidate_delivery_items` (`expected = 1`). A later revocation never shrinks the coverage denominator. A notice's snapshot is taken after its recipients are written, so it is the notice's own audience.
- **Inbox offered.** The first offer is stored, and `offer_count` counts every offer, including the re-offer of an unchanged pending batch.
- **Receipted.** The first authorized receipt is stored. A receipt refused as no longer eligible records nothing. A repeated receipt changes nothing.
- **Items older than the migration.** Their commit, class and snapshot are unknown. Offers and receipts for them create a row with `expected = 0`, so they never error. That includes a batch offered before the upgrade and receipted after it, which records the receipt with the offer left unknown.
- **Clock.** Each stamp holds the instance ID, a monotonic offset from that open, and wall time. An interval is `exact` only when both stamps share an instance. Otherwise it is an `estimate` from wall time.
- **Event projection.** `delivery_events` projects the facts into `DeliveryEvent` (`schema: 1`). An event carries no body and no batch reference. The reference is the receipt capability, so events carry only `batch_tag`, the first 8 bytes of SHA-256 over a domain-separated reference. Events come oldest first. Instances are ordered by when they opened and stamps within an instance by the monotonic clock, so a wall-clock step cannot reorder them.
- **Stages the core cannot observe.** Dispatch, native wake, result written and acted-on are declared in the stage enum but have no write path. They arrive with the authenticated connector route in PR-B.

## Structured logs and correlation

Use one versioned JSON event schema across core, connector and adapters, with a
human-readable development view derived from it. Each event carries UTC event
time, observation time where different, level, event name, component/version,
run and process-instance IDs, outcome/error class, and applicable non-secret CBC
context, room, message sequence, logical recipient, binding epoch, attempt and
trace/span correlation. Redact or pseudonymize native session identifiers in
exported diagnostics. Do not log authentication tokens, receipt capabilities,
environment secrets, message bodies, prompts or reasoning, including in debug.

Trace send, dispatch, native submission, check and receipt as linked operations;
batching and retries must not sever their message/recipient relationship. MCP
stdout remains protocol-only; diagnostics use stderr or a separate sink.

CBC adopts these principles from OpenTelemetry's [log data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/)
and [messaging correlation](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-spans/).
The field choices above are CBC requirements, not a claim that the current
implementation already conforms to an OpenTelemetry instrumentation standard.

## Timing and debug mode

Produce mechanical per-recipient deltas from commit to dispatch, native
acceptance, result write and durable receipt. Report host waiting separately from
CBC scheduling/network time. A post-receipt reply is another measurement, not
"network latency" or proof of semantic understanding.

Use monotonic elapsed time within one process/clock instance. Never subtract
unrelated monotonic clocks. For inter-process, restart or remote comparisons,
retain clock origin/instance and observation timestamps; label wall-clock
estimates and clock uncertainty. Prefer a core-observed commit-to-receipt interval
when both facts can use the same live core clock. Unknown, skewed and missing
measurements remain explicit. See Rust's [Instant contract](https://doc.rust-lang.org/std/time/struct.Instant.html).

Normal mode retains essential lifecycle/error information and durable facts.
Debug adds scoped attempt details and delta reports. A startup debug flag and
live validated level/filter settings use the existing configuration mechanism;
changing verbosity does not restart CBC, create a model turn or widen authority.
Bound retention, event size, queue memory and disk use. Sample/coalesce repetitive
idle waits and retry noise. Keep log-drop counters and failed/pending deliveries
visible; never calculate percentiles from successes alone without their coverage.
Use low-cardinality metric labels, with message IDs in logs/traces rather than an
unbounded metric dimension. Report counts, latency distributions and oldest
pending age by the relevant host/profile and operating condition.

Prefer Rust `tracing` with structured subscribers and equivalent Python records.
[Runtime filter reload](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/reload/)
and a [bounded off-thread writer](https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/index.html)
are available building blocks, not mandatory new services. Distinguish lossy
diagnostics from durable coordination records. Optional exporters must not make
a collector, hosted logging account or monitoring stack a local-test prerequisite.
Test slow/full/unwritable sinks without stalling message delivery. Apply
[OWASP logging guidance](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html)
to escaping, exclusions, access and resource limits. OpenTelemetry's
[messaging metrics](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-metrics/)
separate operation and processing duration; their current development status
requires versioned adoption rather than treating names as an immutable API.

## Trial evidence and decision

Provisional experiment targets for a live, qualified idle desktop profile are
p95 commit-to-wake-accepted within ten seconds and, where observable/applicable,
post-receipt activity within sixty seconds of commit. These are proposed CBC
targets, not measured results, an agreed SLA or guarantees about an exited engine.
Record sample count, maximum, missing observations and host conditions; a tiny
sample does not establish a percentile. A missing activity proxy alone cannot
prove a message was not used. Busy-turn reports need actual turn/tool boundary
evidence and foreground tool duration. Pending delivery aged fifteen minutes
must be explicitly visible in diagnostics; this creates no execution hold or
approval requirement. Review baseline data and criteria before final qualification.

1. Qualify normal/debug logging, IDs, timestamps and receipt semantics first.
   Agree explicit timeliness criteria for each tested native profile before
   declaring it qualified; report baseline measurements without inventing a pass.
2. Implement the bounded assignments in parallel. Send useful coordination while
   recipients reason, edit, test and review, including a longer foreground tool.
   Observe whether input is usable mid-turn, at a safe/tool checkpoint or only
   after the turn. A fabricated sleep alone is insufficient working evidence.
3. Trace every sent item to receipt, failure or still-pending state. Verify native
   uptake and actual relevant response/work separately. Measure duplicate hints,
   delivery attempts and independent progress while another lane is waiting.
   Include a ready request addressed to the busy Primarch: distinguish receipt
   from an actionable disposition and subsequent execution. Record waiting age
   without interpreting it as transport loss. A tool error cannot be a delivered
   send. The [release follow-through case](../case-studies/2026-11-09-release-follow-through.md)
   motivates this scenario; full durable owner obligations remain in #115.
4. Exercise driver/core restart, re-entry after compaction, explicit Stop and an
   exited engine. Record native setup/approval cases distinctly. Do not assume
   a successful queue/open command means the model restarted.
5. Compare normal/debug CPU, RSS, disk and context cost, counting required native
   shims. Include all failed, deferred and unreceived items in the report.
6. Save a reproducible run manifest, bounded correlated logs, deterministic delta
   report and role-behavior findings. Classify each profile PASS/FAIL/unsupported
   with its measured operating conditions. Fix bounded defects; a host limitation
   needs an explicit product decision before claiming timely working delivery.

Plain-language native limits to test: a busy chat may defer input; an exited
engine may not wake; explicit Stop can require genuine user input to rearm; and
native consent differs from a routine CBC message. These are hypotheses or
observations per profile/version, not universal guarantees or excuses to mark
unreceived work complete. Keep unaffected work moving. No native permission
bypass, private protocol workaround or additional polling fleet is authorized.

## Entry checklist and shared recovery

Lead skills begin with three explicit steps: attempt/verify the role's native
rename, create/join the assigned relationship and show its actual room ID once,
then verify receiving/recovery. Preserve a user-specified title; a cosmetic
rename failure never blocks work. An unassigned Primarch creates no Emperor
report edge. Re-entry reuses identity/routes instead of duplicating them.

The proposed ten-minute recovery policy uses core-owned due calculation and
durable message/receipt/attempt facts. The existing shared device connector
executes it: reconcile enrolled bindings/current memberships and pending unread
state, then coalesce/replay wake hints through the qualified adapter. It keeps
no independent persisted responsibility schedule. Ordinary socket reconnect
backoff remains mechanical device lifecycle. Add only this bounded recovery
contract in #113; #115 extends the same core authority with responsibility
obligations rather than introducing a second scheduler.
Empty/unchanged state produces no model turn or chat message. An existing pending
hint is not an excuse for unlimited retries; apply bounded retries and record
unavailable targets. This is separate from #115's hourly responsibility reminder.
The timer cannot bypass explicit Stop, renew revoked identity or guarantee an
exited engine wakes. Settings remain live-configured. Native helpers need no
separate schedule. Today's legacy cursor still has one receiver, never two.

Lifecycle callbacks can reconcile the same authorized registration on entry,
resume and compaction. Claude's [SessionStart and context output](https://code.claude.com/docs/en/hooks#sessionstart)
document this capability, but the actual candidate hook still needs implementation
and profile qualification. They do not wake a closed session. After compaction,
restore bounded current context even if the inbox is empty; routine unchanged
recovery ticks supply nothing. Do not re-enroll a revoked member, rotate epochs
on every process restart or let a fork inherit its parent's identity.

[Codex Scheduled tasks](https://learn.chatgpt.com/docs/automations) and
[Claude scheduled prompts](https://code.claude.com/docs/en/scheduled-tasks) run
prompts rather than documenting a model-free preflight. A native prompt fallback
therefore requires an explicit choice with model cost and host timing disclosed.
If chosen, it only checks the bound inbox, handles unread coordination and emits
no empty-status narration. It is not a generic continue-work task. No native task
or new hook is installed by this plan update.

## Bounded implementation sequence

Keep #113 as one runnable stage with three focused implementation PRs:

- A: Rust event schema and durable first-occurrence message/recipient facts,
  migration and core-clock intervals, verified with real SQLite.
- B: connector/adapter/mod correlation and bounded logging, live filters,
  diagnostic failure behavior, scoped activity proxy and core-directed shared
  recovery path. The full responsibility scheduler stays in #115.
- Record layer (moved forward from #115, T-09): core-bound identity (T-25),
  one active peer room per pair (T-26), work items with the HITL flag (T-07),
  atomic revisions, requests with callbacks, outbox and fact-derived waits.
- Dashboard skeleton (moved forward from #116, T-11, T-12): work items, open
  requests and the outbox timeline, with answers through a browser session.
- C: reproducible manifest and deterministic timing/coverage/pending/wait report,
  then the working-role trial using needed candidate improvements still open
  at trial time, with disjoint ownership.

Claude implements and Codex reviews against the actual candidate base. Reuse
suitable isolated worktrees and keep write ownership separate. Commit reviewed
planning and hierarchy sources rather than leaving installed copies as the only
record. The unrelated approval prototype remains outside these checkpoints.
No candidate build, native run, hook installation, service change or adoption is
authorized by completing this planning review.

## B and C plan (2026-11-11)

The trial report template (section 8 of the
[working trial protocol](working-trial-protocol.md)) decides what B and C must
build. Each line of the template is listed here with the record that feeds it,
whether that record exists, and what fills the gap. Anything with no record is
built, marked `unknown`, or taken from the user's tally file. It is never
estimated.

| Report line | Source | Exists? | Gap and owner |
|---|---|---|---|
| Delivery: committed, receipted, failed, pending, lost | item and recipient facts (PR-A) | yes | none; C reads them |
| Wake latency: commit to wake accepted | wake attempts reported by the device driver | no | B1: wake facts |
| Wake latency: commit to first activity | the member's first non-transport core operation after receipt | no | B1: activity facts |
| Waits: user, Primarch, another Chapter Master | open requests by owner (R3) | yes | none |
| Waits: delivery | committed but unreceipted items | yes | none |
| Waits: ready but idle | receipt to first activity, per recipient | partial | B1 built the activity facts; holds (#114) would say whether something else blocked the member |
| Waits: human review, CI | PR observations | no | `unknown` in trial 1; #115 builds the observer |
| Waits: resource | holds (#114) | no | `unknown` in trial 1 |
| Approval prompts | host events are not exposed to CBC | no | the user's tally, a file of its own given to the report |
| Autonomy: merges without a Primarch yes | work events | partial | C2 counts merges from work events; whether each had a Primarch yes is a trial-manager finding |
| Rooms and identity: duplicates, identity changes | peer room and identity records (R1) | yes | none |
| Rooms and identity: legacy skills loaded | session transcripts | no | the trial manager's check, in the manifest |
| Primarch: interventions, turns per hour | items the Primarch sent | partial | C2 counts the CBC items each repo lead sent; turns and interventions are not observable |
| Cost: CPU, RSS, disk, normal versus debug | process sampling | partial | C3: `cbc-candidate sample` beside the core and driver; the Claude mod runs inside the host app and is not sampled; each tick carries the live log level (B2) |
| Manifest, sha256 | none | no | C: the manifest writer |

### B1: wake facts, activity facts, recovery

- **Wake facts.** The device driver already knows when it wrote a Claude
  channel notification, queued a Codex hint, or answered a native mod wait.
  A new authenticated connector operation, sent with the member's own binding,
  reports one wake attempt: the kind, the outcome (accepted or failed with a
  class), and the highest unread sequence the hint covered. The core stamps it
  with its own clock and keeps the driver's wall time beside it. For each
  covered recipient fact it records the first dispatch and the first accepted
  wake. A wake hint covers a member, not one item, so one attempt can mark many
  items. The commit to wake interval therefore includes the report's own local
  socket hop, and the report says so.
- **Result written is not recorded separately.** Both adapters write the check
  result to the host and then receipt it, so a receipt bounds result writing
  from above. The report labels result writing as bounded by the receipt, not
  measured.
- **Activity facts.** The core already authenticates every member operation.
  The first operation after a receipt that is not transport sets the
  recipient fact's first-activity stamp. Transport is check, receipt, binding
  status, settings, watch, watch members and the wake report itself.
  This is the "first observed post-receipt activity" of the issue: it shows the
  member did something through CBC after receipt, not that it read or used
  the item. Activity outside CBC is not observable and stays `unknown`.
- **Recovery.** A live setting, `recovery_after_ms` (default 600000, ten
  minutes), and a retry bound, `recovery_attempts` (default 3). `binding_status` gains a
  `recovery_due` flag the core derives from facts. It is true only when the
  member has an unreceipted item older than the interval and no wake attempt
  within the interval, and the oldest such item has not used up its attempts.
  The device driver's existing loop checks its bindings once a minute and
  re-hints those that are due. No new process, timer per session, or model turn
  on unchanged state. A member that has used up its attempts stays visible as
  pending, with its age.

### B1 as built

Migration `0014_wake_activity` (schema version 14) adds the
`candidate_wake_attempts` table and eleven nullable columns to each recipient
fact: `dispatched_*`, `woken_*` and `active_*` (each the core instance,
monotonic and wall stamps), `active_operation` and `receipted_wall_us`.

**Wake reports.** The connector operation is `wake`, sent with the member's own
binding: `through`, `kind` (`claude_channel`, `codex_queue` or `claude_mod`),
`outcome` (`accepted` or `failed`), `failure` (a class label, present exactly
when the outcome is failed), `recovery` and `attempt_wall_us`. The reply is
`woken` with `covered`, the number of the member's items with a sequence at or
below `through` that were unread when the driver made the attempt. That is the
items still unread plus the items receipted strictly after `attempt_wall_us`:
reports wait in the driver's queue, so the member can receipt an item before
the report of the wake that announced it arrives.

`attempt_wall_us` is the driver's wall clock in microseconds, taken just before
it notifies the host: before the channel write, before the Codex queue command
runs, and before the native 200 is written. Receipts carry a microsecond stamp
too (`receipted_wall_us`, added by migration 0014; older receipts have
milliseconds only). The comparison is strict because of causality: a receipt
the notification causes needs the host to react and call the core, which takes
far longer than a microsecond, so a receipt in the same microsecond as the
attempt, or earlier, came before it. The driver and the core run on one host
and read one realtime clock. `attempt_wall_us` is clamped to ten minutes before
the core's clock, the most the queue can hold back, so a wrong or stepped clock
cannot reach older receipts. A backward clock step can drop a wake fact; it
never affects delivery. One transaction stores the attempt and, for each covered item,
sets `dispatched_*` on any attempt and `woken_*` only on an accepted one. Both
are stamped when the core learns of the attempt, so commit to wake includes the
report's queue time; it is normally empty. The first stamp wins; later attempts change nothing. A sender is
never a recipient of its own item, and receipted items are not covered.

What "accepted" means, per path:

| Kind | Reported when | Accepted means | `through` |
|---|---|---|---|
| `claude_channel` | the router writes the `changed` bell for a batch | the bell was written to the session's channel socket | highest sequence in the batch |
| `codex_queue` | the queue command actually runs | the command exited 0; a latched hint runs nothing and reports nothing | the hint's sequence (the member's highest unread) |
| `claude_mod` | a native wait answers `pending` | the HTTP 200 was written to the mod's connection; a failed write reports `failed` with `mod_write_failed` | the highest of the first unread, the last delivered feed hint and the feed hint not yet delivered |

None of these proves the model read anything. A wake is the transport's last
observable step; activity is the next one.

The driver never lets a report touch delivery. Reports go into a bounded queue
(256) and one worker sends each with a 2 second socket timeout. A refused,
failed or dropped report emits a `wake_report_failed` or `wake_report_dropped`
event with the error's type name only, and the delivery result is unchanged.

**Activity.** Every connector request is classified before it runs. An
activity operation (discover, connect, send, close, work, request) takes
`BEGIN IMMEDIATE` and stamps `active_*` and `active_operation` on each of the
member's receipted recipient facts in that context that have no activity yet.
The stamp is the request's start on the core clock, taken before it waits for
the store. Only receipts whose transaction had already committed at that start
count: the core keeps, in memory, the newest committed receipt stamp of its
instance, raised only after the commit returns, and each request reads it as it
starts. A request that started while a receipt was waiting for the store or
still being written did not follow it. A request that starts in the instant
between a commit and the raise misses that receipt; the member's next activity
records it, so the error is only ever late, never early. A receipt from an
earlier core instance always counts.
It is recorded before the operation, so a refused send still counts: the member
acted through CBC. The classification is an exhaustive match, so a new
operation does not compile until it is placed.

**Recovery.** `binding_status` carries `recovery_due`. The core computes it
from its own wall clock and the member's live settings. It is false when
recovery attempts are 0, when there is no unread item, when the oldest unread
item was committed within `recovery_after_ms` (a missing commit time counts as
old), when any wake attempt was reported within the interval, or when the
recovery attempts whose `through` reaches the oldest unread item have used up
`recovery_attempts`. Ordinary wakes reset the interval but never use an
attempt. A failure to compute the flag reports `false` instead of failing
`binding_status`, because the native waiter, the Codex latch and startup
verification all read that status.

The driver's recovery loop runs every 60 seconds in a thread of the existing
driver process and stops with the feed. For each binding it reads the status
with the 2 second bounded call and acts only on `recovery_due` exactly `true`
and a positive first unread sequence:

- Codex: `QueueDelivery.recover`, which shares the feed worker's lock and
  bypasses the latch once.
- Claude with a channel: a forced ring, the same call a channel hello makes.
- Claude native: a one-shot flag that makes the next wait answer `recovery`
  instead of `pending`. The mod drops a repeated `pending` cursor (a reconnect)
  but submits its hint on `recovery` even when the cursor is unchanged. A mod
  older than this change rejects `recovery` and stops until human input, so
  the driver and the mod ship together.
- Anything else: a `recovery_skipped` event.

The core counts recovery attempts from wake reports, and reports can be lost.
So before each recovery the driver registers a hold on the binding's oldest
unread item. Any recovery report for the binding, accepted or failed, marks the
hold as attempted; the native path marks it at once, since its report follows
when the mod next waits. The hold is lifted when the core acknowledges a
recovery report (a `woken` reply) or the oldest unread item moves, and is
dropped at once, even if the dispatch raises, when nothing was attempted.
While it stands the driver emits `recovery_unacknowledged` instead of
recovering. Registering it before the wake means an acknowledgement that
arrives before the dispatch returns still finds it. A lost report therefore
costs at most the one attempt it described, and `recovery_attempts` stays a
real bound.

A recovery report's `through` is what that path actually covered, as for any
wake: the batch's highest sequence for a channel, the higher of the first
unread and the latest feed hint for a mod. A Codex recovery hint carries the
first unread sequence, deliberately: it is the item the flag was raised for. The
attempt bound counts every recovery attempt whose `through` reaches the oldest
unread item, and all three paths reach it.

**Compatibility.** `PostReceiptActivity` is a new value in the closed stage
enum, and the timeline types deny unknown fields. A strict reader built before
B1 cannot parse a new timeline; nothing outside this crate reads them, so
`EVENT_SCHEMA` stays 1. The new settings fields default when absent, so an old
settings file loads.

**Waits.** Ready but idle is receipt to first activity per recipient
(`to_activity` minus `to_receipt`, both measured from the commit). A recipient
with a receipt and no activity is idle for as long as the run lasts, and the
report says it is still open.

**Tests.** `tests/wake_activity.rs` has seven tests over real SQLite, one of
them through the real connector socket. The driver tests cover each wake path,
a lost report, the recovery paths for all three hosts and the loop's stop.

Mutants, each run against the full crate suite (17):

- Killed (14): failed counts as woken; dispatch first wins; activity needs a
  receipt; activity first wins; the activity label check; the failure label
  check; a recent attempt blocks recovery; only recovery attempts use the
  bound; commit age; check counted as activity; the status flag wired to
  `binding_status`; the member settings patch applies; the hint bound and the
  wake stage events (both survived the first run and were killed after the
  tests were strengthened).
- Equivalent (2): removing the `through > 0` check (the table's CHECK
  constraint refuses the row anyway) and removing the zero-attempts early
  return (zero attempts already compares as used up).
- Compile error (1): dropping `Check` from the transport list, because the
  match is exhaustive.
- Codex's first review found three ordering races, all fixed with a test that
  failed first: a report that reached the core after the receipt covered
  nothing; a request queued behind a receipt counted as post-receipt activity;
  and `feed_changed` could wake a mod before the delivery handler recorded the
  hint, so the report undercounted. The last fix reads the feed's undelivered
  hint, outside the native lock, because the feed emits `feed_changed` while
  holding its own lock.
- Codex's second review found two boundary errors in that fix, both fixed with
  a test that failed first. The driver took the attempt time after the bell or
  queue command returned, so a host that receipted before the return lost its
  wake; it now takes it before notifying. And a receipt in the same millisecond
  as a later attempt was covered; stamps are now in microseconds and the
  comparison is strict. The new core test builds the tie from the stored
  receipt stamp rather than the clock, so it cannot race.
- Codex's third review found two more, both fixed with a test that failed
  first. Recovery attempts were counted only through lossy reports, so a run of
  lost reports re-woke the same item every loop; the driver now waits for an
  acknowledged recovery report or a moved unread item. And a request that
  started after a receipt's clock read but before its commit counted as
  post-receipt activity; requests now compare against committed receipts only.
  The core test interleaves a receipt with request starts on one task, five
  rounds, and checks that the last start taken before the receipt returned does
  not count.
- Codex's fourth review found two gaps in that recovery hold, both fixed with a
  test that failed first: it was registered after the wake, so an early
  acknowledgement left it stuck; and a failed attempt set none, so a lost
  failure report allowed unbounded retries. The hold now goes up before the
  wake and stays for any reported attempt.
- Codex's fifth review found that an acknowledgement for an older item could lift
  a newer item's hold, when the oldest unread item moved while the older report
  was still queued. Each queued report now carries the hold it marked, and its
  acknowledgement lifts only that hold. The test (red first) delays the older
  acknowledgement until a newer recovery has run and its report has been lost.
  One coalescing remains on the native path: its report comes when the mod next
  waits and marks whichever hold is current, so two native recoveries can share
  one report. That is still at most one unacknowledged report per hold.
- Codex's sixth review found that the native path reported `accepted` before
  the 200 was written, so a mod that hung up first left a false wake fact. The
  attempt time is still taken before the write; the outcome is reported after
  it, `failed` with `mod_write_failed` when the write fails. The test (red
  first) closes the mod's end of a real socket pair during the final status
  check.
- Codex's seventh review found that native recovery never reached Claude: the
  wait answered `pending` with the cursor the mod already held, and the mod
  drops a repeated cursor. The driver now answers `recovery`, which the mod
  submits regardless of the cursor. Tests on both sides went red first: the
  driver's response type and the mod's second prompt.
- Not tested: the clamp on `attempt_wall_us`, which needs a receipt more than
  ten minutes old.
- Not tested: the fail-soft path of `recovery_due` in `binding_status`. It needs
  a store fault the tests cannot inject without a mock.

### B2: structured logs

The driver's JSON log and the core's few `eprintln!` lines become one
versioned schema with run, process instance and correlation fields, bounded in
file size with rotation and a drop counter, and live level changes through the
settings file. The core uses the workspace's `tracing` dependency, which is
already locked. B2 does not feed any report line. It is for diagnosing a failed
run, so it lands after B1 and does not block trial setup.

### B2 as built

A correction first. The plan above said the driver had no log of its own; it
had one. Both routers already wrote `router-events.jsonl` under `--log-dir`,
one unversioned JSON object per line, opened per line and never bounded. B2
converts that log rather than adding a second one.

- **One schema, two writers.** Every line, from the core or the driver, is one
  JSON object: `v` (1), `t_ms` (wall milliseconds), `level`, `component`
  (`core` or `driver`), `run`, `instance` (16 hex characters, random per
  process start), `event`, then the event's own fields.
  - Only allowlisted field names are written: member, chat, context, room,
    batch, reference, delivery, kind, reason, outcome, count, seq, attempt,
    targets, pending, session, retry_ms. Anything else is dropped at the
    writer, so a message body, a token or a path cannot reach a log by
    accident. The core's free-text `message` is dropped too, and a core event
    with no `event` field is written as `unnamed`.
  - The core's list is `FIELDS` in `src/log.rs`; the driver's is `FIELDS` in
    `adapters/shared/log.py`. A Python test parses the Rust list and requires
    the two to be equal.
  - A receipt reference is a capability, so no log holds one. `reference` is
    always the batch tag B1's facts already use (`facts::batch_tag`, the
    first 16 hex characters of a sha256), which the driver computes the same
    way. A log line, a fact and the other side's log line about one batch
    join on it. Both sides pin one input and its tag in a test.
- **Files.** The core writes `<root>/logs/core.jsonl`; the driver writes
  `<log-dir>/router-events.jsonl`. Each is mode 0600, opened with
  `O_NOFOLLOW` in a 0700 directory, and a directory open to others is refused
  at start.
  - Each file is opened when its log is created, so later descriptor pressure
    cannot cost it a line; if that first open fails, every write retries.
  - At 8 MiB the file rotates to `name.1`, keeping three older files.
  - A line that cannot be written is counted. The count is written as a
    `log_dropped` warning (with `count`) on the next line that succeeds.
    Writing never raises or blocks delivery.
- **Run id.** `serve --run <id>`, and `--run <id>` on both routers, put the
  trial's run id on every line. An id is 1 to 64 letters, digits, `.`, `_` or
  `-`; anything else is refused before the process starts. Without the flag,
  `run` is null.
- **The live level.** `log_level` in `runtime-settings.json` is `normal` or
  `debug`; absent means `normal`. It is skipped when serializing at `normal`,
  so a settings file without it keeps its revision (pinned in
  `tests/settings.rs`). An unknown value is refused like any invalid edit, and
  the core keeps its last accepted level.
  - The core applies it at its next settings refresh, with no restart. Normal
    writes info and above; debug adds debug lines; trace is never written.
  - `binding_status` replies now carry `log_level`. The driver takes its level
    from every `binding_status` reply it reads: at startup, and for the device
    router on its recovery pass, every 60 seconds. A level change reaches the
    driver within about a minute. The Claude-only router reads it at startup
    only; the trial runs the device router.
- **What is logged.** In the core, at debug: one `request` line per answered
  request, with the reply's kind, seq, reference, count of items, member,
  context and outcome, with the batch's tag as `reference`. A request is
  logged only when debug held at both its
  start and its end, so the request that reads a level change is never
  logged. At any level: `accept_unavailable` and
  `dashboard_accept_unavailable` (warn, with the error kind) and
  `dashboard_stopped` (error, without its text, which may hold a path).
  - In the driver, every existing event, renamed onto the allowlist: `n` to
    `count`, `error` and the feed's `reply` to `reason`, a handled reply's
    `reply` to `outcome`, `sequence` to `seq`, `retry_in` seconds to
    `retry_ms`, and `listening` now gives a `count` of bindings rather than a
    PID and member list. At debug, one `core_call` line per core round trip:
    the operation as `kind`, the reply type as `outcome`, member, context, and
    the reply's seq, batch tag and item count where present.
- **Not converted.** The Claude adapter's per-chat `adapter-*.jsonl` files and
  the Codex adapter keep their own formats. They are diagnostics for one
  chat, not the run.
- **The sampler.** Each tick is labelled with the level in
  `runtime-settings.json` when the tick is taken, read without a lock or a
  write. If the file is invalid the tick keeps the previous label, as the core
  keeps its last accepted settings; before any valid read the label comes from
  the last accepted settings, else `normal`. `sample --mode` is gone, and the
  file is `samples-<wall_ms>.jsonl`: one sampler can cover both levels.
- **Tests.**
  - Core unit (`src/log.rs`): the header and allowlist, debug only while the
    level is debug, rotation keeping older files, a dropped line counted and
    reported (a link at the log's path is refused, not followed), the request
    line by identifiers only, and a private file in a refused shared directory.
  - `tests/logs.rs`, a real `serve --run trial-1`: no request lines at normal;
    after switching to debug the switching request is not logged and the next
    ones are, with their kinds, member, count, run, instance and component;
    switching back adds none; neither token nor any body is in the file.
  - `tests/settings.rs`: a file without `log_level` keeps its pinned revision
    and reads normal; the level changes live and an unknown value keeps the
    last good one.
  - `tests/connector.rs`: under descriptor exhaustion the core still writes
    its `accept_unavailable` warning.
  - `adapters/shared/test_log.py`: the same header and allowlist, parity with
    the Rust list, debug filtering and an ignored unknown level, rotation, a
    counted and reported drop, a write that never raises, private files and a
    refused shared directory, and the run id rule.
  - `adapters/claude/test_router.py`: a real router at normal writes v1 lines
    and no `core_call`; restarted with `--run` against a core reporting debug,
    a send gives one `core_call` line with member, outcome and seq, a 0600
    file, no token and no body; a bad `--run` exits 2.
  - `adapters/device/test_router.py`: two status reads on the recovery path
    set debug then normal, giving exactly one `core_call` line, a debug event
    after the switch back is not written, and no token is in the file.
  - `tests/sampler.rs`: a sampler switched to debug after its first tick and
    given an invalid file after its second labels its ticks normal, debug,
    debug.
- **Changed tests.** Four assertions changed with the contract they check, not
  to make a test pass: the router's `hello` event names its reply `outcome`
  rather than `reply`; `tests/logs.rs` requires the request line's
  `reference` to be the batch tag, and the raw reference to be absent, where it
  had required any string; the sampler file name no longer carries a mode, and its
  ticks without a settings change read normal; the report's debug-missing
  reason no longer says debug waits for B2. The router's "an unwritable log
  never raises" unit test now builds the new log, removes its directory and
  writes, because the old one set an attribute that no longer exists and would
  have passed without writing anything.

### C: manifest, report, sampler

- **Manifest.** One JSON file written before the go: build commit, host and
  CLI versions, seats with their enrolled identities and role skills, the
  settings revision and the isolated resources. Its sha256 goes in the report.
  The user's approval-prompt tally is a separate file the report reads (see
  C1 as built): the manifest is written once and never edited.
- **Report.** A command that reads the candidate state and the manifest and
  prints the section 8 template deterministically: the same inputs give the
  same bytes. Every distribution carries its sample count, and missing
  measurements are printed as missing.
- **Sampler.** CPU, RSS and disk for the core and driver processes, sampled at
  a fixed interval into a local file, normal mode and debug mode labelled.

### C1 as built: snapshot, evidence, manifest

The core holds an exclusive lease on its database for as long as it runs, so
nothing else may open the live file. Every trial record therefore reads a
snapshot that the running core writes on the owner's request.

- **Snapshot.** Owner operation `snapshot`, which only the core runs. It
  creates `<root>/snapshots/` with mode 0700 if missing. It then creates
  `snapshot-<wall_ms>-<random>.sqlite3` with `create_new`, mode 0600 and
  `O_NOFOLLOW`, and runs `VACUUM INTO` that file. SQLite writes into an
  existing file only when it is empty. So a snapshot never replaces a file,
  never follows a link, and is never readable by another user. If the VACUUM
  fails, the empty file is removed. The reply is `{file, taken_wall_ms,
  build}`, where `build` is the running core's own `{commit,
  binary_sha256}`. The core hashes its executable when it starts, so a binary
  rebuilt on disk afterwards is not credited with the core's work.
- **Evidence.** `evidence::Evidence::open` accepts only a `snapshot-*.sqlite3`
  file directly beneath the snapshot directory. The file must be a private
  regular file with one link. The sandbox is re-inspected first, so the
  protected-database checks apply as well.
  - SQLite opens it `read_only` and `immutable`, with no lease, no migration
    and no core clock.
  - It refuses a file whose `application_id` is not the candidate's, or whose
    `user_version` is not the current schema version.
  - The live database is refused by path, and a unit test shows that a write
    through Evidence fails.
- **Manifest.** `cbc-candidate manifest --root … --protected-db … --out
  <root>/snapshots/<name>.json --trial N [--version k=v]… [--note k=v]…`.
  - The path is checked before anything else, so a bad path or an existing
    file costs no snapshot. Its directory must be named `snapshots` and its
    parent must resolve to the sandbox root, so an alias of the root (on macOS
    `/tmp` is `/private/tmp`) is accepted; the file is then written under the
    sandbox's own spelling of the directory. The command then takes a snapshot, reads the
    settings snapshot, and writes the manifest with `create_new`.
  - Fields:
    - `trial`, `schema_version`;
    - `build {commit, binary_sha256}`, as the core that took the snapshot
      reports it, not the build of the command that asked. The commit comes
      from the compile-time `CBC_BUILD_COMMIT`, which the launch sheet's
      build command sets, and is `unknown` otherwise.
    - `snapshot {file, taken_wall_ms, sha256}`;
    - `versions`, `notes`;
    - `settings`, the full settings snapshot with its revision;
    - `seats`: context, member, role, active, retired, epoch, native session
      and host at the current epoch.
  - No approval-prompt tally. A first draft carried `null` slots for the user
    to fill, which would have meant editing a hashed record after the go, and
    keyed seats as `context/member`, which is ambiguous because both may
    contain `/`. The tally is now its own file, nested by context and member,
    passed to the report.
  - No credential digest or token is read into it, and a test asserts that
    the bytes contain neither.
  - Output is canonical: pretty JSON with keys sorted at every level and a
    final newline. Sorting is explicit because the workspace builds
    serde_json with `preserve_order`. A test shows the canonical form is a
    fixed point.
- **Tests.** `tests/evidence.rs` runs over a real connector:
  - a snapshot taken before a retirement still shows the seat unretired;
  - reading changes no bytes;
  - junk named like a snapshot is refused;
  - the live database is refused;
  - the manifest fields and canonical form are checked;
  - the manifest records the build the core reports;
  - the command refuses a path outside the snapshot directory, accepts an
    alias of the root, and never overwrites a manifest.
- **Review fixes.** Codex's first review of #152 found three P2s, each fixed
  test-first: the root alias above, the build taken from the calling binary
  instead of the core, and the ambiguous tally keys.

### C2 as built: the report

`cbc-candidate report --root … --protected-db … --manifest M --snapshot END
[--from MS] [--until MS] [--tally T]` prints the section 8 template to stdout.
`cbc-candidate snapshot` takes END from the running core. C3 is now only the
sampler and the Cost section it feeds.

- **Inputs.** The manifest and tally are read with the same input checks as
  other documents. The report refuses:
  - a manifest whose baseline snapshot is not a candidate snapshot, or whose
    bytes no longer match the manifest's sha256;
  - an end snapshot taken before the baseline;
  - a window ending after the end snapshot, which cannot show anything later;
  - a tally naming a seat the end snapshot lacks, or an unknown kind.
- **Window.** Defaults to the baseline snapshot's time through the end
  snapshot's time, both from the records (the end time from the file name the
  core gave it). Both ends are inclusive, in wall milliseconds.
- **State at `until`.** Taken from each fact's own timestamp, never from a state
  column, which shows the moment the snapshot was taken. A request closes at
  its first answer on any channel (a relay ends the asker's wait, as in
  `waits_now`) or at its withdrawal time; a receipt or activity
  after `until` counts as absent. A test cuts one end snapshot at three points
  and gets three different reports.
- **Withdrawal time.** Migration 0015 (schema 15) adds
  `candidate_requests.withdrawn_wall_ms`, written by both withdrawals: the
  asker's and the one retirement does. An event alone could not date them: a
  withdrawal emits one only when the owner is a member with an open room, so
  requests to the user and retirement withdrawals had none. The migration
  dates earlier withdrawals from their event where there is one. A withdrawn
  request answered first keeps its wait, which the answer ended. One with
  neither a withdrawal time nor an answer is left out of the waits and counted
  on its own line, never guessed.
- **Waits.** Wall time, the one timeline every source shares, clipped to the
  window; an interval still open at `until` is marked open. Per member and
  cause: count, open, sum, n/median/p90/max by nearest rank, and the share of
  the window covered by the union of its intervals. Causes come from the
  request's owner kind and the owner's role, delivery from commit to receipt,
  and ready but idle from receipt to first activity. Then each partial or
  unknown cause with what it lacks, and the ten longest waits, ordered by
  duration, member and ID.
- **Delivery.** Expected recipients of items committed in the window:
  committed, receipted by `until`, superseded, and pending split into never
  offered and offered but not receipted. Failed prints missing: no record says
  delivery failed. A room notice is superseded when a newer notice for the same
  room reaches the same member while it is unread: unread notices coalesce, so
  it is never offered or receipted. It is not pending, its delivery wait ends
  at the newer notice's commit, and it is not counted missing in wake latency.
  A notice already offered stays in its batch until its receipt. A rebind
  drops the batch and the notice is then replaced, but no record dates the
  drop (a plain rebind writes no identity event) and a later batch may offer
  it again. So an offered notice still held by a pending batch at the end
  snapshot is pending, and one no longer held, never receipted, was replaced
  at an unknown time, though not before the newer notice's commit. A report cut
  before that commit shows it pending with its wait open; one cut after it
  leaves it out of pending and the waits and counts it on its own line. Wake attempts reported in the window, by kind, accepted and failed
  by class.
- **Wake latency.** Per host and profile (for example `claude/desktop`), commit
  to wake accepted and commit to first activity. Each delivery belongs to the
  binding in force when its item was committed: the epoch whose enrolment or
  rebind is the latest at or before the commit, with that epoch's native
  target. A seat that has since been rebound to another host keeps its earlier
  deliveries under the earlier host. An epoch enrolled without a target still
  ends the one before it: its host comes from the invite and its profile is
  `unknown` (`codex/unknown`). A delivery with no binding by then is `unknown`. Monotonic time when both stamps come from one core instance,
  wall time otherwise, and the latter are counted as estimates.
- **Seat names.** Seats are grouped by (context, member), never by a joined
  string, and print as `context/member`, both parts quoted when either
  contains `/`, so two seats never print alike.
- **Approval prompts.** From the tally file,
  `{"context":{"member":{"cbc":n,"native_setup":n,"native_refusal":n,"other":n}}}`,
  nested because IDs may contain `/`. A kind absent from the file prints
  missing. The file's sha256 goes in the header.
- **Autonomy.** Merges are counted from work events whose revision moved the
  item to `merged`, once per revision; an item with no report room is not
  seen. Whether a merge had a Primarch yes, routine permission messages and
  go-ahead requests print missing.
- **Rooms and identity.** Duplicate peer rooms: per unordered pair, chain heads
  (rooms that supersede none) introduced by `until`, beyond the first. Rebinds
  in the window are listed; enrolments and retirements counted. Legacy skills
  print missing.
- **Primarch.** CBC items each repo lead sent in the window, as a proxy only;
  interventions and turns print missing.
- **Header.** Trial, build commit, window, and the manifest, baseline, end
  snapshot and tally by file name and sha256. No absolute path or PID appears,
  because the report goes into the public run log. Result, Phase 1, Outcome,
  Findings and Changes print missing for the trial manager to write.
- **Determinism.** No clock is read; every query has a total order; the text is
  built by one pure function from loaded rows. Loading (`Facts::load`) is kept
  apart from rendering so the render can be tested on hand-built rows. The
  module follows that split: `src/report/facts.rs` loads, `render.rs` prints,
  and `mod.rs` holds the window, the wait derivation, the tally and the
  input checks.
- **Tests.**
  - Unit: nearest-rank percentiles, union of overlapping and touching
    intervals, clipping at both edges, state at `until`, unclassified owners,
    monotonic versus wall latency, the merge filter, the tally format, and the
    whole template byte for byte from hand-built facts. Added after review:
    latency by the binding in force at commit, seats that would print alike,
    a withdrawal with no time, bindings loaded from a real provisioned
    snapshot, and when a notice counts as superseded.
  - `tests/report.rs`, over real snapshots: the three-cut test above, a
    byte-identical repeat with no absolute path, the tally lines, and each
    refusal. At the baseline cut, a wait that began before the window (the
    lead's receipt of the room introduction during setup) is carried in,
    clipped to the window and shown open, rather than dropped. After review: a
    request to the user and a request ended by retirement, neither of which
    emits an event, both close at their withdrawal; the command, run from
    inside the snapshot directory, takes bare file names; and four sends with a
    notice every two messages, then both members drained, give one superseded
    notice, nothing pending and no open delivery wait; and a notice offered,
    re-offered after a rebind and then joined by a newer one stays pending,
    until a further rebind drops it unreceipted and it counts as superseded at
    an unknown time, yet a cut before the newer notice still shows it pending.

### C3 as built: the sampler

`cbc-candidate sample --root … --protected-db … [--interval-s 10]
[--ticks N]` samples until SIGINT or SIGTERM, or for N ticks. It prints
`{"status":"sampling","file":…}` when the file exists and
`{"status":"stopped","ticks":n}` at the end. `report --samples FILE` feeds the
Cost section, repeated once per sampler run; each file's name and sha256 go in
the header. (As merged in C3 the sampler took `--mode`; B2 replaced it with the
live level, read per tick. See B2 as built.) The files
are read in time order, and two that overlap are refused: two samplers running
at once would count every process twice.

- **What a tick holds.** One JSON line, version 1: wall time, mode, interval,
  the candidate processes and the disk. Unknown fields are refused on read.
  - Processes come from `ps -ww -U <uid of the root's owner> -o
    pid=,rss=,time=,args=`. Each keeps a label, PID, RSS in KiB and cumulative
    CPU in milliseconds. A line that does not mention `cbc-candidate` is
    skipped unread. For one that does, the exact argument vector is read from
    the kernel (`KERN_PROCARGS2` on macOS, `/proc/<pid>/cmdline` on Linux),
    because a rendered `ps` line cannot say where a path with a space ends. It
    is used to classify and then dropped: no argument, path or unmatched line
    is stored or printed, and a malformed `ps` line is reported by number only.
  - A process counts only when it runs the code: the `cbc-candidate`
    executable itself, labelled by subcommand (`cbc-candidate serve`), or a
    Python interpreter whose script is `cbc-candidate/adapters/<dir>/<file>.py`
    (`adapters/device/router.py`). A shell, `rtk` or an editor that merely names
    the candidate is not counted, nor is the sampler itself.
  - Disk is allocated bytes (blocks × 512, as `du` counts) under the root,
    without following links, in three parts: the database with its WAL and
    shared-memory files, `snapshots/`, and everything else.
  - `ps` sees every candidate process the user runs, not only this root's.
    One candidate runs during a trial; a second one would be summed in.
- **The file.** `<root>/samples/samples-<wall_ms>.jsonl`, created new
  with mode 0600 and `O_NOFOLLOW` in a 0700 directory, flushed per tick. A
  sampler killed mid-write leaves a partial last line; the report ignores it
  and names the file. Any other bad line, an unknown version or ticks out of order
  are refused, naming the line number.
- **The Cost section.** Ticks are cut on the window by wall time. Per mode:
  tick count, interval and the span covered. Per label: the most processes
  seen in one tick against the expected count, RSS as n/median/p90/max of the
  per-tick sum, and CPU as a share of one core.
  - CPU is each process's growth in cumulative CPU time between consecutive
    ticks of one mode, over the wall time those ticks span. A PID whose time
    goes down, or whose label changed, was reused and adds nothing. A label
    with no such step, seen in one tick only, prints its CPU as missing, not
    zero.
  - Expected counts come from the end snapshot's active seats: one core, one
    device router, and one shim per seat of each host (the Claude adapter per
    chat, the Codex adapter per session). An expected label with no match
    prints missing, so an absent component never passes for free. A label not
    expected is still listed.
  - The Claude mod runs inside the host app and prints missing. With no
    debug-mode tick in the window, the debug line prints missing.
  - Disk: start, end and maximum of each part, per mode, so a normal run
    and a debug run are compared rather than merged.
- **Tests.**
  - Unit: `ps` time formats (macOS `M:SS.ss`, procps `[dd-]hh:mm:ss`), the
    classifier's accepted and refused command lines, `ps` parsing without
    echoing a bad line, a tick round trip, the partial last line, CPU growth
    and PID reuse, shims summed per tick, and the whole Cost section byte for
    byte.
  - `tests/sampler.rs`, over real processes: the candidate binary run through
    a link, sampling a second root, and a Python script under
    `cbc-candidate/adapters/device/`, both under a directory whose name has a
    space, are found by PID with their labels, the test process is not, and the
    file has no path or command line, mode 0600 in a 0700 directory. The
    command run for one tick prints both status lines.
  - `tests/report.rs`: a sampler file with ticks before, inside and after the
    window gives two ticks and only the CPU growth inside it; a file that is
    not sampler output is refused without echoing its content; a normal file
    and a debug file, given in either order, are both named and both measured,
    and two files that overlap in time are refused.
