# ADR-0013 — Chapter Masters delegate, skills extend through scoped files, trackers are fed by the outbox

- **Status:** Accepted (design only; nothing built)
- **Date:** 2026-11-18
- **Supersedes in part:** [ADR-0012](0012-chapter-masters-own-feature-intake.md) decision 5 and the
  tracker part of decision 1 (I-09 to I-12)
- **Related:** [feature intake and tracker profiles](../design/feature-intake-and-tracker-profiles.md)
  (decisions I-15 to I-23), [next trial backlog](../design/next-trial-backlog.md),
  [work records](../design/work-records-and-external-observations.md) (the outbox),
  [ADR-0011](0011-trial-1-decisions.md)

## Context

Three things came out of daily use and a requester handoff from a product team using CBC.

1. **Chapter Masters write the code themselves.** The role skill says "You may code directly",
   so they do. While a Chapter Master is coding, it is not managing the feature: it can't take
   the next task, answer a question or run parallel work. Both host apps (Claude and Codex, app
   and CLI) are built around spawning sub-agents and reading their results. CBC working against
   that is waste.
2. **Per-user workflow can't live in the skills.** CBC is a framework. One user tracks work in a
   task database with features, tasks and synced pull requests; another tracks nothing; a
   consultant on someone else's project uses whatever that organisation uses. ADR-0012 made
   the tracker a profile configured once per installation. The same user needs different setups
   in different projects, so one per installation doesn't fit.
3. **Agents writing to trackers with their own tools drift.** Agents forget, duplicate entries
   and spend tokens on tracker tools in the middle of implementation. ADR-0012's deterministic
   obligations (I-13) still hold, but the writer was the wrong agent. A tracker is a tool people
   edit by hand, so writing to it needs judgement about what is already there. That rules out a
   blind deterministic sync and also argues against the implementing agent doing it in passing.

## Decision

1. **Chapter Masters delegate implementation.** A Chapter Master manages one feature end to end,
   across several PRs and parallel tasks. It discusses details with the user, briefs native
   sub-agents, checks their work and owns the integrated result. It writes code itself only when
   the user explicitly asks it to. Helpers coordinate with it through the host's native
   mechanisms, not CBC, and the user can talk to a helper directly. (I-15)
2. **Skills are extended by scoped files, never edited per user.** Each role can have an
   extension file: instructions every agent in that role must read at start, if the file exists.
   Extensions are committed in the user's own project, at the user, organisation or project,
   repository or subfolder level. The nearest one replaces the outer ones completely. A missing
   extension costs nothing, and installation writes a minimal default. Tracker setup is one use
   of an extension, not a built-in subsystem. (I-16, I-17)
3. **Tracker updates go through the outbox.** A Chapter Master or Primarch records a tracker
   update as a templated event in CBC's existing outbox, the same delivery path as work-item
   revisions. It never calls tracker tools itself. A pending or failed update is visible and
   never blocks implementation. Proposed, not yet confirmed by the user: CBC owns only the
   envelope, meaning a stable ID, an idempotency key, a state (pending, done or failed) and the
   external record reference written back after the update. (I-18, open question 9)
4. **An optional tracker agent carries out the updates.** It is a CBC member like any other, woken
   by the outbox. Its extension file says which tool the team uses and how. It reads what is
   already in the tracker, keeps human edits, asks the Primarch about scope and asks the user only
   when the answer is still unclear. A team can use a different executor (a script, or the
   tracker's own AI) as long as it reports results back the same way. (I-19, I-21)
5. **CBC defines default event names, not workflows.** The defaults are task created, task
   changed, PR linked, blocked or resumed, done, dropped or superseded, and feature created or
   linked. The mapping to a tool's fields, how granular tasks are and how progress is computed
   all belong to the user's extension. ADR-0012's one-task-per-PRD rule and slice-count progress
   become one example setup, not CBC rules. (I-20)

## Consequences

- CBC ships no tracker provider code. Changes on the tracker's side (an API change, a new
  product) are absorbed by the team's extension file and connector, not by CBC releases.
- The role skills change after trial 1: delegation by default, a start sequence that reads
  extensions and renames the chat first, and outbox events instead of tracker tools.
- The tracker agent is a new optional role. It increases the number of long-lived sessions, so
  whether it runs continuously or starts on demand is decided by trial evidence (open question
  in the design doc).
- The Primarch's supervisory role gains a natural job: answering scope and context questions
  from Chapter Masters and the tracker agent. This restates I-02 and I-03; it does not make the
  Primarch an orchestrator again. (I-23)
- Trial 1 runs unchanged. These decisions are exercised in the next trial, on a small real
  feature in a user's product repository (I-22, which changes T-01 for that trial only).
- Deployment visibility (main versus production differences, evidence of what is actually
  serving, worktree clean-up) is not part of this decision.
