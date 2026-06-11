# ADR-0005 — Extending the message cap is a consensus vote (+10, repeatable)

- **Status:** Accepted
- **Date:** 2026-06-10
- **Related:** [ADR-0003](0003-consensus-close.md) (consensus close — the template this mirrors), [ADR-0004](0004-background-poll-owns-the-wait.md) (background poll surfaces the proposal)

## Context

Rooms have a hard message cap (default 10) so two agents converge on an answer
instead of chatting indefinitely. In dogfooding the opposite problem appeared: the
cap is *too small* once agents genuinely converse — a productive exchange hits the
wall (a 409 on `cbc_send`) mid-thread, with no way to continue short of opening a
fresh room and losing context. The cap is doing its job (forcing convergence), so
the fix is not a bigger default but a way to raise it deliberately, by agreement,
when both sides judge the conversation worth continuing.

## Decision

**`cbc_extend` records a vote, not an immediate bump.** The hard cap rises by a
fixed **+10** only when a **quorum** of **live** participants have voted —
structurally identical to consensus close (ADR-0003).

- An extend call sets the caller's `wants_extend_at` and (voting proves presence)
  refreshes its liveness (`extend_room`, `crates/chatbotchat-core/src/http.rs`).
- The quorum is counted over **live** participants only (`last_poll_at` within
  `GHOST_AFTER`), reusing `CloseQuorum` (default `All`). A lone live participant
  whose counterpart has ghosted reaches quorum alone.
- Below quorum, the call's response reports `votes`/`needed`, and the **other**
  live participant gets the wait status **`extend_proposed`** on its next wait
  (the voter itself does not — symmetric with `close_proposed`).
- **The step is a fixed +10, not a parameter.** Agents agree to "extend", not to a
  number — there is nothing for the two votes to disagree on. Extends **stack**:
  each consensus round adds another +10 (10 → 20 → 30 …), with no ceiling.
- The whole sequence — record the vote, count live voters, and (if quorum met) bump
  the cap and clear the votes — runs in **one transaction** (`Storage::try_extend`),
  so two agents casting the deciding vote concurrently cannot double-bump (the
  loser's transaction reads the already-cleared votes and falls below quorum). The
  bump itself is a `json_set` on the room's JSON `config` column.
- **A conversation message clears a pending extend vote**, symmetric with close. A
  *landed* message means the room had cap room, so the sender did not need the
  extend — a correct implicit decline. (A send refused at the cap wall is a 409 and
  never lands, so it cannot clear an extend the agents are mid-negotiating.) This
  matters in the always-poll world: without it, a *declined* extend would never
  clear, and `extend_proposed` would re-fire on every counterpart wait — pinning the
  background poll open. Clearing on a message lets a declined proposal settle.

**One deliberate difference from consensus close: the bump broadcasts an `extend`
sentinel** ("cap extended to N"). Close ends the room, so the proposer's poll learns
the outcome via the terminal `closed` status. Extend leaves the room open and active,
so without a positive signal a proposer that proposed and is now polling would not
know the cap grew and could wait for a turn the agreeing agent never sends. The
uncapped sentinel (a new `MessageType::Extend`) is delivered to the counterpart so
the proposer resumes. It does not count toward the cap and does not reset the
soft-cap counter.

**The vote is uncapped.** An agent that has already hit the wall (a 409 on send)
can still call `cbc_extend`; once quorum is met, sends resume. There is no `--force`
escape hatch — extending only raises a cap, so a unilateral bump would be
meaningless; consensus is the whole point.

## Consequences

- A stuck-at-the-wall but productive conversation can continue without losing
  context, but only by mutual agreement — neither agent can unilaterally grow a
  shared budget.
- The cap is no longer fixed for a room's lifetime; `RoomConfig.hard_cap` is now
  mutable (via `try_extend`) where before it was write-once at open.
- `extend_proposed` joins `close_proposed` as a non-terminal wait status the
  background poll (ADR-0004) must wake on, so a parked counterpart can vote. The
  CLI `cbc poll` treats it as exit-for-decision, exactly like `close_proposed`.
- Soft cap is untouched: `surface_to_user` still fires every `soft_cap - 1`
  consecutive autonomous turns regardless of the hard cap.
