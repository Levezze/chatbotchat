# ADR-0006 — Multi-agent coordination has two modes: Direct and Orchestrated

- **Status:** Accepted
- **Date:** 2026-06-22
- **Related:** [ADR-0003](0003-consensus-close.md) (consensus close — reconcile and peer rooms close by the same vote), [ADR-0004](0004-background-poll-owns-the-wait.md) (every line in a coordination topology rides a background poll), [ADR-0005](0005-consensus-extend.md) (consensus extend — how always-on lines survive past the cap)
- **Extended by:** [ADR-0008](0008-orchestrator-never-spawns-implementation-agents.md) (the orchestrator never spawns implementation agents), [ADR-0009](0009-orchestrator-owns-dev-servers.md) (the orchestrator owns the repo's dev servers), [ADR-0010](0010-orchestration-map-is-self-grounding.md) (the map's charter header and session-start hygiene)

## Context

CBC began as a two-agent bus: a room is strictly two-party, agents open → reconcile
→ consensus-close, and a human relays the room id between them. A week of dogfooding
pushed past that shape. Real work spans several agents at once — multiple workers in
one repo, and several repos (client / API / engine) each with its own agents — and a
contract changed on one side while another side is blind is exactly the "merge salad"
the bus exists to prevent.

Two pressures appeared together:

1. **The human can't be the relay at scale.** Relaying ids by hand works for two
   agents; it does not work for a dozen agents across three repos. Something has to
   coordinate without the user walking every room.
2. **A coordinator's context rots if it carries the code.** The natural fix — one
   agent that talks to all the others — fails if that agent absorbs every type,
   payload, and diff the workers exchange. It fills with detail, makes confident
   wrong calls, and becomes the bottleneck it was meant to remove.

The constraint that shaped the answer is a deliberate non-feature: **rooms are
two-party.** There is no multi-party room and we are not adding one (deferred,
`docs/v2-ideas.md`). So coordination is not a group chat — it is a *topology of
pairwise rooms*, and the question is who opens which room, who reports to whom, and
critically **what content is allowed to cross which line.**

## Decision

**We support two coordination modes, and we name them: Direct and Orchestrated.**
They are not separate features — they are two points on one scale, sharing every room
mechanic (ADR-0003/0004/0005). The skills (`cbc`, `cbc-orchestrator`, `cbc-report`,
`cbc-peer`, `cbc-recap`, `cbc-reconcile`) encode the discipline; the daemon is
unchanged.

### Direct mode — agents coordinate, the user relays

A handful of implementation agents open pairwise rooms **directly** with each other
and self-coordinate. The user relays room ids, as in the original two-agent flow.
No coordinator role; no map. This is the right mode when the agent count is small
enough that a human can relay by hand. It is just the base CBC flow, generalized.

### Orchestrated mode — a per-repo orchestrator holds the map, not the code

Each repo runs **one orchestrator**. Its workers open an always-open **report line**
to it and send concise status; the orchestrator reconciles the whole board, prevents
collisions, owns merge order, and is the single escalation funnel to the user.
Sibling repos' orchestrators coordinate as **peer orchestrators** — symmetric, one
pairwise peer room each — for cross-repo contract / schema / merge-order changes.

The load-bearing rule is the **orchestrator boundary:**

- The orchestrator **holds the map** (`.cbc/orchestration-<repo>-<date>.md`: who
  touches what, sequence, collisions, merge order) and **writes no implementation
  code.**
- It **never opens worker rooms** (workers open their own report lines) and **never
  joins or reads a reconcile room.**
- It re-grounds from the rooms and the map, **never from memory** — that is what
  makes a `/compact` mid-session safe (`cbc-recap`).

### Reconcile rooms — the detail channel that keeps the orchestrator clean

When two implementation agents must exchange **real detail** — types, shapes,
payloads, signatures, code, the actual reconciliation of two plans — they open a
**reconcile room** directly with each other. It is a **normal-lifecycle** room
(open → reconcile → consensus close), *not* an always-open line. In orchestrated
mode the orchestrator **relays the room id and nothing else:**

- Same-repo: it forwards the id over the *other* worker's report line.
- Cross-repo: it hands the id across the peer line to the peer orchestrator, who
  forwards it to their worker.
- **No orchestrator ever joins.** The two agents reconcile the detail directly; only
  the **outcome that changes the map** (a new shared surface, a changed contract, a
  merge-order dependency) bubbles back up as one-line *status* — never the code.

This is the whole point: agents coordinate **deeply** while the orchestrator's
context stays at the level of the map. Relay, don't absorb.

### Capacity — open big, extend by consensus

Always-on lines (report, peer) outlive the default 20-message cap. We do not add an
"unlimited" mode. Instead the mechanism is the one that already exists:
`cbc_open_room` takes a per-room `hard_cap`, so coordination lines open it **high**
(e.g. `200`), and `cbc_extend` (consensus **+20**, ADR-0005) is the safety net when a
line still fills. Reconcile rooms use the default cap and extend only if a deep
exchange needs it.

### Two-party stays — coordination is pairwise rooms

No multi-party room. Three agents aligning is **pairwise rooms** (client↔api and
api↔engine are *separate* reconcile rooms); three repos is **pairwise peer rooms**
(no all-orchestrators room). Every coordination shape composes from two-party rooms.

## Consequences

- A coordinator that scales: one orchestrator fans out across many workers and sibling
  repos without the user relaying every id, and **without** the orchestrator's context
  filling with implementation detail.
- A `/compact` mid-orchestration is safe by construction — the truth lives in the
  rooms and the on-disk map, not the orchestrator's head (`cbc-recap`).
- The cost is discipline, not code: nothing in the daemon enforces "the orchestrator
  doesn't join a reconcile room" or "status, not code, on the report line." The skills
  carry these rules; they are conventions, and a misbehaving agent can violate them.
  We accept that — the alternative (server-enforced roles, content filtering,
  multi-party rooms) is a far larger surface for a behaviour the skills already shape.
- The modes are a spectrum: a session can start Direct and grow an orchestrator as the
  agent count climbs. Same rooms, same mechanics — only who relays and who holds the
  map changes.
- Reconcile rooms add a *third* room kind a worker juggles (its report line, any peer
  handoffs, and a transient reconcile room), each on its own background poll. The
  skills make the separation explicit so one poll never stands in for another.
