# Isolated native test session for #113

Updated acceptance, 2026-11-09: the protocol probes below are preparation, not
full working-framework acceptance. Follow the
[observability and working hierarchy trial](../../docs/design/message-observability-and-working-pilot.md)
for a fresh Primarch with parallel implementing Chapter Masters, correlated
timestamps, debug deltas and actual mid-turn uptake evidence. Prepare that trial
before dashboard/work-map work; do not run it merely because these docs changed.

Run this after the candidate's synthetic suites pass. This session qualifies actual Codex and Claude Code apps separately from their CLIs. It does not install CBC, migrate daily rooms or change native permissions. See [device driver](adapters/device/README.md), [Claude adapter](adapters/claude/README.md), and [native mod](adapters/claude/mod/README.md) for protocol details. Historical private-socket experiments in those documents are not supported-channel acceptance.

For the Desktop mod test, install only the candidate plugin in the designated disposable
project's local scope, following its README. Put the private device socket in that project's
`.cbc-candidate-wake.json`. Keep MCP for send/check; the mod supplies attributed native hints.
It uses no channel-preview flag or permission hook. The mod and channel paths have separate
acceptance evidence; a successful channel write never substitutes for model uptake.

## Added acceptance requirements

Follow the [current lifecycle and room policy](../../docs/design/cbc-agent-lifecycle-and-rooms.md).
For #113 enrollment, record the configured target, actual host/device/native session, repository
and saved project/environment evidence where supported. Verify execution target and UI grouping
separately. A matching folder or a headless transcript does not qualify the chosen Desktop environment.
Read [the native target contract](NATIVE_TARGETS.md) for the mandatory owner target, v2 private
profile and current per-host observation limits before provisioning.

Milestone notices are implemented. Send beyond 100 and 200 distinct
messages without an extension/approval/pause, verify parent-only report notices and both-peer
notices, and prove retries/notices do not count themselves. Check restart and live cadence changes
without duplicate notices. A notice must not create a decision card, hold or required response.
The automated suite exercises 100/200; a disposable native run can lower
`defaults.room_notice_every_messages` to avoid spending 200 model turns. This does not replace
the automated boundary tests. Report cadence uses the verified parent's effective policy;
peer cadence uses defaults and context policy, shared by both peers.

Full create/bind/initial-assignment/message/resume acceptance belongs to #117. Prove a standing
Primarch target grant, actual saved environment/UI placement, one child under duplicate or
uncertain creation, and no computer use where supported. Native permission exceptions remain
separate. Do not silently substitute CLI workers or mark documented-but-untested handoff as PASS.

## Prepare one private case

Use the checkout containing this file. Keep one core and one device driver in dedicated terminals. Host-required MCP stdio shims still exist; no chat gets a polling shell.

```sh
CBC113_REPO="$PWD"
CBC113_CASE=$(rtk proxy mktemp -d /private/tmp/cbc113.XXXXXX)
CBC113_ROOT="$CBC113_CASE/core"
CBC113_BIN="$CBC113_REPO/target-candidate/debug/cbc-candidate"
CBC113_PROTECTED="$HOME/.chatbotchat/state.db"
CBC113_RUN="run-1"
rtk proxy mkdir "$CBC113_CASE/codex-project" "$CBC113_CASE/claude-project"
rtk proxy chmod 700 "$CBC113_CASE/codex-project" "$CBC113_CASE/claude-project"
rtk proxy "$CBC113_BIN" init --root "$CBC113_ROOT" --port 18485 --protected-db "$CBC113_PROTECTED"
rtk proxy "$CBC113_BIN" serve --root "$CBC113_ROOT" --protected-db "$CBC113_PROTECTED" --run "$CBC113_RUN"
```

The last command owns that terminal. Reuse these exact variable values in the other terminals. Set `CBC113_PROTECTED` to the actual daily DB if it differs. `18485` is reserved candidate metadata, not a TCP listener. Daily `8484` is forbidden. Init requires a new root; keep socket paths below 104 bytes. Do not print `owner.key`, the generated profile, credentials or receipt references. Keep test documents mode 0600 and projects private. Record build commit, test results, actual executable versions and core/driver PIDs in `CASE/evidence.md`, never secrets.

## Load project-only adapters and collect identity

Create only these new projects' local configurations, expanding every absolute path before saving. No shell-variable expansion is promised inside host configuration. Use a distinct server name `cbc113`.

Codex `codex-project/.codex/config.toml`:

```toml
[mcp_servers.cbc113]
command = "/ABSOLUTE/PYTHON3"
args = ["-B", "-I", "/ABSOLUTE/CHECKOUT/crates/cbc-candidate/adapters/codex/adapter.py"]
env = { CBC_CANDIDATE_PROFILE = "/private/tmp/ACTUAL_CASE/core/device-profile.json", CBC_CANDIDATE_CONNECTOR = "/private/tmp/ACTUAL_CASE/core/device.sock" }
```

Claude `claude-project/.mcp.json`:

```json
{"mcpServers":{"cbc113":{"command":"/ABSOLUTE/PYTHON3","args":["-B","-I","/ABSOLUTE/CHECKOUT/crates/cbc-candidate/adapters/claude/adapter.py"],"env":{"CBC_CANDIDATE_CONNECTOR":"/private/tmp/ACTUAL_CASE/core/device.sock","CBC_CANDIDATE_CHANNEL":"0"}}}}
```

This config uses the mod for wake notifications. For a separate channel trial, enable
`CBC_CANDIDATE_CHANNEL=1` and complete the host's supported per-session channel opt-in.

Open designated disposable chats in those projects through supported host actions. Collect their actual native IDs from supported host thread tools/exports and host-owned session records. For Claude, verify the real sessions directory and the live parent PID/start-time record used by the [router corroboration](adapters/claude/README.md). For Codex, verify that native `tools/call` metadata supplies the actual `_meta.threadId`. A display name, model argument, copied UUID or `evidence_ref` label alone proves nothing. Keep a dated, nonsecret reference to the evidence. Do not create fake session records or inject metadata to pass a native test.

If the app has not loaded project MCP, record `notLoaded`. Complete genuine project trust, MCP consent and supported channel preview enablement only when required by the host. The environment flag alone does not enable Claude channels. No global MCP edit, permission-bypass flag, approval-mode change or computer-use workaround belongs in this procedure. A required native action that cannot be performed through a supported interface is an explicit test gate.

### Claude CLI mod branch

Use a fresh disposable project and case. Load the reviewed mod with `--plugin-dir`, the
project's adapter with `--strict-mcp-config --mcp-config /ABS/PROJECT/.mcp.json`, and
`--tools ""` to retain MCP while disabling built-ins. Keep authentication and permission
mode unchanged. Supply a real interactive terminal. On macOS, when RTK captures stdout,
the tested wrapper is:

```sh
rtk proxy /usr/bin/script -q /dev/null /ABS/claude --ax-screen-reader --plugin-dir /ABS/MOD --strict-mcp-config --mcp-config /ABS/PROJECT/.mcp.json --tools ""
```

Corroborate the generated native session ID and live PID/UID/start before provisioning.
Start with a no-tools READY prompt while the operator prepares the core and driver.
Require actual marker fetch, echo and cleared unread state after an idle interval and
after driver restart. If the sender is an operator fixture, label it that way; it does
not establish native Codex sending. The [CLI trial](EVIDENCE.md#native-cli-mod-trial)
records those results separately from the native Desktop pair.

For Stop/rearm, interrupt an active native CLI turn with Escape and verify the abort.
Send a new marker after Stop; observe it through `binding_status` only and require it to
remain unread without a model wake for at least 40 seconds. Genuine operator composer
input must then rearm the mod and recover the marker. The recorded CLI Stop interval
exceeded 85 seconds. For CLI exit/resume, exit through `/exit`, verify the actual process
is gone, send while absent, then launch public `--resume ACTUAL_UUID` with the same project
and session flags. Require a newly corroborated PID and automatic pending recovery without
reprovisioning or another send. That CLI probe passed. Desktop Stop/rearm and single-engine
exit/reopen also passed separately; full app quit and autonomous restart remain unproven.

## Enroll once and start the shared driver

Write `ROOT/provision.json` privately using the following shape. Replace every `ACTUAL_*` from verified evidence. Choose fresh logical member IDs. Owner provisioning trusts the operator's corroboration; the manifest is not a native identity verifier.

```json
{"context":"cbc113-test","device":"test-device","members":[
 {"member":"test-primarch","role":"repo_lead","host":"codex","session":"ACTUAL_CODEX_UUID","evidence_ref":"ACTUAL_CODEX_EVIDENCE_REF","repository":"cbc-test","label":"Test Primarch","summary":"Disposable delivery test","target":{"v":1,"profile":"unknown","repository":"cbc-test","worktree":null,"cwd":"/ACTUAL/CODEX/PROJECT","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_CODEX_PLACEMENT_REF"}}},
 {"member":"test-chapter","role":"feature_lead","host":"claude","session":"ACTUAL_CLAUDE_SESSION","evidence_ref":"ACTUAL_CLAUDE_EVIDENCE_REF","repository":"cbc-test","label":"Test Chapter Master","summary":"Disposable delivery test","target":{"v":1,"profile":"desktop","repository":"cbc-test","worktree":null,"cwd":"/ACTUAL/CLAUDE/PROJECT","saved_project":{"status":"not_exposed"},"saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"ACTUAL_CLAUDE_PLACEMENT_REF"}}}
],"grants":[{"requester":"test-chapter","target":"test-primarch","kind":"report","scope_ref":"delivery-test"}]}
```

```sh
rtk proxy chmod 600 "$CBC113_ROOT/provision.json"
rtk proxy "$CBC113_BIN" provision --root "$CBC113_ROOT" --protected-db "$CBC113_PROTECTED" --spec "$CBC113_ROOT/provision.json" --profile-out "$CBC113_ROOT/device-profile.json"
rtk proxy python3 -B -I "$CBC113_REPO/crates/cbc-candidate/adapters/device/router.py" --profile "$CBC113_ROOT/device-profile.json" --listen "$CBC113_ROOT/device.sock" --sessions-dir /ACTUAL/CLAUDE/SESSIONS --codex-executable /ACTUAL/INSTALLED/CODEX --log-dir "$CBC113_ROOT/device-log" --run "$CBC113_RUN"
```

Set `CBC113_RUN` to one id per run (1 to 64 of letters, digits, `.`, `_`, `-`). The core logs to `$CBC113_ROOT/logs/core.jsonl` and the driver to `device-log/router-events.jsonl`, both v1 lines that carry the id. Switch to debug by setting `"log_level":"debug"` in `$CBC113_ROOT/runtime-settings.json` through the settings mechanism, and back by removing it; neither needs a restart. The core applies it at its next request; the driver within its 60-second recovery pass. Neither log holds a token or a message body.

Use `profile:"cli"` for an observed Claude CLI entrypoint. Provisioning rejects unsupported
Codex execution-profile claims or configured saved selectors before changing state. Its output
profile is v2 and includes each complete target. Provisioning prints only paths/counts. Do not retry an uncertain provision against the same members or delete its output reservation to force replay. Preserve evidence and resolve custody first. The driver uses the same private profile for both hosts and stays in its terminal. `connector.sock` is core RPC; `device.sock` is the shared driver. Never point either adapter at daily CBC.

## Delivery sequence and proof

Give both chats this bounded task: use only candidate `cbc113` tools, call `cbc_check {}` on a hint, echo each test marker through CBC once, and report no new permissions. In each chat call `cbc_discover {}`. Establish the report route with `cbc_connect {"target":"test-primarch","kind":"report","scope_ref":"delivery-test","key":"connect-1"}` from the Chapter Master. Repeat the exact call and verify the same room and single introduction. No human room-ID courier is needed.

Use unique bodies `CASE-M1-codex-to-claude`, `CASE-M2-claude-to-codex`, `CASE-M3-active`, `CASE-M4-reconnect`, replacing `CASE` with the actual case basename. Send with `cbc_send {"room":"DISCOVERED_ROOM","body":"MARKER","key":"UNIQUE_MARKER_KEY"}`. Record each returned server sequence, monotonic send/hint/check time, native chat ID and actual tool-result marker. Receipt is transport delivery after the adapter writes the tool result, not comprehension, completion or human approval.

1. Idle Claude, send M1 from Codex; then idle Codex, send M2 from Claude. Each recipient must actually fetch its marker and send its echo. Check again and require `empty` after all test items drain.
2. During a bounded, harmless active turn in each app, send M3 from its peer. Record whether uptake occurs mid-turn, at a tool boundary or after the turn. Do not infer mid-turn support from queue acceptance. Use a bounded foreground tool only, never a per-chat background poll.
3. Stop only the owned device driver, send M4, then restart the exact driver command without sending again. Require unread delivery and echo through the existing bindings. Repeat with a graceful core stop/restart and the same root/profile. Do not provision again.
4. While a recipient has not checked, send two distinct markers and repeat one identical send key/body. Verify one sequence for the retry, both distinct items eventually fetched, and bounded/coalesced hints. After receipt, verify no further wake for those items. A reconnect may legitimately re-hint unread work; it must not duplicate stored messages or claim receipt.
5. Close a disposable native chat and send another marker. Record `dormant` separately. Codex `queue` exit 0 is only queue acceptance. A Claude channel write is only delivery to a live channel. Neither proves that a closed Desktop chat woke. Resume through supported host actions, then verify pending recovery.

Preserve sanitized adapter/core evidence of the receipt and a following empty check. Do not extract receipt tokens into the evidence log. An unavailable adapter, missing consent, unsupported metadata or dormant wake is a limitation, never an empty inbox or PASS. CLI acceptance does not substitute for either Desktop app.

### Target drift probe

After successful v2 native delivery, preserve the exact private profile. Change only one
target field while keeping credentials/epoch fixed, then call the native adapter. Require
rejection with no receiver receipt progress. A device SIGHUP refresh must also reject drift
and retain its prior registry. Restore the exact profile and require normal recovery. The
core's target for that epoch is the authority; changing a local file is not re-enrollment.
Use only the disposable case and preserve all private evidence.

## Change policy without restart

```sh
rtk proxy "$CBC113_BIN" settings --root "$CBC113_ROOT" --protected-db "$CBC113_PROTECTED"
```

Copy its nonsecret `revision` and `settings` into the test record. Save only the `settings` object as `ROOT/settings-next.json`; change `defaults.retry_initial_ms` to `500` and `defaults.retry_max_ms` to `2000`. Preserve any overrides. Then apply:

```sh
rtk proxy "$CBC113_BIN" settings --root "$CBC113_ROOT" --protected-db "$CBC113_PROTECTED" --file "$CBC113_ROOT/settings-next.json" --expected-revision ACTUAL_PREVIOUS_REVISION
```

Require a new revision and unchanged core/driver PIDs. Reusing the old revision must conflict. An invalid policy must fail without replacing the accepted policy. During an actual unavailable-target retry, observe the new backoff without a model timer. Last-good recovery after corrupt primary settings is a synthetic-suite test until a dedicated safe fault-injection step is supplied; do not corrupt live files by improvisation.

## Revocation and shutdown

After the delivery/reconnect tests, revoke one disposable binding using its original epoch from the private profile. Read that value locally without printing credentials. Keep the other chat live:

```sh
rtk proxy "$CBC113_BIN" revoke --root "$CBC113_ROOT" --protected-db "$CBC113_PROTECTED" --context cbc113-test --member test-chapter --expected-epoch ACTUAL_ORIGINAL_EPOCH
```

Require that binding's next check/send/discovery to fail and the driver's watch to quarantine it while the other binding remains usable. Repeat the same revoke and require stale-request rejection. Revocation preserves logical membership and unread history; it neither closes the native chat nor changes host permissions. Rebinding/re-enrollment recovery remains a separate native setup capability. Do not edit SQLite, mint replacement grants or put a stale credential into chat. A fresh native session requires fresh corroboration.

Keep per-host results as `PASS`, `FAIL`, `notLoaded`, `dormant` or `notRun`, with actual evidence. Record CPU/RSS for core, driver and required MCP shims during an idle interval and the marker sequence; count any extra processes. Missing supported project/channel activation, actual cross-app fetch/echo, reconnect evidence or native stale-binding test remains visible before #113 completion. Stop only the two owned terminals with Ctrl-C/SIGTERM; preserve the private case for review. Do not stop daily services or remove project trust globally.

For mod cleanup, revoke its disposable binding first so HTTP returns 401 and the receiver
stops retrying. Do not kill host-owned MCP shims: Codex can retain their closed transports
without respawning them. Use supported native lifecycle controls when a host reconnect is
needed. Keep the prior profile/context evidence when provisioning a fresh disposable pair.
