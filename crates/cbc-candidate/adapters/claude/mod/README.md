# Candidate Claude mod wake transport

This prototype uses Claude Code's public in-process mods API. It adds no process,
model polling task, approval hook, MCP server, native permission rule or tool.
The existing MCP adapter still handles discovery, connection, messages and receipts.
The approved channel path remains available separately. Actual Desktop and CLI results
are recorded in [EVIDENCE.md](../../../EVIDENCE.md). The automated tests below use a fake host.

## Public contracts

- [Mods overview](https://code.claude.com/docs/en/plugins/mods/overview): Desktop
  supports mods from engine 2.1.286, CLI from 2.1.287. Installed Desktop 2.1.317
  and CLI 2.1.312 satisfy those stated minimums. Hooks run inside Claude Code.
- [Mods API](https://code.claude.com/docs/en/plugins/mods/api): background work
  may call `$.prompt.submit({text, asUser:false})`. Claude attributes the text to
  the mod and waits until idle. A user interruption can drop or refuse a prompt.
- [Public API declarations](https://github.com/anthropics/claude-code/blob/main/mods/types/claude-code.d.ts):
  `$.http.fetch(url, {socketPath})` supports HTTP over a Unix socket. The published
  declarations identify themselves as 2.1.277; native-generated types take
  precedence if a newer build differs. There is no direct Node socket access.
- [Reference limits](https://code.claude.com/docs/en/plugins/mods/reference#limits):
  HTTP calls have a 30-second total timeout and read their complete response body.
  This cannot be an unbounded SSE reader.

## Wire and identity

Configure only the disposable native project's `.cbc-candidate-wake.json`:

```json
{"v":1,"socketPath":"/private/tmp/ACTUAL_CASE/core/device.sock"}
```

The mod reads this exact file through `$.session.cwd()` and `$.fs.read()` on
session start and accepted human rearm, only while transport is inactive. It
accepts at most 4096 UTF-8 bytes, exactly those keys, and an absolute Unix socket
path shorter than 104 bytes. There is no ambient or global fallback and no
credential field. Normal request renewals reuse the validated configuration.

One in-process request waits on the shared device socket:

```text
GET /cbc/v1/wake?after=<empty-or-lowercase-hex64> HTTP/1.1
```

The shared driver must corroborate the actual native host process from the
kernel peer PID/UID and host-owned PID/start-time/session record, then validate
the current binding/epoch before returning a hint. The request contains no
claimed identity or authentication token. The HTTP route is an addition to the
existing private socket, not a public TCP listener. Its implementation is tracked
separately from this mod.

New enrollment uses [the native target contract](../../../NATIVE_TARGETS.md). The driver
requires actual cwd/entrypoint observations and agreement with the core's target for the
current binding epoch. Audit labels cannot supply missing saved project/environment IDs.

| HTTP response | Mod behavior |
| --- | --- |
| 204, after at most 25 seconds parked | Renew transport after one second; no prompt |
| 200, exact `{"v":1,"type":"pending","cursor":"<hex64>"}` | Submit one fixed, attributed, content-free `cbc_check` hint for a changed cursor |
| 200, exact `{"v":1,"type":"recovery","cursor":"<hex64>"}` | Submit the same hint even if the cursor is unchanged: the driver is re-hinting overdue unread work |
| 401 or other unexpected status | Stop until accepted human input |
| 429, 503 or network failure | Retry transport with bounded 1-30 second backoff; no prompt |
| Invalid, body-bearing or over-256-character 200 payload | Stop; no prompt |

The driver computes the opaque progress cursor from binding epoch and first
unread item. The mod compares equality only. Older-room backlog may move the
underlying sequence down. A hint is neither a batch receipt nor proof of model
uptake. Prompt acceptance advances only the mod's wake cursor; ordinary adapter
checks and exact receipts advance durable core delivery.
Native-mode MCP hello and post-receipt wake also use non-consuming binding status.
They neither prefetch the next batch nor report the missing historical inbox socket as
a dead native chat. Connected channels keep their existing delivery behavior.

Main-turn interruption and `session.end` cancel scheduled callbacks and invalidate
late HTTP/prompt results. HTTP has no public abort argument in the inspected
contract, so an already-running request may remain until the API timeout, but its
result cannot start another prompt. A later accepted `composer` prompt may reread
the local configuration and rearm. Peer, plugin, channel, SDK and unclassified
messages cannot rearm. A dropped/rejected wake prompt leaves core work unread and
stops automatic retries until accepted human input.

## Disposable Desktop loading

No installation has been performed by creating these files. The native test
operator can copy this directory under a new private catalog at
`MARKET/plugins/cbc-candidate-wake`, with `MARKET/.claude-plugin/marketplace.json`:

```json
{"name":"cbc113-live-UNIQUE","owner":{"name":"CBC"},"plugins":[{"name":"cbc-candidate-wake","source":"./plugins/cbc-candidate-wake"}]}
```

Use a fresh catalog name. In the disposable project's working directory, the
[supported local-scope commands](https://code.claude.com/docs/en/plugins/cli-reference)
are:

```sh
rtk proxy /ABSOLUTE/claude plugin marketplace add /ABSOLUTE/MARKET --scope local
rtk proxy /ABSOLUTE/claude plugin install cbc-candidate-wake@cbc113-live-UNIQUE --scope local
```

These declare/enable the new plugin in that project's local settings. Native
plugin registry/cache records can also be created. Avoid the combined install
`--marketplace` shortcut: installed CLI help says it declares a new marketplace
in user settings. The plugin has no `userConfig`, so it needs no global
`pluginConfigs` value. Open a fresh disposable Desktop session, or apply the
documented plugin reload. Verify actual mod loading and native-generated API types.
Do not change installed daily CBC, global enablement or native permissions.

## Disposable CLI loading

Load this reviewed directory for one interactive session with `--plugin-dir /ABS/MOD`.
Use `--strict-mcp-config --mcp-config /ABS/PROJECT/.mcp.json` for the project's adapter
and `--tools ""` to disable built-ins while keeping MCP. No marketplace installation
or permission-mode change is needed for this session-scoped load. Supply a real terminal;
on macOS the native trial used `rtk proxy /usr/bin/script -q /dev/null /ABS/claude`
followed by those flags and `--ax-screen-reader` because RTK alone captured stdout.

CLI 2.1.312 passed idle fetch/echo, Escape Stop with over 85 seconds of pending unread work,
genuine composer rearm, driver reconnect and operator-initiated `--resume` after process
exit. Its sender was an operator fixture, separate from the native Desktop pair's Codex
sender. See [TESTING.md](../../../TESTING.md#claude-cli-mod-branch) for the bounded procedure.

## Verification and remaining native gate

```sh
rtk proxy node --test crates/cbc-candidate/adapters/claude/mod/test_mod.mjs
rtk proxy /ABSOLUTE/claude plugin validate crates/cbc-candidate/adapters/claude/mod --strict --json
```

The Node suite uses a fake mods host. It checks idle/no-model behavior, opaque
cursor changes, coalescing, one outstanding operation, backoff, revocation,
human-only rearm, late-result cancellation and strict local/wire configuration.
The native CLI validator reports the API calls it sees. Expected calls are only
`$.clock.after`, `$.fs.read`, `$.http.fetch`, `$.prompt.submit` and `$.session.cwd`.
It labels the `prompt.submit` observer as gating-capable because of the event;
this implementation always passes it through unchanged and never returns a
permission decision. Directory publishing advice about an icon is separate from
the successful native manifest/hooks validation.

The public docs do not promise that an outstanding fetch or timer prevents
Desktop from retiring an idle native subprocess. Before claiming automatic
Desktop delivery, leave the genuine session idle beyond the observed two-minute
retirement interval and send a real CBC marker. Capture PID lifetime, fetch/echo,
receipt, resource usage and absence of extra processes/empty model turns. Also
test active-turn delivery, Stop cancellation, fresh binding, driver reconnect and
closed-chat recovery. The recorded Desktop trial passed automatic delivery after idle;
its later follow-up passed Stop/rearm, 697.5 seconds idle and recovery after a single-engine
exit plus genuine composer restart. Full app quit and autonomous dormant restart remain
unproven. CLI operator-initiated resume passed and
does not establish autonomous Desktop restart. If Desktop retires the process, report
that result; this prototype has no private restart or keepalive override.

`asyncRewake` is a separate documented command-hook path that wakes on exit code
2, but it creates a supervised subprocess per hook invocation and enforces its
timeout. Neither that feature's docs nor the mods docs establish dormant Desktop
restart. The existing MCP stdio process cannot supply a hook's exit status while
continuing to serve MCP; adopting it as a command hook would require a different
host process arrangement, not reuse of an already-running process.
