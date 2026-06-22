# ADR-0010 — The orchestration map is self-grounding

- **Status:** Accepted
- **Date:** 2026-06-22
- **Related:** [ADR-0006](0006-coordination-modes-direct-and-orchestrated.md) (the map this convention extends), [ADR-0008](0008-orchestrator-never-spawns-implementation-agents.md), [ADR-0009](0009-orchestrator-owns-dev-servers.md) (the role rules the charter restates)

## Context

Two compounding failure modes appeared in long-running orchestrated sessions:

**Role decay.** The orchestrator's role — four hard rules, worker responsibilities, relay
discipline — is loaded from the skill at invocation. Claude Code sessions accumulate context
over hours of work, then compact. After one or more compactions, the skill instructions that
defined the role are gone from the active context window. The orchestrator drifts: it starts
joining reconcile rooms, spawning workers, or writing code in ways that would have been
immediate catches before compaction. There is no current mechanism to re-inject the skill
instructions without re-invoking the skill.

**Stale map inheritance.** The orchestration map persists across sessions. When a new
orchestrator launches into a repo, the existing map may describe finished work (merged features,
closed rooms, completed agents) or an entirely different session's work. A freshly launched
orchestrator that silently inherits that map runs on a polluted board — treating closed rooms as
open, workers that no longer exist as active, ports that are no longer running as live.

The map file is the one artifact the orchestrator re-reads continuously — on every recap, on
every reconnect, on every context reset. It persists where skills do not.

## Decision

**The map is self-grounding.** Two practices are established:

### 1. Role charter — always first, always verbatim

The first block of every orchestration map is a fixed, deterministic **role charter** (~10
lines) re-emitted verbatim on every wipe or compact. It states the orchestrator's four hard
rules and worker responsibilities. Because the map is re-read continuously, the charter is
always in the orchestrator's context — it does not depend on the skill still being in the
conversation window.

The skill carries the exact charter text as a fenced block, so it is reproducible and kept in
sync with the four hard rules: the skill defines both the rules and the charter, so editing the
skill to change a rule automatically updates the template for new map writes.

### 2. Session-start hygiene — wipe, compact, or keep

When a fresh orchestrator launches, it reads the existing map (if any), summarizes what it
holds, and asks the user to choose one of three paths before proceeding:

- **Wipe** — prior work is fully done or unrelated; start with a blank map, re-emit the charter.
- **Compact** — some threads are still live (open rooms, in-flight workers, running servers,
  pending merge order); keep only what is active, drop the rest, re-emit the charter at the top.
- **Keep** — resuming mid-session; leave the file as-is (prepend the charter if the map predates
  this convention).

The orchestrator never decides silently. Silently inheriting a stale map is the failure this
prevents.

## Consequences

- Role identity is durable across context compaction without re-invoking the skill. The
  orchestrator always has its four hard rules in view.
- Fresh sessions start on a known-good map state rather than on yesterday's noise.
- The charter is a standing ~10-line overhead in every map file. Acceptable given the durability
  it provides.
- One user prompt is required at every fresh orchestrator launch (wipe/compact/keep). This is a
  deliberate forcing function — it makes the implicit state of the previous session explicit.
- The skill-carried convention: no server enforcement. A misbehaving orchestrator can skip the
  hygiene step or omit the charter. We accept this — the alternative (server-level session
  tracking) is out of scope. The skill and the docs shape the behavior.
- Encoded in `cbc-orchestrator` (map section, the charter block, session-start hygiene, and
  anti-patterns) and in `COORDINATION_MODES.md`.
