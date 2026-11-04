# Stage 1 local checkpoint

Date: 2026-11-04. Tracks [issue #113](https://github.com/Levezze/chatbotchat/issues/113).
This checkpoint reuses the earlier isolated candidate foundation. It does not install or
replace daily CBC. Native acceptance is still open; this document is not a completion claim.

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
- Exact-epoch revocation, selective invalid-binding quarantine, SIGHUP profile refresh,
  bounded native calls, coalesced hints and unread recovery after reconnect.

## Verification run

Rust 1.96.0, Python 3.14.6 on macOS arm64. Cargo ran one build job with a separate
`target-candidate` directory; Rust tests ran serially. Socket tests used disposable private
roots and synthetic bindings. No real native chat was messaged by these tests.

| Suite | Passing tests |
| --- | ---: |
| Rust core, isolation, storage, enrollment, settings, owner CLI and socket integration | 101 |
| Shared device driver | 20 |
| Shared change feed | 13 |
| Codex MCP adapter | 21 |
| Claude MCP adapter/router | 53 |
| Total | 208 |

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

## Required native session next

Follow [TESTING.md](TESTING.md) in disposable projects. Both Codex Desktop and Claude Desktop
Code must actually fetch and reply to markers through the same candidate core/driver, with
active, idle, reconnect, receipt, revocation and resource evidence. CLI-only tests or a queued
hint are insufficient. Native IDs, project MCP loading and Claude channel consent must be
corroborated rather than simulated. Dormant wake limitations must remain explicit.

No global native settings, permission modes, installed skills, daily service/database or
active project agents were changed. No candidate service was installed to run persistently.
The earlier approval prototype remains outside this stage and its source checkpoint.
