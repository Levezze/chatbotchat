# Feature intake and task-tracker profiles

Decisions taken with the user on 2026-11-11, for PRD #112. Nothing here is
built yet. The intake rules change the role skills and the work-item fields;
the tracker profiles are a deferred stage that follows #115 and #116. Neither
is a prerequisite for the #113 working trial.

Architecture changes are summarised in
[ADR-0012](../decisions/0012-chapter-masters-own-feature-intake.md).

**Updated 2026-11-18.** Decisions I-15 to I-23 (section 2a,
[ADR-0013](../decisions/0013-delegating-chapter-masters-and-tracker-outbox.md))
replace the tracker profile in I-09 to I-12. Chapter Masters now delegate
implementation, skills are extended by scoped files, and tracker updates go
through the outbox to an optional tracker agent. Superseded rows and section 3
stay below as history, each marked. **Updated 2026-11-21:** I-24 to I-27
(section 2b) settle who creates tracker events, retries, pending events and
the start order.

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
| I-01 | The Chapter Master owns its feature's intake: the grill session, the PRD or issue, the tracker entries, and their updates through completion. *Tracker part superseded by I-18: the Chapter Master emits tracker events and no longer writes entries itself.* | It has the whole context of the task. Taking intake out of the Primarch's chat is the point. | The Primarch writing issues before it assigns work. |
| I-02 | During intake the Chapter Master may ask its Primarch for context, and the Primarch may ask the Emperor. These are consultations. The responsibility stays with the Chapter Master. | The Primarch sees the whole repository; its context is useful without making it the owner. | Routing intake decisions through the Primarch. |
| I-03 | The Primarch supervises and does not orchestrate. It reads the map, steps in with context or a hold when something needs it, and otherwise watches. It owns timelines, repository-wide issues, and merge and promotion order. | One supervisor can watch many features; one orchestrator can run only one. Merge order is already the Primarch's under the existing role contract. Promotion stays human-owned by default (T-08); the Primarch sequences it only where a project assigns it. This restates both, it does not move them. | The Primarch as an active per-feature coordinator. |
| I-04 | A Chapter Master is created by the user or by the Primarch. If the user creates it, it registers itself with its Primarch's map. The Primarch is told it has a new Chapter Master, and its messages reach the Primarch automatically from then on. | The user must be able to start work without going through the Primarch. | Requiring the Primarch to create every Chapter Master. |
| I-05 | Every new feature starts with a grill session between the Chapter Master and the user. It settles the goals, whether HITL testing is required, whether a human must look at the result (usually frontend work), or whether the work is fully AFK. Each answer can apply per issue, per PRD or to the whole feature. | These answers decide how the work runs and when it counts as done. Guessing them is how features ship wrong. | Grilling only when the agent thinks it is needed. |
| I-06 | The grill is user intake. It is not a startup GO from the Primarch, and "fully AFK" is a possible outcome of the grill, not a way to skip it. | The PRD's no-GO rule stays: the Primarch does not approve starts. | Treating the grill as a Primarch gate. |
| I-07 | The Chapter Master proposes either a full PRD with slice issues or a single issue. The user decides. | Some tasks are small. The user knows which. | A size rule decided by the agent. |
| I-08 | What "complete" means is a grill answer, recorded on the work item: for example merged, merged and deployed, or signed off by the user after review. It can differ per issue, per PRD or per feature. | Completion depends on the work. A fixed rule would be wrong for half of it. | PR merged as the universal definition. |
| I-14 | The grill ends with a work breakdown: one issue, or a PRD split into slice issues, each marked AFK or HITL with its completion definition. The Chapter Master shows it and asks whether the user accepts. The user accepts or asks for changes, and the breakdown is revised until accepted. Nothing is created before that: no issues, no tracker entries, no work items. | The user sees and owns the plan before any work exists. Whether an item needs a human is the user's call, not the agent's. | The Chapter Master setting AFK or HITL itself. Creating issues before the user has seen the breakdown. |
| I-09 | *Superseded by I-16.* Task-tracker integration is an optional profile, configured once per CBC installation, during setup with the Emperor or later. | Every engineer and team tracks work differently. Many will use none. | Building one tracker into CBC. |
| I-10 | *Superseded by I-20: now one example setup, not a CBC rule.* The task board holds one task per PRD, written for people, not engineers. A single issue with no PRD also gets one task. The task shows progress as closed slice issues over total slice issues, counted by number, not effort: 3 of 5 closed shows 60%. | The board is for humans to follow. GitHub issues are the technical record. A count is a stated basis; PRD #112 forbids invented percentages. | One task per GitHub issue. Effort-weighted progress. |
| I-11 | *Superseded by I-18 and I-19. The credentials point stands: CBC still holds no tracker credentials.* The Chapter Master creates and updates the tracker entries with its own tools (the `gh` CLI, the Notion MCP connector). CBC stores the links and the last state pushed, and holds no tracker credentials. | The agents already have authorised access. Credentials in the core would widen what a compromised core can reach. | A CBC-side Notion or GitHub client. |
| I-12 | *Superseded in part by I-19: human edits on the tracker are kept, not overwritten. Whether the executor may report them back is open question 7.* Status flows one way: from CBC's work record to the trackers. A manual edit on the board is overwritten or reported as drift, and never read back. | Two-way sync is where integrations like this fail. | Two-way sync. |
| I-13 | *Kept. Since I-18 the obligation is an outbox event and its executor is the tracker agent.* Tracker updates are obligations created deterministically on work-item transitions, not left to the agent's memory. The dashboard shows any work item whose tracker state is behind. | A missed update has to be visible without anyone checking. This is the same reliability problem as message delivery. | Relying on the skill text alone. |

## 2a. Decision record, 2026-11-18

Taken with the user after a requester handoff from a product team using CBC,
and after Chapter Masters in daily use kept implementing instead of managing.

| ID | Decision | Why | Rejected |
|---|---|---|---|
| I-15 | A Chapter Master delegates implementation. It manages one feature end to end, across several PRs and parallel tasks: it discusses details with the user, briefs native sub-agents, checks their work and owns the integrated result. It writes code itself only when the user explicitly asks. Helpers coordinate with it through the host's native mechanisms, not CBC, and the user can talk to a helper directly. | A Chapter Master that is coding can't take the next task, answer a question or run work in parallel. Both host apps are built around spawning sub-agents and reading their results; CBC should use that, not fight it. | "You may code directly" (the current role skill). A CBC room per helper. |
| I-16 | Skills are never edited per user. Each role can have an **extension file**: instructions every agent in that role must read at start, if the file exists. Extensions are committed in the user's own project at one of four levels: user, organisation or project, repository, or subfolder. The nearest extension replaces the outer ones completely. A missing extension costs nothing. Installation writes a minimal default. | CBC is a framework and every team works differently. The same user runs different setups in different projects, so one configuration per installation is wrong. Extensions are small, so reading them at every start is cheap. | Editing the skills per user. One profile per installation (I-09). Merging extensions from several levels. |
| I-17 | *Order superseded by I-27.* A new agent's start sequence: read the role skill, read the applicable extension, rename the chat for its task, then register. Registration creates the report room in the core, and the Primarch is told by a push, with no room ID passed by the user and no polling. | Chapter Masters skip the rename today. Cause not known: the skill in this repository already puts the rename first and preserves only an explicit user title, so either the installed copy differed or the agent mistakes the host's automatic title for a user title. To be checked against the installed revision. | Relying on the skill text alone for the rename. |
| I-18 | *Who records the event: superseded by I-24. Envelope and storage: settled by I-25 and I-26.* Tracker updates are outbox events. A Chapter Master or Primarch records the update in a fixed template and never calls tracker tools itself. Pending and failed updates show on the dashboard and never block implementation. *Proposed by the implementing agent, not yet confirmed by the user (open question 9): the events use the existing outbox with no second delivery path, and CBC owns only the envelope, meaning a stable ID, an idempotency key, a state (pending, done or failed) and the external record reference written back afterwards.* | The implementing agent stops spending tokens and attention on tracker tools. A retry can't create a second task. A missed update is visible without anyone checking (I-13). | The Chapter Master writing with its own tools (I-11). A separate tracker queue beside the outbox. |
| I-19 | An optional **tracker agent** carries out tracker events. It is a CBC member woken by the outbox, and it is shown on the dashboard alongside the Emperor. Its extension file says which tool the team uses and how. It reads what is already in the tracker, keeps human edits, and consults the Primarch about scope. It asks the user only when the answer is still unclear. A team may use another executor (a script, or the tracker's own AI) if it reports results back the same way. | A tracker is a tool people edit by hand, so writing to it needs judgement about what is already there. Only one agent needs the tracker's details, and nobody else carries them. | A deterministic sync with no model involved. Every agent knowing the tracker. |
| I-20 | CBC defines default event names, not workflows: task created, task changed, PR linked, blocked or resumed, done, dropped or superseded, and feature created or linked. The mapping to a tool's fields, task granularity and progress calculation belong to the user's extension. Trackers get only these major events; the dashboard gets the detailed updates. | A tool-specific rule in CBC (I-10's one task per PRD, slice-count progress) is right for one team and wrong for the next. | Provider adapters or a fixed schema inside CBC. |
| I-21 | Feature association needs no question when the user or the assignment already states it. Otherwise the tracker agent asks the Primarch, and asks the user only if the Primarch can't settle it. | Asking the user about something already decided is the old approval tax. | Always asking the user. The agent guessing silently. |
| I-22 | Trial 1 runs unchanged. I-15 to I-21 are exercised in the next trial, on a small real feature in a user's product repository, with no production promotion unless the result is good. This changes T-01 for that trial only. | A mid-run change makes trial measurements incomparable (T-15). A structured real product shows whether the tracker flow helps, which a CBC feature can't. | Changing trial 1 mid-run. Testing the tracker flow on CBC itself. |
| I-23 | The Primarch answers scope and context questions from Chapter Masters and the tracker agent. This restates I-02 and I-03 and adds the tracker agent as a consulter. It does not make the Primarch an orchestrator. | The Primarch is no longer occupied with PRs and reviews, and it is the agent that holds the whole repository's context. | Routing tracker questions to the user first. |

## 2b. Decision record, 2026-11-21

Taken with the user after the review of PR #169 found four gaps in the tracker
design (open questions 4 and 10 to 12).

| ID | Decision | Why | Rejected |
|---|---|---|---|
| I-24 | The core creates tracker events, in the same transaction as the work-item revision that causes them, and only for projects with a tracker extension. The extension maps each event to its effect in the tool. An agent records an event only for a lifecycle point the core can't observe. | If the agent records the event after the transition, a crash between the two leaves nothing pending, and a missed update is invisible, which is what I-13 forbids. The core already records transitions as atomic revisions. | The agent recording events (I-18 as first written). A periodic reconcile to catch missed ones. |
| I-25 | The executor writes the CBC event's stable ID onto the external record (a property or a hidden marker, as its extension says) and searches for that ID before creating anything. It marks the event `in_progress` before the external write; an event found `in_progress` after a crash is checked against the tracker before any retry. | A crash after the tracker creates the record but before the reference is written back would otherwise duplicate the record on retry. Most trackers offer no idempotency key of their own, but nearly all can store a field. | Relying on the tracker's own idempotency key. CBC's idempotency key alone. |
| I-26 | Tracker events have their own table and state (`pending`, `in_progress`, `done`, `failed`), independent of rooms and room delivery. An executor drains the pending events when woken; an executor configured later claims the backlog, within an age limit the extension may set. This answers open question 9: the envelope fields stand, but the events are not delivered through room inboxes. | The existing outbox delivers only to open report rooms, so with no room an event would be pending nowhere and nothing would be left for a later executor to collect. | Reusing room delivery for tracker events. |
| I-27 | A new agent's start order is: register; read the extension whose path registration returns; rename the chat to the name the core chose; report the rename's result. The core resolves the extension from the member's working directory and chooses the name. | The core becomes the single source of the extension path and the name, and every step leaves a record the trial can check, including the rename agents skip today. The Primarch hears of the member before the chat is renamed, which doesn't matter because its map uses the core's name. | I-17's order (extension and rename before registering). A separate read-only lookup call before registration. The agent searching the four levels itself. |

## 3. Tracker profile (superseded 2026-11-18, kept as history)

*Replaced by section 3a. The profile below is the I-09 to I-12 design.*

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

## 3a. Extensions and the tracker outbox (2026-11-18)

### Extension files

| Level | Typical use |
|---|---|
| User | Personal defaults for every project |
| Organisation or project folder | One tracker for every repository of a product |
| Repository | A repository with its own conventions |
| Subfolder | A part of a repository that works differently |

The nearest extension that exists replaces the others for that role (I-16).
Board and database identifiers live in the user's own project, never in this
repository. Credentials stay in the connector, an environment file or the
Keychain, never in an extension. The file names and the lookup rule are open
question 4.

### Tracker update flow

*Updated 2026-11-21 by I-24 to I-26.*

1. A work-item revision commits. In the same transaction, the core creates the
   tracker events that revision causes, if the project has a tracker
   extension (I-24). An agent records an event only for a lifecycle point the
   core can't observe.
2. The events are stored in their own table with their envelope and state
   `pending` (I-26). The tracker executor is woken like any member and drains
   them; one configured later claims the backlog.
3. The executor marks an event `in_progress`, reads its own extension, searches
   the tracker for the event's stable ID, and creates or updates the record,
   storing that ID on it (I-25). It then writes back the external record
   reference with `done`, or `failed` with the reason.
4. Anything pending past its due time, or failed, shows on the dashboard for
   that work item. Implementation never waits for it.

Proposed template fields (not decided): event name, work item, feature
reference if known, PR references, and a one-line summary written for people.

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
2. **A Chapter Master retired before its tracker work is done.** Mostly
   resolved by I-18: a tracker event belongs to the outbox, not to the agent
   that emitted it, so it survives the emitter's retirement. With no executor
   configured, it stays pending and visible.

Opened 2026-11-18:

3. **Sub-agent or session.** I-15 says the user can talk to a helper directly
   and the Chapter Master stays free while helpers work. A harness sub-agent
   and a separate native session behave differently on both counts, and
   between Claude and Codex. Whatever the choice, spawning must not block the
   Chapter Master, each writing helper needs its own worktree, and the sampler
   must measure the extra memory. To be tested in the next trial, not decided
   from documentation.
4. **How an agent finds its extension.** Either the agent searches the four
   levels itself, or registration returns the resolved extension path and
   the chat name to use. The second can be checked; the first depends on each
   host's file tools. Proposal: the second.
   *Review of PR #169:* the second conflicts with I-17, which reads the
   extension and renames before registering. **Resolved 2026-11-21 by I-27:**
   registration comes first and returns the path and the name.
5. **One Chapter Master per feature, so what is a task?** The requester
   handoff maps one Chapter Master assignment to one task. Under I-15 a Chapter
   Master holds a whole feature, so the natural task is the unit it hands to a
   helper. The Chapter Master emits that task's events. Undecided.
6. **Tracker agent: always running or on demand.** A long-lived session is one
   more thing that must survive restarts and compaction, and every spurious
   wake costs a model turn. An on-demand executor that drains the outbox and
   exits avoids both but rereads its extension each time. Decide from trial
   evidence.
7. **Human edits seen by the executor.** I-19 keeps them. Whether the executor
   may report a conflicting human edit back into CBC's records, or only note it
   on the event, is undecided. I-12 rejected reading tracker state back.
8. **Deployment visibility.** The requester handoff also asks for main versus
   production differences, evidence of what is actually serving, and a
   disposition for old worktrees. That is a separate problem from tracking and
   is not part of I-15 to I-23. Whether it becomes its own stage is the
   user's call.
9. **The outbox envelope.** I-18's envelope fields and the reuse of the
   existing outbox are the implementing agent's proposal. The requester
   handoff asks for idempotent delivery and a stable external identity, which
   this meets. **Resolved 2026-11-21 by I-26:** the envelope stands; tracker
   events get their own table rather than room delivery.

Opened 2026-11-21, from the review of PR #169:

10. **Who creates the tracker event.** In section 3a the agent records it. If the
    work transition commits and the agent stops before recording the event,
    nothing shows as pending, which breaks I-13. Alternative: the core creates
    the event in the same transaction as the work transition. **Resolved by I-24.**
11. **Retry across the external write.** If the tracker creates a record and the
    executor stops before writing the reference back, a retry creates a second
    record. CBC's idempotency key alone can't prevent that; the executor needs
    the provider's idempotency key or a stable ID stored on the external record.
    The next trial should exercise that crash point. **Resolved by I-25.**
12. **Pending with no executor.** The current outbox delivers to open report
    rooms only; a revision with no room has an empty delivery list and is
    pending nowhere. A tracker event needs a durable record that doesn't depend
    on a room, and a way for a newly configured executor to claim it.
    **Resolved by I-26.**

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
| Tracker profiles and sync obligations (I-09 to I-13) | Superseded by the row below |
| Delegating Chapter Masters, start sequence and extensions (I-15 to I-17) | Skill source update after trial 1; exercised in the next trial |
| Tracker events in the outbox, tracker agent, default event names (I-18 to I-21) | Built for the next trial, on top of the existing outbox |
| Next trial on a product repository (I-22) | [Next trial backlog](next-trial-backlog.md) |
| Dashboard conversation with a Chapter Master | After #119 and existing-session messaging qualification |
