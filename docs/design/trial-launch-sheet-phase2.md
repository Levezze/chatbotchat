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

```sh
<BINARY> init --root <CORE> --port 18485 --protected-db ~/.chatbotchat/state.db
<BINARY> serve --root <CORE> --protected-db ~/.chatbotchat/state.db --run <CONTEXT>-run1 --dashboard
```

`init` needs a new root. 18485 is metadata, not a listener; the daily 8484 is never used. `serve`
keeps this terminal. Do not print `<CORE>/owner.key` or the generated profile.

## 2. Host preparation (user steps)

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

### Stage A message, primarch (Codex, `<PRIMARCH_WORKTREE>`)
### Stage A message, cm-a (Claude Code, `<CM_A_WORKTREE>`)
### Stage A message, cm-b (Claude Code, `<CM_B_WORKTREE>`)

The same text for all three:

> This is a CBC trial seat. Do not load any skill and do not call any tool yet. Look at the tools
> you have. List every tool whose name contains `cbc`, with the server it comes from. If any of them
> comes from a server other than `<SERVER>` (for example `chatbotchat` or `mcp__chatbotchat__`),
> reply `STOP` and that server name, and do nothing else. Otherwise reply `READY` and wait.

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

## 7. Stage B: the role messages (the go)

Send each seat its message. From here, count every CBC-related approval prompt per seat (T-27), for
the report's tally file.

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
> a PR once, then track the checks for its current head and how fresh they are. `cm-b` builds the
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
> the `wip-cbc-feature-lead` skill. Your assignment (T-14): build the feature/timeline view (#116) on
> the existing dashboard skeleton. It consumes the PR/CI facts `cm-a` builds: agree that contract
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

1. `<BINARY> snapshot --root <CORE> --protected-db ~/.chatbotchat/state.db`
2. Write the prompt tally as `<CORE>/tally.json`, shaped `{"<CONTEXT>":{"cm-a":{"cbc":0},...}}`.
3. `<BINARY> report --root <CORE> --protected-db ~/.chatbotchat/state.db --manifest <CORE>/snapshots/manifest-run1.json --snapshot ACTUAL_END_SNAPSHOT --tally <CORE>/tally.json`
4. Retire each member. Retiring ends the binding, closes the member's rooms and withdraws its requests.

   ```sh
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member primarch --reason "run ended"
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member cm-a --reason "run ended"
   <BINARY> retire --root <CORE> --protected-db ~/.chatbotchat/state.db --context <CONTEXT> --member cm-b --reason "run ended"
   ```

5. Stop the driver (terminal 2), then the core (terminal 1). Stop nothing else; the hosts' own MCP processes belong to the apps.
6. Desktop seats: `claude plugin uninstall <PLUGIN>@<MARKET_NAME> --scope local` and `claude plugin marketplace remove <MARKET_NAME>` in each Claude worktree.
7. Remove each seat's wiring. A file changed since `prepare` is kept and listed.

   ```sh
   <SETUP> teardown --case <CASE> --seat primarch
   <SETUP> teardown --case <CASE> --seat cm-a
   <SETUP> teardown --case <CASE> --seat cm-b
   ```

8. Keep `<CASE>` as the private evidence (T-22). Only the report goes to GitHub.
