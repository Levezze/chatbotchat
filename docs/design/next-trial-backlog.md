# Next trial backlog

What the next working trial changes, and why. It collects findings from trial 1 as they come in,
and the design decisions taken since trial 1 started
([ADR-0013](../decisions/0013-delegating-chapter-masters-and-tracker-outbox.md), I-15 to I-27 in
[feature intake and tracker profiles](feature-intake-and-tracker-profiles.md)).

Trial 1 is not changed mid-run (T-15, I-22). Nothing here is built. Raw evidence stays in the
local trial case directory (T-22); this file carries only summaries.

## 1. Trial 1 status (2026-11-18)

Phase 1 steps 1 to 3 passed. Step 4 (identity across compaction, restart and fork) has not run
yet. Results are published by the trial report once the run ends (T-22), not here.

## 2. Trial 1 findings to fix

1. **Wakes that found nothing.** 3 of about 12 wakes led to an empty check: one Claude wake after
   the room had closed, and two Codex hints for items the seat had already fetched. Each costs a
   model turn. Suppress a wake when nothing is unread at delivery time.
2. **The device driver is never idle.** It uses about 1.2 s of CPU per hour with no traffic.
   Find what it wakes for and remove the timer if it isn't needed.
3. **The close doesn't tell the closer about the replacement.** Only the reconnecting side learns
   the new room ID.
4. **Logs to check before the report.** The core's own log was empty after the messaging, and
   phase 1 step 6 expects receipts there. The driver's log recorded only the Claude seat's sends, not
   the Codex seat's. Either may be by design; neither is explained yet.
5. **Codex defers MCP tools.** A Codex seat sees only the tool namespace until it searches, so
   its first "list your CBC tools" answer showed 3 tools and it then denied having the others.
   The setup step for a Codex seat must load the namespace before listing.
6. **Busy test command.** Claude Code blocks a plain foreground `sleep`. The script uses a
   Python sleep instead.
7. **`init` output** lists the protected database three times. Cosmetic.
8. **Codex re-checks after its own sends**, following the adapter's "check until the inbox is
   empty" instruction. Bounded by the inbox, so it is not polling, but the report must not count
   it as such.

## 3. What the next trial adds

| Change | Decision | What the trial must show |
|---|---|---|
| Chapter Masters delegate and stay free | I-15 | The Chapter Master spends its time briefing, answering and reviewing, and takes a second task while helpers run. Memory per helper is measured. |
| Start sequence: extension, rename, register | I-16, I-17 | Every new agent reads its extension, renames its chat and registers before other work, and the Primarch learns of it with no user relay. |
| Tracker events in the outbox | I-18, I-20 | Each event has its configured effect once (create, link or update, which may hit the same tracker record), no duplicate on retry, replay or restart, and pending or failed updates visible on the dashboard. |
| Tracker agent | I-19, I-21, I-23 | It updates the user's real tracker, keeps human edits, asks the Primarch before the user, and no implementing agent touches tracker tools. |
| Fixes from section 2 | — | No empty wakes, an idle driver, and the closer told about the replacement. |

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

Questions to settle before the trial is designed:

1. Main host or second host. Moving the host and the ADR-0013 role changes in the same trial
   changes two variables at once; phase 2 (T-30) is the baseline that makes the comparison
   possible.
2. Do T3's child threads satisfy I-15 (helpers on the host's native mechanism, the Chapter
   Master free while they run, the user able to talk to a helper directly)? CBC stays between
   leads and across hosts either way.
3. Does a T3 Codex thread see the user's connectors? Checked by asking one to list its tools.
4. Mid-turn delivery: a signed-in MCP client, queue only, or the upstream request.
5. A short peer smoke on T3 (like phase 1) before any hierarchy runs on it.

Not for the next trial: a plugin or native integration in T3. T3 has no plugin system today;
its supported route for outside tools is the MCP server.
