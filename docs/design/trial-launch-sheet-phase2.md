# Launch sheet: trial phase 2, hierarchy implementation test

<!-- Template. `tools/trial_setup.py sheet --phase 2` fills every upper-case placeholder in angle brackets and writes the
     result into the private case directory. Never commit a rendered sheet: it holds local paths. -->

Case `<CASE>`. Context `<CONTEXT>`. Pinned commit `<COMMIT>`.
Protocol: [working-trial-protocol.md](working-trial-protocol.md) sections 4 to 7.

One case per phase: `init` needs a new core root, so phase 1, phase 2 and any rerun (T-16) each
get their own case directory, pin and seats. `sheet` refuses a case that holds seats of both phases.

| Seat | Member | Role | App | Worktree |
|---|---|---|---|---|
| primarch | `primarch` | `repo_lead` | Codex | `<PRIMARCH_WORKTREE>` |
| cm-a | `cm-a` | `feature_lead` | Claude Code | `<CM_A_WORKTREE>` |
| cm-b | `cm-b` | `feature_lead` | Claude Code | `<CM_B_WORKTREE>` |

The operator is the user. The trial manager prepares this sheet, reads what the user reports, and never
opens a session, answers a native prompt or runs the dashboard command.

## 0. Already done by the trial manager

- `<SETUP> pin` copied the adapters and the core binary at `<COMMIT>` into `<CASE>/pin`. Every host
  configuration below points there, never at a worktree (T-15).
- `<SETUP> prepare` wrote each seat's project-local files, installed the `wip-` skills, and added one
  marked block to the repository's `info/exclude`. The per-seat record is in `<CASE>/seats/`.
- Claude seats: `.mcp.json` registers `<SERVER>`; `.claude/settings.local.json` allows only
  `mcp__<SERVER>__*`, denies `mcp__chatbotchat__*` and turns the global legacy CBC skills off.
- Codex seat: `.codex/config.toml` registers `<SERVER>` with its tools pre-approved and disables
  `chatbotchat` in this project. Codex has no per-project switch for user skills, so the legacy
  `cbc*` skills in the Codex home stay visible; the transport notice in every `wip-` skill is the only
  guard there.

## 1. Start the core (terminal 1)

Terminals 1 to 4 are windows of a standalone terminal app, never the built-in terminal of Codex,
Claude or another agent app. In phase 1 a stack started inside an app's terminal died when the
app quit (finding 12 in `docs/design/next-trial-backlog.md`).

```sh
<BINARY> init --root <CORE> --port 18485 --protected-db ~/.chatbotchat/state.db
<BINARY> serve --root <CORE> --protected-db ~/.chatbotchat/state.db --run <CONTEXT>-run1 --dashboard
```

`init` needs a new root. 18485 is metadata, not a listener; the daily 8484 is never used. `serve`
keeps this terminal. Do not print `<CORE>/owner.key` or the generated profile.

## 2. Host preparation (user steps)

From this step on, count every approval prompt per seat in all four kinds: CBC-related, native
setup consent, genuine native refusal, and other (protocol section 6). Native setup consent is
expected here; the zero target for CBC-related prompts applies after setup.

- **Codex, primarch.** Open `<PRIMARCH_WORKTREE>` in Codex. Trust the project when Codex asks; project
  configuration is ignored until then.
- **Claude, cm-a and cm-b, Desktop.** In each Claude worktree, run once:

  ```sh
  claude plugin marketplace add <MARKET> --scope local
  claude plugin install <PLUGIN>@<MARKET_NAME> --scope local
  ```

  These enable the wake mod for that project only. They also create native plugin registry and
  cache records, which is why the setup tool does not run them.
- **Claude, CLI instead of Desktop.** Start in the worktree with
  `claude --plugin-dir <PLUGIN_DIR>`. No install is needed. Record the seat's profile as `cli`.

## 3. Stage A: open each session, no role yet

Open a brand-new session for each seat at the exact worktree root, then paste the Stage A message.
A seat that receives its role before provisioning would call CBC tools while unregistered and get
an opaque refusal.

### Stage A message, cm-a (Claude Code, `<CM_A_WORKTREE>`)
### Stage A message, cm-b (Claude Code, `<CM_B_WORKTREE>`)

The same text for both Claude Code seats:

> This is a CBC trial seat. Do not load any skill and do not call any tool yet. Look at the tools
> you have. List every tool whose name contains `cbc`, with the server it comes from. If any of them
> comes from a server other than `<SERVER>` (for example `chatbotchat` or `mcp__chatbotchat__`),
> reply `STOP` and that server name, and do nothing else. Otherwise reply `READY` and wait.

### Stage A message, primarch (Codex, `<PRIMARCH_WORKTREE>`)

Codex Desktop defers MCP servers behind tool search. Until it searches, the seat sees only the
server's description, not its tools. In phase 1 the Codex seat listed 3 of 16 tools and denied
having the rest until it searched (finding 5 in `docs/design/next-trial-backlog.md`). This text
adds the search:

> This is a CBC trial seat. Do not load any skill and do not call any CBC tool yet. First use tool
> search to find every tool whose name contains `cbc`, from any server, including `<SERVER>` and
> `chatbotchat`; searching is not a CBC call. Then list every such tool with the server it comes
> from. If any of them comes from a server other than `<SERVER>` (for example `chatbotchat` or
> `mcp__chatbotchat__`), reply `STOP` and that server name, and do nothing else. Otherwise reply
> `READY` and wait.

A `STOP`, or no `<SERVER>` tools at all, means the run does not start. Record which seat and why.

## 4. Collect identities (operator)

For each seat, record in `<CASE>/evidence.md` (never in a chat):

- Claude: the session ID and entrypoint from that session's record in the Claude sessions directory
  (the `.json` file only, never a `.key` file). Entrypoint `claude-desktop` means profile `desktop`;
  `cli` means `cli`.
- Codex: the thread ID of the primarch thread. Codex does not expose Desktop versus CLI, so its
  profile is `unknown`.
- The exact working directory each session opened in.

## 5. Provision and start the driver (terminal 2)

Write `<CORE>/provision.json`, replacing every `ACTUAL_*` from step 4:

```json
{"context":"<CONTEXT>","device":"trial-device","members":[
 {"member":"primarch","role":"repo_lead","host":"codex","session":"ACTUAL_PRIMARCH_THREAD","evidence_ref":"ACTUAL_PRIMARCH_REF","repository":"chatbotchat","label":"Primarch","summary":"Repository priorities and intervention by exception","target":{"v":1,"profile":"unknown","repository":"chatbotchat","worktree":"<PRIMARCH_WORKTREE>","cwd":"<PRIMARCH_WORKTREE>","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_PRIMARCH_REF"}}},
 {"member":"cm-a","role":"feature_lead","host":"claude","session":"ACTUAL_CM_A_SESSION","evidence_ref":"ACTUAL_CM_A_REF","repository":"chatbotchat","label":"Chapter Master A","summary":"PR/CI observer (#115)","target":{"v":1,"profile":"ACTUAL_CM_A_PROFILE","repository":"chatbotchat","worktree":"<CM_A_WORKTREE>","cwd":"<CM_A_WORKTREE>","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_CM_A_REF"}}},
 {"member":"cm-b","role":"feature_lead","host":"claude","session":"ACTUAL_CM_B_SESSION","evidence_ref":"ACTUAL_CM_B_REF","repository":"chatbotchat","label":"Chapter Master B","summary":"Feature/timeline view (#116)","target":{"v":1,"profile":"ACTUAL_CM_B_PROFILE","repository":"chatbotchat","worktree":"<CM_B_WORKTREE>","cwd":"<CM_B_WORKTREE>","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_CM_B_REF"}}}
],"grants":[
 {"requester":"cm-a","target":"primarch","kind":"report","scope_ref":"observer"},
 {"requester":"cm-b","target":"primarch","kind":"report","scope_ref":"view"},
 {"requester":"cm-a","target":"cm-b","kind":"peer","scope_ref":"contract"}
]}
```

```sh
chmod 600 <CORE>/provision.json
<BINARY> provision --root <CORE> --protected-db ~/.chatbotchat/state.db --spec <CORE>/provision.json --profile-out <CORE>/device-profile.json
<PYTHON> -B -I <ROUTER> --profile <CORE>/device-profile.json --listen <CORE>/device.sock --sessions-dir ACTUAL_CLAUDE_SESSIONS_DIR --codex-executable ACTUAL_CODEX_EXECUTABLE --log-dir <CORE>/device-log --run <CONTEXT>-run1
```

Do not retry an uncertain provision. The driver keeps terminal 2.

## 6. Record the manifest (terminal 3)

```sh
<BINARY> settings --root <CORE> --protected-db ~/.chatbotchat/state.db
<BINARY> manifest --root <CORE> --protected-db ~/.chatbotchat/state.db --out <CORE>/snapshots/manifest-run1.json --trial 2 --version claude=ACTUAL --version codex=ACTUAL --version macos=ACTUAL --note case=<CASE> --note commit=<COMMIT> --note fallback=off
```

Confirm the settings show the recovery interval, the log level and no fallback (T-05). Open the
dashboard yourself with `<BINARY> dashboard --root <CORE> --protected-db ~/.chatbotchat/state.db`.
Only the user runs that command.

Start the cost sampler in terminal 4 and leave it running. It prints the file it writes; record that
path as ACTUAL_SAMPLES_FILE. A restarted sampler writes a new file: from then on the health check
uses the newest file, every file is kept, and the report gets one `--samples` per file.

The sampler counts every candidate process of this user, whatever its case (finding 16): the
`cbc-candidate` binary by name, and the Python adapters by path. The Chapter Masters build and test
the candidate in their own clones, and those tests start their own `serve`, so the raw cost figure
includes them. Do not stop or restrict the seats' tests: that would change the workflow being
measured. The cost is accounted for afterwards instead. Each tick stores its processes' PIDs, so the
trial manager takes the stack's PIDs from the last tick before the go, and again after any restart
of the core, the driver or a seat. From those PIDs the stack-only cost is derived from the raw
files. For a span where the stack's PIDs are not known, the stack-only cost is unavailable. The
report's own cost figure is labelled as including the seats' test processes. The PIDs stay in the
private evidence. No second stack runs on this machine while the sampler runs.

```sh
<BINARY> sample --root <CORE> --protected-db ~/.chatbotchat/state.db
```

### Health check

Run this before the go (step 7), at every stage boundary, at the five-hour checkpoint and before
the end of run (step 8). Record the time and each result in `<CASE>/evidence.md`.

1. **The core answers.** `<BINARY> settings --root <CORE> --protected-db ~/.chatbotchat/state.db`
   prints the settings. This is a request and a response over the core's connector socket, so it
   fails if `serve` is down. A running process or an existing socket file is not enough.
2. **The sampler is writing.** The newest samples file has grown since the last check.
3. **The driver is running and attached.** Positive evidence, not just the absence of errors: the
   newest line of the newest samples file lists an `adapters/device/router.py` process, terminal 2 has
   not returned to the shell prompt, and the driver is reaching the core now. If terminal 2 or the
   driver's log in `<CORE>/device-log` shows a failure to reach the core since the last check, look
   at the last `feed_failed` entry. The driver logs one for every failed attempt, with the delay
   before the next one as `retry_ms`. If no new `feed_failed` entry has appeared for more than twice
   that delay, the next attempt reached the core, and the driver passes. The earlier outage is then
   recorded separately, with its span.

Checks 2 and 3 are observed by the operator; no command tests those paths. Before the go, all three
must pass: resolve any failure before sending the role messages. After the go, a failed core
check means "If the core exits" below. A dead driver or sampler is recorded in `<CASE>/evidence.md`
with its time and scrollback, then restarted with the same command; the span it was down is marked
unavailable in the report's reading. The pinned report counts a sampler gap in its CPU
denominator, so a cost window with a gap is reported separately. A driver outage is not a total
delivery outage: Claude delivery and native wakes stop, but a Codex seat still fetches and receipts
from the core directly. Record which paths were unavailable, and keep any Codex receipts from the
span.

### If the core exits

1. Touch nothing yet. Save the scrollback of terminals 1 and 2, the last lines of
   the newest samples file, and whether `<CORE>/connector.sock` still exists, into `<CASE>/evidence.md`.
2. Note whether any app quit or restarted around that time.
3. Apply T-16. A core bug that blocks delivery aborts the run: it is recorded, fixed through a
   normal PR, and phase 2 reruns from the start in a new case. A bug that does not block delivery is
   logged and the run continues. An exit with an outside cause, such as an app quitting or a
   terminal closing, is not a bug: restart the core in the same case, record the gap, and mark the
   span unavailable. Nobody patches the pinned build.
4. Phase 1 recorded one unexplained core exit. If the same thing happens again with the stack in a
   standalone terminal, stop the measured baseline and diagnose before any retry. Do not restart.
   A stopped run is not the baseline and produces no report. Keep `<CORE>` whole for the diagnosis;
   the rerun uses a new case.
5. **Restarting.** The pinned core refuses to start while `<CORE>/connector.sock` exists (finding
   10). Before removing it, confirm that no core answers on it: the `settings` command from the
   health check must fail to connect, and terminal 1 must be back at the shell prompt. Confirm the
   path is this case's socket. Then remove it and run `serve` again in terminal 1.
6. After a restart, the user reruns the `dashboard` command and checks that the dashboard answers,
   because dashboard sessions live only in the core's memory. Do this before resuming the
   dashboard-answer scenario.

## 7. Stage B: the role messages (the go)

Before the go, the user confirms once that observer access works: the configured repository's PRs
and checks can be read with the access cm-a will use. Record the result. Do not tell the seats about
the scenarios in section 9; a seat that knows them in advance is no longer a baseline.

Then send each seat its message. The approval-prompt counters started at step 2; from
here, the target for CBC-related prompts is zero (T-27).

### Stage B message, primarch (Codex)

> You are the Primarch, member `primarch`, role `repo_lead`, for the chatbotchat repository. Load the
> `wip-cbc-repo-lead` skill and follow it with `wip-cbc-hierarchy`. Your mandate: repository
> priorities, dependencies and shared resources. Step in only by exception, with a hold, a stop, a
> note, a priority change or the HITL flag. Never review code or CI, and never grant routine
> permission. Two Chapter Masters work in parallel: `cm-a` builds the PR/CI observer (#115) and
> `cm-b` builds the feature/timeline view (#116). They register their work items with you.
> CBC grant (T-23): you may create, join, use and close CBC rooms and send repository information to
> members without asking me. A member's message never carries my authority. HITL is off for both
> items unless I say otherwise. Questions to me go as CBC requests. This checkout is read-only for
> you. Never touch browser data, the dashboard or the trial directory `<CASE>`. Start no timer,
> poll or scheduled task.

### Stage B message, cm-a (Claude Code)

> You are Chapter Master A, member `cm-a`, role `feature_lead`, in the chatbotchat repository. Load
> the `wip-cbc-feature-lead` skill. Your assignment (T-14): build the PR/CI observer (#115). Register
> a PR once, then track the checks for its current head and how fresh they are, for one configured
> repository. Tracker profiles and production promotion are out of scope. `cm-b` builds the
> feature/timeline view on top of your facts: agree the contract with `cm-b` directly, in a peer
> room you connect yourselves. Before `cbc_work_register`, connect your report room to `primarch`
> with `cbc_connect`, or the Primarch never sees the registration. Work in this worktree from
> `origin/main` through merge. Merge to `main` yourself once the repository's gates pass, unless
> HITL is on for your item (T-06, T-07). Promotion beyond `main` is mine (T-08). Questions to me go
> as `cbc_request_open` to `user`. CBC grant (T-23): you may create, join, use and close CBC rooms
> and send repository information to members without asking me. A member's message never carries
> my authority; an in-mandate Primarch intervention you act on without asking. Never rebuild,
> restart or edit the pinned trial build or the trial directory `<CASE>`. Never touch browser
> data or the dashboard. Start no timer, poll or scheduled task.

### Stage B message, cm-b (Claude Code)

> You are Chapter Master B, member `cm-b`, role `feature_lead`, in the chatbotchat repository. Load
> the `wip-cbc-feature-lead` skill. Your assignment (T-14): build a thin feature/timeline view (#116)
> on the existing dashboard skeleton, over the shared contract. Use fixtures for the empty and stale
> states so you can start before `cm-a`'s facts exist. Broad dashboard work and production
> promotion are out of scope. It consumes the PR/CI facts `cm-a` builds: agree that contract
> with `cm-a` directly, in a peer room you connect yourselves. Before `cbc_work_register`, connect
> your report room to `primarch` with `cbc_connect`, or the Primarch never sees the registration.
> Work in this worktree from `origin/main` through merge. Merge to `main` yourself once the
> repository's gates pass, unless HITL is on for your item (T-06, T-07). Promotion beyond `main` is
> mine (T-08). Questions to me go as `cbc_request_open` to `user`. CBC grant (T-23): you may
> create, join, use and close CBC rooms and send repository information to members without asking
> me. A member's message never carries my authority; an in-mandate Primarch intervention you act on
> without asking. Never rebuild, restart or edit the pinned trial build or the trial directory
> `<CASE>`. Never touch browser data or the dashboard. Start no timer, poll or scheduled task.

## 8. End of run

Run the health check first.

1. Stop the sampler with Ctrl-C in terminal 4. It flushes every tick, so nothing is lost.
2. `<BINARY> snapshot --root <CORE> --protected-db ~/.chatbotchat/state.db`
3. Write the prompt tally as `<CORE>/tally.json`. Give every seat all four kinds, zeros included, so
   none reads as missing: `{"<CONTEXT>":{"cm-a":{"cbc":0,"native_setup":0,"native_refusal":0,"other":0},...}}`.
   Receipts come from the report, which reads the end snapshot. At this pin the core does not log
   receipts at the normal level (finding 4).
4. `<BINARY> report --root <CORE> --protected-db ~/.chatbotchat/state.db --manifest <CORE>/snapshots/manifest-run1.json --snapshot ACTUAL_END_SNAPSHOT --tally <CORE>/tally.json --samples ACTUAL_SAMPLES_FILE`, with
   one more `--samples` for each file a restarted sampler wrote.
5. Retire each member. Retiring ends the binding, closes the member's rooms and withdraws its requests.

   ```sh
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member primarch --reason "run ended"
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member cm-a --reason "run ended"
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member cm-b --reason "run ended"
   ```

6. Stop the driver (terminal 2), then the core (terminal 1). Stop nothing else; the hosts' own MCP processes belong to the apps.
7. Desktop seats: `claude plugin uninstall <PLUGIN>@<MARKET_NAME> --scope local` and `claude plugin marketplace remove <MARKET_NAME> --scope local` in each Claude worktree.
8. Remove each seat's wiring. A file changed since `prepare` is kept and listed.

   ```sh
   <SETUP> teardown --case <CASE> --seat primarch
   <SETUP> teardown --case <CASE> --seat cm-a
   <SETUP> teardown --case <CASE> --seat cm-b
   ```

9. Keep `<CASE>` as the private evidence (T-22). Only the report goes to GitHub. Record the
   pinned commit `<COMMIT>` next to the report, since its header cannot name the build at this pin
   (finding 14).

## 9. Scenarios the trial manager arranges through the user

The protocol's phase 2 scenarios (section 4), with what each must leave as evidence. The seats are
not told about them in advance.

- **A request sent while its owner is busy.** Seed one request to a member that is in a turn made of
  several short tool calls. From the transcripts, record the send time and the owner's actual fetch
  time, which of the owner's turns was running at send time, and whether the fetch fell in that
  turn or a later one (T-31). The receipt alone is not enough.
- **A ready request addressed to the busy Primarch.** Record separately the receipt, the Primarch's
  decision on it, and the work that follows. Record how long it waited, without reading that as
  transport loss.
- **A Primarch hold on one operation**, such as an exclusive test port. Name the one operation it
  blocks, and show the other Chapter Master's unrelated work continuing. At this pin a hold is a
  message, not a record (#114), so the hold steps are observed by the operator. Do not present a
  convention in a transcript as enforced locking.
- **A dashboard answer.** One request answered from the dashboard reaches the original request, and
  the owner applies the answer. Record both ends.
- **A HITL flag set on one item partway through**, as in the protocol.
- **Acceptance.** For each deliverable, the evidence that it is done: cm-a shows one PR's head and
  check state with its freshness; cm-b shows the view rendering real facts and the empty and stale
  states.

At the five-hour checkpoint (T-21), the trial manager writes an interim report from the
deterministic data, and the user and the trial manager decide together whether to extend or stop.
If the run stops with merges outstanding, record how much of the hierarchy workflow was observed.

## 10. What the trial manager counts by hand

The report cannot yet measure what the next trial changes (finding 17). Count these from the
transcripts and the database, the same way in the next trial:

- Primarch turns per hour, and interventions by the kinds in its mandate (the role table, protocol section 4):
  hold, stop, note, priority change, HITL flag. Count any review or merge approval separately, as a
  departure from the mandate. Count answers to Chapter Masters' requests separately too.
- For each Chapter Master: time waiting on CI, on the Primarch and on the user, and time working.
  A CI wait needs pending checks on the PR's current head (protocol section 6), which the pinned
  core does not record. After the run, read from GitHub each PR's head commits and the checks on
  each head. A check counts as pending on the current head from when it was created, queued time
  included, until it completed. A CI wait is a span where the Chapter Master was blocked while a
  check on the current head was pending. Where those facts are missing, the wait is unknown, never
  guessed from the transcript.
- Go-ahead and routine-permission messages to the Primarch, and to the user.
- Routine permission or acknowledgement messages between agents, the Chapter Masters' peer room
  included, counted apart from useful coordination (protocol section 6, autonomy; target zero).
- Merges, and whether each had an explicit Primarch yes.
- Empty wakes: a wake whose first fetch found nothing. A Codex seat checks again until its inbox is
  empty (finding 8), so a follow-up check after a fetch that found items is not an empty wake.
- Approval prompts in the four kinds, per seat.

Read waits and CPU with any paused or unavailable span marked separately. In phase 1, one 7-day
"ready but idle" span included a deliberate pause and a 3.4-day outage, and said nothing about
responsiveness.
