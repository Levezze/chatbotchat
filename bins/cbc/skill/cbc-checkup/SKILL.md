---
name: cbc-checkup
description: Run a sweep of all worker rooms and flag any that have gone dark. A healthy cbc poll refreshes its participant every ~50 s — workers whose seconds_since_poll climbs past ~150 s have likely let their poll die. Use when the orchestrator's checkup timer fires (CHECKUP_TICK) or when the user manually invokes /cbc-checkup. Also instructs the orchestrator to keep the timer armed while work is live.
disable-model-invocation: false
---

The checkup is the orchestrator's **fallback heartbeat** — a periodic sweep that catches
workers whose poll has died without the orchestrator noticing. It does not replace the
primary per-room `cbc poll`; it backs it up.

**Read `/cbc` and `/cbc-orchestrator` first.** This skill adds only the sweep procedure
and the timer state machine on top of the orchestrator's existing rule set.

## Why a fallback heartbeat exists

Workers run on short-context models and sometimes let their background poll die between
turns — no crash, no explicit signal, the poll just stops refreshing. The orchestrator's
per-room polls fire `counterpart_stale` the moment a worker's poll drops past the server's
15-min ghost window, waking the orchestrator event-driven. But *before* that 15-min window
closes, a dead worker is invisible. The checkup catches that gap cheaply: a single
`cbc_status` call per room is **read-only and free** (no message, no cap burn); it reads
the server-stamped `seconds_since_poll` value added in the `cbc_status` response for each
participant and detects a dead poll within one tick (~5 min) instead of 15.

**Pull-only constraint.** Because CBC is pull-only, a dead worker cannot be resurrected by
the orchestrator sending it a message — it can't receive one. The checkup *detects and
escalates*; the human is the only actor who can reopen a dead worker's chat. The
escalation message tells the user precisely who to reopen and why.

## The sweep — one tick

**Input: the `agents:` registry in the orchestration map file.** Each registry entry is one
worker room. Do not sweep rooms not in the registry.

For each room:

1. `cbc_status <room-id>` — free read, no message, no cap burn. Find the worker
   participant's entry; read `seconds_since_poll` and `stale`.
2. **Classify:**
   - `seconds_since_poll < 150` → **alive** (`✓`) — poll fresh, no action.
   - `seconds_since_poll` small but no new message since last tick → **quiet** — the poll
     is alive; a worker mid-tool-call has a live parked poll and will answer late. Do NOT
     escalate on quiet alone; the poll is the signal, not message frequency.
   - `seconds_since_poll ≥ 150` OR `stale: true` → **dark** (`⚠dark`) — poll has likely
     died (≥3× the poll cap) or the server has confirmed a ghost (>15 min). **Escalate.**
3. **Update the board marker** in the orchestration map (`✓` / `quiet` / `⚠dark`).
4. **Determine change vs no-change** for the backoff:
   - **change** = any marker transitioned this tick, OR any room's message count increased.
   - **no-change** = all markers held, no new messages anywhere.

**Escalation line (dark workers only):** name the worker by its registry label, its
subject, the server-authoritative silence ("`seconds_since_poll` = Ns ≈ Mm ago"), and
whether it is **newly dark** or **continuing dark** (diff against the prior board marker
— `✓`/`quiet` → `⚠dark` is newly dark; `⚠dark` → `⚠dark` is continuing). Do NOT track
tick-counts in memory (compaction erases it) — the board marker is the durable state. Tell
the user explicitly: *"worker chatbotchat-worker-auth last polled 6m ago — poll dead; I
can't reach it. Reopen its chat / relaunch it?"*

**Zero messages by default.** Never `cbc_send` a probe to every room every tick — it burns
the 20-message hard cap and bricks the room if the worker is dead (reviving needs a
co-vote from the dead worker). The ONLY exception: a single `cbc_send "status?"` to a
specific `quiet` room when the user explicitly requests a fresh status line. Escalation of
a dark worker is a message to the USER, not to the room.

**Update the board and re-arm the timer** as the last step of every sweep that stays armed
(see state machine below). The only sweep that does not re-arm is the one that transitions
to dormant — and that one MUST announce dormancy first.

## Backoff state machine

The checkup is the **fallback**, not the primary detector. When nothing is moving, it
backs off rather than burning tokens on a still pond. Each worker's own per-room poll still
watches for `counterpart_stale` event-driven; the checkup's job is the gap up to the 15-min
ghost window.

Durable state (board-backed — survives compaction):
```
checkup-level: 0          # 0=5m | 1=10m | 2=20m | dormant
no-change-streak: N       # consecutive no-change ticks at the current level
```

**Levels:**
- `0` — 5 min interval. After **3 consecutive no-change ticks** (~15 min), escalate to
  level 1 (reset streak to 0).
- `1` — 10 min interval. After **1 no-change tick** (~10 min), escalate to level 2.
- `2` — 20 min interval. After **1 no-change tick** (~20 min), go dormant.
- `dormant` — no ticks. The sleep shell is NOT relaunched.

Total stillness before dormancy ≈ 15 + 10 + 20 = **~45 min**.

A **change** tick always: (a) resets the streak to 0 at the current level, AND (b) resets
`checkup-level` all the way back to 0 (base interval), whether the change was a marker
transition or a new message in any room. Activity always restores the full sensitivity.

On a **no-change** tick:
1. Increment `no-change-streak`.
2. If the streak reaches the level's threshold: escalate level (or go dormant), reset
   streak to 0.
3. Re-arm the sleep shell at the new (or same) level's interval.

**Go dormant only after announcing it.** The sweep that decides to go dormant must first
tell the user: *"All N workers have been idle with no movement for ~45 min — pausing
checkups to save tokens. I'll restart automatically when any worker sends a message or you
reopen one."* Then do NOT relaunch the sleep shell. Dormancy that is invisible from the
user side is indistinguishable from the original hang bug.

## Revival from dormant

The checkup revives when:
- The orchestrator's **per-room poll** delivers a real message (not a timeout) from any
  worker. On processing that message, re-arm the checkup at level 0 before composing the
  reply.
- The **user** restarts or relaunches a worker, or tells the orchestrator new work is
  starting.
- The user **manually invokes** `/cbc-checkup` — this runs a sweep and, regardless of the
  dormancy state, re-arms the timer at level 0.

When reviving from dormant, say so briefly: *"Checkup restarted at 5 min."*

## The sleep shell

```bash
# arm the timer (run this at the end of every non-dormant sweep)
sleep <interval_secs>; echo CHECKUP_TICK
```

Run it with Bash `run_in_background`. On exit it wakes the orchestrator → the orchestrator
sees `CHECKUP_TICK` in the task output → runs this sweep → re-arms or goes dormant. The
`CHECKUP_TICK` marker is self-identifying: a post-compaction orchestrator that sees it in
its background task output knows to run a sweep without needing to remember what it was
waiting for.

Interval in seconds: level 0 = 300, level 1 = 600, level 2 = 1200.

**Relaunch the shell on every wake, BEFORE composing any reply** — just as polls are
relaunched before composing. The orchestrator may spend several minutes writing a reply;
the timer must be running the whole time. If the checkup shell is dead when a poll wakes
the orchestrator (e.g. from a crash), and the orchestrator is not dormant, relaunch it
immediately (idempotent — a brief overlap with any surviving shell is fine).

**One shell, not many.** Do not let multiple checkup sleep shells accumulate. If the prior
shell's label is visible in the task list, `TaskStop` it before relaunching.

## Manual `/cbc-checkup`

Invoking `/cbc-checkup` manually:
1. Runs one sweep immediately (full procedure above).
2. Resets `checkup-level: 0` and `no-change-streak: 0` regardless of current state
   (including dormant).
3. Re-arms the sleep shell at the base interval (300s).

This is how the user forces the orchestrator out of dormancy or demands an immediate status
read.

## Anti-patterns

- **Message-probing every room every tick.** Burns the hard cap. A dark worker can't
  receive a probe anyway. Use `cbc_status` — it's free.
- **Escalating on `quiet`.** The poll is alive; the worker is mid-tool-call. Only escalate
  on `dark` (poll dead or ghost).
- **Going dormant without announcing it.** Invisible dormancy = original hang bug from the
  user's perspective. Always tell the user before the last tick.
- **Forgetting to re-arm after a change-tick.** A sweep that saw a change must still
  relaunch the shell — change resets the level and streak but does not skip the re-arm.
- **Tracking tick-counts in memory.** They compact away. All counters live in the board
  file (`checkup-level` / `no-change-streak`).
- **Keeping the checkup at 5 min forever.** The backoff exists to avoid burning tokens on
  a still pond. If nothing is moving, back off; the per-room polls are still watching.
