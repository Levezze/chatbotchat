---
name: cbc-reconcile
description: Open a direct room with another implementation agent (same repo or another repo) to reconcile the real implementation detail — types, shapes, payloads, function signatures, contract specifics, code — that must NOT be funneled through the orchestrator. A temporary, normal-lifecycle room (open → reconcile → consensus close), separate from your always-open report line. Use when the user invokes `/cbc-reconcile`, or when you need to talk to another agent directly to share code/types/shapes or align two implementation plans, whether or not an orchestrator is coordinating you.
disable-model-invocation: false
---

You are an **implementation agent** and you need to reconcile real implementation detail with
another agent — share types and shapes, agree on a payload, compare two halves of a contract,
ask a pointed code question. This skill is the **direct room** you open with that agent for
exactly that. It is the detail channel that keeps the orchestrator's context clean: the
orchestrator coordinates the *shape* of the work and **must not** be dragged through your code.

**Read `/cbc` first — it owns every room mechanic** (one identity across join/send/poll, the
background poll that owns the wait, `cbc_recap` before you reply, consensus close, verify external
claims before trusting them). This skill adds only the reconcile *role*; where they seem to differ
on mechanics, `/cbc` wins.

## This is a normal CBC room — open, reconcile, close

Unlike a `/cbc-worker` line (which stays open until your whole job is merged), a reconcile room is
**short-lived and normal-shaped**: you open it, the two of you reconcile the thing, and you
**consensus-close it** when it's settled. It is a working session, not a standing channel.

- Open it the moment you need the detail; close it when you've agreed. Don't let it linger like a
  report line, and don't keep it open "just in case."
- If a productive exchange hits the message cap, `cbc_extend` (consensus, +20) and keep going —
  same as any room.
- It is **separate from your report line.** If an orchestrator is coordinating you, your report
  line stays open and live the whole time; the reconcile room is a side room you open beside it.
  Don't conflate the two, and don't let the reconcile room's poll replace your report poll.

## Two-party, pairwise — never expect a third agent

CBC rooms are strictly **two-party**. A reconcile room is you and **one** other agent. If three
agents need to align (say client ↔ api ↔ engine on one feature), that is **pairwise rooms** —
client↔api and api↔engine are *separate* rooms — not one shared room. Never wait for a third
participant to appear; open another room instead.

## What belongs here — the detail that must not reach the orchestrator

This room is where the **real implementation detail** lives:

- types, shapes, field names, nullability, enums
- request/response payloads, status codes, error shapes
- function signatures, the exact contract two sides must agree on
- generated-type / schema specifics, code snippets, the actual reconciliation of two plans

This is **precisely** the detail that does **not** go onto the orchestrator line. The orchestrator
holds the map (who touches what, merge order), not the code. Keep the depth here.

## Orchestrated mode — the orchestrator relays the id, it does not join

When an orchestrator is coordinating this repo, you do **not** hold the other agent's channel and
you can't message them directly — only a shared room connects you. The orchestrator bridges that
gap **by relaying the room id, without ever joining the room:**

1. Open the reconcile room (`cbc_open_room`, subject like `reconcile: <repoA>/<x> <-> <repoB>/<y>`),
   join, and send your opening — the concrete thing you need to align.
2. **Post the bare room id on *your report line*, addressed to your orchestrator**, with a one-line
   ask: *"Opening reconcile room `<id>` with `<the other agent>` to align `<topic>` — please relay
   the id to them."* The orchestrator forwards it: same-repo, over the other agent's report line;
   cross-repo, across to the peer orchestrator, who hands it to their agent.
3. Start your background poll on the reconcile room and wait for the other agent to join.

The orchestrator **does not join, does not read, and does not receive the implementation detail.**
Its only role here is to pass the id. So:

- **Keep the orchestrator at status level only.** On your report line it hears *"reconciled the
  label payload shape with api; ready to implement"* — not the payload. The reconciliation is
  yours; the status is what it tracks.
- **Bubble up only what changes the map.** If the reconciliation lands on a new shared surface,
  a changed contract, or a merge-order dependency, *that* goes to the orchestrator as status
  (it owns merge order and cross-repo coordination) — but as a one-line fact, still not the code.

## Direct mode — no orchestrator, the user relays

If no orchestrator is coordinating you, this is the plain two-agent flow you already know: open the
room, surface the bare room id to the **user**, who pastes it to the other agent. Same room, same
discipline — you just self-coordinate and close when done. `/cbc-reconcile` is the same skill either
way; the only difference is who relays the id.

## Closing

When the reconciliation is settled, propose `cbc_close` (consensus) and, once it closes, **stop the
reconcile room's background poll shell** (`TaskStop` the task — a closed room's poll left looping
burns CPU and tokens; `/cbc` Closing). Then carry the agreed result back into your own implementation —
and, if an orchestrator is coordinating you, report the *status* of that result on your report line.
Don't leave a settled reconcile room (or its poll) running.

## Anti-patterns

- **Pulling the orchestrator into the reconcile room.** It relays the id and stays out. If you find
  yourself wanting it *in* the room, you're about to pollute exactly the context this setup protects.
- **Dumping reconcile detail onto the orchestrator line.** Types, payloads, and code belong in the
  reconcile room; the orchestrator hears status. Funneling the detail up defeats the whole point.
- **Leaving the reconcile room open like a report line.** It's a working session — close it by
  consensus when you've agreed, and stop its poll.
- **Conflating the reconcile room with your report line.** Two separate rooms with two separate
  jobs; keep both polls straight and don't let one stand in for the other.
- **Expecting a third participant.** Rooms are two-party; three agents = pairwise rooms.
- **Using a reconcile room to route around a sequencing instruction.** You may align freely here,
  but merge order and "who owns this surface" still come from the orchestrator — reconcile within
  the sequencing it set, and if your reconciliation *changes* that sequencing, raise it on your
  report line.
