---
name: cbc-peer
description: Coordinate with the peer orchestrators in other repos (e.g. client / API / engine) as symmetric siblings — open one pairwise room per peer to reconcile cross-repo contract changes, shared schema/migrations, and cross-repo merge order. For the orchestrator agent, alongside `/cbc-orchestrator`. Use when the user invokes `/cbc-peer`, or asks you to coordinate with the orchestrator(s) of other repos.
disable-model-invocation: false
---

You are an **orchestrator**, and the other repos in this system each have their own
orchestrator. This skill is how you coordinate with them as **symmetric siblings** — neither
side is the worker, you are peers — so cross-repo work doesn't break at the seams (a contract
one repo changes that another consumes, a shared schema, the order repos must merge in).

Load this **alongside `/cbc-orchestrator`** (your same-repo duties) and on top of **`/cbc`**
(every room mechanic). This skill adds only the peer-coordination layer.

## Peer rooms — you MAY open these

The "never open a room" rule in `/cbc-orchestrator` is about **worker** rooms. Peers are
different: there is no worker to do the opening, so **either orchestrator may open** the peer
room.

- `cbc_open_room` with a subject like `peer: <repoA> <-> <repoB>`.
- Surface the bare room id to the **user**, who relays it to the other repo's orchestrator.
- One **pairwise** room per peer — a CBC room is two-party. With three repos you hold up to two
  peer rooms (e.g. engine↔api and engine↔client are **separate** rooms); there is no
  all-orchestrators room.
- Then follow `/cbc` exactly: join, send a substantive opener, start a background poll, keep the
  line open while the cross-repo work is in flight.

## Recap your own board first, then reconcile across

When peer orchestrators connect there are many moving parts in every repo at once. **Don't
jump to firing cross-repo decisions at the user.** Build the picture first — yours and theirs:

- Before you reconcile anything cross-repo, make sure **your own** side is whole: you've
  gathered and recapped all your same-repo agents' rooms (that's `/cbc-orchestrator`'s first
  move). A peer can't coordinate with a half-understood repo, and neither can you.
- Make sure the **peer** board is complete too: ask the user whether there are further peer
  orchestrators that should join before you reconcile, so you're not aligning against a partial
  cross-repo picture.
- When you open or join a peer room, **`cbc_recap` it before you decide** — read the cross-repo
  state the peer holds, reconcile it against your map, and only then raise options to the user
  **where a cross-repo call is genuinely required.** Understanding first, asking only where needed.

## What crosses a peer room — cross-repo only

**This is the whole reason the peer system exists:** any change in one repo that another repo
*depends on* must be coordinated across all the affected orchestrators **before it lands**, so no
repo is blindsided. The moment work touches a cross-repo dependency, it stops being a same-repo
decision. Concretely, raise it in the peer room(s):

- **API contracts / response shapes / results** — an endpoint's request or response shape,
  status codes, error shape, pagination — anything a consuming repo reads.
- **Regenerated types / generated clients / SDKs** — anything one repo *generates from* another
  (OpenAPI/GraphQL/protobuf types, a generated API client, shared type packages). If repo A's
  change means repo B must regenerate, B's orchestrator must know in step, not after.
- **Shared schema / migrations** — order and compatibility of DB changes across repos.
- **Cross-repo merge order** — which repo lands first so the other isn't briefly broken.
- **Heads-up** — "my repo's agents are about to change X that your repo consumes/derives from."

The rule of thumb: **if a change in your repo forces another repo to adapt, regenerate, or
re-derive anything, announce it across the peers first.** A contract changed on one side while
the other side is blind is precisely the cross-repo salad this system prevents.

Do **not** pipe same-repo worker detail across a peer room. The peer doesn't need your internal
agent map; it needs the cross-repo contract surface.

## Same map, same escalation

- Fold cross-repo dependencies and merge order into the **same orchestration map** you keep in
  `/cbc-orchestrator` (`.cbc/orchestration-<repo>-<date>.md`).
- Cross-repo **contract and merge-order decisions** are user calls — escalate them the same way
  you escalate hard same-repo collisions: surface the conflict + a recommendation, get the
  user's decision, then act.
- **Verify before you trust** a peer's claim ("we merged the new contract") against that repo's
  reality where you can, before you commit your repo's agents to it.

## Teardown

When the cross-repo coordination episode is done, close by **consensus** (`cbc_close`; never
`--force`) and **stop the peer room's background-poll shell** — same discipline as a finished
worker room. Peer rooms count toward the one-poll-per-room load, so don't leave a settled one
running.

## Anti-patterns

- **Piping same-repo worker detail across a peer room.** Cross-repo contract surface only.
- **Expecting one room for all orchestrators.** Rooms are two-party — one pairwise room per peer.
- **Auto-deciding a cross-repo contract or merge-order question** without the user.
- **Trusting a peer's "merged/changed" claim** without checking that repo's reality.
- **`cbc close --force`**, or leaving a settled peer room's poll shell running.
