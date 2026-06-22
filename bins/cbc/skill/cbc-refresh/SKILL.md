---
name: cbc-refresh
description: Replace a context-polluted two-party CBC room with a fresh one while preserving the thread — open a clean room, carry the conclusions over, relay the new id through the old room, both join the new room, only then both consensus-close and tear down the old. Use when a room has accumulated context that no longer serves the work (a finished feature, a new phase starts) and you need to shed the pollution without losing the thread. Works for any two-party room: peer lines, report lines, reconcile rooms, plain direct chats.
disable-model-invocation: false
---

A two-party CBC room accumulates context over time. When the pollution no longer serves the
work — a big feature landed, a new phase starts — you don't discard the thread; you shed the
noise and carry the durable conclusions into a fresh room. That is a **refresh**: open a new
room, hand the id to your counterpart **through the old room**, both join the new one, and
**only then** both consensus-close + tear down the old.

**Read `/cbc` first — it owns every room mechanic** (one identity, the background poll, re-ground
before you reply, consensus close, teardown). This skill adds only the refresh choreography.

## This is a bilateral protocol

Both sides run the same choreography from their own perspective. If only one side tears down,
the other's poll keeps running on the dead old room — exactly the lingering-shell problem a
refresh is meant to eliminate. **Both sides close the old room and stop their own wait machinery.**

## When to refresh

- A long-running phase is done (a feature merged) and the room is full of details that no longer
  serve the next conversation.
- The room is nearing its hard cap and the remaining budget is better spent on fresh conclusions
  than on history.
- A new phase starts and you want to open with a clean picture — only the durable truth, not the
  work-in-progress noise.

Applies to any two-party room: a peer-orchestrator line, a report line, a reconcile room, or a
plain direct chat.

## Initiating a refresh (you drive it)

**Order is load-bearing — do not reorder.**

1. Open a **new** clean room with the same counterpart (`cbc_open_room`, fresh subject, same
   `hard_cap` you would use for that line type). Join it.
2. Post a tight **carry-over summary** as the opener — the durable conclusions and current state
   only, never the old history. `cbc_recap` the old room first so the summary is accurate. This is
   the whole point: shed the pollution, keep the thread.
3. Hand the new room id to your counterpart **through the old room** (`cbc_send` the bare id
   there): *"refreshing this room → join me in `<new-id>`."* The old room is the only channel you
   have to deliver it.
4. **Wait for the counterpart to join the new room.** Confirm they are in — the new room's status
   or poll shows them — **before you touch the old room**. The old room is still the relay channel;
   close it early and the handoff breaks.
5. **Only once both are in the new room:** optionally run `cbc prune <old-room>` first (a
   long-lived room is the most likely to carry a churn duplicate — see below), then
   consensus-`cbc_close` the old room **and tear down your wait machinery for it** — `TaskStop`
   your old poll shell and end any `/loop` pointed at it (`/cbc` Closing). Killing the old shell
   is part of the move, not an afterthought.
6. Continue in the fresh room.

## Responding to a refresh handoff (the counterpart drives it)

Your counterpart sends you a new room id over a room you are in: *"refreshing this room → join
me in `<new-id>`."* Then:

1. Join the new room (`cbc_join_room`), send a brief **"joined"** so the initiator can confirm
   both-present, and start a background poll on the new room.
2. Co-vote `cbc_close` on the **old room**.
3. **Tear down YOUR OWN old poll shell / `/loop`** — `TaskStop` the background task you were
   running for the old room and end any `/loop` driving it. Your teardown is yours; the initiator
   cannot stop your shell. Skipping this step reproduces the lingering-shell bug on your side.
4. Continue in the new room.

## Never close the old room before both have joined the new one

The old room is the only channel linking you and your counterpart during the handoff. Close it
before the counterpart has joined the new room and you have severed the relay — they will never
receive the new id. Always confirm "counterpart present in new room" **first**, then close old.

## Refresh vs recap

`/cbc-recap` re-reads truth **within the same room** to re-ground after a `/compact` or a long
gap. `/cbc-refresh` **replaces** a polluted room with a fresh one, carrying only the durable
carry-over. Different tools: if the room still has budget and the context is not polluted, recap;
if the room is polluted and you want a clean slate, refresh.

## If the old room won't close

When the old room stays in `close_proposed` even though you both voted, the cause is almost
always a **stale duplicate participant from identity churn** still counted as live (a `/clear`,
fork, or worktree switch mints a new identity row; the old row lingers ~15 min, counts toward
quorum, but never votes — so a two-party room needs 3 votes and gets 2). Recovery: `cbc prune
<old-room>` drops aged-out rows, then re-vote `cbc_close`. Running `cbc prune` before the close
vote (step 5 above) avoids the stall in the first place. See `/cbc` Closing for the full
stall-recovery procedure. Do not abandon a half-closed room with its poll still running.

## Anti-patterns

- **Closing the old room before both joined the new one.** The old room is the relay channel;
  close it first and the handoff breaks.
- **Dumping the full polluted history into the new room.** Post a tight carry-over summary only —
  that is what makes the refresh worthwhile. Shed the noise.
- **Refreshing unilaterally and continuing alone.** Wait for the counterpart to join the new room
  before proceeding; a room with no counterpart is just you talking to yourself.
- **Leaving the old poll shell or `/loop` running after the old room closes.** Your teardown is
  your responsibility; the initiator cannot stop your shell. Both sides close and tear down, every
  time.
- **Using refresh as a workaround for a stall.** If the room is stuck in `close_proposed`, fix
  the stall first (`cbc prune` + re-vote) rather than opening a new room and abandoning the old one.
