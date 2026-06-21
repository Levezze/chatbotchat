---
name: cbc-recap
description: Re-ground a RUNNING orchestrator when the board has drifted back into a mess or your own context is polluted — stop-order every room, pull fresh status from all of them, survive a `/compact` while the answers come back, then rebuild a clean picture from the rooms and the map. Use when the user invokes `/cbc-recap`, or asks the orchestrator to re-recap / reset / re-ground / start over mid-session.
disable-model-invocation: false
---

This is the **mid-flight reset** for an orchestrator that's already running. `/cbc-orchestrator`
grounds you at startup; this re-grounds you when things have drifted — the board is a mess
again, or **your own context is polluted/overlong** and you no longer trust the picture in your
head. It is built to survive a `/compact`: the rooms and the orchestration map hold the truth,
so your session can be reset to a fresh, current context without losing the board.

It **layers on `/cbc-orchestrator`** (same role, same map, same single-ownership discipline) and
on `/cbc` (every room mechanic). It doesn't replace them — it deliberately re-runs the grounding
loop from scratch.

## The procedure

1. **Stop the board first.** Send a hold into **every** room you hold: *"Orchestrator
   re-grounding. Pause implementation and hold — don't write more code or decide anything. Send
   me a fresh status, then wait."* Freeze everything so the board stops moving while you rebuild
   — re-grounding against agents that are still implementing just reproduces the mess.
2. **Ask all of them for fresh status.** Request each agent's *current* state — where it sits in
   its sequence, surfaces it's touching, blockers, and **what changed since the last grounding**.
   Concise status, not a code dump. Fire these into every room, then let the background polls
   collect the answers.
3. **Now you can `/compact` — that's the point.** With the stop-and-status request already out,
   the truth lives in the rooms and the map, not your context. While the answers come back, the
   user can compact you (or `/clear` and resume): you'll lose the polluted in-head picture but
   **not** the rooms (your polls re-attach by session identity) and **not** the map on disk.
   *Tell the user this is a safe moment to compact* if their context is heavy.
4. **Rebuild from scratch — from the rooms, never from memory.** Once the fresh statuses are in
   (and after any compaction), `cbc_recap` every room and re-read the orchestration map, then
   reconstruct the picture from those alone. Do **not** trust any pre-compact recollection.
   Verify external claims (merged / deployed / contract is now X) against `git`/`gh` as always.
   Overwrite the map with current truth (create one if there isn't yet).
5. **Reprint the deterministic recap, then release.** Print the same clean "stop to breathe"
   board recap `/cbc-orchestrator` defines (roster + per-agent sequence + collisions /
   merge-order). Then hand each held agent its single, clear responsibility and release the holds
   one by one. You're now orchestrating from a fresh, current context.

## Why this works

A long orchestration session accretes stale, half-true context — the exact thing `/cbc` warns
against ("re-ground from the room, not memory"). Polluted context is *worse* than no context: it
makes confident wrong calls. `/cbc-recap` throws the polluted picture away on purpose and
rebuilds from the two durable sources of truth — the live rooms and the on-disk map — so a reset
session is not a setback but a clean restart.

## Anti-patterns

- **Re-recapping from your polluted memory** instead of `cbc_recap` + the map. That just re-launders
  the mess you're trying to clear.
- **Compacting before you've sent the stop + status request.** Then you wake to a fresh context
  with no answers in flight and nothing asked — re-grounding stalls.
- **Skipping the freeze.** Rebuilding while agents keep implementing reproduces the drift.
- **Releasing agents before the rebuild and reprint.** Ground fully, print the recap, *then* hand
  out single responsibilities.
