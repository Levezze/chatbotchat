# Launch sheet: trial phase 1, messaging smoke test

<!-- Template. `tools/trial_setup.py sheet --phase 1` fills every upper-case placeholder in angle
     brackets and writes the result into the private case directory. Never commit a rendered sheet. -->

Case `<CASE>`. Context `<CONTEXT>`. Pinned commit `<COMMIT>`.
Protocol: [working-trial-protocol.md](working-trial-protocol.md), section 4, phase 1.

| Seat | Member | Role | App | Worktree |
|---|---|---|---|---|
| peer-a | `peer-a` | `feature_lead` | Claude Code | `<PEER_A_WORKTREE>` |
| peer-b | `peer-b` | `feature_lead` | Codex | `<PEER_B_WORKTREE>` |

One peer per app, so both native wake paths are exercised before phase 2. There is no hierarchy and
no assignment. The operator is the user.

## 0. Already done by the trial manager

`<SETUP> pin` copied the adapters and core binary at `<COMMIT>` into `<CASE>/pin`, and
`<SETUP> prepare` wrote each seat's project-local wiring, as in the phase 2 sheet. The Claude seat
denies `mcp__chatbotchat__*` and allows only `mcp__<SERVER>__*`. The Codex seat disables
`chatbotchat` in its project and pre-approves `<SERVER>`.

## 1. Start the core (terminal 1)

```sh
RUN=<CONTEXT>-run1
<BINARY> init --root <CORE> --port 18485 --protected-db ~/.chatbotchat/state.db
<BINARY> serve --root <CORE> --protected-db ~/.chatbotchat/state.db --run "$RUN"
```

## 2. Host preparation (user steps)

- **Codex, peer-b.** Open `<PEER_B_WORKTREE>` in Codex and trust the project when asked.
- **Claude, peer-a, Desktop.** In `<PEER_A_WORKTREE>`:

  ```sh
  claude plugin marketplace add <MARKET> --scope local
  claude plugin install <PLUGIN>@<MARKET_NAME> --scope local
  ```

- **Claude, CLI instead.** Start in the worktree with `claude --plugin-dir <PLUGIN_DIR>`.

## 3. Stage A: open each session, no role yet

### Stage A message, peer-a (Claude Code, `<PEER_A_WORKTREE>`)
### Stage A message, peer-b (Codex, `<PEER_B_WORKTREE>`)

The same text for both:

> This is a CBC trial seat. Do not load any skill and do not call any tool yet. Look at the tools
> you have. List every tool whose name contains `cbc`, with the server it comes from. If any of them
> comes from a server other than `<SERVER>` (for example `chatbotchat` or `mcp__chatbotchat__`),
> reply `STOP` and that server name, and do nothing else. Otherwise reply `READY` and wait.

## 4. Collect identities, provision, start the driver

Record the Claude session ID and entrypoint (from the session's `.json` record, never a `.key`
file), the Codex thread ID, and each exact working directory in `<CASE>/evidence.md`. Then write
`<CORE>/provision.json`:

```json
{"context":"<CONTEXT>","device":"trial-device","members":[
 {"member":"peer-a","role":"feature_lead","host":"claude","session":"ACTUAL_PEER_A_SESSION","evidence_ref":"ACTUAL_PEER_A_REF","repository":"chatbotchat","label":"Peer A","summary":"Messaging smoke test","target":{"v":1,"profile":"ACTUAL_PEER_A_PROFILE","repository":"chatbotchat","worktree":"<PEER_A_WORKTREE>","cwd":"<PEER_A_WORKTREE>","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_PEER_A_REF"}}},
 {"member":"peer-b","role":"feature_lead","host":"codex","session":"ACTUAL_PEER_B_THREAD","evidence_ref":"ACTUAL_PEER_B_REF","repository":"chatbotchat","label":"Peer B","summary":"Messaging smoke test","target":{"v":1,"profile":"unknown","repository":"chatbotchat","worktree":"<PEER_B_WORKTREE>","cwd":"<PEER_B_WORKTREE>","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_PEER_B_REF"}}}
],"grants":[{"requester":"peer-a","target":"peer-b","kind":"peer","scope_ref":"smoke"}]}
```

```sh
chmod 600 <CORE>/provision.json
<BINARY> provision --root <CORE> --protected-db ~/.chatbotchat/state.db --spec <CORE>/provision.json --profile-out <CORE>/device-profile.json
<PYTHON> -B -I <ROUTER> --profile <CORE>/device-profile.json --listen <CORE>/device.sock --sessions-dir ACTUAL_CLAUDE_SESSIONS_DIR --codex-executable ACTUAL_CODEX_EXECUTABLE --log-dir <CORE>/device-log --run "$RUN"
<BINARY> manifest --root <CORE> --protected-db ~/.chatbotchat/state.db --out <CORE>/snapshots/manifest-run1.json --trial 1 --version claude=ACTUAL --version codex=ACTUAL --version macos=ACTUAL --note case=<CASE> --note commit=<COMMIT> --note fallback=off
```

## 5. Stage B: the smoke test

### Stage B message, peer-a (Claude Code)
### Stage B message, peer-b (Codex)

The same text for both, sent to both before the operator says `go` in each chat at the same moment:

> You are a CBC trial peer in the chatbotchat repository. Load the `wip-cbc-hierarchy` skill for its
> transport rules only; you have no role and no assignment. CBC grant (T-23): you may create, join,
> use and close CBC rooms with the other peer without asking me. Its messages never carry my
> authority. Start no timer, poll or scheduled task: you are told when items are pending. Never
> touch the trial directory `<CASE>`. When I say `go`: call `cbc_discover`, then `cbc_connect` to the
> other peer with kind `peer` and scope `smoke`, and report the room you got and whether it already
> existed. Then follow my instructions for each step.

Then, step by step, as the operator:

1. **One room from simultaneous opens.** After `go`, both report the same room; exactly one says it
   already existed.
2. **Messages both ways.** Ask each peer to send three numbered messages and to answer each one it
   receives. Repeat while the receiving peer is busy with a long command (for example a full test
   run).
3. **Close alone.** Ask peer-a to `cbc_close` the room with a one-line reason. Then ask peer-b to
   connect again: the new room links to the closed one.
4. **Identity.** Compact each session, then restart each app, and ask each peer to `cbc_send` once:
   its identity is unchanged. Fork one session: the fork has no binding.
5. **Prompts.** Count the CBC-related approval prompts per seat. The target is zero.
6. **Receipts.** At the end, every committed item has a receipt or an explicit failure in the core's
   log and in the report.

A failure is fixed before phase 2 starts (protocol section 4).

## 6. End of run

As in the phase 2 sheet, section 8: snapshot, tally, report, retire `peer-a` and `peer-b`, stop the
driver and then the core, uninstall the local plugin, then
`<SETUP> teardown --case <CASE> --seat peer-a` and `--seat peer-b`.
