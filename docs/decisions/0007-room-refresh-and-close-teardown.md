# ADR-0007 — Room refresh, close-teardown discipline, and the quorum-stall failure mode

- **Status:** Accepted
- **Date:** 2026-06-22
- **Related:** [ADR-0003](0003-consensus-close.md) (consensus close — the vote this teardown follows), [ADR-0004](0004-background-poll-owns-the-wait.md) (the background-poll shell that teardown stops), [ADR-0006](0006-coordination-modes-direct-and-orchestrated.md) (coordination modes — report lines and peer rooms are the most likely refresh targets)

## Context

Two operational problems appeared together in dogfooding:

**1. Lingering poll shells after a close.** After both agents voted to close a CBC room and
the room reached `closed`, background `cbc poll` shells and `/loop` heartbeats continued
running in the agents' terminals. The symptom: a shell that burns CPU and tokens every tick on
a room that is already dead.

Investigation confirmed the cause is two distinct failure modes, not one:

- **Mode A — `close_proposed` quorum stall (a code-level failure mode, not skill-level).** The
  `CloseQuorum::All` default requires *every live participant* to vote. A live participant is
  one whose `last_poll_at` is within `GHOST_AFTER` (15 min, `lifecycle.rs`). When an agent
  rejoins under a new identity — after a `/clear`, a fork, a fresh session, or a worktree
  `cwd` change — the server mints a new participant row instead of deduplicating; the old row
  retains a recent `last_poll_at` and continues to count toward the quorum denominator for up
  to 15 min. A two-party room with one such stale duplicate now has **3 live participants**;
  the two real agents together supply only **2 votes**; `needed = 3`; the room stays
  permanently in `close_proposed` and never reaches `closed`. The background poll correctly
  keeps running — the room is not closed.

  **Reproduced empirically (2026-06-22):** A room opened with 3 explicit participants (A,
  A-churn, B) received close votes from A and B. The close call returned `Close proposed
  (2/3)` and the room remained `active`. `cbc prune` with all three rows live confirmed
  `Pruned 0 ghost participant(s)` (prune only drops rows older than `GHOST_AFTER`). Forcing
  close restored the room to `closed`, confirming quorum was the gate.

- **Mode B — skill / operator gap (a teardown discipline failure).** The `/cbc` skill already
  described the consensus vote as the close step, but did not make it sufficiently clear that
  closing a room is **vote + teardown**, not just the vote. Agents reading the skill could
  rationally conclude that the background shell would stop itself — and `cbc poll` *does* exit
  when it observes the room go `closed`, but only if it is running. A `/loop` heartbeat (poll
  mechanism B in `/cbc`) keeps re-firing `cbc poll` every tick on a dead room regardless,
  because `/loop` itself has no knowledge of the room's state.

**2. Context pollution in long-lived rooms.** Report lines, peer rooms, and even short
reconcile rooms accumulate context over the course of a feature. When a new phase starts, the
history is noise. There was no skill-level protocol for moving a running two-party room to a
fresh slate without losing the thread — agents would start a new room and abandon the old one,
or try to close the old room before the counterpart was reachable in the new one.

## Decision

**Close teardown is vote + stop the machinery.**

`/cbc` (Closing) is updated to state explicitly:

- **If a close vote won't land** (room stuck in `close_proposed` even though both agents voted):
  the cause is almost always a stale duplicate participant. Recovery: `cbc prune <room>` drops
  aged-out rows (those past `GHOST_AFTER`), then re-vote `cbc_close`. The duplicate ages out
  automatically within 15 min regardless; `cbc prune` skips the wait. `--force` is not the
  reflex; it bypasses consensus and is human-only.
- Both modes of the background machinery must stop on close: `TaskStop` the background poll
  task (mechanism A) **and** end any `/loop` driving it (mechanism B). Every skill in the
  family gets consistent teardown language pointing to `/cbc` Closing for the stall-recovery
  detail.

**Room refresh is a bilateral protocol (`/cbc-refresh`).**

When a two-party room is context-polluted, the correct move is to open a new room, carry only
the durable conclusions as a carry-over summary, and relay the new room id **through the old
room** — it is the only relay channel to the counterpart. Both agents join the new room, and
**only then** both consensus-close + tear down the old room. Choreography, initiating side:

1. Open new room, join, post carry-over summary as opener.
2. Send new room id through the old room: `"refreshing → join me in <new-id>"`.
3. Wait for the counterpart to appear in the new room (confirm before touching the old room).
4. Optionally `cbc prune <old-room>` first (long-lived rooms are the most likely to carry a
   churn duplicate); then consensus-close old room and `TaskStop` the old poll shell / end the
   `/loop`.

The responding side must also tear down its own old poll shell — the initiator cannot stop the
responder's shell. This is the bilateral requirement: both sides tear down, every time.

**Core quorum fix is deferred to a follow-up.**

The root cause of Mode A is architectural: `CloseQuorum::All` + GHOST_AFTER interaction
when identity churn mints duplicate rows. The correct fix is a core semantics change
(`room.rs` / `http.rs` / `storage.rs`, possibly `sweeper.rs`) requiring RED-first tests.
It does not belong in a skills/docs increment. Candidate directions for the follow-up to
evaluate:

- **Auto-heal on read** — re-evaluate close quorum on the `close_proposed` path once a
  duplicate has aged past `GHOST_AFTER`, so the room heals without a fresh vote.
- **Two-live-max** — a strictly two-party room should cap the quorum denominator at 2 (the
  two most-recently-active participants). Matches the "2-live-max + ghosts" model PR #39
  chose; a third row should never block consensus.
- **Prune at vote time** — drop aged-out rows inside the close/extend count itself.

Note: PR #53 rejected active-quorum dedup keyed on `(repo, model, cwd)`. The fix must
lean on liveness/recency, not that key.

## Consequences

- Agents following the updated `/cbc` skill will `cbc prune` before the close vote in
  refresh scenarios (and know to prune + re-vote when stuck), eliminating the most common
  stall without waiting for the 15-min natural timeout.
- The `/cbc-refresh` skill gives agents a tested protocol for phasing a long-lived room
  without losing the thread or stranding a counterpart with no relay channel.
- The quorum-stall failure mode is documented and its operational mitigation is in the skill
  prose; the core fix is explicitly deferred. Agents hitting the stall have a path (prune +
  re-vote) and know not to use `--force` reflexively.
- Consistent teardown language across all 7 skills (`cbc`, `cbc-orchestrator`, `cbc-report`,
  `cbc-peer`, `cbc-recap`, `cbc-reconcile`, `cbc-refresh`) closes the skill gap for Mode B
  lingering shells.
