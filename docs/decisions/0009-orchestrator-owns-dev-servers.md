# ADR-0009 — The orchestrator owns the repo's dev servers

- **Status:** Accepted
- **Date:** 2026-06-22
- **Related:** [ADR-0006](0006-coordination-modes-direct-and-orchestrated.md) (the orchestrator boundary this extends), [ADR-0008](0008-orchestrator-never-spawns-implementation-agents.md) (the sibling role rule)

## Context

In a typical orchestrated run, implementation agents each work in their own git worktree. Each
worktree is an independent checkout, and each agent tends to start whatever dev server the repo
needs: `npm run dev`, `cargo run`, a test server, a mock API. With no coordination, multiple
agents independently bind to the same port. One clobbers the other's running instance; the second
start fails silently or the processes fight; no one has a reliable source of truth for which port
is actually serving what.

This is the same pattern CBC's coordination topology exists to prevent in the code domain: two
agents independently solving the same problem in their own contexts and colliding at merge. The
port domain has the same structure — the orchestrator already holds the cross-agent picture, so
the natural answer is to extend that picture to include running dev servers.

A secondary concern: running a dev server is not the same as writing code. The orchestrator's
hard rule 1 ("you write no code, ever") is about authoring and editing source. Launching a
background process to serve an already-built codebase is operational coordination, not
implementation. The two must not be conflated.

## Decision

**The orchestrator owns and runs the repo's dev servers.** Workers never start their own dev
server; when they need the app running (to test, to hit an endpoint, to verify a change), they
ask over their report line. The orchestrator decides:

- **Reuse:** if a running server already serves the worker's need, point it at that URL/port.
- **Start:** if the feature needs isolation (breaking change, divergent env/config, disruptive
  migration), start a new server on a **verified free port** and hand the worker the URL.

The orchestrator launches dev servers as labeled background tasks in its own session and
records each in the **Servers** section of its map (`port | command | agent/feature | status`).

**Lifecycle:** servers live in the orchestrator's session. If it goes down, they stop. On
reconnect the orchestrator re-verifies which ports are actually up before trusting the registry
(same discipline as the poll-crash reconnect: a dead task hides truth). It relaunches any server
the map says should be running if it is not.

**Teardown:** when a feature is done and its isolated server is no longer needed, the orchestrator
stops the background task and removes the entry from the Servers section. Orphaned servers pile up
exactly as orphaned poll shells do — explicit teardown is required.

**Cross-repo:** each orchestrator owns its own repo's servers. When agents in one repo need to
hit a server from another repo, the consumable URL/port is shared across the peer line; workers
in one repo never start the other repo's server.

Running a dev server is operational coordination, not authoring source — rule 1 still holds.

## Consequences

- No port collisions: one agent (the orchestrator) owns the binding decisions for the whole repo.
- Single registry: the map's Servers section is the source of truth for what is running and where.
- The orchestrator's role expands from pure map-holder to running operational processes. This is a
  deliberate extension of the boundary, not a violation of it — operational != implementation.
- Tradeoff: servers die with the orchestrator session. Reconnecting orchestrators must re-verify
  and relaunch. This is the same tradeoff as poll shells: stateless restarts are the expected
  recovery path.
- Encoded in `cbc-orchestrator` (rule 4, the "Running the dev servers" section, Servers map
  subsection, and anti-patterns) and in `cbc-report` (worker discipline).
