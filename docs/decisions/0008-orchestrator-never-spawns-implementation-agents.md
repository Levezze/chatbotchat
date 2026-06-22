# ADR-0008 — The orchestrator never spawns implementation agents

- **Status:** Accepted
- **Date:** 2026-06-22
- **Related:** [ADR-0006](0006-coordination-modes-direct-and-orchestrated.md) (the orchestrator boundary this extends), [ADR-0007](0007-room-refresh-and-close-teardown.md)

## Context

The orchestrator's boundary, as established in ADR-0006, is clear: it holds the map, writes no
implementation code, never opens worker rooms, and never joins reconcile rooms. In practice,
however, orchestrators began exploiting native Claude Code capabilities — the Agent tool,
worktree creation, subagent spawning — to launch implementation workers from within their own
session. On the surface this looked efficient: the orchestrator sees a gap, spins up a worker,
keeps moving. In reality it undermined the entire topology.

The CBC worker model is **user-opened sessions connected via report lines.** Workers are separate
Claude Code sessions the user started and planned with. That separation is load-bearing: it gives
each worker its own context, its own plan, its own escalation path, and its own merge authority.
When the orchestrator spawns a worker, it collapses that separation — the orchestrator becomes an
implementer-by-proxy, its context fills with the detail it was designed to stay above, and the
boundary ADR-0006 drew dissolves silently.

## Decision

**The orchestrator never spawns implementation agents.** Not via the Agent tool, not via
worktree creation, not via any other mechanism that launches an implementation process from the
orchestrator's own shell or session.

Workers are sessions the user opened and connected to the orchestrator via a report line. If no
worker exists for a piece of work the orchestrator has identified, it surfaces the gap to the user
— *"no worker is covering `<surface>` — you will need to open one and paste me the room id"* — and
waits. It does not fill the gap itself.

**Soft exception:** if the user explicitly asks the orchestrator to do implementation work in
that conversation, it may. This is a one-off override for the session, not a license to keep
spawning workers.

## Consequences

- The boundary ADR-0006 established (orchestrator coordinates, never implements) is preserved and
  made explicit.
- Workers remain user-started, user-planned sessions with their own context and authority.
- The orchestrator's context stays at the level of the map — it does not absorb implementation
  detail from workers it spawned.
- The prohibition is **skill-carried convention**, not server-enforced. Nothing in the daemon
  prevents the Agent tool from being called; the discipline lives in the skill and is a convention
  the orchestrator must uphold. We accept this — the alternative (server-level role enforcement)
  is out of scope and was explicitly deferred in ADR-0006.
- Encoded in `cbc-orchestrator` (rule 3 + anti-pattern) and in the COORDINATION_MODES roles table.
