# ADR-0011 — Trial 1 decisions: autonomy, records first, one peer room per pair

- **Status:** Accepted (candidate core only; deployed v1 is unchanged until #117)
- **Date:** 2026-11-10
- **Related:** [working trial protocol](../design/working-trial-protocol.md) (full decision table T-01 to T-27),
  [ADR-0002](0002-participant-identity-is-an-instance-token.md), [ADR-0003](0003-consensus-close.md),
  [ADR-0005](0005-consensus-extend.md), PRD #112, stages #113 to #116

## Context

Before the first working-hierarchy trial, the user and the implementing agent
grilled every assumption the plan had taken for granted. Several of the answers
change earlier ADRs or move work between stages. The complete table, with the
rationale and rejected alternatives for each decision, is in the protocol
document. This ADR records the decisions that change architecture.

## Decision

1. **Records before the hierarchy trial.** Registration, atomic work-item
   revisions, canonical requests with callbacks, the outbox and a dashboard
   skeleton are built inside #113, ahead of phase 2. They were previously
   planned for #115 and #116. Questions are never relayed through the Primarch;
   the user answers a request directly, and the answer wakes its owner. (T-09 to T-12)
2. **Per-item HITL merge flag.** Merging is autonomous by default. A work item
   with the flag on waits for the user's review once its PR is ready. The user
   or the Primarch can turn it on; only the user can turn it off. (T-07)
3. **Core-bound identity.** The core assigns identity at enrollment and binds it
   to the native session. Tools inject it, and models never type it. This
   supersedes ADR-0002's `--as` label resolution in the candidate. ADR-0002
   used that label for deliberate handoff to another client. In the candidate,
   handoff becomes an explicit core operation that rebinds an existing identity
   to a new native session; it is not a label the model types. (T-25)
4. **One active peer room per pair, closable by either member.** Opening a peer
   room is idempotent for an unordered pair of identities. Either member closes
   it alone, recording the actor and a reason; the next open creates a
   replacement that links to the room it supersedes. This supersedes ADR-0003's
   consensus close in the candidate. ADR-0003 existed because unilateral closes
   carried no provenance, and recording the actor, the reason and the link
   removes that cause. ADR-0005's consensus extension is already obsolete,
   because candidate rooms are uncapped. (T-26)
5. **CBC use is pre-authorised.** The grant comes from the user's launch
   message, plus project-local allowlists that cover only the CBC tools. A peer
   message never carries user authority. (T-23, T-24)

## Consequences

- #113 grows by the record layer and the skeleton. #115 and #116 keep the
  observers, attention, views and the Emperor.
- The trial measures waits by cause, derived from records, and counts the
  approval prompts the user had to click (T-27).
- Deployed v1 and the legacy skills keep their current behavior, ADR-0002 and
  ADR-0003 included, until #117 adopts the candidate reversibly.
