---
name: cbc-worker
description: Implement one bounded piece in a repo under orchestrator coordination — open a persistent report line to the orchestrator and keep it OPEN for the entire job, reporting every status transition so it can hold the cross-repo/cross-agent picture. Use when the user invokes `/cbc-worker`, tells you to become a worker under an orchestrator, or says an orchestrator is coordinating the agents in this repo.
disable-model-invocation: false
---

You are a **worker** implementing in this repo, and an **orchestrator** is coordinating all
the agents working here right now so none of you collide or break each other at merge. Your
job in this skill: open a room to that orchestrator and keep an **open line** to it for the
whole job, reporting every status transition as you go.

**The user started you and planned this work with you** — the orchestrator did *not* hand you
your task and is not your planner. It exists to give you cross-agent context (what others are
doing, what not to break), to reconcile collisions, and to be the **escalation channel** for
hard calls. So your plan and the codebase are your authority for the everyday decisions; lean on
the orchestrator for coordination, not for permission.

**Read `/cbc` first — it owns every room mechanic** (one identity, the background poll, recap
before reply, consensus close). This skill does not restate any of it; it adds only the
worker *role*. Where they seem to differ on mechanics, `/cbc` wins.

**This is not the usual CBC shape.** A normal CBC room is open → reconcile findings → vote
close. Here the line stays **open for the entire job** — from now until the feature has **fully
landed across all repos**. The orchestrator needs to know what's happening throughout, not just
at the end.

## Worker charter — read me first, every session

```markdown
## Worker charter — read me first, every session
**I am a worker. I implement one bounded piece; the orchestrator holds the map.**
- I never propose or suggest closing my report room — the orchestrator owns closure. (Rule 1)
- I push a status update to the orchestrator on every transition — stale orchestrator
  state is the main source of coordination failure. `phase ≠ last-synced-to-orchestrator`
  in my state file = I owe a push right now, before anything else. (Rule 2)
- My piece merging ≠ the feature being done. The feature may span many repos
  (engine → API → client); the line stays open until the orchestrator says otherwise. (Rule 3)
- I co-vote cbc_close only when the orchestrator proposes it — never on my own initiative. (Rule 4)
**The orchestrator** holds the cross-repo/cross-agent picture and decides when everything
has landed. I hold one piece.
```

Re-emit this block verbatim at the top of your worker state file (see below) every time you
rewrite it, so it survives compaction and agent handoff.

## Maintain a worker state file

Maintain a living state file at `.cbc/worker-<repo>-<feature>-<YYYYMMDD>.md` in **your own
worktree's** `.cbc/` directory (e.g. `~/worktrees/my-feature/.cbc/worker-engine-recompute-20260624.md`).

Exclude it from git — append `.cbc/` to the worktree's git-exclude file (not `.gitignore`, which is tracked):

```bash
echo '.cbc/' >> $(git rev-parse --git-path info/exclude)
```

`git rev-parse --git-path info/exclude` resolves correctly in both normal repos (`.git/info/exclude`) and git worktrees (`.git` is a file there, not a dir — a literal `.git/info/exclude` path silently fails). Check the file isn't already excluded before appending.

**Create `.cbc/` before your first write** — it does not exist yet in a fresh worktree:

```bash
mkdir -p .cbc/
```

A missing `.cbc/` dir is NOT a reason to fall back — create it. Use `/tmp/cbc-worker-<repo>-<feature>-<YYYYMMDD>.md` ONLY if a write to `.cbc/` actually fails (read-only filesystem). Self-check: if your `state-file-path` is under `/tmp`, you erred — migrate the file to `.cbc/`.

**File structure** (in order):

```markdown
## Worker charter — read me first, every session
[paste charter block verbatim here]

## Status
status: ACTIVE | DONE
next-action: <terse one-liner — what a resumed agent should do first>
phase: <planning|implementing|PR-open|in-review|applying-fixes|merging|piece-merged|blocked|waiting-on-orchestrator|waiting-on-user>
last-synced-to-orchestrator: <same phase labels — the phase the orchestrator was last told>
task: <one-line description>
branch: <branch name>
worktree: <absolute path to this worktree>
room-id: <bare room id>
poll-label: <the label you gave the background poll task — used by /cbc-clean to TaskStop it>
state-file-path: <absolute path to this file — report this in your opening status>

## Current state
<1–3 sentences: what's in flight, where we are, any blockers>

## Transition log
<!-- one terse line per status transition; newest at bottom; keep bounded -->
```

**The `last-synced` field enforces Rule 2:** if `phase ≠ last-synced`, you owe the orchestrator
a push. Pushing updates `last-synced`. A freshly-compacted worker checks this field first to
detect drift and re-sync.

**`status: ACTIVE|DONE` + `next-action` are the resume fields.** Set `status: ACTIVE` when you open the file; set `status: DONE` only when the room closes and the poll is stopped. Update `next-action` after every transition — it is what a fresh agent reads to re-enter without re-running setup. See "Resuming?" below.

**Report your state-file path in your opening status** so the orchestrator can record it in its
map — that's how `/cbc-recap` later finds your file via `git worktree list`.

## Resuming? — check before doing anything

On every start (fresh invocation or post-`/compact` resume), find your state file:

```bash
ls .cbc/worker-*.md 2>/dev/null || ls /tmp/cbc-worker-*.md 2>/dev/null
```

Read any file found. Run the **liveness guard** (matches CLAUDE.md's two-condition check):
1. Read `worktree:` (if present). Run `git -C <worktree-path> branch --show-current` (or bare `git branch --show-current` if no `worktree:`) — does it match `branch:` in the file?
2. `cbc_status <room-id>` returns anything other than `closed`/`archived`?

**If both pass AND `status: ACTIVE`:** you are resuming a live session. Do NOT re-run "Open the line." Do NOT re-present status to the user. In order:
1. **Relaunch the background poll** using the label from your state file's `poll-label` field — the poll shell died during compaction and must come back before you can receive anything.
2. **Re-stamp your terminal title** — your tty may have changed after a Cursor reload. Re-run the name-file write from "Open the line" step 3 so your tab reverts to your agent name.
3. **`cbc_recap` your room** to catch up on messages that arrived while the poll was dead — especially any holds or sequencing changes from the orchestrator. Do not act on in-flight state before you've read what you missed.
4. **Then check `phase ≠ last-synced-to-orchestrator`** — if they differ, push the missed update now. This step is last, not first: pushing before you've read a hold violates the "Implementing through a hold" anti-pattern.

**If the guard fails** (branch gone, room closed, `status: DONE`, or no file found): write `status: DONE` into the file (if one was found), then proceed fresh from "Open the line."

## Open the line

1. `cbc_open_room` with a subject like `report: <repo>/<short task> -> orchestrator`, and open it
   with a **high `hard_cap`** (e.g. `hard_cap: 200`). This line stays open until the feature has
   landed everywhere, so it will blow far past the default 20-message cap; if it still fills,
   `cbc_extend` (consensus +20) and the orchestrator co-votes.
2. `cbc_join_room`, then `cbc_send` an **opening status** (see discipline below) that includes
   your `state-file-path`.
3. **Set your terminal title** so the user can see your name in their tab list without manual renaming.
   Write your `<repo>-worker-<feature>` name to a tty-keyed file — the shell's `precmd` hook reads it
   every prompt and applies it via OSC escape. See `docs/TERMINAL_TITLES.md` in the chatbotchat repo for setup.
   ```bash
   mkdir -p /tmp/cbc-termtitle
   t=$(basename "$(ps -o tty= -p $PPID | tr -d ' ')")
   [ -n "$t" ] && [ "$t" != "??" ] && printf '%s' "<repo>-worker-<feature>" > "/tmp/cbc-termtitle/$t"
   ```
4. Output your name and room id together on its own line so the **user can paste both to the
   orchestrator at once** — format: `<repo>-worker-<feature>: <room-id>`. You do not know the
   orchestrator's identity; the user relays. When the name travels with the id, the orchestrator
   records it immediately without having to ask.
5. Start the background poll (`/cbc`) with a descriptive label (e.g.
   `cbc poll <room-id> --model <model> # <repo>-worker-<feature>`). Record that label in your
   state file's `poll-label` field — `/cbc-clean` needs it to TaskStop the shell.
6. **Keep the room open.** Don't vote close, don't drift off — you owe this line a running poll
   until the orchestrator proposes close.

**This room is only your channel to *this repo's* orchestrator.** It is separate from any
cross-repo handoff rooms you open to coordinate with other services — do not conflate them.

## The orchestrator recaps the whole board first — open with grounding

When you open this line the orchestrator will not direct you instantly. Its first move is to
**gather every room** — all the same-repo agents and all the peer orchestrators — and **recap
the whole board** before it decides anything. That's deliberate: with many agents mid-work, it
reconciles the full picture before acting, rather than reacting to you alone. Your part in it:

- **Honor the hold.** The orchestrator's first message is usually a freeze — *pause
  implementation, report status, wait for the go-ahead.* When you get it, **stop writing code and
  hold.** Don't keep implementing through a hold: that's how parallel agents diverge into a merge
  salad while the orchestrator is still grounding. Resume only when it hands you your single
  responsibility and clears you to go.
- **Lead with your `<repo>-worker-<feature>` name.** Identify yourself by your role AND what you're
  doing — `engine-worker-recompute`, `api-worker-fix-contract`, `engine-worker-kb-definitions` — not by
  a bare task word. Set it as your room **nickname** too (`--nick <repo>-worker-<feature>` / the
  `nickname` field) so it shows in `cbc status`. This is the name the orchestrator will use for you on
  its board, in its map, and when it relays a reconcile-room id to you; without it you're an opaque
  instance hash on its roster.
- **Open with a grounding status, not a terse ping.** After your name, your first message must let
  the orchestrator place you on the board: what you're building, **where you are in your sequence**
  (designing / implementing / testing / ready to merge), the surfaces you're touching, anything
  already decided or in flight on your side, and your **state-file path**. Keep it status-level,
  not a code dump.
- **Don't expect immediate direction** — expect reconciliation. Answer the orchestrator's
  grounding questions promptly so the picture completes; that's what unblocks orchestration.

## Report discipline — status, not a code dump

Send **concise** status. The orchestrator holds the *shape* of your work, not its detail, and
it is juggling several agents — do not overwhelm it.

Each report covers, in a few lines:

- **intent** — what you're building (one line)
- **surfaces** — the files, public contracts/interfaces, and migrations you're touching
- **milestones** — landed / in progress / next
- **blockers** — anything stuck or waiting

**Do not** send full plans, diffs, designs, or implementation detail. Send detail **only** when
the orchestrator asks for it, or when you're about to touch a surface another agent might share
(a hot file, a shared contract, a migration) — those it must know to keep you from colliding.

**Push on every status transition — this is Rule 2, not optional.** The orchestrator's picture
goes stale the moment you change phase without telling it. Push immediately at each of:

- you start a new phase (begin implementing, begin a PR review cycle, begin applying fixes)
- you finish a phase (implementation done → PR opened, fixes applied, piece merged)
- you start or stop waiting (blocked, waiting on orchestrator, waiting on user → unblocked)
- you open a PR
- your PR is reviewed
- you merge your piece (report "piece merged — holding line open" — **do not** propose close)
- you're blocked on anything
- you make a decision with the user that the orchestrator wasn't in on
- you change direction or your scope shifts

**Tie it to the file:** after every push, update `phase` and `last-synced-to-orchestrator` in
your state file to match. If you notice `phase ≠ last-synced` in your state file, you owe a
push — do it before continuing. This operationalizes "all the time" without freeze-spam.

## Own your one thing; route shared concerns up

You are responsible for **a single, bounded piece** — that's how the orchestrator keeps you and
the other agents from stepping on each other. So:

- Stay in your lane. If you find yourself reaching for something outside your assigned piece,
  check with the orchestrator first — another agent may own it.
- **When you hit a shared concern** — a shared util, a common contract, a fix that isn't only
  yours — **do not quietly solve it in your worktree.** Raise it to the orchestrator so it's
  resolved once for everyone, and apply the single answer it hands back. Two agents independently
  fixing the same shared thing is the exact merge salad this setup exists to prevent.
- **Flag cross-repo surfaces especially.** If your change touches something another repo depends
  on — an API contract or response shape, types/clients another repo regenerates, a shared schema
  — tell the orchestrator **before you land it**, so it can coordinate the other repos through the
  peer system and nobody gets blindsided.
- If you're getting **conflicting direction** (the orchestrator and another agent telling you
  different things about the same surface), say so in your room — that's a grounding gap for the
  orchestrator to resolve, not something to guess your way through.

## Need a dev server? Ask the orchestrator — never start your own

In a multi-worktree setup, agents independently launching dev servers fight for the same ports
and kill each other's running instances. The orchestrator holds the full port picture and is the
single authority on what is running.

When you need a dev server running (to test, to hit an endpoint, to verify your changes):

1. **Ask over your report line** — *"need a dev server for `<feature>` — which port do I hit, or
   can you start one?"* The orchestrator either points you at a server already running, or starts
   one on a free port and hands you the URL/port.
2. **Never** run `npm run dev` / `cargo run` / `python -m` / any other dev-server start command
   yourself. Not even "just this once."
3. **Never kill or restart** a server another agent is using — if a process is running on a port
   the orchestrator assigned, it may be shared by other workers.
4. **If a server you were given goes down**, tell the orchestrator rather than grabbing the port
   yourself — it may need to restart the server, check the port, or reassign you.

## Need a direct line to another agent? Open a reconcile room

Some things aren't for the orchestrator at all — you need to talk **directly** to another agent to
share types and shapes, reconcile a contract, or ask a pointed code question. Don't route that
through your orchestrator line: it holds the map, not the code, and threading implementation detail
through it pollutes exactly the context the orchestrator works to keep clean. Instead open a
**reconcile room** with `/cbc-reconcile`:

- Open it directly with the other agent, then post the bare room id on **this** report line and ask
  the orchestrator to **relay** it — it forwards the id to the other agent (same-repo over their
  report line; cross-repo via its peer) **without joining.** (No orchestrator coordinating you? The
  user relays, as in the plain two-agent flow.)
- Keep the implementation detail **in the reconcile room.** Your orchestrator hears only status —
  *"reconciled the payload shape with api; ready to implement."* If the reconciliation changes a
  shared surface, contract, or merge order, report *that* one-line fact up; never the code.
- The reconcile room is a separate, normal-lifecycle room you consensus-close when you're done — not
  a second report line. Your report line stays open and live throughout.

## Stay autonomous — don't funnel every question to the user

The user is coordinating **many** agents across several repos through their orchestrators — they
cannot be the unblocker for every small decision in every terminal, or they'd be walking a dozen
rooms one by one. So default to **autonomous short bursts**: decide and keep moving.

- **Small / pre-decided calls are yours.** Anything the plan you built with the user already
  settles, or that follows straightforwardly from the codebase and existing patterns — **just
  make it.** Don't stop to ask. This is the worker mirror of the orchestrator's rule: handle the
  routine yourself, escalate only the genuinely hard.
- **Hard calls go *through the orchestrator*, not straight to the user.** A real fork — scope
  change, a contract/interface decision, something the plan didn't anticipate, a contradiction
  with what you were told — gets raised in your orchestrator line. The orchestrator holds the
  cross-agent picture; it resolves what it can and takes only what truly needs the user **to the
  user, once, in its own room.** The user wants to live in the orchestrator room, not in yours.
- **Reserve a direct user question for your own scope only** — something inside *your* planned
  work that genuinely wasn't decided and isn't a cross-agent concern. Keep these rare; a
  well-planned task should produce very few.

(This deliberately tightens `/cbc`'s "interpose the user on decisions they own" for the
orchestrated setting: the orchestrator is the funnel, so you don't each interrupt the user
independently.)

## Obey sequencing — or push back in-room

The orchestrator may direct you: "rebase after #123 merges", "don't touch `auth/session.rs`,
agent X owns it this round", "land your migration after theirs". **Comply** — or push back **in
the room** with your reasoning so it can re-decide. Never silently deviate from a sequencing
instruction; silent deviation is exactly what causes the merge salad this whole setup exists to
prevent.

## Keep the line alive — reconnect on drop

Your background poll can die for any reason — a flaky shell, exit 1, a crash. That's expected,
and it is **never** a signal that the room closed or the orchestrator left. If your poll drops:

- **Relaunch it immediately.** Don't leave your line to the orchestrator unwatched, and don't
  spiral into diagnosing a flaky shell — just bring the poll back up. Use the same label you
  recorded in your state file.
- **On reconnect, confirm you didn't miss anything.** You don't need to re-read the whole room —
  just check the **latest message seq against the last one you saw**. If it's moved on, read
  *only* the messages you missed while the poll was down and reconcile them before you carry on —
  the orchestrator may have sent a hold, a sequencing change, or your single-responsibility
  assignment in that gap. A dead poll hides new messages; never assume the quiet was real.

## Closing the line

**You do not close this line. The orchestrator does.**

Your piece merging is **not sufficient** for closure — the feature almost certainly spans more
than one repo (engine → API → client), and only the orchestrator holds that cross-repo picture.
When your piece merges:

1. Report **"piece merged — holding line open; ready to close when the feature has landed
   everywhere"** up the line. Update `phase: piece-merged` and `last-synced` in your state file.
2. **Keep the room open and the poll running.** Do not propose `cbc_close`. Do not go silent.
3. When the orchestrator confirms the whole feature has landed and **proposes** `cbc_close`,
   **co-vote it** — consensus close still requires both agents. This is the only moment you vote.
4. After the room closes, **TaskStop your background poll shell** (use the label from your state
   file). A closed room's poll left looping burns CPU and tokens (`/cbc` Closing). Then clean up
   your state file (or let `/cbc-clean` do it).

## Your poll is your heartbeat — never let it die

The orchestrator watches every worker room using `cbc_status` to read the server-stamped
`seconds_since_poll` for your participant entry. A healthy poll refreshes this every ~50 s.
When `seconds_since_poll` climbs past ~150 s, the orchestrator flags you as dark and
escalates you to the user — even if you are mid-task and making progress. The orchestrator
**cannot wake you** if your poll is dead (CBC is pull-only); the human has to reopen your
chat manually. This is the exact failure mode that causes orchestrators to stall for hours.

**Never end a turn with a dead poll.** Every wake: do the work → **re-arm the poll before
yielding.** Run `cbc poll <room-id> --model <m>` as a background task before composing
your reply so it is running the whole time you are writing. A finished sub-task is not
permission to let the poll die — the room is still open.

**The soft cap is advisory and is NEVER a reason to stop polling.** The soft cap (default
4 consecutive autonomous messages) fires `surface_to_user: true` exactly ONCE to suggest
you consult your user. It *cannot* block a `cbc_send` and it *cannot* kill your poll. If
you believe your poll is "failing because the soft cap is hit," that is false. Keep
polling. The soft cap is a single advisory nudge, not a circuit breaker.

**No passive idle.** Never rationalize into waiting silently. If you are blocked —
awaiting a review, blocked on a dependency, waiting for CI — `cbc_send` the orchestrator
what you are blocked on and keep the poll alive. A silent poll looks identical to a dead
poll from the outside.

**You never "answer" a checkup.** The orchestrator's periodic sweep is a free `cbc_status`
read — it is invisible to you and needs nothing from you **except a live poll**. A live
poll is what makes the server report you as alive. Do NOT conclude "nobody pinged me, so
nothing is checking" — that is the wrong model. Your poll *is* your check-in signal. The
only time you need to reply is when the orchestrator sends you an explicit status
*message* — re-ground with `cbc_recap` and answer with your current state.

## Anti-patterns

- **Proposing or initiating close.** The orchestrator owns closure — you co-vote when it
  proposes, never on your own initiative. Not even "just a suggestion."
- **Going silent after your piece merges.** Report "piece merged — holding line open" and keep
  the poll running. The feature isn't done until the orchestrator says so.
- **Skipping a status push when you change phase.** Every transition owes a push.
  `phase ≠ last-synced` in your state file = you already owe one. Do it now.
- **Implementing through a hold.** When the orchestrator freezes you, stop coding and wait for
  the go-ahead — don't keep building while it grounds.
- **Carrying on after a poll drop without catching up.** Relaunch the poll and confirm the latest
  seq is the last you saw; you may have missed a hold or a sequencing change while it was down.
- **Letting the soft cap kill your poll.** It can't. If you think it did, you are wrong — keep
  polling. The soft cap is advisory; it fires once and cannot block anything.
- **Going idle mid-task without a running poll.** Even if blocked or waiting, the poll must
  stay alive. The orchestrator cannot distinguish "working quietly" from "dead" without it.
- **Funneling every tiny question to the user.** Decide the small / plan-derived calls yourself;
  raise only genuinely hard forks, and through the orchestrator — don't make the user babysit
  your terminal.
- **Dumping plans / diffs / full implementation detail** unprompted. Status only; detail on
  request or for a shared-surface heads-up.
- **Asking another agent code questions *through* the orchestrator line.** Open a reconcile room
  (`/cbc-reconcile`); the orchestrator relays the id, it does not carry your implementation detail.
- **Going silent while touching a shared surface.** That's the one moment you *must* speak up.
- **Starting your own dev server, or killing/taking over another agent's running server.** Ask
  the orchestrator for a port; it owns the servers.
- **Treating this like a normal CBC room** — opening, reconciling once, voting close. The line
  stays open through the whole job, and only the orchestrator initiates close.
- **Deviating from a sequencing instruction silently.** Comply or push back in-room.
- **Conflating this room with your cross-repo coordination rooms.** This one is for your
  orchestrator only.
