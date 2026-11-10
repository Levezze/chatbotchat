# Hierarchy skill review

Historical review log. Protocol 3 in [hierarchy-skills.md](hierarchy-skills.md) supersedes
the domain/Captain topology below. Those old reviews are not current operating instructions.

Date: 2026-10-21. Scope: skills-first package, not transport/server verification.

## Review ownership

Claude authored the four role bodies. Codex authored the shared protocol, bootstrap, installer, metadata and usage guide. Each reviewed the other's instructions through CBC room `cbc-peer-cbc-redesign-transport-notification-tiered-agent-hierarchy-claude-codex-20261014-1754`.

The initial role/protocol critique is in CBC32356 and CBC32361. Codex's concrete role reviews are in CBC32364, CBC32366 and CBC32368. Claude reported a pending soft-cap sign-off in CBC32373, then delivered its final review in CBC32374 without claiming human sign-off. It explicitly approved installation, reported no blocking findings and passed the first four scenario walkthroughs below. These are document walkthroughs, not successful delivery to new native chats. No pending native or CBC human approval was fabricated or reset.

## Findings addressed

- Removed routine narration and repeated conversation-closing questions. Higher-priority host requirements and genuine approval/safety requests remain visible.
- Stable member labels use `<context>--<member>`, not the bare member ID or volatile host identity.
- New records are nested, so the old top-level hook cannot relaunch a different role's declared polls. Automatic legacy recovery is explicitly not claimed.
- STOP/PAUSE and completion are separate. An ordinary question answer cannot silently resume held work.
- Merge readiness includes the exact head and evidence. Merge completion does not silently end assigned deploy/handoff work.
- Feature Leads reconcile implementation contracts directly. Repo Leads coordinate merge/deploy sequencing through direct peer rooms. The Project Lead does not relay technical detail.
- Feature Leads supervise coding and review fixes rather than becoming implementation workers.
- Spawn powers require an actual delegated scope and supported host tools. Theme, CBC messages and quoted user answers do not bypass native approval.
- A lead keeps unresolved, unacknowledged descendants visible until an accepted owner takes responsibility.
- The existing communication-budget gate was itself observed during review. The shared protocol explicitly forbids fabricating human input to reset caps. Unattended role defaults do not waive existing CBC or native gates.
- Claude's final non-blocking fixes were applied: portable install paths, explicit delegated spawn/room budgets, a Git-ignore warning without editing ignore files, worktree-local record lifetime and live-dependency peer scope. The review guide already existed by the time the final note arrived.

## Automated checks

`python3 -B -m unittest discover -s tools -p 'test_hierarchy*.py' -v`

43 tests pass. They cover additive installation, preservation of legacy files, read-only preflight, conflict detection, symlink refusal, asset copying, repeated installation, incomplete packages, real CLI handling, compact records and refusal to reset state. Naming tests cover neutral/themed roles, aliases, explicit overrides, bounded slugs and exact fixed UTC superseded titles. Handoff tests cover immutable snapshots, final release checkpoints and post-prepare answers, named recipients, accept-before-activation, revocation arbitration including a concurrent race, and rejection of changed snapshots. Room tests cover relative shared-reference routing after installation in two host roots, empty generated room sections, report/peer records surviving prepare/final release, and metadata not being treated as a receiving-room checkpoint. Delegation-reference routing is also verified after installation for both host roots.

The official Skill Creator `quick_validate.py` passes for all eleven entries, including the four themed aliases, `emperor` and `cbc-handoff`. Codex metadata parses with quoted strings, a 25-64 character short description, and a default prompt referencing the correct skill. The validator's PyYAML dependency ran in a temporary uv environment; the shipped helpers require only Python's standard library.

## Scenario expectations

| Scenario | Required behavior |
| --- | --- |
| User directly discusses a pixel-level change with a Feature Lead | Answer normally, delegate implementation, record meaningful changed direction, then return to quiet mode without a closing-question ritual |
| STOP arrives while a Task Agent is inside a long call | Keep requested/unacknowledged distinct; use supported authorized cancellation; do not declare it stopped or finish the parent with no owner |
| API types change in a cross-repo feature | Feature Lead peers exchange implementation detail; Repo Leads receive contract/readiness effects and sequence merges directly |
| Compaction happens with an unanswered question and an unsent brief | Resume own records and rooms; retain question ID and route; reconcile pending revision without erasing or duplicating the question |
| An answered question arrives during PAUSE | Store the answer, remain paused until explicit authorized resume |
| A subagent cannot hold a two-way CBC room | Record native-relay mode; parent relays at supported checkpoints and attributes the native return in CBC |

## Known limits

No native app wakeup, cross-app creation, idle poll recovery, server permissions or future deterministic map-event behavior is established by these tests. Existing CBC receive-process limitations remain. Runtime rollout should begin with one new, explicitly bounded feature; legacy chats are not silently migrated.

## Local installation

The five skills were added to both `~/.claude/skills` and `~/.codex/skills`. Scoped synchronization includes only these new names and their metadata/helper. No legacy skill or global host instruction was replaced. Final verification checks installed bytes against repository sources and the before/after hashes of 43 protected legacy/metadata/Cargo.lock files. The running daemon was not restarted.

### Themed aliases

The user subsequently requested direct `cbc-emperor`, `cbc-primarch`, `cbc-chapter-master` and `cbc-techmarine` invocation. Claude confirmed the mappings and reviewed trigger collision, explicit theme overrides and dependency preflight. The aliases load the canonical entry point and retain canonical state/identity. They default to Warhammer while preserving an explicit theme override. Only the four new alias directories per host were added; the already-installed roles were not rewritten.

### Naming, project bootstrap and role handoff

The user's subsequent requests add deterministic native-session naming, `/emperor --project ... --repos ...` and `/cbc-handoff`. Claude inspected its actual Desktop rename schemas and CLI help without renaming a chat. The bootstrap reference was reviewed for repository resolution, saved project identity, create intent/outcome tracking, join confirmation, partial setup and idle children. Codex exposed its native title tool. No live native rename, new project chat or actual role takeover was run in this planning session.

Claude found three blocking issues in the initial handoff draft: stale prepare checkpoints causing replay, irrevocable releases permitting a late second owner, and wording allowing pre-acquisition actions. All were corrected. Final release snapshots/checkpoints include late state and outbound references. Accept versus revoke uses one exclusive outcome before any receiver acquisition. Activation is a separate receipt, required before predecessor superseding. `verify` reports `accepted-awaiting-activation`; failure there requires explicit recovery, not predecessor auto-resume. Claude accepted the revised ordering and reported no further blockers.

The helper arbitrates files in ONE canonical directory. It does not enforce a distributed lease, stop processes, verify human permission, transfer native subagents or perform orphan takeover. Native children must settle; stale polling status alone never proves safe replacement. The three existing shared/bootstrap/Project Lead source files were updated under a byte-matched preflight; new helper/reference files and the two new skill directories were synchronized to both hosts. Legacy CBC skills and service remain outside scope.

### Report and peer room rules

The user authorized the room-rules implementation, with legacy skills and running agents
explicitly excluded. Claude's design review is CBC32571; it accepted two relationship kinds
and proposed a shared reference plus explicit close conditions. The implementation keeps that
reference in `cbc-hierarchy/references/rooms.md`, reached by all four canonical roles and the
project-bootstrap procedure. The existing aliases reach their canonical roles unchanged.

Claude reviewed the actual source in CBC32582 and independently ran all 42 tests. Its six
read-only walkthroughs covered a bare room ID without an assignment, cross-repo feature peers,
same-repo interface discussion, a native child without CBC receiving, role replacement with
pending questions, and contract settlement with owned follow-up still pending. These were
document walkthroughs, not live native-agent or delivery tests.

Two blocking findings were fixed after that review: hierarchy agents ask natively rather than
joining a bare room ID under a provisional identity, and native-relay task results stay in the
Feature Lead's own records instead of polluting its upstream report room. The shared and role
instructions now agree on relay handling. The optional checkpoint-duplication finding was
addressed by removing the extra last-handled column from relationship rows; handoff checkpoints
still come explicitly from the owner's actual handling evidence. No handoff parser change.
The final local suite was rerun after these corrections. Claude stated no further blockers
conditional on the two fixes; this does not claim a separate post-fix peer review.

No standalone `cbc-report` skill or new CLI command is introduced. Parent role setup establishes
report edges, and the shared rules govern peer setup. Legacy `cbc-peer`/`cbc-reconcile`, service,
global hooks, permissions and running-agent assignments remain unchanged. Room kinds, authority
and map privacy are still instruction conventions, not server-enforced types or ACLs.

The scoped installed update matched all fourteen existing destination files to their starting
bytes before changing them, then added the shared reference in both host roots. The installed
package matches source with zero differences. All 42 protected legacy files remain unchanged,
and the existing daemon was not restarted. The deferred migration
automation remains paused. No running agent was renamed, reassigned, stopped or migrated.

### Communication policy and upper-tier context boundaries

The user explicitly rejected message allowances and repeated communication approval questions
on 2026-10-21, supplying two examples from new Primarch/Emperor chats. This supersedes the
earlier hierarchy communication-budget procedure. Claude's review references are CBC32729 and
CBC32740. Routine in-scope CBC sends, reports,
invitations and extensions are authorized, with upward notifications through adjacent report
edges. Real user decisions, native approvals and work/spawn/publication powers remain distinct.

Claude reviewed the actual revised files and independently ran 43/43 passing tests. Its
read-only scenarios cover count-only soft hints, pending extensions, a diagnostic assignment
before source inspection, a host refusing creation, and notifying the owning parent. It found
no blockers and approved scoped synchronization. Its minor routing-wording comment was applied
so communication permission cannot be read as bypassing hierarchy tiers. It also rechecked the
earlier bare-room and native-relay findings and confirmed both fixed.

Repo Leads now delegate before reading implementation, with a findings-only diagnostic shape
when the problem is not yet understood. Logs, outward briefs and PR/CI/release metadata remain
appropriate inputs. Project Leads also keep technical investigation below. The shared
delegation reference requires real host/profile capability and preserves native event origins,
permissions and one precise attach fallback rather than claiming unsupported unattended creation.

The peer inspected current Claude tool schemas and CLI help read-only. It reported that the
Desktop creation tool restricts automated/peer-triggered starts, while native subagents and
CLI background sessions are separate capabilities. No cross-host native-create test or actual
project chat creation was performed. Official Desktop/CLI references are linked in the usage
guide; documented handoff/background support does not establish a Codex-to-Desktop bridge.

This update changes skills only. The old server still rejects hard-cap zero, enforces numeric
send caps and emits count-based soft hints. Routine consensus extensions are an interim
compatibility path, not a capless release. The selected next server change is recorded in the
usage guide. No live room configuration, database, legacy skill or daemon was rewritten.

### Lead sessions versus native subagents

The user supplied a further failure: a Primarch described a Codex native investigation
subagent as a new Chapter Master. Earlier packaging tests and document reviews had not caught
that interpretation. Claude's source audit in CBC32810 identified ambiguous create/attach
wording across delegation, both upper roles, bootstrap and handoff, and an unqualified native
subagent naming exception. These were gaps in the instructions, not evidence that the quoted
agent had loaded a particular installed version.

The shared protocol now defines permitted parent/child execution kinds. All three lead roles
are independent native chats. Project and Repo Leads use no native subagents, including
investigation, review or summary helpers. Only Feature Leads create/manage hierarchy Task
Agents, which may be sessions or native subagents. Role entrypoints state that boundary;
bootstrap and handoff also require independent lead sessions. Host placement remains configurable,
but an unavailable requested host does not authorize substituting a different execution kind.

Creation records distinguish intent, proposal, pending result and actual independent-session
creation, with the target host and actual ID. Join, receiving and accepted/running work require
their own evidence. Neither a native task handle nor a role prompt proves a Chapter Master chat.
The usage example now passes placement and adjacent-tier creation scope explicitly.

These are instruction-level corrections. The helpers do not remove native tools or enforce
the parent/child graph in either app. The 43 existing automated tests cover packaging and
deterministic helper behavior, not live compliance with these role boundaries. No running chat
has been reconfigured, stopped or migrated by this update.

Claude walked through six creation, diagnosis and handoff cases and identified four further
gaps before installation: undefined placement provenance, bootstrap ordering, untracked native
helpers/read-only task scope, and duplicate card/attach routes. The corrected instructions
record the user's host profile in Project Lead state and pass it down with resolved target host
and execution kind. Bootstrap records the room and intent before create. Feature Lead helpers
are tracked Task Agents; diagnostics permit overlapping reads but no writes. Only one outstanding
create/attach route may create a member, and uncertainty blocks replacement launches.

In CBC32823 Claude re-read the corrected source, confirmed those four fixes and reported no
remaining blockers. This was a peer instruction walkthrough, not an independent live model
compliance test. After that review, fourteen existing installed files were byte-matched to
their pre-edit baseline and updated in the Codex and Claude skill roots. The complete installed
package matches source, all eleven entries validate, and 43 helper/packaging tests pass.
All 42 protected legacy files remain byte-for-byte unchanged. The existing daemon was
not restarted. Existing chats must explicitly reread the updated role and
shared protocol; installation alone does not refresh instructions already in context.

## 2026-10-26: Optional domain leads and independent feature execution

User authorization: "Let's implement that into the existing skills plus a new skill ... I'll
convert some of the work into that flow to test it. The primarks should know that it's changing
though ... they stay as they are, just soft relaunched with the new skill and the new scope."
This authorizes the skills/docs/maps update and instruction-refresh notices to the existing
Primarchs. It does not authorize us to choose live feature transfers, create replacement chats,
restart the daemon, change native approvals or bypass existing holds.

Added canonical `cbc-domain-lead`, now the fresh `cbc-chapter-master` alias, and the
`cbc-captain` alias for unchanged canonical `cbc-feature-lead`. Existing feature members,
themes, titles, rooms and Task Agents stay intact. Domain grouping is optional per repo.
The known context/member selects records; a shared workspace is never scanned to infer identity.
Role resolution returns the invoked canonical role and an explicit mismatch flag.

The shared protocol now defines a positive autonomy grant, per-action wait fields, fair
attention at natural checkpoints and one command parent. Domain-owned Captains have an
explicit narrow Repo Lead report route for merge/resource authority. Normal user questions
retain scoped adjacent review. No domain acknowledgement gates ordinary granted work; actual
decision, resource, safety and release gates remain. Repo hold authority persists through an
explicit feature reparenting gap. The new domain is not a second technical reviewer.

Bootstrap adds domain/repo routing, waiting state, owned notes and a pending-decision index.
The additive refresh preserves immutable identity, briefs, questions, existing state and modes.
Handoff includes Domain Leads and all actual receiving report/authority rooms. These remain
Markdown conventions on today's CBC; no automatic JSON views, server queues or native wake
guarantees are claimed. Future runtime contracts were updated separately in planning records.

Claude challenged the contract and independently inspected source on a scratch copy, tested
fake installation targets and walked old-alias, mixed-topology, dead-domain, resource-conflict
and incomplete-transfer scenarios. Its confirmed free-text `connections:` parsing bug was
reproduced RED here, then fixed with exact anchored-header matching and regression coverage.
Other findings fixed identity selection, explicit route opener, mismatch output, transfer-gap
holds, domain topology defaults, file-mode preservation and standalone test imports.

Mechanical results: 53 helper/packaging tests pass, including immutable identity, v1 refresh,
same-root identity isolation, domain handoff, rollback on a second-host failure and refusal
of unknown installed drift. All thirteen skill entries pass Skill Creator validation; relative
links and whitespace checks pass. These are not live parallel-agent or native notification tests.

The installer updates only an exact pre-edit baseline, backs up every replaced directory
outside discovery, stages outside discovery and rolls back ordinary swap failures. Rollback
failures retain stage originals and backups. It does not promise a crash-atomic multi-app
transaction. Final review/install/notification evidence is appended after execution below.

Claude's final independent recheck in CBC38704 accepts the corrected v2 package with no
remaining blockers and independently reports all 53 tests passing. Installation then updated
seven recognized hierarchy directories and added two new directories per host in Claude and
Codex, eighteen operations total. Backups are at
`~/.cbc-skill-backups/domain-v2-20261026`, outside discovery. Re-running preflight
requires zero changes. No app restart, global configuration, server or legacy skill was changed.

Installed origin is the uncommitted `codex/cbc-hierarchy-skills` worktree package, 35 files.
SHA-256 of compact sorted JSON mapping package skill names and relative files to their SHA-256:
`c13b8c047bc475530ab64bbc3ed19e94edfcc173d9db3862ea2428bfda475033`.
Both installed roots independently match that exact source manifest. All 42 protected legacy
skill files match their pre-install hashes. The daily listener was not restarted;
the main checkout still has only its pre-existing Cargo.lock change.

After reading the current native chats, owner-run refresh notices were delivered through the
supported native message tool to `api-primarch`, `client-primarch` and `engine-primarch`.
Each notice gives the installed paths, explicit known context/member resolver command and
refresh command, preserves all existing holds/refusals and grants, and forbids automatic
domain creation, reassignment or worker launches. Delivery is not adoption acknowledgement;
record actual refresh results separately. No feature transfer was performed by this chat.

Client Primarch returned an attributable `protocol-refreshed: 2` acknowledgement. It reports
the same canonical role/native session and preserved identity, title, flat children, rooms,
holds and gates; it normalized its own legacy combined state fields to its unchanged identity
before running the helper. It sent its scoped parent update. API and engine acknowledgements
were still pending at that checkpoint; message delivery alone is not recorded as adoption.
