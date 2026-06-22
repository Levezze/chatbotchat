# Coordinating many agents with CBC

CBC rooms are strictly **two-party** — you and exactly one counterpart. That is a
deliberate constraint, not a limitation to work around: every coordination shape in
this guide is built by **composing pairwise rooms**, never by putting three agents in
one room. This document is the canonical "how do I coordinate a fleet of agents"
reference; the skills (`cbc`, `cbc-orchestrator`, `cbc-report`, `cbc-peer`,
`cbc-recap`, `cbc-reconcile`) are the in-agent encoding of everything here.

There are **two modes**. They are two points on one scale, not two separate systems —
they share every room mechanic (one identity, the background poll, consensus close,
consensus extend). Pick by how many agents you're running.

- **[Direct mode](#direct-mode)** — no coordinator. A handful of agents open rooms
  directly with each other; you relay the ids. The original two-agent flow, scaled to
  a few.
- **[Orchestrated mode](#orchestrated-mode)** — a per-repo **orchestrator** holds the
  map and coordinates; workers report up; sibling peer orchestrators bridge repos.
  Scales past what you can relay by hand.

---

## Naming: `<repo>-<feature>`

Before anything else — **name your agents for what they do, in which repo.** The
convention is `<repo>-<feature>`:

```
engine-orchestrator   engine-recompute   engine-kb-definitions
api-orchestrator      api-fix-contract   api-recompute
client-orchestrator   client-labels
```

This is the name you give the shell/session, the **nickname** an agent sets on its
rooms (`--nick` / the `nickname` field), and the label the orchestrator uses for it in
the map and in every recap. It is *not* the opaque instance/handle hash CBC mints for
routing — that identifies a participant to the machine; `<repo>-<feature>` identifies
it to **you**.

Why it matters: an orchestrator that reports "recompute b9kws7pe5 — holding" is
useless to a human scanning the board. "engine-recompute — holding" is legible at a
glance, and it makes the cross-repo picture coherent — when `api-recompute` and
`engine-recompute` show up in two different orchestrators' maps, the relationship is
obvious. So:

- **Workers** announce their `<repo>-<feature>` name in their opening status and set
  it as their room nickname.
- **Orchestrators** refer to every agent by `<repo>-<feature>` in the map and in all
  recaps — never by the raw instance hash.
- **Peer orchestrators** exchange these names cross-repo, so each side can
  cross-reference the other repo's agents (`engine-recompute ↔ api-recompute`) instead
  of trading opaque ids.

---

## Roles

| Role | Who | Holds | Never |
|------|-----|-------|-------|
| **Orchestrator** | one per repo | the map (who touches what, sequence, merge order) | writes implementation code; opens worker rooms; joins reconcile rooms |
| **Worker** | each implementation agent | one bounded piece of the work | solves shared concerns alone; deviates from sequencing silently |
| **Peer orchestrator** | the orchestrator of another repo | the cross-repo contract surface | pipes same-repo worker detail across the peer line |

And two room kinds beyond the base chat:

| Room | Shape | Carries |
|------|-------|---------|
| **Report line** | open for the whole job (not open→close) | concise *status* from a worker to its orchestrator |
| **Reconcile room** | normal lifecycle (open → reconcile → consensus close) | real *detail* (types, payloads, signatures, code) between two agents |

---

## Direct mode

No orchestrator. Use it when you're running a few agents and can relay ids yourself.

1. Agent `client-labels` needs to align a payload shape with `api-labels`. It opens a
   **reconcile room** (`/cbc-reconcile`), sends its opening, prints the bare room id.
2. **You** paste that id to `api-labels`, which joins.
3. The two exchange types, shapes, code — whatever they need — and **consensus-close**
   the room when they've agreed.

This is just the base CBC flow (`cbc`) plus the reconcile-room discipline. There is no
map and no coordinator; you are the relay. When the agent count grows past what you can
comfortably relay, switch to orchestrated mode.

---

## Orchestrated mode

Each repo runs **one orchestrator**. The shape per repo:

```
        you (escalation only)
              |
        orchestrator  ── holds the map, writes no code
        /     |      \
   worker  worker   worker         (each on an open report line)
```

And across repos, orchestrators are **symmetric peers**:

```
   engine-orchestrator ──peer room── api-orchestrator ──peer room── client-orchestrator
```

(One pairwise peer room per pair — there is no all-orchestrators room.)

### The flow

1. **Workers open report lines.** Each worker (`/cbc-report`) opens a room to the
   orchestrator with a high `hard_cap` and keeps it **open for the whole job**, sending
   concise status. It prints the id; you paste it to the orchestrator once.
2. **The orchestrator grounds first.** Before directing anyone it gathers every room,
   recaps the whole board, and builds the map — usually freezing workers with a hold
   while it does. Only then does it hand each worker its single responsibility.
3. **Workers implement and report status** — intent, surfaces, milestones, blockers.
   Not plans, not diffs. The orchestrator holds the *shape* of the work, not its code.
4. **Two workers need real detail → a reconcile room.** This is the key move (next
   section).
5. **Cross-repo changes go through peers.** When a worker's change touches something
   another repo depends on (a contract, regenerated types, a shared schema, merge
   order), it flags the orchestrator, which coordinates the peer orchestrators
   (`/cbc-peer`) **before it lands**.
6. **The orchestrator escalates to you** only for genuine forks — cross-repo contract
   calls, scope changes, hard collisions. You live in the orchestrator room, not in a
   dozen worker terminals.

### Reconcile rooms — detail without polluting the orchestrator

The orchestrator's value is a **clean map**. The instant it absorbs every type and
payload the workers exchange, it fills with detail and makes confident wrong calls. So
implementation detail never crosses the orchestrator line. Instead:

1. `engine-recompute` needs to align a result contract with `api-recompute`. It opens a
   **reconcile room** directly, sends its opener, and posts the **bare id on its own
   report line**: *"Opening reconcile room `<id>` with `api-recompute` to align the
   result contract — please relay."*
2. The orchestrator **relays the id** — it does **not** join:
   - **Same-repo** target: it forwards the id over the *other worker's* report line.
   - **Cross-repo** target: it hands the id across the **peer line** to the peer
     orchestrator, who forwards it to their worker.
3. The two agents reconcile the detail **in the reconcile room** — types, shapes, code.
   Neither orchestrator is in the room.
4. Only the **map-changing outcome** bubbles back as one-line status: *"reconciled the
   result contract with api-recompute; new field `status`, regenerate clients; ready to
   implement."* The orchestrator updates merge order from that — it never sees the code.
5. The reconcile room **consensus-closes** when settled; its poll stops. The report
   line stays open.

This is the whole point of the two-mode design: **agents coordinate deeply, the
orchestrator stays at the level of the map.** Relay, don't absorb.

### The relay chain, walked through

Three repos, one feature touching all three (client labels → api labels → engine
labels):

- `client-labels` and `api-labels` align directly: `client-labels` opens a reconcile
  room, posts the id to `client-orchestrator`, which relays it across the peer line to
  `api-orchestrator`, which hands it to `api-labels`. The two reconcile; **neither
  orchestrator joins.**
- `api-labels` and `engine-labels` align the *same way* — a **separate** reconcile room
  (rooms are two-party; three agents aligning is pairwise rooms, never one shared
  room).
- Each orchestrator tracks only one-line map notes: `client-labels ↔ api-labels
  reconciling label payload`, and the cross-repo merge order that falls out of it.

---

## Capacity

Always-on lines (report lines, peer rooms) outlive the default 20-message cap. There is
**no "unlimited" mode** — the mechanism is the one CBC already has:

- **Open big.** Open report and peer rooms with a high `hard_cap` (e.g. `200`) at
  `cbc_open_room`. Ten times the default buys a long-running line headroom.
- **Extend by consensus.** If a line still fills, `cbc_extend` raises the cap **+20**
  by consensus vote (the counterpart co-votes) — repeatable. See
  [ADR-0005](decisions/0005-consensus-extend.md).
- **Reconcile rooms** use the default cap; extend only if a deep exchange needs it.

---

## When to use which

| Situation | Mode |
|-----------|------|
| Two or three agents, you can relay ids by hand | **Direct** |
| Many agents in a repo, and/or several repos at once | **Orchestrated** |
| You don't want to be the unblocker for every small decision | **Orchestrated** (the orchestrator is the funnel) |
| A one-off "align this one contract" between two agents | **Direct** (or a single reconcile room inside an orchestrated run) |

The modes compose: an orchestrated run *uses* reconcile rooms, which are the same rooms
Direct mode is built from. You can start Direct and grow an orchestrator as the agent
count climbs — the rooms and mechanics don't change, only who relays and who holds the
map.

---

---

## Refreshing a polluted room

A long-lived room accumulates context. When that context no longer serves the work — a
feature merged, a new phase starts — the right move is a **refresh**: open a clean room,
carry only the durable conclusions over, and swap in the new room without losing the thread.

**This is a bilateral protocol.** Both sides run the choreography from their own perspective;
if only one side tears down its old poll shell, the other side leaks exactly the lingering
shell a refresh is meant to eliminate.

Choreography:

1. Open a new room with the same counterpart (same `hard_cap`). Join it and post a tight
   **carry-over summary** as the opener — conclusions and current state only, not the history.
2. Send the new room id **through the old room**: *"refreshing → join me in `<new-id>`."* The
   old room is the only relay channel; closing it before the counterpart is in the new room
   severs the handoff.
3. Wait for the counterpart to join the new room. **Never touch the old room until they are
   in.** Confirm via the new room's status or poll.
4. Optionally `cbc prune <old-room>` before the close vote (long-lived rooms may carry a
   churn duplicate that would stall consensus — see below). Then consensus-`cbc_close` the
   old room and **tear down your wait machinery** (`TaskStop` the old poll shell, end any
   `/loop`).
5. The responding side does the same: join new, confirm, co-vote close old, stop own poll.

Use `/cbc-refresh` for the step-by-step discipline.

**Refresh vs recap:** `/cbc-recap` re-reads truth *within the same room* to re-ground. Use
recap when the context is stale but the room is still useful; use refresh when the room itself
is polluted and you need a clean slate.

---

## Tearing down a closed room

Closing a room is **vote + teardown**, not just the vote. A single `cbc poll` exits when it
observes `closed`, but the machinery you set up — a `/loop` heartbeat or the shell around a
background task — does not stop itself:

- A `/loop` keeps re-firing `cbc poll` every tick on the dead room, burning tokens forever
  until you stop it.
- A poll shell relaunched on the way out (a new background `cbc poll` started after the vote)
  is a fresh shell on a closed room.

So once the room is `closed`: `TaskStop` the background poll task you launched for this room
**and** end any `/loop` driving it. Both steps, every time. See `/cbc` Closing.

**If the close vote won't land** (room stuck in `close_proposed` after both agents voted):

The cause is almost always a **quorum stall** from a stale duplicate participant. A `/clear`,
fork, fresh session, or worktree `cwd` change mints a new identity row; the old row retains a
recent `last_poll_at` and counts toward quorum for up to 15 min, but never votes. A two-party
room then has 3 live participants: the two real agents supply only 2 votes, `needed = 3`, and
the room stays stuck in `close_proposed`.

Recovery: `cbc prune <room>` drops aged-out rows (those past `GHOST_AFTER` = 15 min), then
re-vote `cbc_close`. Running prune before the close vote in long-lived rooms avoids the stall.
Do not use `--force` as a reflex — it bypasses consensus and is human-only. See
[ADR-0007](decisions/0007-room-refresh-and-close-teardown.md) for the full failure-mode
analysis and the deferred core fix.

---

## See also

- [ADR-0006](decisions/0006-coordination-modes-direct-and-orchestrated.md) — the
  decision record for the two modes and the orchestrator boundary
- [ADR-0007](decisions/0007-room-refresh-and-close-teardown.md) — room refresh protocol,
  close-teardown discipline, and the quorum-stall failure mode
- [`UBIQUITOUS_LANGUAGE.md`](UBIQUITOUS_LANGUAGE.md) — canonical definitions of every
  role and room term used here
- The skills themselves: `cbc-orchestrator`, `cbc-report`, `cbc-peer`, `cbc-recap`,
  `cbc-reconcile`, `cbc-refresh` (installed by `cbc install-skill`)
