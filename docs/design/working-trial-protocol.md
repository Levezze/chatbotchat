# Working hierarchy trial: protocol and decision record

- **Status:** Agreed 2026-11-10. Trial 1 not started; it begins only on the user's explicit go.
- **Stage:** [#113](https://github.com/Levezze/chatbotchat/issues/113), under [PRD #112](https://github.com/Levezze/chatbotchat/issues/112).
- **Companion docs:** [message observability and working pilot](message-observability-and-working-pilot.md)
  (delivery facts, logging, timing semantics), [work records](work-records-and-external-observations.md),
  [agent lifecycle and rooms](cbc-agent-lifecycle-and-rooms.md), [ADR-0011](../decisions/0011-trial-1-decisions.md).

This document has four jobs:

1. Record what was decided before trial 1 and why, including the alternatives that were rejected.
2. Define how a trial is set up, run, measured and stopped.
3. Hold the run log and results of every trial, so the history can be audited later.
4. Say how the next trial should improve on the last one.

The companion pilot document says *what a delivery fact means*. This document
says *how the trial uses those facts*. Where they disagree, the decision record
below is newer and wins; the pilot document carries pointers to it.

## 1. Why the trial exists

CBC is being rebuilt because the old system failed in two ways:

- **Infrastructure.** Every chat ran its own polling shell. Receivers were reaped
  by app restarts, the laptop ran out of memory and battery, and agents waited
  for hours on messages nobody delivered.
- **Workflow.** A busy coordinator became a serial bottleneck. Routine work was
  turned into permission requests. Ready work sat idle while being described as
  "still implementing". The user spent the day clicking approvals in each chat.
  See the [2026-11-06](../case-studies/2026-11-06-notification-activation-coordination.md)
  and [2026-11-09](../case-studies/2026-11-09-release-follow-through.md) case studies.

The candidate core (#120) fixes the transport: one core, one connector per
device, native wake paths instead of per-chat polls. Its native runs so far are
preliminary probes. They show that a marker message can wake a chat. They do not
show that a hierarchy of agents can implement real features in parallel without
the work stopping and waiting.

**The trial answers one question: when agents do real work through the new CBC,
how often and for how long does work stop and wait, and why?** Every other
measurement supports that one.

## 2. Decision record, 2026-11-10

Decided by the user in a grilling session with the implementing agent (Claude),
after Codex handed over the full rationale and incident trail. IDs are stable;
later trials append new decisions rather than editing these.

| ID | Decision | Why | Rejected alternatives |
|---|---|---|---|
| T-01 | Trial 1 builds real chatbotchat features with brand-new agents. | We know exactly what we want built here, so results can be judged against clear requirements. Stakes are real but contained to this repo. | An aevark product repo (real risk to product work). A throwaway repo (proves little). |
| T-02 | Two phases. Phase 1 is a messaging smoke test between two peers. Phase 2 is the implementation test with the full hierarchy. | Phase 1 isolates transport, identity and room defects so they don't contaminate the hierarchy measurement. | Hierarchy from the first minute. |
| T-03 | A Primarch takes part in phase 2. | Intervention by exception is half of the design. A trial without a Primarch can't show whether it stays out of the way. | Add the Primarch in a later trial. |
| T-04 | No polling fallback by default. A native scheduled task, at roughly 5 minutes, is an explicit per-session opt-in with token and idle limits. | Per-chat polling is what we are removing. Scheduled tasks rerun model prompts, so they cost tokens on every tick. | A default fallback. Per-chat polls. |
| T-05 | The fallback is off for the measured run. A session where it was turned on is marked in the report. | A fallback would mask native wake failures, and the trial exists to test native wake. | Fallback on for safety. |
| T-06 | A Chapter Master merges to `main` by default once repository gates pass. | Autonomy through merge is the execution contract. Waiting for a routine yes was one of the recorded failures. | Primarch approval per merge. |
| T-07 | Every work item has a **HITL flag**. When it is on, a ready PR waits for the user's review. When it is off, the Chapter Master merges once ready and reviewed. You or the Primarch can turn it on; only you can turn it off; the Chapter Master can't change it. A HITL wait is counted as "waiting on human review, by policy", not as a stall. | Autonomy is the default, but some work needs a human eye, and that should be a declared property of the item rather than a judgement made in chat. | All-or-nothing autonomy. A Chapter Master deciding for itself when to ask. |
| T-08 | Promotion (for example `main` to `production`) is set per project and defaults to human-owned. The user can tell a Chapter Master to promote against policy; that instruction becomes a durable fact and the Primarch is notified, not asked. | Repositories differ in how they deploy. A merge does not prove deployment, so promotion needs a named owner. | One universal rule. Primarch sign-off on overrides. |
| T-09 | The record layer is built before phase 2: registration, atomic work-item revisions, canonical requests with an owner and a callback, and the outbox. This is a thin slice of #115 moved forward. | Without records, the Primarch can only learn status through chat. Chatty status is the old failure, so the trial would measure the wrong topology. | Run the hierarchy trial on messaging alone. |
| T-10 | Questions are never routed through the Primarch. A question becomes one canonical request, which the user sees and answers in one place. The answer wakes the owner directly. The Primarch reads the same records and may add a note that never blocks. | Routing every question through one agent recreates the serial bottleneck from the 2026-11-09 incident. What the user actually needs is one place to see every question, and records give that without a relay. | The user's first proposal: questions go via the Primarch for now. The implementing agent argued against it, and the user agreed. |
| T-11 | A dashboard skeleton is built before phase 2. It is a plain page served by the core over the same records: work items, open requests and the outbox timeline. | The "read-only listing" of open requests already is a dashboard skeleton. It doesn't need to be pretty, only to read the deterministic records. | A CLI-only listing. |
| T-12 | The user can answer from the dashboard through a browser-held session, created by a command the user runs. The token goes straight to the browser. It is never printed, and never written to the workspace, the CBC state directory or a chat. The browser's own cookie storage is still a file an agent with shell access could read, so trial skills forbid agents from touching browser data or the answer endpoint. Every answer records the channel it came through. The user can still answer directly in a chat. | Agents share the user's shell, so on one machine nothing can stop them cryptographically until device identity arrives (#118). Policy plus an audit trail is the honest version, and an answer through the wrong channel shows up as a violation. | Read-only page with CLI answers. No answers until #118. |
| T-13 | Seats: both Chapter Masters run in Claude Code and the Primarch runs in Codex. | The user's choice. It narrows how much of Codex's native wake path the trial exercises (only the Primarch's wakes); the report says so. | One Chapter Master per app. |
| T-14 | Assignments: CM-A builds the PR/CI observer (#115): register a PR once, then track the checks for its current head and how fresh they are. CM-B builds the feature/timeline view (#116) on top of the skeleton. | Both are real work and neither touches delivery code. CM-B consumes CM-A's facts, which gives them a genuine contract to coordinate directly. | CM-B building the skeleton itself (the skeleton moved earlier under T-11). |
| T-15 | The trial runs a pinned build of the core from its own state directory. No trial agent rebuilds, restarts or migrates it. | Agents build CBC while running on it. A change to the core mid-run would make the measurements incomparable. | Running the core from a worktree an agent can edit. |
| T-16 | If the core breaks mid-trial: a bug that blocks delivery aborts the run, gets recorded, gets fixed through a normal PR, and phase 2 reruns from the start. A bug that doesn't block delivery is logged and the run continues. Nobody hot-patches the pinned build. | Measurements from two builds can't be compared. | Patching in place and continuing. |
| T-17 | Trial skills are installed project-local, under a temporary `wip-cbc-` prefix. Each skill's first line tells the agent to ignore the legacy `cbc-*` skills. The report flags any session that loaded a legacy skill. | Claude Code's documented precedence is enterprise over personal over project, so a project skill can't override a global skill with the same name. The global `~/.claude/skills/` already holds older `cbc-hierarchy`, `cbc-repo-lead`, `cbc-feature-lead` and `cbc-project-lead`. For Codex, repo skills live in `.agents/skills`, and the docs don't say which skill wins when two share a name. | Same names, relying on override (doesn't work in Claude). A project plugin (namespaced, but adds a plugin install). Updating the global skills (breaks the rule that legacy CBC stays untouched until #117). |
| T-18 | Code and data use neutral role names (`project_lead`, `repo_lead`, `feature_lead`). The Warhammer theme lives only in skill names and prose. | Nothing in the schema should depend on a theme. | Themed names in the schema. |
| T-19 | The implementing agent (Claude) merges its own build PRs once three gates pass: the Codex CLI review, `/pr-review`, and green CI on the head SHA. It tells the user afterwards. Anything outside that comes to the user first. | It is the same autonomy contract asked of the Chapter Masters. | Asking before every merge. |
| T-20 | The user opens the three trial sessions from a launch sheet the implementing agent prepares. | The standing rule is that the implementing agent creates no chats except disposable probes. | The implementing agent creating them. |
| T-21 | Phase 2 has a checkpoint at about 5 hours, not a hard stop. At the checkpoint, if it worked well but the time box was unrealistic, it is extended. | AI agents work continuously, so a time box in days is meaningless. | A 2-working-day box. |
| T-22 | Raw evidence (logs, timings, transcripts) stays local in the trial state directory. GitHub gets only the deterministic summary report, with no message bodies. | This repeats the evidence-handling lesson from the 2026-11-09 incident. | Publishing raw evidence. |
| T-23 | Using CBC is pre-authorised. Agents create, join, use and close rooms and send repository information to peers without asking the user. A Chapter Master acts on an in-mandate Primarch intervention without asking. That authority comes from the user's launch message, which states the grant; a peer message never carries user authority. | The user spent months clicking "approve" in chat after chat for routine coordination. Peer messages must stay non-authoritative, because that is a real safety property, so the grant is given up front instead. | Treating Primarch messages as user approval. |
| T-24 | Native permission prompts for CBC are removed with project-local allowlists in the trial worktrees. They cover only the `wip-cbc-` CBC tools and the CBC CLI binary: no broad shell or file allowances, no bypass modes, no global settings. | Host permission prompts are real and CBC can't bypass them. The supported fix is to allow exactly the coordination tools. | Bypass mode. Global allowlists. |
| T-25 | The core assigns an agent's identity at enrollment and binds it to the native session. Tools inject the identity; the model never types it. Compaction, resume and an app restart keep the identity. A fork or a new session gets a new one. When a session ends, its identity is retired. A deliberate handoff is an explicit core operation that rebinds an existing identity to a new session. | Identities typed by the model (`--as <label>`) were a recurring source of mistakes and hallucinated identities. | Model-supplied labels. |
| T-26 | There is at most one active peer room per unordered pair of identities. Opening is idempotent: the second agent to open gets "a room already exists" with its ID and joins it. Either member can close the room alone with a short reason; no consensus vote is needed. Closing frees the pair, and the next open creates a replacement that records which room it supersedes. When a member's session ends, its peer rooms go inactive. | Duplicate rooms between the same two peers split the conversation. A creator-only close fails when the creator's session is gone. ADR-0003 introduced consensus close because unilateral closes carried no provenance; recording actor, reason and the supersede link fixes that root cause directly. | Consensus close (ADR-0003). Close by the creator only. One room per pair *and* topic. |
| T-27 | Waits are derived from facts, not from what agents say, and they are the headline metric (section 6). The approval prompts the user had to click are the second headline metric, with a target of zero CBC-related prompts after setup. | The user's words: what matters is how often work stops and waits, measured as a distribution rather than a single threshold. | Agent-reported blockers. A single pass mark. |

## 3. What is built before the trial

Each piece goes through the normal PR process (T-19).

| # | Piece | Stage | Contents |
|---|---|---|---|
| 1 | Skills PR | #112 skills | Question routing (T-10), the Emperor's scope (dashboard plus project conversation, outside execution), the `wip-cbc-` names (T-17), the CBC authorisation wording (T-23). |
| 2 | Event and timing facts | #113 A | A durable event schema and first-occurrence message and recipient facts in the core, per the pilot document. |
| 3 | Record layer | #113, moved from #115 | Identity binding (T-25), unique peer rooms (T-26), work items with the HITL flag (T-07), atomic revisions, requests with callbacks (T-10), the outbox, and waits derived from those facts (T-27). |
| 4 | Dashboard skeleton | #113, moved from #116 | Work items, open requests and the outbox timeline, plus answering through a browser session (T-11, T-12). |
| 5 | Instrumentation and report | #113 B and C | Connector instrumentation, bounded recovery, the run manifest, and the deterministic report, including the wait and prompt measurements. |
| 6 | Trial setup | #113 | The pinned build (T-15), local skills and allowlists (T-17, T-24), the launch sheet (T-20). |

Explicitly not built before the trial:

- PR observers: that is CM-A's assignment.
- Feature and timeline views: that is CM-B's assignment.
- Responsibility attention, the hourly reminder (#115).
- The Emperor (#116).
- Managed runtime and adoption (#117).
- Multi-device support (#118).

## 4. Trial design

### Roles

| Seat | App | Skill | Owns |
|---|---|---|---|
| Primarch (`repo_lead`) | Codex | `wip-cbc-repo-lead` | Repository priorities, dependencies, shared resources. Steps in by exception with a hold, stop, note, priority change or the HITL flag. Never reviews code or CI and never grants routine permission. |
| CM-A (`feature_lead`) | Claude Code | `wip-cbc-feature-lead` | The PR/CI observer (T-14), from worktree through merge. |
| CM-B (`feature_lead`) | Claude Code | `wip-cbc-feature-lead` | The feature/timeline view (T-14), from worktree through merge. |
| User | dashboard or chat | none | Answers requests, sets priorities, overrides. |
| Trial manager (Claude, the implementing agent) | outside the hierarchy | none | Sets up the trial, watches for aborts, reviews progress, results and measurements. Never acts as a hierarchy role during the run. |

Native subagents belong to Chapter Masters and have no CBC role.

### Phase 1: messaging smoke test

Two fresh peers on the pinned build, with no hierarchy and no assignments. The phase passes when all of these hold:

1. Both peers try to open a peer room at the same moment, and exactly one room results. The second opener is told the room exists and joins it.
2. Messages flow both ways, including while the recipient is busy with a longer tool call.
3. One peer closes the room alone, with a reason. The next open creates a replacement that links to the closed room.
4. Each peer's identity survives a compaction and an app restart. A forked session gets a new identity.
5. After setup, the user clicks zero CBC-related approval prompts.
6. Every committed item reaches a receipt or an explicit failure.

A failure here is fixed before phase 2 starts.

### Phase 2: hierarchy implementation test

1. The user opens three sessions from the launch sheet. Each launch message states the role, the assignment (or the repository mandate, for the Primarch), the CBC grant (T-23) and the HITL default.
2. The Chapter Masters register their work items and start without a go-ahead. The Primarch sees the registrations as they arrive.
3. The Chapter Masters agree the observer-to-view contract directly, in a peer room they create themselves.
4. Questions go to the user as requests (T-10). The user answers from the dashboard or in the chat.
5. The run ends when both PRs are merged, or at the 5-hour checkpoint (T-21).

Scenarios to exercise during phase 2, which the trial manager may arrange through the user:

- A Primarch hold on one operation, such as an exclusive test port, while the other Chapter Master's source work continues.
- A HITL flag set on one item partway through.
- A request sent while its owner is busy.
- A ready request addressed to the busy Primarch (pilot document, step 3).

## 5. Setup checklist

Each item is recorded in the run manifest before the go.

- [ ] Core build pinned. Record the commit, the binary hash and the state directory. Record that no agent has write access to that binary or directory path through its allowlist.
- [ ] Candidate settings recorded: recovery interval, log level, and fallback **off** (T-05).
- [ ] `wip-cbc-*` skills installed project-local in each trial worktree, for both Claude and Codex. Record the skill source commit.
- [ ] Project-local allowlists written (T-24). Record their exact contents.
- [ ] Worktrees created from `origin/main`, one per Chapter Master, plus the Primarch's read-only checkout.
- [ ] Launch sheet: each seat's first message, verbatim, including the grant.
- [ ] Versions recorded: Claude Code, Codex app or CLI, and the operating system.
- [ ] Dashboard reachable. The browser session is created by the user, never by an agent.
- [ ] Permission-prompt capture enabled where the host exposes prompt events; otherwise the user keeps a tally sheet.

## 6. Measurements

### Waits (headline)

A **wait interval** opens when an owner has an open obligation and isn't acting on it. It closes at the owner's next attributable action on that obligation. Obligations are:

- an item delivered to the owner with no turn after it, or an item not yet delivered;
- an unanswered request;
- a ready status, or a stated next action, with no step taken.

Each interval gets one cause, derived from the records:

| Cause | Source |
|---|---|
| waiting on the user | an open request owned by the user |
| waiting on human review, by policy | a HITL flag on a ready PR (T-07) |
| waiting on the Primarch | an open request or hold owned by the Primarch |
| waiting on a resource | a hold on a named resource |
| waiting on CI | a PR observation with checks pending on the current head |
| waiting on another Chapter Master | an open request to a peer |
| waiting on delivery | an item committed but not yet receipted |
| ready but idle | a runnable obligation with nothing blocking it and no activity |

"Ready but idle" is the old failure, and it gets the most scrutiny. For each owner and each cause, the report shows:

- the count;
- the total time;
- the distribution (median, p90, maximum, with the sample count);
- the ten longest waits, each with its timeline;
- the share of the owner's wall-clock time spent waiting.

A wait whose cause can't be derived is reported as `unknown`, never guessed.

### Approval prompts (headline)

Every approval prompt the user had to click, by session and by kind:

- **CBC-related:** should be zero after setup.
- **Native setup consent:** first-time consent, expected.
- **Genuine native refusal:** the host refused an action.
- **Other.**

They are captured from host events where available; otherwise the user's tally is the record.

### Supporting measurements

- **Delivery:** every committed item has a receipt, an explicit failure, or is still pending at the end. Lost items must be zero.
- **Wake latency:** commit to wake accepted, and commit to the first activity after receipt, per profile. Provisional targets for an idle native chat:
  - p95 commit-to-wake-accepted within 10 seconds (from the pilot document);
  - median commit-to-first-activity under 60 seconds (T-27).

  Both are reported with their sample counts, and neither is a guarantee.
- **Autonomy:** merges made without a Primarch yes; routine permission or acknowledgement messages between agents (target zero); go-ahead requests (target zero).
- **Rooms and identity:** duplicate peer rooms (target zero); identity changes outside a fork or a new session (target zero); legacy skills loaded (target zero).
- **Primarch footprint:** interventions by kind, and the Primarch's turns per hour. A quiet Primarch is a good result.
- **Cost:** CPU, memory and disk for the core and connector, normal versus debug mode, counting native shims.

### What does not count

- A synthetic test never counts as a native pass.
- A queue acceptance or a receipt is not comprehension, a start or completion.
- A missing measurement stays missing; it is never filled with an estimate.

## 7. Rules during a run

- **The 5-hour checkpoint (T-21).** The trial manager writes an interim report from the deterministic data. The user and the trial manager then decide together whether to extend or stop.
- **Aborts (T-16).** Only a bug that blocks delivery aborts the run. The abort is recorded in the run log with the evidence.
- **Overrides.** User overrides are recorded as facts (T-08). The trial manager never acts as a role and never answers on the user's behalf.
- **Fallback.** If the user turns the fallback on for a session (T-05), that is recorded with the time and the reason.
- **Evidence.** Raw evidence stays local (T-22).

## 8. Report template

```
Trial <n> — <date> — build <commit> — result: completed | aborted | checkpoint-extended
Manifest: <local path>, sha256 <hash>
Phase 1: pass/fail per criterion (section 4)
Waits: per owner × cause table; ten longest with timelines; ready-but-idle detail
Approval prompts: per session × kind
Delivery: committed / receipted / failed / pending; lost items
Wake latency: per profile, n, median, p90, max, missing
Autonomy: merges without a Primarch yes; routine permission messages; go-ahead requests
Rooms and identity: duplicates; identity changes; legacy skills loaded
Primarch: interventions by kind; turns per hour
Cost: CPU, RSS, disk; normal vs debug
Outcome: PRs merged, PRs open, reviewer findings
Findings: what failed, root cause, fix PR or decision needed
Changes for the next trial
```

## 9. Run log

| Trial | Date | Build | Result | Report |
|---|---|---|---|---|
| 1 | not started | to be pinned | waiting on the pre-trial build (section 3) and the user's go | none yet |

## 10. Improving future trials

Each trial changes as few variables as possible relative to the last one, and its manifest lists what changed and why. The current candidates for later trials:

- Chapter Masters in Codex, to exercise Codex's native wake under implementation load (narrowed by T-13).
- CLI profiles alongside the desktop apps.
- Two repositories with one Primarch each, plus a cross-repository contract between Chapter Masters.
- The fallback turned on deliberately, measured against a native-only baseline.
- Longer runs with compaction under real load.
- A second device, once #118 exists.
- Responsibility attention (#115) and the Emperor (#116), once built.

Comparability rules:

- Compare like profiles only.
- Report sample counts with every distribution.
- Never compare runs made on different core builds without saying so.
- Never drop failed or pending items from a denominator.
