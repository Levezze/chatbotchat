# Feature intake and task-tracker profiles

Decisions taken with the user on 2026-11-11, for PRD #112. Nothing here is
built yet. The intake rules change the role skills and the work-item fields;
the tracker profiles are a deferred stage that follows #115 and #116. Neither
is a prerequisite for the #113 working trial.

Architecture changes are summarised in
[ADR-0012](../decisions/0012-chapter-masters-own-feature-intake.md).

## 1. Why

Two problems came out of daily use.

- **The Primarch was acting as an orchestrator.** It grilled features, wrote
  issues and relayed decisions for several Chapter Masters at once, and it
  could only manage one feature at a time without losing track. The planning
  work needs the full context of one feature, and the Chapter Master is the
  only agent that has it.
- **Task tracking lives outside CBC and drifts.** Engineers keep plans and work
  in their own tools. The reference setup keeps PRDs and implementation slices
  as GitHub issues and tracks tasks on a Notion board. An agent that forgets
  to update the board leaves it wrong, and nothing notices.

## 2. Decision record

| ID | Decision | Why | Rejected |
|---|---|---|---|
| I-01 | The Chapter Master owns its feature's intake: the grill session, the PRD or issue, the tracker entries, and their updates through completion. | It has the whole context of the task. Taking intake out of the Primarch's chat is the point. | The Primarch writing issues before it assigns work. |
| I-02 | During intake the Chapter Master may ask its Primarch for context, and the Primarch may ask the Emperor. These are consultations. The responsibility stays with the Chapter Master. | The Primarch sees the whole repository; its context is useful without making it the owner. | Routing intake decisions through the Primarch. |
| I-03 | The Primarch supervises and does not orchestrate. It reads the map, steps in with context or a hold when something needs it, and otherwise watches. It owns timelines, repository-wide issues, and merge and promotion order. | One supervisor can watch many features; one orchestrator can run only one. Merge order is already the Primarch's under the existing role contract. Promotion stays human-owned by default (T-08); the Primarch sequences it only where a project assigns it. This restates both, it does not move them. | The Primarch as an active per-feature coordinator. |
| I-04 | A Chapter Master is created by the user or by the Primarch. If the user creates it, it registers itself with its Primarch's map. The Primarch is told it has a new Chapter Master, and its messages reach the Primarch automatically from then on. | The user must be able to start work without going through the Primarch. | Requiring the Primarch to create every Chapter Master. |
| I-05 | Every new feature starts with a grill session between the Chapter Master and the user. It settles the goals, whether HITL testing is required, whether a human must look at the result (usually frontend work), or whether the work is fully AFK. Each answer can apply per issue, per PRD or to the whole feature. | These answers decide how the work runs and when it counts as done. Guessing them is how features ship wrong. | Grilling only when the agent thinks it is needed. |
| I-06 | The grill is user intake. It is not a startup GO from the Primarch, and "fully AFK" is a possible outcome of the grill, not a way to skip it. | The PRD's no-GO rule stays: the Primarch does not approve starts. | Treating the grill as a Primarch gate. |
| I-07 | The Chapter Master proposes either a full PRD with slice issues or a single issue. The user decides. | Some tasks are small. The user knows which. | A size rule decided by the agent. |
| I-08 | What "complete" means is a grill answer, recorded on the work item: for example merged, merged and deployed, or signed off by the user after review. It can differ per issue, per PRD or per feature. | Completion depends on the work. A fixed rule would be wrong for half of it. | PR merged as the universal definition. |
| I-14 | The grill ends with a work breakdown: one issue, or a PRD split into slice issues, each marked AFK or HITL with its completion definition. The Chapter Master shows it and asks whether the user accepts. The user accepts or asks for changes, and the breakdown is revised until accepted. Nothing is created before that: no issues, no tracker entries, no work items. | The user sees and owns the plan before any work exists. Whether an item needs a human is the user's call, not the agent's. | The Chapter Master setting AFK or HITL itself. Creating issues before the user has seen the breakdown. |
| I-09 | Task-tracker integration is an optional profile, configured once per CBC installation, during setup with the Emperor or later. | Every engineer and team tracks work differently. Many will use none. | Building one tracker into CBC. |
| I-10 | The task board holds one task per PRD, written for people, not engineers. A single issue with no PRD also gets one task. The task shows progress as closed slice issues over total slice issues, counted by number, not effort: 3 of 5 closed shows 60%. | The board is for humans to follow. GitHub issues are the technical record. A count is a stated basis; PRD #112 forbids invented percentages. | One task per GitHub issue. Effort-weighted progress. |
| I-11 | The Chapter Master creates and updates the tracker entries with its own tools (the `gh` CLI, the Notion MCP connector). CBC stores the links and the last state pushed, and holds no tracker credentials. | The agents already have authorised access. Credentials in the core would widen what a compromised core can reach. | A CBC-side Notion or GitHub client. |
| I-12 | Status flows one way: from CBC's work record to the trackers. A manual edit on the board is overwritten or reported as drift, and never read back. | Two-way sync is where integrations like this fail. | Two-way sync. |
| I-13 | Tracker updates are obligations created deterministically on work-item transitions, not left to the agent's memory. The dashboard shows any work item whose tracker state is behind. | A missed update has to be visible without anyone checking. This is the same reliability problem as message delivery. | Relying on the skill text alone. |

## 3. Tracker profile

A profile is local, nonsecret configuration. It never lives in a repository:
board and database identifiers are private, and the public repository must not
carry them.

| Field | Reference setup |
|---|---|
| Plan provider | GitHub: PRD issue with slice issues, or a single issue |
| Board provider | Notion task database |
| Board granularity | One task per PRD, or per standalone issue |
| Progress basis | Closed slice issues / total slice issues |
| Status mapping | Work-item stage to board status, set per profile |
| Completion | Taken from the grill answer (I-08) |
| Profile `none` | No tracker work; intake still records the PRD or issue links |

The role skills have one fixed section on tracking that reads the active
profile. Installing or changing a profile changes configuration, not skill
text, so installs cannot drift apart.

### What the work record stores

For each work item: the PRD issue reference (if any), the slice issue
references, the board task reference, the last state pushed to each, and the
work-item revision it was pushed at. These sit beside the existing PR
references and deployment references from #115, with the same rule that
external observations never overwrite owner-written semantic fields.

### Sync obligations

| Transition | Obligation |
|---|---|
| Intake finished | Create the PRD or issue, then the board task, then record the links |
| Slice issue closed | Recompute progress and update the board task |
| Stage change (blocked, in review, merged) | Update the board status |
| Completion criteria met (I-08) | Close the remaining issues and mark the task done |

The core creates the obligation and its due time. The Chapter Master performs
it and records the result. An obligation still open past its due time shows on
the dashboard as tracker drift for that work item.

## 4. Open questions

**Resolved 2026-11-11: grill answers and the HITL flag.** This looked like a
conflict with T-07, under which a Chapter Master can't change an item's HITL
flag. I-14 removes it. Each work item is created with the flag the user
accepted in the breakdown, so the initial value is the user's decision and the
Chapter Master only records it. T-07 is unchanged for every later change: the
user or the Primarch can turn the flag on, and only the user can turn it off.
The accepted breakdown is stored with the channel the acceptance came through
(dashboard, or chat stored as relayed under R3), so it can be audited.

Still open:


1. **How issue closure is observed.** The #115 GitHub observer covers pull
   requests only. Either add issue state to that observer, or derive closure
   from a merged PR that closes the issue. The first is simpler to reason
   about; the second needs no new observation. Undecided.
2. **A Chapter Master retired before its tracker work is done.** The open
   obligations need an owner. Proposal: they pass to the Primarch as
   repository-wide work, shown on the dashboard. Undecided.

## 5. Later: talking to a Chapter Master from the dashboard

The user wants to run a grill or answer a Chapter Master from the dashboard,
reading the chat's latest messages and sending into it, with the native chat
still available for full control. This depends on two capabilities that are
not qualified yet: messaging an existing native session (not qualified in
Claude Desktop) and bounded reads of recent native-chat history (#119, the
last stage). A message sent this way must carry dashboard-authenticated user
origin (T-12), so an agent can never send one that looks like the user. It is
recorded as a goal, not a promise.

## 6. Where this lands

| Piece | Stage |
|---|---|
| Role skill changes: Chapter Master intake, grill-first, Primarch as supervisor, user-created registration, breakdown acceptance (I-01 to I-08, I-14) | Skill source update, after the #113 trial. Not installed into live chats as part of this record. |
| Work-item fields: completion criteria, intake answers, tracker references | #115 work records |
| Emperor setup conversation that writes the profile | #116 |
| Tracker profiles and sync obligations (I-09 to I-13) | New deferred stage, blocked by #115 and #116 |
| Dashboard conversation with a Chapter Master | After #119 and existing-session messaging qualification |
