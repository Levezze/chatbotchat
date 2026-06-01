# ADR-0001 — Rescope to a usable alpha

- **Status:** Accepted
- **Date:** 2026-05-31
- **Supersedes priority ordering in:** [`docs/v1-design-locked.md`](../v1-design-locked.md)

## Context

`chatbotchat` is a local server that lets two AI coding agents hold a structured
conversation in a room that can be closed and made read-only. The original goal
was a small, local, non-live alpha backed by SQLite — something to stand up
quickly and dogfood, then grow based on what real use revealed.

Twelve vertical slices later (PRs #2–#26), the build had drifted well past that
goal. A review on 2026-05-31 found a specific inversion:

- **Built:** the full 6-state lifecycle machine, an hourly sweeper with
  auto-archive, ghost detection, hard + soft caps with auto-surface, sentinel
  messages with severity, polling backoff with 1.5× time-decay, and an events
  log + `on_archive` hook for a v2 vector-search feature that is not in v1 at all.
- **Not built:** `cbc list`, `cbc show`, `cbc search`, `cbc summary` — the
  surface that lets a human actually browse and use the thing.

We built the autonomic nervous system before the eyes. The machinery to *manage*
rooms exists and is well-tested (TDD, real SQLite, real loopback daemon, ~129
green tests); the basic surface to *read* rooms does not. `search` and `summary`
were in the "locked v1" spec, so the build is not even v1-complete by its own
definition.

### Where the over-scope came from

The scope creep originated in the design phase, not the implementation. The
`/grill-me` session that produced `v1-design-locked.md` resolved every branch of
the decision tree ("what about ghosted participants? cap loops? stale rooms?
auto-archiving?") into the spec, and the implementation faithfully built it. The
lesson: `grill-me` expands scope by construction, and labelling its output
"locked v1" let beta-grade machinery masquerade as alpha scope.

## Decision

Stop polishing the speculative machinery. Prioritize the small, load-bearing
surface that makes the tool dogfoodable, ship it, run it between two real agents,
**and then** let real use decide whether the deferred machinery earns its keep.
Nothing already built is removed — it is tested and working, and rolling it back
would cost more than it returns.

Feature buckets, current status:

| Bucket | Features | Action |
|---|---|---|
| **Core loop — built, verified working** | open, join, send, wait, status, close→read-only | Done. Keep. |
| **Skipped, load-bearing for the goal** | `cbc list`, `cbc show` | **Promote — build now** (milestone `alpha-usable`). |
| **Install / always-on** | launchd plist, global MCP registration, port checks | **Promote — build now** (`alpha-usable`, issue #10). |
| **Built, premature** (right idea, too early) | caps / loop-insurance | Keep dormant; revisit after dogfooding. |
| **Built, likely misconceived at this scale** | auto-archive sweeper, severity backoff + time-decay, ghost detection, `on_archive` v2 hook | Keep the code (tests pin it); stop polishing. |
| **Deferred enhancements** | `cbc search` (FTS5), `cbc summary`, raise-cap-mid-room (#16), capacity policy (#12) | Defer to milestone `v1.5-later`; keep open, de-prioritized. |

## Consequences

- **Verified, not inferred:** the core loop was exercised by hand against a live
  daemon on 2026-05-31 (open → join → send → wait → status → close, with a 409 on
  send to a closed room). It works today.
- **Real residual risk** is operational, not in the unit logic (which is tested):
  WAL under concurrent long-polls, daemon restart/drain on launchd, the
  clock-driven transitions (sweeper, ghost, decay) over real wall-time, and MCP
  tool-timeout vs the 10-minute `wait` cap. None of these have been dogfooded.
  These are the design's own "known considerations" — now explicitly parked, not
  solved.
- **Issue tracker** re-ranked to match: `alpha-usable` milestone holds list/show
  + install; `v1.5-later` holds search, summary, and the extra-caps work. See the
  status banner in `v1-design-locked.md`.
- **`v1-design-locked.md` remains the design reference** but is no longer the
  priority ordering. This ADR is the priority ordering.
