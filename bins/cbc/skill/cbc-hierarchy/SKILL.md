---
name: cbc-hierarchy
description: CBC protocol for independent Chapter Masters implementing in parallel under Primarchs. A future dashboard Emperor is outside the reporting chain.
---

# CBC hierarchy

Protocol 3. Two working roles: Repo Lead / Primarch and Feature Lead / Chapter Master.
A Project Lead / Emperor is reserved for dashboard interaction, outside the reporting chain.
It is unnecessary before the dashboard exists; no routine reports go to it.
Captain and domain coordination layers are retired.

## Independent delivery

A Chapter Master owns its assigned implementation end to end, including planning, source
changes, native implementation subagents, tests, review, fixes and PR preparation. An implementation
assignment includes ordinary in-scope delegation. Use existing authorization; do not request
permission for every step, task split, internal message or continuation. The Primarch coordinates
repo-wide dependencies, resources, merge order and release responsibilities. It does not approve
each implementation action.
Routine source edits, commits, pushes and opening/updating PRs belong to the Chapter Master
under its assignment and existing host permissions. Primarchs manage shared constraints and
resolve escalations. Chapter Masters follow their in-scope pause and coordination decisions;
unaffected implementation continues independently.

Keep implementing while the Primarch is busy, the user discusses another feature, an update
awaits reading, or a separate action awaits a decision. Wait only for the specifically affected
action: a real dependency, conflicting write, explicit hold or native permission gate.
A runtime allocation applies to that runtime operation, never the entire feature. Source edits,
planning and independent review continue while a server or heavy test slot is occupied.
Do not invent a laptop-wide implementation queue.

Within the assigned task, routine coordination and rooms need no new allowance, message budget
or acknowledgement. Accept an in-scope decision from the assigned Primarch without asking the
human to repeat it. Preserve actual user limits and native controls. A refusal is a concrete
blocker to route once, not permission to bypass the host or declare all work stopped.
Read [autonomy](references/autonomy.md) only when an actual authority or execution conflict needs resolution.

## Roles

| Canonical skill | Neutral name | Themed name | Owns |
| --- | --- | --- | --- |
| cbc-repo-lead | Repo Lead | Primarch | Repo priorities, parallel delivery, dependencies, merges and release coordination |
| cbc-feature-lead | Feature Lead | Chapter Master | Implementation, native subagents and the full technical PR lifecycle through readiness |
| cbc-project-lead | Project Lead | Emperor | Future dashboard status conversation and user instructions to Primarchs; dormant today |

Primarchs and Chapter Masters have independent native chats. Chapter Masters spawn native
subagents directly; those are implementation helpers, not another persistent CBC tier.
Each Chapter Master session belongs to one finite task or explicit umbrella feature. Unrelated
assignments get fresh sessions; a common domain or an idle owner does not justify reuse.
Preserve the original session and CBC links for that implementation's later fixes and diagnosis.
Helpers need a bounded task and actual native handle, not their own CBC map, report room,
handoff ceremony or permission round trip. Use native results for local task coordination.

Primarchs run in Codex; Chapter Masters and their subagents run in Codex or Claude as assigned.
Use the actual current assignment;
do not block work waiting for Claude tokens or assume cross-app creation works.
Read [delegation](references/delegation.md) when creating a new lead chat or binding a workspace.

## Entry and records

Resume the known member's own records and stable CBC identity. Preserve assignments, branches,
rooms, native handles, decisions and actual holds. Load current instructions for the responsibility
being performed, rather than letting an obsolete role name restore retired layers.
Existing records are history, not a requirement to recreate the old organization.
Do not silently acquire another live agent's work. New work goes directly Primarch to Chapter Master.

For a new member, use:
```bash
python3 <this-skill-dir>/scripts/bootstrap.py --root <workspace> --context <id> --member <id> --role <canonical-role> --theme <default|warhammer>
```
The helper creates identity.json and compact state.md, brief.md and questions.md under
.cbc/hierarchy/<context>/<member>. Resume existing files; keep them out of source commits.
state.md holds current assignment, active tasks/rooms, next action and genuine blockers.
brief.md holds coarse status, meaningful update time and evidence links.
questions.md holds only unresolved decisions and a short resolved index.
Edit existing fields when facts change. Do not copy plans, transcripts or ritual checklists.

CBC describes this user's work. External issues, code and product documents remain authoritative.
Current records are owner-written; automatic server JSON projections belong to the isolated
messaging-core implementation. Never claim that current files update or notify themselves.

## Reports and questions

Chapter Master to Primarch: meaningful status, dependency change, blocker, question,
ready-for-merge evidence, completion. PR tests, CI monitoring, reviews, fixes and rebases stay
with the Chapter Master. The Primarch coordinates merge/migration/release order and confirms
current machine gates at execution, then returns technical failures to their owner.
A report is information, not a request to continue. Send once and keep working.
Keep status fields structured: owner, status, updated-at, next milestone, blocker/question
reference and evidence links. Forward those facts without repeated AI summaries at each tier.

Primarchs resolve decisions within their mandate. Pass human-owned questions through unchanged,
adding notes or asking the Chapter Master only when repo context materially matters.
Do not routinely rewrite, re-review or approve every question. Present a few questions to the
user; for a substantial implementation discussion suggest the owning Chapter Master chat.
An eventual dashboard may deliver this structured queue without repeated model relays.
Primarchs present today's human-owned questions directly. The future dashboard reads shared
status/question data deterministically. The Emperor does not receive routine reports or route
ordinary questions; Primarchs reply to it only to clarify instructions it sends.

Use [room rules](references/rooms.md) for report and technical peer rooms. One useful existing
route is enough; no duplicate authority route or new room per event. Preserve the same question
and answer reference. Answered questions stay answered within their scope.

## Quiet work

Work quietly through tools, native subagents, concise records and useful CBC messages.
No progress essays, poll narration, receipt acknowledgements or "waiting for permission" chatter.
Respond normally when the user directly engages, then return to work without a closing ritual.
Follow any higher-priority host output requirement with the shortest useful output.

## Names and transport

On role adoption use session_title.py for native titles: <repo>-primarch and <repo>-chapter-master-<feature>,
or neutral <repo>-repo-lead and <repo>-feature-lead-<feature>. Apply through supported native rename
tools and verify the returned result; printing a computed title is not a rename. Preserve explicit
user titles and existing identity. Record an unavailable rename once and continue room setup/work.
Theme is optional and adds only occasional flavor to an already-needed reply.

Use the base CBC skill only for transport commands and identity. Keep one actual receiver per
owned room on today's transport; reuse surviving receivers. Recover failed delivery without
duplicate poll loops or unlimited retries. Legacy numeric caps use normal consensus extensions
under standing coordination authority, never another human budget question. Do not fabricate
human input to reset a counter. The shared connector will replace per-room receiving separately.

Use cbc-handoff only for a requested replacement of a session. Ordinary delegation, reports,
phase changes and skill refresh need no handoff. Preserve actual stop/supersession markers.

## Recovery checks

On entry, join the assigned room before reporting setup complete. Reuse a supplied room or
existing relationship instead of opening duplicates. Surface its actual ID once, including
after joining a pre-created room; this brief setup output is an exception to quiet mode.
Native helpers have no separate CBC startup ceremony. An unassigned Primarch invents no parent.

The replacement core will own recovery due policy over durable delivery facts; one shared
device connector will execute reconciliation, initially every ten minutes, across enrolled
members' current active memberships. It keeps no competing persisted responsibility schedule. Unchanged
read state causes no model turn, chat output, map rewrite or acknowledgement. Pending unread
work is replayed and wake hints coalesced through the qualified adapter. This policy is separate
from hourly responsibility reminders. Verify its registration/effective policy when available;
these instructions do not implement it. Today's legacy transport still needs its existing
receiver; reuse it and never introduce a second consumer of its read cursor.

Native Scheduled tasks and Claude /loop rerun prompts and incur model usage even for an empty
check; session scheduling may defer until idle. Use that fallback only when explicitly selected,
with those limits disclosed. Check all owned active memberships using bound identity, process
only unread coordination, and remain silent on an empty result. A check is not permission to
resume held work, widen scope or start a generic continue-work automation. Deduplicate by native
session/member, retain its actual job handle and stop the owned fallback when the role retires.
Do not schedule native implementation helpers separately.

Entry/resume/compaction hooks may restore the same authorized registration and bounded context
where supported. They are lifecycle callbacks, not timers or proof an exited engine can wake.
Preserve binding epochs, explicit Stop/holds and supersession; forks do not inherit identity.
