# Optional scope supervisor

Status: draft captured 2026-11-06 for discussion on Monday, 2026-11-09.
This is an implementation proposal for the CBC redesign, not an agreed solution
or a change to installed skills, permissions or the current execution hierarchy.
Implementation remains stopped at the existing checkpoint.

## Problem

Agents can turn a small authorized task into a larger engineering project.
Agent-to-agent questions compound the expansion: each answer introduces more
possibilities, dependencies and coordination, without establishing that they are
necessary for the user's intended outcome. Technical plausibility alone does not
justify the time, tokens, code or subsequent maintenance.

The [notification activation case study](../case-studies/2026-11-06-notification-activation-coordination.md)
is the motivating example. An authorized OFF-to-ON activation expanded into hours
of coordination and a broader fallback approach before the original route was
completed. The original request failure remains unexplained; this proposal does
not investigate or change that application.

Primarchs hold the wider repository picture, while Chapter Masters accumulate
implementation detail. Neither context reliably preserves a clean account of
one feature's original intent, exclusions and urgency throughout the work.

## Proposed role

Add an optional agent that acts as an adversarial senior engineer and scope
reviewer. It preserves the feature's concept and challenges unnecessary expansion.
It sits alongside the Primarch and Chapter Master, outside the execution and
approval chain. Its independence is the point of the proposal.

The user can plan the feature with this agent. A structured, challenging planning
interview, described by the user as a built-in "grill me" session, establishes:

- The intended outcome and smallest useful deliverable.
- Explicit non-goals, constraints and assumptions.
- Timeline, urgency and release expectations.
- Required evidence of completion and links to the authoritative planning sources.

It retains that concise scope context instead of absorbing the implementation
transcript, the entire codebase or every feature in the repository. CBC remains
a coordination system; project requirements and final decisions can still live
in the user's chosen planning tools.

The role would receive a dedicated skill and integrate with CBC's structured
updates, questions and dashboard. The normal implementation chain remains
Primarch → Chapter Master → native implementation subagents. This proposal does
not restore Captains or introduce another mandatory management handoff.

Working name: **Supervisor / scope reviewer**. The optional Warhammer name is
undecided; "chapter psyker" was a placeholder, not a selected name.

## Intended intervention

When an approach starts expanding, the supervisor should challenge it with
concrete questions:

- What observed problem requires this additional work?
- Is it necessary for the current deliverable, or merely a possible improvement?
- Has the exact failure been understood before replacing the approach?
- Is there a smaller existing route, including a specific action the user can take?
- What does the expansion cost in time, tokens, engineering and maintenance?
- Can the authorized deliverable ship first, with this improvement recorded separately?

The desired outcomes are to continue the original task, discard unnecessary work,
record a useful follow-up, or present a justified scope expansion. The supervisor
must also challenge under-delivery: preserving scope does not mean skipping
required verification or claiming a reduced deliverable as the whole feature.

A material expansion goes to the Primarch and, through the planned dashboard,
to the user for a decision. The user can approve it, defer it as a follow-up or
reject it. Independent authorized work should keep moving unless an actual
dependency prevents it. A useful future improvement need not delay the current
implementation, deployment or promotion.

## Boundaries to preserve

- No implementation ownership, resource allocation, merge scheduling or release orchestration.
- No permission-granting authority, native approval override or manufactured human consent.
- No mandatory sign-off on every action, routine coordination message or PR operation.
- No automatic execution hold or new user-click dependency merely because it raised a concern.
- No continuous debate that consumes more time and tokens than the drift it prevents.

An exact native/security refusal, an already-authorized routine action and a
material scope change remain different situations. This role should help agents
recognize the distinction, not turn all three into approval requests. Requests
from the user to stop and explain must be honored promptly without waiting for
the supervisor to intervene.

## Questions for Monday

1. Should this be a persistent feature-scoped agent, an on-demand review mode, or both?
2. Which events justify involving it, and which checks can CBC perform deterministically?
3. What minimal scope record and update inputs keep its context useful without duplicating plans?
4. How does it challenge a Chapter Master or Primarch without becoming another blocking authority?
5. How are disagreements and genuine scope changes presented concisely through the dashboard?
6. When does it retire, and how do we measure whether it reduced drift, delays and token cost?

After discussion, incorporate the agreed role, skill, data and interaction rules
into the implementation plan under PRD #112 and choose an appropriate slice.
No new skill, agent session, automation, policy rollout or implementation is
created by recording this draft.
