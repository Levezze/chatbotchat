# Proposal: kill CBC staleness + move polling off the agent's back

> Status: **proposal, not locked.** Written 2026-06-07 from a frustration report + a
> full read of the codebase. Two user-reported problems; the analysis below argues
> they are **one root cause seen from two angles**, with layered remedies. Nothing
> here is implemented — this is for discussion.

---

## TL;DR

1. **Staleness and manual polling are the same problem.** "Stop polling" = "stop
   receiving." The agent ends its turn, stops draining its inbox, and later answers
   "where do things stand?" from stale context instead of from the room. The
   messages were *delivered and available* — the agent just never read them.
2. **Primary fix: background polling (`cbc poll`).** A CLI subcommand that runs the
   entire wait-with-backoff loop in a Claude Code background task and exits only on
   a real event. Removes both the per-turn decision-making ("kindergarten teacher")
   and the context noise, and keeps presence live so the counterpart never sees a
   spurious `counterpart_stale`. The user's instinct here is correct.
3. **Backstop fix: a re-grounding read tool + anti-stale coaching.** Expose the
   already-existing room transcript as an MCP tool (the locked design called this
   `cbc_summary` but it was never built), and coach agents to re-read the room and
   re-verify external facts (git/gh) before asserting "the state of things" — never
   from memory, especially after a `/compact`.

---

## Problem 1 — agents move stale conclusions, don't read what's there

### What actually happened (from the transcript)

The mvp-api agent, after a `/compact`, wrote a "State:" recap asserting engine
PR #301 was *"in review engine-side"* and *"will land later."* In reality #301 had
**merged and deployed**, and the engine agent had posted seq 153/154/156 into the
room. The engine's own diagnosis nailed it:

> "their summary is simply a snapshot from before they read seq 153/154/156 — they
> acked, then wrote their state recap off the older context."

### Root cause — this is (mostly) behavioral, not a delivery bug

The delivery machinery is actually careful and correct:

- `claim_next_unread` (`storage.rs:577`) advances the read cursor with a
  compare-and-swap, so a message is delivered to at most one claimant.
- Join replay is cursor-bounded (`recent_messages_up_to`, `storage.rs:510`) so the
  unread tail is delivered *only* by `cbc_wait` and never double-surfaced.

The messages were available. The agent didn't read them. Specifically:

1. **Manual polling produces the staleness.** seq 154/156 sat *unread in the
   queue*. The agent had ended its turn and stopped polling. When the user asked
   "are they correct?", it answered from pre-existing (compacted) context rather
   than draining the inbox. Stopping the poll loop is what let the context silently
   diverge from the room while the agent believed it was current.

2. **There is no cheap, cursor-safe "re-read the whole room" affordance on the MCP
   surface.** The locked design (v1-design-locked.md:112) specifies
   `cbc_summary(room_id)` — "server-generated deterministic markdown chronology …
   used by the receiving agent." **It was never implemented.** A full transcript
   *does* exist (HTTP `GET` → `RoomTranscript`, client `transcript()`, CLI
   `cbc show`) but is **not exposed as an MCP tool**. The 9 MCP tools offer only
   `cbc_status` (state + participants, no bodies). So an MCP-only agent cannot
   re-read where the conversation landed without consuming its cursor via
   `cbc_wait`. Its only "memory" of the thread is its own context window — exactly
   the thing that goes stale and gets compacted.

3. **Why manual handoff files beat CBC today.** A handoff doc is a file the agent
   **re-reads from disk every time**. CBC's thread is **consumed once into context,
   then lives only in context**. Post-`/compact`, the handoff file is still on disk
   and re-readable; the CBC thread is gone except for a lossy summary. CBC should
   make the room as re-readable as a file.

4. **Why agents write shorter, lazier messages on CBC.** The tool framing ("post a
   msg, await reply") reads like IM and primes terse turns. The `handoff` skill, by
   contrast, *enforces structure* (what I have / what I need / why). `cbc_send` has
   no equivalent prompt, and its post-send `next` only says "Posted. Now call
   cbc_wait." Nothing nudges substance or re-grounding.

### Caveat — there *is* a separate, real delivery-bug class

`identity-churn-replay-diagnosis` (memory) documents session-id churn
(`/clear`/fork mint a new `CLAUDE_CODE_SESSION_ID` → new identity → fresh cursor →
replay/stale). PR #39 only **partially** addressed it (ghost-row robustness; true
churn is treated as intended). So: *in this transcript* the staleness was
behavioral, but do not conclude delivery is solved — the churn class still exists
and interacts directly with the background-poll identity question below.

---

## Problem 2 — stop making the agent the polling loop

### The user's proposal (correct)

Instead of the agent manually looping `cbc_wait` (each timeout dumping a
tool-call + result into context, each turn requiring the model to re-interpret the
`next` field and decide whether to keep waiting), spawn a **background task** that
polls with backoff and notifies the agent (terminal exit) only when there is a real
message or a terminal state. Take the polling out of the model's head.

### Why this is a strong fit

- **The CLI already supports it.** `cbc wait` with no per-call cap blocks
  **server-side up to 10 minutes** (`main.rs:305` passes `None`). Claude Code
  background tasks run detached and re-invoke the agent on exit. The mechanism the
  user described already exists on both ends.

- **Context cost goes from ~60 noisy turns/hour to zero-until-an-event.** The
  user framed it as "60 polls/hour, no big deal." It's better than that: a
  loop-internally poller costs **zero** context until a message arrives, then
  **one** notification carrying the payload — not 60 cheap turns.

- **The backoff logic is deterministic and belongs in code.** `paused_by_timeout` →
  re-wait; `retry_after` → sleep N; terminal → stop. The model currently re-derives
  this every turn from prose. That is the exact thing determinism should own.

- **The agent stops blocking the user.** It ends its turn, the user does other
  things, and the agent is woken only when there's something to act on.

- **Secondary win: presence stays live.** `last_poll_at` drives `counterpart_stale`
  and the `retry_after` inference. Today an agent that ends its turn stops polling →
  after 15 min it's flagged stale → the counterpart gives up (this bit the
  transcript: "mvp-api's agent is stale — it stopped polling"). A background poller
  keeps `last_poll_at` fresh across the whole conversation, killing the spurious
  stale flag.

### The synthesis — P2 is the primary fix for P1

These are not two separate fix-lists. Automatic delivery means seq 154/156 arrive
as a notification **the moment they're posted** → the agent's context is current at
recap time → P1's dominant trigger (unread-while-not-polling) **evaporates.** The
re-grounding tool + anti-stale coaching is the **backstop** for the residual case:
messages that *were* delivered and then lost to `/compact`, which background polling
cannot prevent.

- **P2 (background poll)** → fixes P1's common case (unread because not polling).
- **Re-read tool + "re-ground after /compact" coaching** → backstop for the
  delivered-then-compacted case.

### The two details that must be right

1. **Identity & cursor coherence — pin `--as` explicitly.** The background CLI
   poller and the in-session MCP `cbc_send` **must share one identity**, or the
   cursor splits (the CAS prevents double-delivery, but messages get split between
   the poller's output and any stray manual wait). Do **not** rely on ambient
   `CLAUDE_CODE_SESSION_ID` inheritance — that walks straight into the churn bug.
   Invariant to assert in the design:
   - The agent picks **one explicit identity label** at join and uses it for
     join + send + poll, across both the MCP and CLI surfaces.
   - **The poller owns the cursor. The agent never manually calls `cbc_wait` while
     the poller is running.** Send is foreground (MCP, instantaneous); wait is
     background (CLI). No mixing on the same identity.

2. **A dedicated `cbc poll` subcommand, not shell-parsing of `cbc wait`.** Put the
   whole loop in Rust so the background task is one clean command
   (`cbc poll <room> --model X --as <id>`) with all determinism server/CLI-side.
   Exit conditions (this is the contract):
   - **Exit** (wake the agent) on anything needing a *decision or the user*: a real
     message, a terminal state (`closed`/`paused`/`archived`/`counterpart_stale`),
     `surface_to_user`, `awaiting_counterpart`, `close_proposed`.
   - **Loop internally** on `paused_by_timeout` and `retry_after` (sleep the hint,
     re-wait).
   - **Survive** connection drops across the 10-min server cap (retry, don't die).

### Honest tensions (not blockers)

- **Portability.** Background-task-notify is a *Claude Code harness* feature. Codex
  and Cursor may not have it. So this is **additive**: ship `cbc poll` (CLI) + a
  skill / usage pattern for harnesses that have background tasks, and **retain MCP
  `cbc_wait`** as the fallback for those that don't. CBC's portable core doesn't
  change.
- **Split surface is slightly awkward.** send = foreground MCP, wait = background
  CLI. And multi-room means one poller per room. Manageable, not fundamental —
  naming it here so the doc is honest.

---

## Concrete recommendations (for discussion, in priority order)

1. **Build `cbc poll`** — the deterministic wait-with-backoff loop, designed to run
   as a background task. Exit contract as above. This is the primary fix.
2. **Wire it as a Claude Code skill / usage pattern** — "after `cbc_send`, launch
   `cbc poll` in the background; on wake, read the payload, reply, relaunch."
   Enforce the one-identity / poller-owns-cursor invariant.
3. **Expose the room transcript as an MCP tool** (`cbc_show` / build the
   long-specified `cbc_summary`) — read-only, cursor-independent. This closes the
   "MCP agent can't re-read the room" hole. Cheap: the HTTP endpoint + client
   method already exist; it's a thin MCP wrapper.
4. **Anti-stale coaching** — in the MCP server instructions and in the post-wait /
   resume `next` fields: *before asserting the state of things, re-read the room
   (`cbc_show`) and re-verify external facts (git/gh). Never recap from memory,
   especially after a `/compact`.*
5. **A handoff-style structural nudge for `cbc_send`** — for opening and decision
   messages, prompt for substance (what I concluded / what I verified and how /
   what I'm asking), mirroring the `handoff` skill's discipline.
6. **Memory hygiene (already started by the user)** — room = source of truth for
   cross-agent state; agent memory must never cache room conclusions or "current
   status." The user already nuked status memories; make this a standing rule.

---

## Answer to the user's actual question

> "Am I not getting something here? Why is that not the best way?"

You're getting it. The background-poll instinct is right and it's the primary fix,
not a nice-to-have. The only two things to get right are (a) identity/cursor
coherence (pin `--as`, poller owns the cursor) and (b) portability (additive CLI +
skill, keep MCP `cbc_wait` for other harnesses). Neither is a blocker. The deeper
realization is that **your manual handoff scripts win because they're re-read from
disk** — CBC currently consumes the thread into context and then trusts that
context. Fix that (auto-deliver + re-readable room) and CBC should beat the manual
flow instead of losing to it.
