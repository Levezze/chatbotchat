# CBC working hierarchy

Two operational roles. A future dashboard Emperor sits outside their reporting chain.

| Invocation | Neutral name | Owns |
| --- | --- | --- |
| /cbc-primarch | Repo Lead | Parallel delivery across Chapter Masters, repo priorities, dependencies, resource allocation, merge order and release coordination |
| /cbc-chapter-master | Feature Lead | One implementation end to end, including direct coding and native implementation subagents |
| /cbc-emperor or /emperor | Project Lead | Future dashboard status conversation and user instructions to Primarchs; dormant before the dashboard |

Primarchs run in Codex. Chapter Masters and their native subagents run in Codex or Claude as
the user assigns. Theme remains optional.
Generic entrypoints are cbc-repo-lead, cbc-feature-lead and cbc-project-lead.
Captain and domain coordination layers are retired.

## Working contract

A Chapter Master is a finite task or umbrella-feature owner, not a permanent domain session.
Its related parallel implementations, fixes and later diagnosis retain the original owner and
history. Unrelated assignments get fresh sessions even in the same domain or repository.
For example, an existing Membership/Onboarding owner is suitable only if the requested change
belongs to its actual assigned feature; the name alone is insufficient. Prefer many focused
sessions over a few sessions containing unrelated tasks. Keep the assignment, native session,
CBC identity/rooms and PR/source links after completion, then retire the owner without repurposing it.

A Chapter Master independently plans, edits, delegates to native subagents, tests, reviews,
fixes and prepares its PR within its implementation assignment. It may write code directly.
Ordinary in-scope delegation is included. It does not ask the Primarch for permission at each
step, wait for report acknowledgements, or stop because the parent is discussing another feature.
Routine edits, commits, pushes and PR creation use the assignment's existing authorization
and host permissions directly. The Primarch manages dependencies, access exceptions and shared
resources, without becoming a per-PR approval gate. Its in-scope pause or reconsideration
instructions apply to the affected work; other work continues.

The Chapter Master owns its PR after opening it: tests, CI monitoring and diagnosis, reviews,
fixes, rebases and renewed validation until the current head is technically ready.
The Primarch discusses repo-level scope, resolves dependencies and sequences ready merges,
migrations and releases. At execution it confirms the reported head and required machine
gates. A failed gate goes back to its Chapter Master; the Primarch moves to other actionable
work instead of entering a PR's logs, code or fix/watch loop. Independent work stays parallel.

Only an affected action waits for a real dependency, conflicting write, explicit hold or native
gate. A scarce server or heavy test slot limits its actual operation. It does not serialize
source implementation, require every feature to enter a laptop queue or stop independent work.
Actual resource limits still matter; scheduling belongs at the operation that consumes them.

Reports are meaningful status, dependency changes, genuine blockers/questions, readiness and
completion. Task splitting and review/fix churn stay local. Native subagents need bounded write
ownership and results, not their own CBC role maps, report rooms or handoff rituals.
Use peer rooms for relevant implementation contracts, and Primarch peers for repo sequencing.

## Questions and decisions

A Chapter Master sends a genuinely unresolved question through its existing Primarch route.
The Primarch decides within its mandate or presents the original question to the user, adding
notes only when its repo context matters. A large technical discussion can move to the owning
Chapter Master chat. A routine question does not require an Emperor or another approval tier.
Preserve the answer and act on it; do not ask the user to repeat the same decision in each chat.

Native permission review remains a host mechanism. These instructions remove invented CBC
gates, not native controls. Route a real denial once with its exact action and supported remedy;
continue unaffected work. Do not proxy a refused operation through another actor.

## Records and dashboard

The current helper creates identity.json plus compact state.md, brief.md and questions.md
under .cbc/hierarchy/<context>/<member>. It refuses overwrite. Keep records out of source commits.
Source plans, issues and product decisions remain in the user's chosen tools.

State records current work/rooms/helpers, next action and genuine blockers. Brief records coarse
status, meaningful update time and evidence links. Questions records unresolved decisions.
Update existing fields at meaningful transitions, not every tool action.

The isolated messaging core will own deterministic JSON projections for room unread state,
status updates and the eventual dashboard. Current owner-written files do not provide that
automation. The dashboard reads shared status and question data directly. Its future Emperor
discusses that data with the user and sends user instructions to Primarchs. Primarchs may reply
to clarify those instructions; they send no routine reports or escalations to the Emperor.
Today's human questions go directly to the user through Primarchs. No Emperor setup, report
receiver or periodic status-chasing is needed before the dashboard exists.

## Installation and refresh

The targeted installer updates only the hierarchy package, preserving legacy CBC skills and
runtime. Dry-run with:
```bash
python3 tools/install_hierarchy_skills.py --check --target ~/.codex/skills --target ~/.claude/skills
```

For an update, supply --previous-manifest with a reviewed exact baseline and a new --backup-root.
The installer refuses unknown installed drift, stages changes and retains recoverable backups.
It does not edit host permissions, restart the daemon or migrate active work.

Installed files do not replace a running chat's context. Reload the current shared protocol
and role in that same chat. Keep its assignment, branch, actual native handles, decisions,
rooms and explicit holds. New work goes directly Primarch to Chapter Master.
Existing implementation owners keep their work; do not duplicate writers or transfer branches
merely to adopt a new label. Retired domain sessions provide a short ownership inventory when needed.

## Reload after compaction

An enrolled Primarch's `SessionStart` event with source `compact` reloads the current installed
Repo Lead instructions before continuation and points to the shared protocol. This is a bounded
command invocation, not polling, a scheduled model turn or a background listener. It reads no
project history and makes no permission decisions. Other chats and event types receive nothing.

After installing the skill package, configure each desired host with Python 3.11+:
```bash
rtk proxy python3 tools/install_primarch_compaction.py --host codex --backup-root /private/tmp/cbc-codex-hook-backup
rtk proxy python3 tools/install_primarch_compaction.py --host claude --backup-root /private/tmp/cbc-claude-hook-backup
```

Use new private backup paths. The installer adds only the compact hook and preserves existing
hooks, settings, native trust decisions and permissions. A Primarch invocation enrolls its native
session with the installed helper. See [compaction setup](../bins/cbc/skill/cbc-hierarchy/references/compaction.md).
Codex requires native `/hooks` review/trust for the new definition. Do not claim live activation
from a configuration write or synthetic hook test; existing sessions may need a supported
reload/resume, and managed policy can prevent a hook from running. Never bypass those controls.

Historical cbc-captain invokes the same Chapter Master implementation role. cbc-domain-lead is
a compatibility notice, not a new coordinator. cbc-task-agent and cbc-techmarine remain compatible
for old independently assigned workers; new native helpers need no separate CBC registration.
The helper retains old identity formats for recovery but creates only the three active lead types.

## Names, rooms and recovery

The Repo Lead and Feature Lead entrypoints start with three explicit actions: apply/verify
the role's native title, create/join the assigned relationship and show its ID once, then
verify receiving/recovery. Aliases reach those same entrypoints. A rename failure stays
cosmetic. An unassigned Primarch creates no Emperor relationship.

The replacement's proposed ten-minute recovery check belongs to the shared connector and
causes no model invocation when nothing is unread. It is separate from hourly attention.
The current legacy receiver remains necessary until adoption. Native prompt-based schedules
are an explicitly selected fallback with model/idle limits, never a claimed token-free check.
See [recovery checks](../bins/cbc/skill/cbc-hierarchy/SKILL.md#recovery-checks).

New themed names are <repo>-primarch and <repo>-chapter-master-<feature>.
Use supported native naming tools; a cosmetic rename failure does not stop work.
Resume stored identities and user titles. Theme adds no authority or flavor-only messages.

Today's CBC rooms remain two-party report or peer relationships. Reuse one route per active
relationship. Routine coordination needs no per-message human permission or budget.
The current daemon's numeric caps still require normal consensus extensions; the capless
messaging core is separate. Do not claim a skill rewrite removed a server-enforced cap.

cbc-handoff remains an on-demand session replacement utility, preserving unfinished work and
preventing two sessions using one identity. It is not the normal assignment, status or phase
transition mechanism. Legacy cbc-orchestrator and cbc-worker stay untouched.

## Validation

```bash
python3 -B -m unittest discover -s tools -p 'test_hierarchy*.py' -v
```

Tests cover names, active role creation, historical record preservation, scoped installation,
rollback and cooperative replacement artifacts. They do not prove agent behavior or native
app permissions. Monitor actual delivery and shorten instructions when a rule adds ceremony
without improving the result.
