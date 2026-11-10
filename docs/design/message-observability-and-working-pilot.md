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
- **Items older than the migration.** Their commit, class and snapshot are unknown. Offers and receipts for them create a row with `expected = 0`, so they never error.
- **Clock.** Each stamp holds the instance ID, a monotonic offset from that open, and wall time. An interval is `exact` only when both stamps share an instance. Otherwise it is an `estimate` from wall time.
- **Event projection.** `delivery_events` projects the facts into `DeliveryEvent` (`schema: 1`). An event carries no body and no batch reference. The reference is the receipt capability, so events carry only `batch_tag`, the first 8 bytes of SHA-256 over a domain-separated reference.
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
