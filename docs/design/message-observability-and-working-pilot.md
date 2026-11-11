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
built, marked `unknown`, or taken from the user's tally in the manifest. It is
never estimated.

| Report line | Source | Exists? | Gap and owner |
|---|---|---|---|
| Delivery: committed, receipted, failed, pending, lost | item and recipient facts (PR-A) | yes | none; C reads them |
| Wake latency: commit to wake accepted | wake attempts reported by the device driver | no | B1: wake facts |
| Wake latency: commit to first activity | the member's first non-transport core operation after receipt | no | B1: activity facts |
| Waits: user, Primarch, another Chapter Master | open requests by owner (R3) | yes | none |
| Waits: delivery | committed but unreceipted items | yes | none |
| Waits: ready but idle | receipt to first activity, per recipient | no | B1: activity facts |
| Waits: human review, CI | PR observations | no | `unknown` in trial 1; #115 builds the observer |
| Waits: resource | holds (#114) | no | `unknown` in trial 1 |
| Approval prompts | host events are not exposed to CBC | no | the user's tally, recorded in the manifest |
| Autonomy: merges without a Primarch yes | work item revisions and requests | partial | C counts merges from work items; the rest are trial-manager findings |
| Rooms and identity: duplicates, identity changes | peer room and identity records (R1) | yes | none |
| Rooms and identity: legacy skills loaded | session transcripts | no | the trial manager's check, in the manifest |
| Primarch: interventions, turns per hour | core operations by the Primarch | partial | C counts the Primarch's core operations; turns are not observable |
| Cost: CPU, RSS, disk, normal versus debug | process sampling | no | C: a sampler run beside the core and driver |
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
- Claude native: a one-shot flag that makes the next wait answer `pending`
  even with an unchanged cursor.
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

### C: manifest, report, sampler

- **Manifest.** One JSON file written before the go: build commit, host and
  CLI versions, seats with their enrolled identities and role skills, the
  settings revision, the isolated resources, and the user's approval-prompt
  tally fields to fill during the run. Its sha256 goes in the report.
- **Report.** A command that reads the candidate state and the manifest and
  prints the section 8 template deterministically: the same inputs give the
  same bytes. Every distribution carries its sample count, and missing
  measurements are printed as missing.
- **Sampler.** CPU, RSS and disk for the core and driver processes, sampled at
  a fixed interval into a local file, normal mode and debug mode labelled.
