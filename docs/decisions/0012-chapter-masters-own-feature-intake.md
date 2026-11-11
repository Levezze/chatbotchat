# ADR-0012 — Chapter Masters own feature intake; tracker sync is an optional profile

- **Status:** Accepted (design only; nothing built)
- **Date:** 2026-11-11
- **Related:** [feature intake and tracker profiles](../design/feature-intake-and-tracker-profiles.md)
  (decisions I-01 to I-14), [ADR-0011](0011-trial-1-decisions.md), PRD #112,
  stages #115 and #116

## Context

In daily use the Primarch ended up grilling features, writing issues and
relaying decisions for several Chapter Masters at once. It could hold only one
feature at a time. Separately, the user tracks work in tools outside CBC
(GitHub issues for PRDs and slices, a Notion board for tasks), and agents leave
those tools out of date.

## Decision

1. **Intake belongs to the Chapter Master.** It runs the grill with the user,
   writes the PRD or issue, creates the tracker entries and keeps them current
   through completion. It may consult its Primarch for repository context, and
   the Primarch may consult the Emperor; the responsibility does not move.
   (I-01, I-02)
2. **The Primarch supervises.** It reads the map, steps in with context or a
   hold, and owns timelines, repository-wide issues and merge order. Promotion
   stays human-owned by default (T-08); the Primarch sequences it only where a
   project assigns it. It does not run features. (I-03)
3. **User-created Chapter Masters register themselves** with their Primarch's
   map, and the Primarch is notified. (I-04)
4. **Every feature starts with a grill.** It records goals, HITL testing,
   human review of the result, or fully AFK, and what counts as complete, per
   issue, PRD or feature. It ends with a work breakdown (one issue, or a PRD
   with slice issues), each item marked AFK or HITL. The user accepts it or asks
   for changes, and nothing is created before acceptance. The grill is user
   intake, not a Primarch GO, so the no-startup-GO rule in #112 stands.
   (I-05 to I-08, I-14)
5. **Tracker integration is an optional, local profile.** One human-readable
   board task per PRD, progress counted as closed slice issues over total.
   Agents write to the trackers with their own tools; CBC stores links and
   pushed state, holds no tracker credentials, and syncs one way. Missed
   updates surface as deterministic obligations on the dashboard. (I-09 to I-13)

## Consequences

- The role skills change after the #113 trial. Nothing is installed into live
  chats by this record.
- #115 work items gain completion criteria, intake answers and tracker
  references. #116's Emperor setup conversation writes the profile.
- Tracker profiles become a new deferred stage, blocked by #115 and #116.
- No conflict with T-07: each work item starts with the HITL flag the user
  accepted in the breakdown, and T-07 governs every later change.
- CBC still does not replace GitHub, Jira or Notion, or own the product plan.
  It records links and drives a status mirror for the work it manages.
