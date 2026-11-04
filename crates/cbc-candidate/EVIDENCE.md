# Stage 1 local checkpoint

Date: 2026-11-04. Tracks [issue #113](https://github.com/Levezze/chatbotchat/issues/113).
This checkpoint reuses the earlier isolated candidate foundation. It does not install or
replace daily CBC. Native acceptance is still open; this document is not a completion claim.

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
- Codex queue hints and explicit receipt nudges; Claude channel-only delivery in the shared
  driver. Hints contain no message bodies or human approval authority.
- Live revision-checked retry settings, scoped inheritance and last-good preservation.
- Atomic distinct-message counts and informational notices, report parent only or both peers.
  Notices/retries/introductions do not count themselves. Unfrozen notices coalesce; an issued
  batch preserves exact contents through restart and receipt. Cadence changes live, with
  verified report-parent policy and shared context policy for peers.
- Exact-epoch revocation, selective invalid-binding quarantine, SIGHUP profile refresh,
  bounded native calls, coalesced hints and unread recovery after reconnect.

## Verification run

Rust 1.96.0, Python 3.14.6 on macOS arm64. Cargo ran one build job with a separate
`target-candidate` directory; Rust tests ran serially. Socket tests used disposable private
roots and synthetic bindings. These automated suites do not message real native chats.
The separate native run below does.

| Suite | Passing tests |
| --- | ---: |
| Rust core, isolation, storage, enrollment, settings, owner CLI and socket integration | 112 |
| Shared device driver | 20 |
| Shared change feed | 13 |
| Codex MCP adapter | 21 |
| Claude MCP adapter/router | 53 |
| Total | 219 |

Commands:

```sh
rtk cargo test -p cbc-candidate --target-dir target-candidate -j 1 --offline -- --test-threads=1
rtk cargo clippy -p cbc-candidate --all-targets --target-dir target-candidate -j 1 --offline -- -D warnings
rtk cargo fmt -p cbc-candidate -- --check
rtk proxy python3 -B -I crates/cbc-candidate/adapters/device/test_router.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/shared/test_feed.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/codex/test_adapter.py
rtk proxy python3 -B -I crates/cbc-candidate/adapters/claude/test_router.py
rtk git diff --check
```

Clippy with denied warnings and formatting pass. Independent read-only subagent review
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

### Remaining host gate

Claude project MCP loading and ordinary two-way tool exchange work. Automatic Desktop wake
does not yet qualify. A channel write is not host uptake. Official
[channel guidance](https://code.claude.com/docs/en/channels#security) requires session opt-in;
[development channels](https://code.claude.com/docs/en/channels-reference#test-during-the-research-preview)
require an interactive startup flag. No supported equivalent for enabling our custom channel
in the existing Desktop session was found. The environment flag alone is insufficient.

The documented [inbox socket](https://code.claude.com/docs/en/cross-session-messaging#the-sessions-inbox-socket)
publishes authentication framing but not its message envelope. The public SDK's stdin
envelope is not evidence for that socket protocol. The shared driver did not fall back to
the historical private-socket probe or change native permission settings.

Automatic Claude idle/active uptake, closed-chat wake, the remaining per-host lifecycle and
re-enrollment cases remain open. CLI-only success cannot replace Desktop acceptance.

## Remaining native acceptance

Follow [TESTING.md](TESTING.md) in disposable projects. Both Codex Desktop and Claude Desktop
Code must actually fetch and reply to markers through the same candidate core/driver, with
active, idle, reconnect, receipt, revocation and resource evidence. CLI-only tests or a queued
hint are insufficient. Native IDs, project MCP loading and Claude channel consent must be
corroborated rather than simulated. Dormant wake limitations must remain explicit.

No global native settings, permission modes, installed skills, daily service/database or
active project agents were changed. No candidate service was installed to run persistently.
The earlier approval prototype remains outside this stage and its source checkpoint.
