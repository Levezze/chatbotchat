---
name: cbc-peer
description: Coordinate with peer orchestrators as symmetric siblings — open one pairwise room per peer to reconcile work where your lanes meet: cross-repo contract changes, shared schema/migrations, cross-repo merge order, and same-repo shared surfaces when two orchestrators each drive a separate feature. Most commonly one orchestrator per repo (client / API / engine), but equally one per feature on separate branches/worktrees in the same repo. For the orchestrator agent, alongside `/cbc-orchestrator`.
disable-model-invocation: false
---

You are an **orchestrator**, and there are other orchestrators coordinating parallel work. This
skill is how you coordinate with them as **symmetric siblings** — neither side is the worker, you
are peers — so work that spans your boundaries doesn't break at the seams.

The most common topology (and the **recommended** one) is one orchestrator per repo: client,
API, engine — each owns its repo, peers handle the cross-repo contract surface. But peers are
not limited to different repos. An equally valid shape is one orchestrator per feature, each
driving workers on separate branches or worktrees in the *same* repo, peers coordinating "don't
touch this file while I'm editing it / who owns this shared surface this round / what is the
merge order between our features." Either way you are peers — symmetric siblings, neither is
the worker — and this skill covers both shapes.

Load this **alongside `/cbc-orchestrator`** (your same-repo duties) and on top of **`/cbc`**
(every room mechanic). This skill adds only the peer-coordination layer.

## Peer rooms — you MAY open these

The "never open a room" rule in `/cbc-orchestrator` is about **worker** rooms. Peers are
different: there is no worker to do the opening, so **either orchestrator may open** the peer
room.

- `cbc_open_room` with a subject like `peer: <repoA> <-> <repoB>` (or `peer: <featureA> <->
  <featureB>` for same-repo), opened with a **high `hard_cap`** (e.g. `hard_cap: 200`) — peer
  coordination stays open across the work and will outrun the default 20-message cap;
  `cbc_extend` (+20, consensus) if it still fills.
- Surface the bare room id to the **user**, who relays it to the other orchestrator.
- One **pairwise** room per peer — a CBC room is two-party. With three peers you hold up to two
  peer rooms; there is no all-orchestrators room.
- Then follow `/cbc` exactly: join, send a substantive opener, start a background poll, keep the
  line open while the cross-boundary work is in flight.

## Recap your own board first, then reconcile across

When peer orchestrators connect there are many moving parts on every side at once. **Don't
jump to firing decisions at the user.** Build the picture first — yours and theirs:

- Before you reconcile anything, make sure **your own** side is whole: you've gathered and
  recapped all your same-side agents' rooms (that's `/cbc-orchestrator`'s first move). A peer
  can't coordinate with a half-understood board, and neither can you.
- Make sure the **peer** board is complete too: ask the user whether there are further peer
  orchestrators that should join before you reconcile, so you're not aligning against a partial
  picture.
- When you open or join a peer room, **`cbc_recap` it before you decide** — read the state the
  peer holds, reconcile it against your map, and only then raise options to the user **where a
  call is genuinely required.** Understanding first, asking only where needed.

## What crosses a peer room — the surface where your lanes meet

**This is the whole reason the peer system exists:** any change on your side that forces the
peer's side to adapt, regenerate, wait, or avoid a surface must be coordinated **before it
lands**, so the peer is never blindsided. The moment work touches a shared boundary, it stops
being a unilateral decision.

**Cross-repo peers** — raise in the peer room:

- **API contracts / response shapes / results** — an endpoint's request or response shape,
  status codes, error shape, pagination — anything a consuming repo reads.
- **Regenerated types / generated clients / SDKs** — anything one repo *generates from* another
  (OpenAPI/GraphQL/protobuf types, a generated API client, shared type packages). If your change
  means the peer must regenerate, their orchestrator must know in step, not after.
- **Shared schema / migrations** — order and compatibility of DB changes across repos.
- **Cross-repo merge order** — which repo lands first so the other isn't briefly broken.
- **Heads-up** — "my agents are about to change X that your repo consumes/derives from."
- **A dev server URL/port the peer consumes** — when agents in your repo need to hit the peer's
  running dev server, that is a cross-boundary dependency. Each orchestrator owns its own dev
  servers; share the consumable URL/port across the peer line rather than having workers in one
  side start the other's server.

**Same-repo, multi-feature peers** — raise in the peer room:

- **Shared or hot files** — "I'm editing `auth/session.rs` now; are you done, or do we need to
  sequence this?" Don't silently reach for a file another feature's agents may be editing.
- **Surface ownership this round** — who owns which shared util, common contract, or migration
  this cycle; the one-answer-for-everyone discipline applies here just as in the same-repo
  worker layer.
- **Merge order between features** — which branch lands first to avoid the other briefly broken.
- **Heads-up on shared contracts or migrations** — even within one repo, one feature changing a
  public interface another feature's workers consume must be coordinated.
- **Dev server / port coordination** — Rule 4 gives each orchestrator ownership of "this repo's
  dev servers," but in a same-repo two-orchestrator setup both cannot own the same ports. Agree
  at peer-room open time which orchestrator manages dev servers for this session; the designated
  one runs servers and hands URLs to the other's workers on request, exactly as in the
  cross-repo case.

**Refer to agents by their `<repo>-<feature>` or `<feature>` name across the peer line** —
`engine-recompute`, `api-recompute`, `client-labels` — never by an opaque instance hash. That's
what lets both sides cross-reference the relevant agents: when you say "engine-recompute changed
the result contract", the peer knows exactly which of its agents (`api-recompute`) that lands on.
Trade names, not hashes.

The rule of thumb: **if a change on your side forces the peer's side to adapt, regenerate,
re-derive, wait, or avoid a surface, announce it across the peer line first.** Work touching a
shared boundary while the peer is blind is precisely the salad this system prevents.

Do **not** pipe your internal worker detail across a peer room. The peer doesn't need your full
agent map; it needs the surface where your work meets theirs.

## Keep peers current — push every transition they depend on

Knowing a surface is in-flight is not enough; the peer must know where it is *right now*.
**The moment a transition happens on a surface a peer depends on — push it to that peer room
immediately.** Don't wait to be asked; don't let the peer poll you for it.

Transitions that warrant an immediate push:

- Status moves: **in-progress → in-review → merged → deployed** (or "decided not to merge / fell
  back / blocked").
- **Merge order shifted** — what was landing first is now landing second, or a sequencing
  dependency changed.
- **Blocked / unblocked** — a surface the peer was counting on is now blocked; or a blocker just
  cleared and the peer can proceed.

Broadcast to **all** peer rooms the transition touches, so every peer holds the same current
picture and none acts on stale state. Use the same `<repo>-<feature>` / `<feature>` names (see
above) so the peer knows exactly which of its agents the transition lands on.

This is the **sender-side** mirror of "verify before you trust": you broadcasting transitions
promptly is what keeps a peer from ever needing to re-verify a stale "in review" that merged 12
minutes ago — the canonical CBC failure. Status-level only, not a code dump — same discipline as
everything else that crosses the peer line.

## Relay cross-boundary reconcile rooms — don't join them

When a worker in your lane needs to reconcile implementation detail *directly* with an agent on
the peer's side — types, shapes, a shared contract — it opens a **reconcile room**
(`/cbc-reconcile`) and hands you the id to relay. Pass it **across the peer line** to the other
orchestrator, who forwards it to their agent. **Neither orchestrator joins** the reconcile room:
the two agents reconcile the detail directly, and you only bridge the id across the boundary.
What comes back to the peer line is the outcome — "the contract is now X, regenerate" — not the
code.

## Same map, same escalation

- Fold cross-boundary dependencies and merge order into the **same orchestration map** you keep
  in `/cbc-orchestrator` (`.cbc/orchestration-<repo>-<date>.md`).
- Cross-boundary **contract and merge-order decisions** are user calls — escalate them the same
  way you escalate hard same-side collisions: surface the conflict + a recommendation, get the
  user's decision, then act.
- **Verify before you trust** a peer's claim ("we merged the new contract") against that side's
  reality where you can, before you commit your agents to it.

## Teardown

When the coordination episode is done, close by **consensus** (`cbc_close`; never `--force`) and
**stop the peer room's background-poll shell** (`TaskStop` the background poll task and end any
`/loop` driving it; see `/cbc` Closing). Peer rooms count toward the one-poll-per-room load, so
don't leave a settled one running.

## Anti-patterns

- **Piping your internal worker detail across a peer room.** Share the surface where your work
  meets the peer's, not your full agent map.
- **Sitting on a transition a peer depends on.** Merged, in-review, deployed, blocked/unblocked,
  merge-order change — push it the moment it happens; don't let the peer run on stale state.
- **Joining a cross-boundary reconcile room.** You relay its id across the peer line and stay
  out; the two agents reconcile the detail directly, and only the outcome returns to the peer
  line.
- **Expecting one room for all orchestrators.** Rooms are two-party — one pairwise room per peer.
- **Auto-deciding a cross-boundary contract or merge-order question** without the user.
- **Trusting a peer's "merged/changed" claim** without checking that side's reality.
- **`cbc close --force`**, or leaving a settled peer room's poll shell running.
