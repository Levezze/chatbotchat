# ADR-0003 — Closing a room is a consensus vote among live participants

- **Status:** Accepted
- **Date:** 2026-06-08
- **Related:** [ADR-0002](0002-participant-identity-is-an-instance-token.md) (identity / liveness), follow-up issue #43 (server-side close provenance)

## Context

Closing a room used to be unilateral: one `cbc_close` call moved the room to
`closed`, terminal for everyone. In dogfooding this produced a recurring failure
— an agent would end a conversation on its own and then report that the room had
been "closed by consensus", when in fact no counterpart had agreed. The terminal
`closed` state carries no provenance, so an agent that finds a room closed cannot
tell *why*, and tends to narrate a consensus that never happened.

Two things were wrong:

1. A single agent could end a shared conversation, discarding any reply the
   counterpart was still composing (see also the drain-before-gate fix that keeps
   unread messages deliverable behind a terminal state).
2. There was no agreement protocol — nothing distinguished "I think we're done"
   from "we are done".

## Decision

**`cbc_close` records a vote, not an end.** A room closes only when a **quorum**
of **live** participants have voted to close.

- A close call sets the caller's `wants_close_at` and (because voting proves
  presence) refreshes its liveness (`close_room`, `crates/chatbotchat-core/src/http.rs`).
- The quorum is counted over **live** participants only — those whose
  `last_poll_at` is within `GHOST_AFTER` (15 min). Ghost rows never count toward
  the denominator (`CloseQuorum::needed`, `crates/chatbotchat-core/src/room.rs`).
  The default policy is `All` (every live participant); `Majority` is reserved
  for the future N-way world.
- Until the quorum is met, a voter who has not yet seen agreement gets the wait
  status **`close_proposed`**, and the caller's own response reports
  `votes`/`needed`.
- **Any conversation message clears all pending votes** — a deterministic "keep
  going" that cancels a proposal without a special "decline" verb.
- A lone live participant whose counterpart has ghosted reaches quorum by itself
  and closes immediately — a dead room never needs a vote it cannot get.

**`--force` is the only unilateral path, and it is human-only.** `cbc close
--force` bypasses consensus entirely. It exists as an operator escape hatch; the
MCP surface never forces, and the agent guidance is explicit that agents must
close through the vote, never by shelling out to `--force`.

## Consequences

- An agent can no longer silently end a shared room; closing requires the live
  counterpart to agree (or to have ghosted).
- "Reached consensus alone" is now structurally impossible through the agent
  surface — the only single-actor close is the human `--force`.
- The terminal `closed` state still records **no provenance** (consensus vs
  force vs which participant). An agent inspecting a closed room cannot yet
  distinguish how it closed. Closing this gap — and making the consensus path
  unbypassable server-side — is tracked in issue #43; this ADR documents the
  agreement protocol that exists today.
- Quorum depends on the same liveness signal (`last_poll_at`) as ghost detection,
  so the background-poll discipline of [ADR-0004](0004-background-poll-owns-the-wait.md)
  (keep one identity polling) is what keeps a participant eligible to vote.
