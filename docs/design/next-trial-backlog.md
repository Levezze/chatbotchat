# Next trial backlog

What the next working trial changes, and why. It collects findings from trial 1 as they come in,
and the design decisions taken since trial 1 started
([ADR-0013](../decisions/0013-delegating-chapter-masters-and-tracker-outbox.md), I-15 to I-27 in
[feature intake and tracker profiles](feature-intake-and-tracker-profiles.md)).

Trial 1 is not changed mid-run (T-15, I-22). Nothing here is built. Raw evidence stays in the
local trial case directory (T-22); this file carries only summaries.

## 1. Trial 1 status (2026-11-23)

Phase 1 passed all six checks on the pinned build 40e1f4d (0.33.0). Step 4 passed on its second
attempt: the first was invalid because the stack ran inside an app's terminal and died when the app
quit. Every committed item reached a receipt.

What phase 1 shows: delivery and receipts on two desktop hosts, and identity surviving compaction,
an app restart and a core outage, with state persisting and the room resuming afterwards. What it
does not show: automatic recovery from an outage (the core stayed down for 3.4 days until it was
restarted by hand), continuous availability, mid-turn delivery, or more than two seats. The core
exited once with no crash report and no log line; the cause is unknown.

Phase 2 runs next on the same pin, as the baseline for the next trial (T-30). The full report is
published once the trial ends (T-22).

## 2. Trial 1 findings to fix

1. **Wakes that found nothing.** 3 of about 12 wakes led to an empty check: one Claude wake after
   the room had closed, and two Codex hints for items the seat had already fetched. Each costs a
   model turn. Suppress a wake when nothing is unread at delivery time.
2. **The device driver is never idle.** It uses about 1.2 s of CPU per hour with no traffic.
   Find what it wakes for and remove the timer if it isn't needed.
3. **The close doesn't tell the closer about the replacement.** Only the reconnecting side learns
   the new room ID.
4. **Logs to check before the report** (explained). At this pin the core does not log receipts at
   the normal level, so phase 1 step 6 could never find them there; the report reads them from
   the end snapshot instead, and the sheets now say so. The driver's log does not see Codex
   sends or other Codex tool calls, because the Codex adapter talks to the core directly. It does
   log Codex wake attempts and their outcomes, so it is still the place to look for a missed Codex
   wake. Fix: log receipts at info, and document which log sees what.
5. **Codex defers MCP tools.** A Codex seat sees only the tool namespace until it searches, so
   its first "list your CBC tools" answer showed 3 tools and it then denied having the others.
   The setup step for a Codex seat must load the namespace before listing.
6. **Busy test command.** Claude Code blocks a plain foreground `sleep`. The script uses a
   Python sleep instead.
7. **`init` output** lists the protected database three times. Cosmetic.
8. **Codex re-checks after its own sends**, following the adapter's "check until the inbox is
   empty" instruction. Bounded by the inbox, so it is not polling, but the report must not count
   it as such.
9. **The driver retries a dead core forever.** It already backs off (1, 2, 4, 8 and 16 seconds),
   but caps at 30 seconds and logs every attempt: about 17,000 events and 3 MB over the 3.4-day
   outage. Fix: a much slower retry once an outage has lasted a while, and a log of the first
   failure plus a periodic summary.
10. **A stale socket blocks restart.** A killed core leaves its connector socket behind and refuses
    to start until it is removed by hand. Fix: on start, remove a socket nothing is listening on,
    as the driver already does.
11. **No supervision.** A dead core went unnoticed for days. Now: a health check in the sheet before
    each stage. Later: a supervisor.
12. **The stack dies with its host app** when started from that app's terminal. Now: both launch
    sheets require a standalone terminal app.
13. **One Codex adapter error covers two causes** ("core unavailable or native target authority
    does not match"). Fix: two errors.
14. **The report cannot name the build.** The manifest already has a build field, but its commit
    reads "unknown" because the pinned binary was compiled without `CBC_BUILD_COMMIT` set. Now: the
    sheet records the pinned commit next to the report. Later: set `CBC_BUILD_COMMIT` when
    building a pin.
15. **The tally carried only `cbc`**, so the other three prompt kinds read as missing. Now: the sheet
    tallies all four kinds, zeros included.
16. **The sampler is not scoped by case root.** It counts every candidate process of the user by
    executable and script name, so a second stack, or a test run of the candidate that starts its
    own `serve`, is counted as this run's cost. Phase 2's Chapter Masters work on the candidate
    itself and run its tests, so this happens in every phase 2 run. Now: the phase 2 sheet labels
    the raw cost and derives the stack-only cost afterwards from the PIDs each tick stores. Later:
    scope the sampler to the case root.
17. **The report cannot measure what the next trial changes**: autonomy, Primarch interventions, CI
    waits and turns per hour all read as missing. Now: phase 2 counts them by hand from a fixed
    list in the sheet. Later: decide which become core records.
18. **The legacy poll's hourly give-up.** Outside the trial, while a peer stayed silent for hours,
    the legacy `cbc poll` gave up every hour and woke its model with nothing new, needing a manual
    relaunch each time. The candidate removes this: in phase 1's quiet stretch of about a week,
    every wake followed a send. This belongs in the comparison, not in a fix.

### What blocks the next trial

Not every fix is needed before the next trial runs unattended.

- **Blocks:**
  - health and exit visibility (11);
  - bounded backoff against a dead core (9);
  - restart with stale-socket recovery (10);
  - unread items reconciled durably with no duplicate delivery, on every wake path;
  - wakes suppressed when nothing is unread at dispatch, with the race tested (1);
  - the build and receipts reportable, from snapshots if the evidence is unambiguous;
  - the closer told about its replacement (3), if the workflow relies on one side closing and
    reconnecting; otherwise recorded as a known limitation.
- **Refinements, not gating:**
  - receipts logged at info (4);
  - the build commit set at compile time (14);
  - zero idle CPU (2);
  - log polish.
- **Proposed as refinements, under review:**
  - the split Codex error (13);
  - a root-scoped sampler (16), worked around by deriving the stack-only cost from PIDs;
  - Codex tools not deferred (5), worked around by the setup step's tool search.

## 3. What the next trial adds

| Change | Decision | What the trial must show |
|---|---|---|
| Chapter Masters delegate and stay free | I-15 | The Chapter Master spends its time briefing, answering and reviewing, and takes a second task while helpers run. Memory per helper is measured. |
| Start sequence: extension, rename, register | I-16, I-17 | Every new agent reads its extension, renames its chat and registers before other work, and the Primarch learns of it with no user relay. |
| Tracker events in the outbox | I-18, I-20 | Each event has its configured effect once (create, link or update, which may hit the same tracker record), no duplicate on retry, replay or restart, and pending or failed updates visible on the dashboard. |
| Tracker agent | I-19, I-21, I-23 | It updates the user's real tracker, keeps human edits, asks the Primarch before the user, and no implementing agent touches tracker tools. |
| Fixes from section 2 | — | The blockers listed under "What blocks the next trial". Refinements are welcome but not required. |

Setting: a small real feature in a user's product repository (I-22), not CBC itself, with
promotion to production only if the result is good.

## 4. Open questions for the next trial

From [feature intake and tracker profiles](feature-intake-and-tracker-profiles.md#4-open-questions):
sub-agent or session (3), how an agent finds its extension (4), what a task is when a Chapter
Master holds a whole feature (5), tracker agent always running or on demand (6), human edits
(7), and deployment visibility (8).

Trial 1 phase 2 is settled: it runs as planned and is the baseline for the next trial (T-30).

## 5. Other test additions

- **A busy test with several short tool calls.** Phase 1's busy test was a single 90-second
  call, so a message could only land after it. A run of short calls shows whether a host
  delivers at the next tool call or only at the end of the turn (T-31 records the phase 2
  reading).

## 6. T3 Code as a candidate host (added 2026-11-21)

The user wants T3 Code in the next trial: at least as a second host beside the desktop apps, and
possibly as the main one. T3 Code is an open-source control surface that runs Claude Code, Codex
and other agents on the user's own subscriptions. The question for the trial is whether most of
the orchestration (Primarch, Chapter Masters and their helpers) can run in T3, while the desktop
apps keep running the work that needs their connectors, such as nightly tasks, and how the two
work together through CBC. CBC stays the record and the bus between hosts; T3 is one host
behind an adapter, not a replacement for the core.

What its source shows (read at commit `fcf50973`, not tried live):

- **Its MCP server is the integration point.** It is built for outside agents to connect to,
  with sign-in and access levels. A thread can be sent a message that lands inside a running
  turn at the next tool call (Claude and Codex both support this), queued for the next turn, or
  started if idle. A CBC adapter could deliver "unread in room X" this way, with the CBC core
  keeping the record and the receipt.
- **The no-sign-in host command only queues.** Mid-turn delivery needs a signed-in MCP client,
  re-approved every 30 days, or a small upstream request for a mode flag on that command.
- **Its scheduler can run a prompt in an existing thread** on an interval, at a time, or when a
  webhook URL is called. The core could call the webhook only when there is something to read,
  instead of a fixed interval that spends a model turn each time.
- **It has its own delegation**: a thread can create child threads, and a finished child wakes
  its parent. This overlaps with Chapter Masters briefing helpers (I-15).
- **Connectors:** Claude threads load the user's normal Claude setup, including claude.ai
  connectors. Codex threads appear to keep apps and connectors, but this is unverified.
- **Worth borrowing regardless:** T3 registers its MCP server for Codex so that the tools are not
  deferred behind tool search, which may fix finding 5.

Decided (2026-11-23):

- **Second host first.** T3 is qualified on its own before the next trial depends on it: a short
  pinned smoke on the phase 1 script, then a cross-host room, then child threads. The smoke pin is
  the phase 1 pin plus a T3 delivery adapter and nothing else, so its result compares the host
  alone. Whether T3 becomes the main host is decided after that.
- **In the next trial, planned: one Chapter Master on T3**, confirmed after the smoke. The
  Primarch and the other Chapter Master stay on the desktop apps, since the Primarch's turns and
  interventions are the main comparison with phase 2.
- **Order.** Phase 2 first, then the T3 smoke. T3 setup and preparation may overlap phase 2. Measured
  runs may not share a machine (finding 16): the T3 smoke runs outside phase 2's measurement windows
  or on another machine.

Questions still open:

1. ~~Main host or second host.~~ Decided above.
2. Do T3's child threads satisfy I-15 (helpers on the host's native mechanism, the Chapter
   Master free while they run, the user able to talk to a helper directly)? CBC stays between
   leads and across hosts either way.
3. Does a T3 Codex thread see the user's connectors? Checked by asking one to list its tools.
4. Mid-turn delivery: a signed-in MCP client, queue only, or the upstream request.
5. ~~A short peer smoke on T3 before any hierarchy runs on it.~~ Decided above.

Not for the next trial: a plugin or native integration in T3. T3 has no plugin system today;
its supported route for outside tools is the MCP server.
