---
name: cbc-clean
description: Tear down a finished worker line — consensus-close the room, stop the background poll shell, and (with a hard safety check and explicit user confirmation) remove the worktree. Use when a worker's feature is fully merged and you want to clean up the room, the shell it ran in, and optionally the worktree.
disable-model-invocation: false
---

A finished worker line leaves three things running after it closes: the room, the
background poll shell, and the worktree. This skill tears all three down cleanly — in
order, with the destructive step guarded by a hard check and explicit confirmation.

**Read `/cbc` first — it owns every room mechanic** (consensus close, stall recovery,
poll teardown). This skill adds only the full teardown sequence and the worktree safety
guard.

## When to use `/cbc-clean`

- The worker's feature is **fully merged** to `main` (or the agreed base branch).
- The orchestrator is ready to close the report line to that worker.
- You want to remove the worker's worktree after the line is torn down.

Do not use `/cbc-clean` to close a line mid-feature or to force-close a hung room — use
`cbc prune` + `cbc_close` for stall recovery, and `cbc_close` alone for a normal close
mid-work.

## Teardown sequence — order is load-bearing

### Step 1 — Consensus `cbc_close` the room

`cbc_close` is a **vote**. In a 2-agent room it closes only once the other live participant
also votes. Follow the standard `/cbc` Closing procedure:

1. `cbc_recap` the room first — re-ground, make sure you have sent everything substantive.
   Do not vote close with an unsent reply.
2. (Optional but recommended) `cbc prune <room-id>` before voting — long-lived rooms
   accumulate identity-churn ghost rows that block consensus. Pruning first avoids the
   stall. See `/cbc` Closing for full stall-recovery procedure.
3. `cbc_close` — cast your vote.
4. If the room lands in `close_proposed`, it is waiting on the worker's co-vote. Keep
   polling (`cbc_wait`) until you receive `closed`. When the room closes, move to Step 2.

If the worker is dark (its poll is dead) and the room will never reach quorum: `cbc prune`
the ghost rows first; if the worker row ages out within 15 min, a single live vote then
closes the room. If you cannot wait, tell the user — do not `cbc close --force` as an
agent (that is a human-only escape hatch).

### Step 2 — Stop the background poll shell

`TaskStop` the background poll shell you were running for this room (use the label you
recorded in the `agents:` registry — see `/cbc-orchestrator`). Also end any `/loop`
pointed at this room. The poll is your presence in the room; once the room is closed, the
poll must stop.

If the shell label is no longer visible in the task list (it crashed or was already
stopped), move on — stopping an already-dead shell is not an error.

### Step 3 — Worktree removal (guarded)

This step is **optional and guarded**. The room and poll can be torn down without touching
the worktree. Only proceed here if the user wants to clean up the worktree as well.

**Hard guards — ALL must pass before offering removal:**

1. The worker's branch is **merged to `main`** (or the agreed base branch). Check with:
   `git branch --merged main | grep <branch-name>` — must appear. OR the PR is merged:
   `gh pr view <N> --json state -q .state` — must return `MERGED`.
2. The worktree is **clean** — no uncommitted or unpushed changes. Check with:
   `git -C <worktree-path> status --porcelain` — must return empty. AND
   `git -C <worktree-path> log @{u}.. --oneline` — must return empty (no unpushed commits).
   If no upstream is set: skip the second check and note it.

**If any guard fails:** do NOT offer removal. Report exactly which guard failed and why.
Never skip a failing guard — the failure is information (e.g. an unpushed hotfix, a branch
someone else is using). Leave the worktree intact.

**If all guards pass:** show the user exactly what will be removed:
- Worktree path: `<absolute-path>`
- Branch: `<branch-name>`
- The `git worktree remove` command you will run.

Then **wait for explicit `yes`** before removing. A vague "yeah" or "go ahead" counts; a
non-response, a change of subject, or a "not yet" does not. If the user does not confirm,
skip removal silently.

On confirmation: `git worktree remove <path>` (add `--force` ONLY if git reports lingering
lock files after verifying the path is genuinely clean). Then delete the branch if the user
asks.

## Anti-patterns

- **Removing the worktree without guard + confirm.** Destroys uncommitted or unpushed work.
  The guard is not optional.
- **Skipping Step 1 and jumping to Step 3.** The room close is the handshake that tells the
  worker the line is done. Close first, always.
- **Using `cbc close --force` as an agent.** `--force` is a human-only escape hatch for
  genuinely abandoned rooms. As an agent, close only through the consensus vote.
- **Treating `close_proposed` as closed.** It is not. Keep polling until `closed` lands.
- **Removing a worktree whose PR is open.** A merged PR is the gate. `OPEN` → do not remove.
