# Stage 1 local checkpoint

Date: 2026-11-04. Tracks [issue #113](https://github.com/Levezze/chatbotchat/issues/113).
This checkpoint reuses the earlier isolated candidate foundation. It does not install or
replace daily CBC. Native acceptance is still open; this document is not a completion claim.

Acceptance clarification, 2026-11-09: the native runs below are preliminary
transport/capability probes, including bounded foreground fixtures. They are not
a realistic hierarchy implementing features while coordinating mid-turn. The
next #113 gate requires structured logging, deterministic delivery timing and a
[working-role trial](../../docs/design/message-observability-and-working-pilot.md)
before dashboard/work-map implementation. Exact receipts already exist; the
comprehensive timed observability model is not implemented by this checkpoint.

The later [lifecycle and room policy](../../docs/design/cbc-agent-lifecycle-and-rooms.md)
adds saved-environment targeting, standing native lifecycle grants and informational 100/200
message milestones. This checkpoint implements capless sends and milestone generation.
Zero-click creation in a selected saved native environment remains separate acceptance work.
The tests below were rerun after the milestone implementation.

## Implemented

- Candidate SQLite core with ordered/idempotent messages, fair bounded checks, exact
  transport receipts, restart recovery and no lifetime room message budgets.
- Owner provisioning through the running core, one-use evidence-bound invitations,
  authorized directory discovery, deterministic report/peer connection setup and introductions.
- A single device driver with one shared watch and two delivery workers. Required native
  MCP stdio shims remain separate processes. No per-room polling shell or empty model timer.
- Codex queue hints and explicit receipt nudges; Claude channels and the native mod transport
  described below. The shared driver never uses the historical private-socket probe. Hints
  contain no message bodies or human approval authority.
- Live revision-checked retry settings, scoped inheritance and last-good preservation.
- Atomic distinct-message counts and informational notices, report parent only or both peers.
  Notices/retries/introductions do not count themselves. Unfrozen notices coalesce; an issued
  batch preserves exact contents through restart and receipt. Cadence changes live, with
  verified report-parent policy and shared context policy for peers.
- Exact-epoch revocation, selective invalid-binding quarantine, SIGHUP profile refresh,
  bounded native calls, coalesced hints and unread recovery after reconnect.
- Mandatory new-owner native targets, immutable invitation/binding-epoch target history,
  v2 private profiles, and runtime/core target agreement. See [NATIVE_TARGETS.md](NATIVE_TARGETS.md)
  for host observation limits and legacy compatibility.

## Verification run

Rust 1.96.0, Python 3.14.6 on macOS arm64. Cargo ran one build job with a separate
`target-candidate` directory; Rust tests ran serially. Socket tests used disposable private
roots and synthetic bindings. These automated suites do not message real native chats.
The separate native run below does.

| Suite | Passing tests |
| --- | ---: |
| Rust core, isolation, storage, enrollment, settings, owner CLI and socket integration | 122 |
| Shared device driver | 70 |
| Shared change feed | 13 |
| Codex MCP adapter | 31 |
| Claude MCP adapter/router | 53 |
| Native Claude mod, fake host | 11 |
| Total | 300 |

Commands:

```sh
rtk cargo test -p cbc-candidate --target-dir target-candidate -j 1 --offline -- --test-threads=1
rtk cargo clippy -p cbc-candidate --all-targets --target-dir target-candidate -j 1 --offline -- -D warnings
rtk cargo fmt -p cbc-candidate -- --check
rtk proxy python3 -B -I crates/cbc-candidate/adapters/device/test_router.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/shared/test_feed.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/codex/test_adapter.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/claude/test_router.py
rtk proxy node --test crates/cbc-candidate/adapters/claude/mod/test_mod.mjs
rtk git diff --check
```

The Rust run passed 121 tests before the final owner capability gate; its targeted owner
suite then passed all 8 tests, including that added regression, for 122 current Rust tests.
All 167 Python tests passed. The 11 Node mod tests are unchanged from their passing run.
Current strict Clippy passed; formatting also passed. Independent read-only subagent review
identified and led to fixes for blocking file/lock reads, duplicate hints, revoked-member
starvation, profile-size mismatch, unbounded reply writes and implicit old Claude fallback.

The initial whole-workspace GitHub CI run stopped under Rust 1.99 on an unchanged legacy
test at `crates/chatbotchat-core/tests/storage.rs:61`, flagged by
`clippy::useless_borrows_in_formatting`. That line is also present in base `1c582af`.
Legacy source and the existing CI workflow remain unchanged. A separate candidate-only
workflow verifies Rust and all four Python suites on Linux without suppressing that failure.
Its actual run result is tracked on draft PR #120, separately from the local results above.

## Actual native run, 2026-11-04

Private case `/private/tmp/cbc113-live-aqyctpbd` used one candidate core and one shared
driver, with project-only `cbc113` adapters. No synthetic native records or injected identity
metadata were used. The Codex project was the existing disposable
`/private/tmp/cbc-native-handoff.w7UBh3`. Claude ran in the case's private `claude-project`
folder, in Desktop's **This computer** environment. This proves that folder/execution target,
not selection of a separately saved named Desktop environment.

- Codex native chat `01a17a9e-c9b0-7223-b222-8f04798c03ed`, inspected through `read_thread`.
  The installed queue command was `codex-cli 0.165.0`.
- Claude native session `bfde8be2-b697-4fc0-9af6-442ab1cb24ae`, Desktop engine 2.1.317.
  Its host-owned session record matched the live PID, UID, start time and exact test cwd.
  Its project MCP actually ran discovery, connection, send and check calls.
- Room `cbc-report-002577f5f375a2fd8dcdeee8895ead59` was created by authorized discovery and
  `cbc_connect`, not human room-ID relaying. Repeating `connect-1` reused the room and its
  original introduction at sequence 1.

| Native observation | Result |
| --- | --- |
| Claude sends M2, Codex receives native queue hint, fetches and echoes | PASS, send 2, echo 3, then empty |
| Codex sends M1, Claude checks and echoes through the same driver/core | PASS for tool exchange, send 4, echo 5, then empty; manual check required |
| Automatic Claude Desktop channel uptake | NOT PROVEN, channel written at 21:14:35 UTC; no autonomous fetch before manual recovery at 21:15:44 UTC |
| Marker during Codex's 25-second foreground tool | Fetched and echoed at the tool boundary, send 7, echo 8; no preemption claim |
| Identical Claude send key/body repeated | PASS, both sends returned 7; one stored message |
| Driver graceful restart, same profile | PASS for pending recovery through native Claude checks; no reprovision or resend |
| Core graceful restart, same database/bindings | PASS, M4 send 10, native Claude echo 11, then empty |
| Live notice cadence changed from 100 to 2 | PASS, notices 6/9/12 addressed only to Primarch; child checks received none |
| Revoked Claude binding | PASS, driver quarantined one binding, native check rejected as unregistered |

Notices 6/9/12 corresponded to participant counts 4/6/8. Codex treated them as informational,
without reply, approval or stop. Eight automated notice tests separately prove normal
100/200 thresholds, both-peer delivery, recipient privacy, immutable batches, restart,
closure, revocation and upgrade from a real schema-4 fixture with 99 participant messages.

The foreground Codex timing fixture ran once and exited successfully. Its native turn
`01a1a86b-7b51-7b02-9b5c-97187d022ab4` fetched M3 after the command returned. A queue hint
can race with an explicit check and produce one empty follow-up turn, observed for notice 9;
this run does not claim zero redundant native turns or hard realtime interruption.

One resource snapshot showed core 9,456 KiB RSS, shared driver 15,888 KiB, Claude stdio shim
10,688 KiB and Codex stdio shim 11,760 KiB, about 46.7 MiB combined. It excludes native apps,
shell wrappers and transient CLI commands. This is one sample, not a peak/load benchmark.
There were no per-room polling processes. The project-only shims remain host-required.

### Channel-only host gate

Claude project MCP loading and ordinary two-way tool exchange work. The channel-only path
did not qualify automatic Desktop wake. A channel write is not host uptake. Official
[channel guidance](https://code.claude.com/docs/en/channels#security) requires session opt-in;
[development channels](https://code.claude.com/docs/en/channels-reference#test-during-the-research-preview)
require an interactive startup flag. No supported equivalent for enabling our custom channel
in the existing Desktop session was found. The environment flag alone is insufficient.

The documented [inbox socket](https://code.claude.com/docs/en/cross-session-messaging#the-sessions-inbox-socket)
publishes authentication framing but not its message envelope. The public SDK's stdin
envelope is not evidence for that socket protocol. The shared driver did not fall back to
the historical private-socket probe or change native permission settings.

The native mod trial below subsequently proved automatic Desktop uptake through a different
documented API. It does not claim that custom Desktop channels started working.

## Native in-process mod trial, 2026-11-04

The [candidate mod](adapters/claude/mod/README.md) uses documented Claude APIs available on
the installed Desktop and CLI versions. `$.http.fetch` supports a Unix `socketPath`;
`$.prompt.submit({text, asUser:false})` submits an attributed notification when idle.
The mod adds no receiver process and does not call a model for unchanged state.

The shared driver accepts bounded HTTP waits on its existing private Unix socket. It
corroborates the actual native caller PID, UID, process start and host-owned session record.
The request supplies no identity or credential. One existing core watch wakes these requests;
they read non-consuming binding status, never freeze or receipt an inbox. An opaque cursor
covers binding scope/epoch and first unread item. It is an equality token, not a global
message cursor that would skip newly visible older backlog. Idle responses are 204.

There is one in-process request per native session, parked for at most 25 seconds under the
host's 30-second HTTP limit. A timeout reopens the wait without a model call or transcript
entry. The driver limits parked waits and total clients. The mod backs off on transient
failures, suppresses duplicate cursors, and ignores late callbacks after interruption.
401 or a refused/dropped native prompt stops it until accepted human composer input.
No `asUser:true`, tool approval hooks, native permission changes or private inbox frames.

The private marketplace and plugin were enabled with **local scope only** in
`/private/tmp/cbc113-live-aqyctpbd/claude-project`. Its exact socket config is that project's
`.cbc-candidate-wake.json`; no global plugin option was written. Native `/reload-plugins`
confirmed re-reading the final mod source from that private folder. Installed CLI strict
plugin validation passed. The current automated results are recorded in the verification
table above; the native observations below retain their original case and sequences.

Actual observations:

| Probe | Evidence |
| --- | --- |
| Operator-generated initial wake | Marker 14, native Claude check/receipt, echo 15; UI explicitly attributed the notification to `cbc-candidate-wake` |
| Real Codex-to-Claude exchange | Fresh Codex chat `01a17a75-029b-7862-a69d-5ee06baf9acc` sent M6 at 18; Claude automatically fetched/echoed at 19; Codex automatically fetched echo/notice and then empty |
| Bounded foreground fixture | One 15-second Claude foreground command around M7; marker 21, automatic check, echo 22. This is not a hard mid-turn/preemption guarantee |
| Shared driver offline/restart | M8 committed at 24 while the driver was stopped; same profile restarted; Claude automatically recovered/echoed at 25 with no resend/manual prompt; Codex fetched echo and empty |
| Final reviewed driver diagnostics | `native_mod_attached`, M9 send 27, `native_mod_pending`, native check/receipt, echo 28; Codex fetched echo and empty |

The final context was `cbc113-mod-pair`, room
`cbc-report-aa7b2b7ea1ef627fac6d59e9aebe4750`. Claude retained native session
`bfde8be2-b697-4fc0-9af6-442ab1cb24ae`, live corroborated PID 65066. At 21:55:49.381 UTC
M9 committed, the mod pending event followed at 21:55:49.383, and the native check followed
at 21:55:51.432. The notification contained no marker body; Claude fetched that through CBC.
Earlier tests using the previous native Codex chat remain separately recorded above.

A sample showed the candidate core at 9,488 KiB RSS, driver at 19,808 KiB, and the existing
native Claude host at 474,896 KiB. That native host survived over eight minutes in the test.
These are samples, not peak/load results. The mod does not eliminate the native app/session's
own memory cost, and this does not prove indefinite idle retention or restart of an exited
Desktop process. The 25-second waits are host callbacks, not terminal polls or model timers.

Test cleanup initially terminated the first Codex chat's host-owned MCP shim. That chat
retained a closed transport instead of respawning it. The native pair trial therefore reused
an existing spare disposable chat; it did not restart the app or create a production chat.
Future cleanup must revoke test bindings and stop the owned driver/core, leaving host-owned
shim lifecycle to the app. Do not treat a killed cached MCP transport as an empty inbox.

The final Codex binding was intentionally revoked; its native check rejected the operation
without retry. The Claude binding was then revoked to stop its mod's HTTP receiver. Both
owned candidate processes were stopped, with test profiles/history preserved privately.
Host-owned shims were left to the apps for this cleanup.

Sources: [mod support](https://code.claude.com/docs/en/plugins/mods/overview),
[prompt and network APIs](https://code.claude.com/docs/en/plugins/mods/api),
[public API declarations](https://github.com/anthropics/claude-code/blob/main/mods/types/claude-code.d.ts).

## Native CLI mod trial

Claude Code CLI 2.1.312 ran interactively in the private project
`/private/tmp/cbc113-cli-bawqyuqb/claude-project`, with the same public mod transport.
The session loaded the reviewed mod with `--plugin-dir` and only the project's `cbc113`
MCP server with `--strict-mcp-config`. Built-in tools were disabled with `--tools ""`.
The macOS `script` utility supplied the actual terminal because RTK's output capture alone
did not provide a native interactive terminal. No Desktop handoff, print mode or permission
mode override was used. Kernel PID/UID/start evidence and the host session record verified
native session `5c929307-3923-4cff-9144-fdaec228e997`, initially PID 53342, entrypoint `cli`.

The sender was a foreground operator fixture using the provisioned `cli-primarch` binding.
Only the actual Claude receiver belonged to the shared driver's profile. These sends prove
native CLI receipt and echo, not another native Codex execution. The separate Desktop pair
above remains the native Codex-to-Claude proof. Receiver observations used non-consuming
`binding_status`; the operator never checked or receipted the receiver's inbox.

| Probe | Evidence |
| --- | --- |
| Idle delivery | After about 65 seconds idle, `CLI-M1-REQUEST` committed at 2; the native CLI automatically fetched it and echoed at 3; unread then cleared |
| Native Stop and human rearm | Escape interrupted an active CLI turn. `CLI-M3-REQUEST` committed at 4 and stayed unread for over 85 seconds with last receipt 2. Genuine composer input rearmed the mod; native fetch and echo 5 followed |
| Shared driver offline/restart | `CLI-M2-REQUEST` committed at 6 while the owned driver was stopped. Restarting with the same profile recovered it automatically and produced echo 7, without another send or manual prompt |
| CLI exit and operator-initiated resume | `/exit` returned 0 and the old process disappeared from native session records. M4 committed at 8 while absent. Public `--resume` restored the same UUID in verified new PID 85191; an automatic attributed mod notification led to native check/receipt and echo 9, without reprovisioning, another send or manual check |
| Final receiver state | `first_unread_sequence=null`, `last_receipted_sequence=8` |

Private operator checkpoints and sanitized results remain under the case directory.
Native-mode MCP hello and post-receipt wake now use non-consuming binding status, so they
do not freeze the next inbox batch or report a false dead-chat diagnostic. Connected
channels retain their existing delivery behavior. Three additional device tests cover
these paths, rejected hello arguments and error propagation.

On resume, the driver recorded hello at Unix time 1793847744.6035, a mod pending event at
1793847744.692, native check at 1793847747.063 and echo send at 1793847750.910. A positional
READY-style prompt supplied with resume did not appear, so it is not counted as accepted
human input or the cause of recovery. The operator explicitly launched `--resume`; this
proves unread recovery after CLI restart, not autonomous restart of a closed Desktop chat.
Cleanup revoked both fresh bindings from epoch 1 to 2, exited the owned CLI and stopped
only the owned candidate core/driver, preserving private history.

## Desktop Stop, idle and engine recovery follow-up

The same native Claude Desktop session `bfde8be2-b697-4fc0-9af6-442ab1cb24ae` ran in
engine 2.1.317, initially PID 30178. Native Stop interrupted a bounded generation at 420 of
1000. Marker M2 at sequence 34 remained unread for over 100 seconds. Genuine composer input
`RESUMED` rearmed the mod, followed by automatic fetch, echo 35 and an empty check.

A measured 697.5-second idle interval produced no CBC hint, check, receipt or send. M3 then
committed at 37 and was fetched automatically, echoed at 38, and drained. This proves that
11-minute 37.5-second interval, not indefinite idle retention or a peak-memory bound.

For a single-engine failure probe, an identity-fenced SIGTERM stopped only that disposable
native PID. M4 committed at 40 while the process was absent; native records were empty and
the item stayed unread. Opening its Desktop view did not restart the engine. Genuine
composer input `RESTARTED` started new PID 49706 with the same UUID/version, then the mod
automatically recovered M4, echoed at 41 and cleared unread state with receipt 40. This is
store-and-forward recovery after user-initiated engine restart. Full app quit was not tested
because it would interrupt unrelated sessions; autonomous dormant restart is not established.

## Native v2 target acceptance

Fresh context `cbc113-target-pair` used the same corroborated Claude session and Codex chat
`01a17a75-029b-7862-a69d-5ee06baf9acc`. Authorized native discovery/connect established room
`cbc-report-1d813ec5ce21200ef6b8836d6b69ca26`. Both used new v2 profiles with complete targets.
Codex sent at 44; Claude automatically checked and echoed at 45; Codex fetched that echo
and informational notice 46 during an explicitly requested check. Its ordinary queue hint
timed out after 10 seconds under heavy machine load. Automatic v2 return uptake is therefore
not established by this exchange; the earlier mod pair's automatic return remains separate
evidence. These are native peers, not operator-fixture sends.

The Codex saved project ID `ab71799e-d999-40e8-bfe5-8e4db004ea3d` came from supported
`list_projects` and remains audit evidence only. Current Codex MCP supplies thread identity
and shim cwd, not a runtime Desktop/CLI distinction or saved UI IDs. Its target therefore
uses `profile:"unknown"` and `not_exposed` selectors. Claude qualifies actual cwd/entrypoint
from native process observations. New owner capability checks reject currently unobservable
configured selectors before provisioning. See [the contract](NATIVE_TARGETS.md).

For a native drift probe, only the Codex profile target's worktree changed from null to `/`,
with credentials and epoch preserved. Its next `cbc_check` failed with
`core unavailable or native target authority does not match`; unread/receipt state did not
advance beyond last receipt 46. Restoring the exact private profile recovered an empty check.
An edited local target cannot replace the core's target authority for that binding epoch.

During the final reverse-direction probe, Claude engine PID 49706 became absent again while
idle. The cause is unverified. Item 47 remained unread with last receipt 44. Supported
`claude --desktop --resume UUID` exited 0 and printed an opening-session message, but native
records stayed empty and the item remained unread. This result is `dormant`, not wake PASS.
Installed Codex 0.165 also rejected `queue --resume` as unsupported. Neither a successful
open command nor an unsupported flag resolves native dormant wake.

Cleanup revoked both target-pair bindings from epoch 1 to 2 and stopped only owned candidate
core PID 36360 and driver PID 57179. Pending item 47 and private history were preserved.
Host-owned shims, daily CBC and unrelated sessions were left untouched.

## Remaining native acceptance

The Desktop pair and CLI trial prove automatic fetch/reply, Stop/rearm and reconnect through
the shared core/driver. Native v2 target binding and drift rejection are also proven above.
Full app quit/restart and autonomous dormant wake remain unproven. Saved native environment
selection belongs to #117 and is not implied by cwd or audit labels. Follow [TESTING.md](TESTING.md)
for isolated procedures and preserve these capability limits. #113 remains open at this checkpoint.

## Next continuation

Prepare the logging/timing contract and bounded working-trial manifest before
running the candidate again. Recheck actual native versions and identities, then
qualify implementing Primarch/Chapter Master sessions under busy/mid-turn, idle,
restart and Stop conditions. Include v2 Claude-origin delivery and automatic
Codex uptake. Preserve unread-47 evidence and distinguish busy deferral, exited
engine activation and explicit Stop rearm. Old CLI/open results are not current
capability guarantees. Dashboard/work-map stages follow this working gate.
No main merge, daily cutover or Pi deployment is authorized by the plan update.

No global native settings, permission modes, installed skills, daily service/database or
active project agents were changed. No candidate service was installed to run persistently.
The earlier approval prototype remains outside this stage and its source checkpoint.
