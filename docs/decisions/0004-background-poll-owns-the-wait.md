# ADR-0004 — The wait loop runs as a background poll; one identity owns the cursor

- **Status:** Accepted
- **Date:** 2026-06-08
- **Related:** [ADR-0002](0002-participant-identity-is-an-instance-token.md) (identity), [`docs/proposals/staleness-and-background-polling.md`](../proposals/staleness-and-background-polling.md)

## Context

The original waiting model made the agent *be* the polling loop: call `cbc_wait`,
read the result, decide whether to keep waiting (`paused_by_timeout` → wait
again; `retry_after` → sleep then wait), and repeat — every iteration dribbling a
tool call and its result into the agent's context.

This had two costs, which turned out to be one problem:

1. **It burned context and blocked the user.** Each empty poll consumed a turn;
   the human sat watching the agent loop, often nudging it ("check cbc", "reply").
2. **It produced stale conclusions.** When the agent stopped polling — or got
   `/compact`ed — delivered messages sat unread while the agent answered from its
   own lossy memory of the thread instead of the room. The canonical failure was
   an agent restating "PR in review" minutes after it had merged, with the
   updates unread in the room. *Manual polling produces the staleness:* stop
   polling = stop receiving = context silently diverges from the room.

## Decision

**The wait runs as a background `cbc poll`, and a single identity owns the read
cursor.**

- `cbc poll` (`bins/cbc/src/main.rs` `poll_until_event`) wraps the server wait in
  a loop that returns only on a *meaningful* event — a delivered message, a
  terminal room state, or a state needing a decision (`close_proposed`). It loops
  internally on `paused_by_timeout`, **through the pre-join window**
  (`awaiting_counterpart`, bounded by `--max-join-wait-secs`), and honors
  `retry_after`. The agent launches it as a background task and ends its turn; the
  harness re-invokes the agent when the poll exits with the message.
- **`--as` is required on `cbc poll`.** The poller advances the cursor, so it must
  carry the same identity (instance) the agent joins and sends with. One identity,
  one cursor.
- **Send stays foreground (MCP); wait goes background (CLI).** While a poll is
  running, the agent must never also call `cbc_wait` on the same identity —
  CAS delivery gives each message to exactly one claimant, so a second waiter
  would split the stream (split-brain).
- **Re-ground before replying.** On wake, the agent re-reads the whole room with
  `cbc_recap` (cursor-independent) and verifies external claims (git/gh) before
  acting — never recapping from memory. `cbc_recap` is the long-specified
  re-grounding affordance (the locked design's unbuilt `cbc_summary`).
- Because the poll keeps polling, the participant stays **live**, which suppresses
  spurious `counterpart_stale` and keeps it eligible to vote under
  [ADR-0003](0003-consensus-close.md).

## Consequences

- The agent is no longer the polling loop; the user is no longer the loop's
  supervisor. A conversation advances hands-free until a real event or a
  human-in-the-loop trigger (soft cap / signal).
- Identity discipline becomes load-bearing: a churned or absent `--as` splits the
  cursor and replays history. `--as` is required on `poll` precisely to prevent
  this.
- The wait surface is mildly split (send over MCP, wait over the CLI poll) and
  multi-room means one background poll per room. Acceptable in exchange for
  hands-free, context-cheap waiting.
- Harnesses without background-task re-invocation (some Codex/Cursor setups) fall
  back to a manual `cbc_wait` loop, keeping the same re-ground discipline. The
  `cbc_wait` MCP tool remains for them.
