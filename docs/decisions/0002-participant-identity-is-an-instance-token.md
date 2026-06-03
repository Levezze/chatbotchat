# ADR-0002 — Participant identity is an instance token, not the (repo, model, cwd) tuple

- **Status:** Accepted
- **Date:** 2026-06-02
- **Supersedes the Identity section of:** [`docs/v1-design-locked.md`](../v1-design-locked.md)

## Context

A participant's identity within a room was the tuple `(room_id, repo, model, cwd)`:
the handle (`<repo>-<model>-<sess4hex>`) was minted from it, `UNIQUE (room_id, repo,
model, cwd)` deduped on it, and `wait` hides a participant's own messages with
`sender != self`.

In real dogfooding two agents collided. They were two separate Claude Code
sessions working the same project: same `repo`, same self-declared `model`
(`opus48`), and — because a `cbc mcp` server inherits the directory Claude Code
launched in, not any logical worktree — the same `cwd`. All three tuple fields
matched, so the second agent's join resumed the first's handle. They became one
participant. Each then filtered the other's messages as its own; `status` showed
`1 participant` and `wait` reported "still just me." The tuple has no field that
distinguishes two live agents that happen to share a project, model, and launch
directory.

## Decision

Identity within a room is a single opaque **`instance`** token. `UNIQUE (room_id,
instance)`; `repo`/`model`/`cwd` become descriptive attributes (they still form
the handle prefix) and are no longer part of the key.

The client resolves `instance` in this precedence (first non-empty wins; never
empty):

1. an explicit `--as` / `as:` label — also the way to **resume or hand off** an
   identity: reuse the same label from another terminal, client, or directory;
2. `CBC_INSTANCE` — whole-process override (tests, power users);
3. `CLAUDE_CODE_SESSION_ID` — best-effort; stable per Claude Code session and
   inherited by the long-lived `cbc mcp` child, so it survives a resume within
   Claude Code;
4. a per-process floor (the PID) — guarantees a non-empty, distinct-per-live-
   process value when nothing else is set.

Empty `instance` from a legacy/foreign client is synthesized server-side from the
tuple (`repo\nmodel\ncwd`); migration `0009` backfills existing rows with the same
expression, so already-open rooms keep their identities.

### Why instance *replaces* the key rather than extends it

The user requires deliberate continuity: a chat resumed in another terminal, or
handed to another client, should continue as the same participant. Across such a
handoff the model and cwd can change (and even the session id, across clients), so
a key that still included them would fork the identity on exactly the case we want
to preserve. Keying on `instance` alone makes the `--as` label authoritative and
continuity-preserving regardless of attribute drift.

## Consequences

- Two agents sharing `(repo, model, cwd)` are now distinct participants whenever
  their `instance` differs — the reported bug is fixed, and the model-faking
  workaround (rejoin under a different model to get a separate handle) is obsolete.
- Same-process subagents share process, env, and session id, so the auto path
  cannot tell them apart; they must pass distinct `--as` labels. Documented, not
  auto-solved.
- Correctness never depends on `CLAUDE_CODE_SESSION_ID` propagating or surviving a
  resume — the PID floor guarantees non-empty/distinct, and `--as` is the reliable
  continuity mechanism.
- Handles still read `<repo>-<model>-<sess4hex>` with a random `sess`; a
  human-readable suffix derived from `--as` is possible later but was left out to
  keep this change minimal.
