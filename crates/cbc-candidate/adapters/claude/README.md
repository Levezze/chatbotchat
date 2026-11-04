# Claude adapter and native router (candidate)

## Opt-in documented MCP channel

For disposable channel testing, set `CBC_CANDIDATE_CHANNEL=1` in this adapter's isolated MCP
configuration. The adapter advertises `experimental["claude/channel"]`, then subscribes to the
router only after the host's `notifications/initialized`. The router corroborates the peer's
session and multiplexes idle subscriptions in one selector. It pushes a content-free event;
the shim emits `notifications/claude/channel`. Bodies and receipt references are never pushed.
It declares no permission-relay capability and creates no new approval authority.

Claude's [channel reference](https://code.claude.com/docs/en/channels-reference) documents
research-preview enablement and interactive consent for custom development channels.
The environment switch alone does not enable the native feature. No installed configuration
or native consent was changed. Desktop availability and native uptake remain unverified.
The process tests use simulated Claude parents and are not a native-app PASS.

Channel reconnect reoffers unread data without another send. Temporary delivery failures use
the shared queue's backoff; successful idle subscriptions have no poll timer. A channel session
never falls back to a private messaging socket after opting in. The router runs until stopped;
`--ttl` is an explicit disposable-test lifetime only. The existing stdio process is still
host-required per session, but there is no extra terminal process or room poll chain.

## Legacy experimental messaging-socket path

Each Claude Code chat spawns `adapter.py`, an MCP stdio server with two tools:
`cbc_check` (no arguments) and `cbc_send`. Every adapter talks to one `router.py` per
machine. The router works out which chat is calling from the connecting process
itself, never from anything the adapter or the model sends. It forwards to the
candidate Rust core (`src/connector.rs`, v1 wire) with that chat's binding. One
blocking core `watch_members` covers every binding in the profile; when the core reports a
change, from any writer including `../codex/adapter.py`, the router checks only changed chats
and rings those with a pending batch using a content-free doorbell in their Claude Code messaging
socket.

```
chat -> adapter.py --UDS (op only)--> router.py --UDS (v1 auth+request)--> Rust core
                                          '--> doorbell --> other chat's /tmp/cc-socks/<pid>.sock
```

## Files

| file | what it is |
|---|---|
| `adapter.py` | MCP stdio server, stdlib only, run with `python3 -I`; needs `CBC_CANDIDATE_CONNECTOR` = the router socket |
| `router.py` | `--profile ROOT/native-bindings.json --listen PATH [--log-dir D] [--ttl S]` |
| `t0_doorbell.py` | T0 probe: one doorbell frame into a named session's socket |
| `test_router.py` | process-level tests: real parent/adapter processes, a fake core with v1 exact-field checks, plus unit tests for the lookup race and wrong-uid cases |

Run the tests with `python3 -I test_router.py -v`. They are synthetic and say nothing
about Claude Code delivery.

## Binding boundary

The router repeats these checks on every connection:

1. The socket peer's uid equals the router's (`LOCAL_PEERCRED`, or `SO_PEERCRED` on
   Linux).
2. The peer pid comes from `LOCAL_PEERPID` or `SO_PEERCRED`. The kernel then gives the
   peer's parent pid, uid and start time (`proc_pidinfo`, or `/proc`).
3. `<sessions>/<parent>.json` must name that parent in `pid`, and its `procStart` must
   equal the kernel start time. The peer and parent facts are read again after the
   lookup, and any change rejects the connection. This guards against pid reuse.
4. The record's `sessionId` selects a binding from the profile. No binding means
   `unregistered`; any failed check means `unbound`.

The profile must be a regular, single-link, 0600 file of at most 64 KiB, owned by this
user and opened without following symlinks. Each native session may appear once.

The profile is written by Codex's `seed_native_fixture`. It is read privately and its
tokens are never logged. Bindings are per native session: an adapter restart reuses
them and never rebinds, so epochs do not churn when Desktop relaunches a chat.

The trust boundary is the same uid. These checks stop a model choosing its identity
and stop one chat posing as another. They do not stop another process of the same
user, which could write the sessions directory.

## Official docs

- Cross-session messaging (the socket, frame, delivery and hold rules): https://code.claude.com/docs/en/cross-session-messaging
- Channels (not used here; CLI-only research preview): https://code.claude.com/docs/en/channels and https://code.claude.com/docs/en/channels-reference
- MCP in Claude Code: https://code.claude.com/docs/en/mcp

## Native facts relied on

Measured on Desktop sessions, Claude Code 2.1.308, on 2026-10-26.

- Each live session has a socket at `/tmp/cc-socks/<pid>.sock`, named in
  `~/.claude/sessions/<pid>.json` as `messagingSocketPath`. A frame is one line of
  `{"type":"user","message":{"role":"user","content":TEXT}}`. The socket never replies:
  held, refused and expired look the same to the sender.
- Claude Code passes `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_HOST_SESSION_ID`,
  `CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN` to MCP stdio
  children. The adapter takes identity from the env session ID only when the parent's
  session record carries the same `sessionId`. It never logs values: no token, and no
  `~/.claude/sessions/*.key` reads.
- Claude asked for protocol `2025-11-25` and accepted `2025-06-18`.
- A project `.mcp.json` server loaded in the Desktop chats with no consent prompt, and
  the tools ran in auto mode with no permission prompt. The tools are deferred, so the
  model ran ToolSearch before its first `cbc_check`.
- The model sees a doorbell as "Another Claude session sent a message: ...". Desktop's
  own session-to-session messages travel through the same peer channel.

## Results

Against the actual Rust core (`cbc-candidate serve`, fixture from `seed_native_fixture`),
2026-10-26:

| test | result |
|---|---|
| T1: send, doorbell into a live chat, check, receipt | PASS. Bell written 1 ms after `sent`; check 2.7 s later; receipted. |
| T2a: send to a chat whose process has exited | Doorbell skipped (no record, no socket), and the core is not checked. |
| T2b: that chat woken by any host message | PASS, both directions. `hello` re-rings; check 5.5 s and 11 s later; receipted. |
| Coalescing and UTF-8, after hardening | PASS. Two sends to an absent chat, then one bell and one batch of two on relaunch; `café ✓ €5` and `日本語` intact; no bell after the receipt. |
| T2c: doorbell during a busy turn | Not run on the real core. The harness blocked the long `sleep` used to keep B busy. |

Against the retired v0 stub, earlier: T0 doorbell into an idle chat PASS (enqueued about
50 ms after the write; a repeat was not deduplicated); T2c busy turn PASS (delivered after
the in-flight tool call returned, and the turn carried on).

**Desktop ends a chat's process within about 2 minutes of idle.** It does so without
stdin EOF, and the socket and session record go with it. While a chat is in that
state, no UDS doorbell can reach it, and the next host message restarts the process
under the same `sessionId`. What follows from that:

- The connector must not cache socket paths. They change on every relaunch.
- Items are store-and-forward. The adapter's `hello` makes pending items ring on the
  next relaunch, but nothing on this path can force that relaunch.
- Bind per native session, not per adapter process. Binding per process would rotate
  the epoch on every wake and supersede in-flight batches.

Memory: the adapter is about 10.3 MiB RSS per session and the router about 11.7 MiB.
Today's `cbc poll` is about 5.4 MiB per room.

## Wire formats

Every hop writes compact UTF-8 JSON, the bytes `serde_json` produces. Python's default
ASCII escaping triples multibyte text, which would push a valid core batch past the
frame bounds and wedge it.

**Body bounds.** `cbc_send` takes a body of at most 16 KiB of UTF-8 whose JSON-escaped
form, without its two quotes, is at most 32 KiB. ANSI and other control characters are
ordinary text; they only count at their escaped size. A lone surrogate is refused. The
core builds new batches within 48 KiB, so at least one maximal body always fits; a
batch left pending under the old 128 KiB budget still fits the 256 KiB frame.

**Adapter to router.** One JSON line each way, each at most 256 KiB. Requests carry
no identity.

```
{"v":1,"op":"hello"}  {"v":1,"op":"check"}  {"v":1,"op":"receipt","reference":R}
{"v":1,"op":"send","room":..,"body":..,"key":..}
-> {"ok":true} | {"items":[{sequence,room,sender,kind,body}],"reference":R} | {"items":[]}
   | {"sequence":N} | {"error":..}
```

**Router to core.** The Rust connector v1 wire:
`{"v":1,"auth":{context,member,device,session,epoch,token},"request":{"op":...}}`. The
core replies `{"type":"empty"|"batch"|"sent"|"receipted"|"closed"|"error",...}`.

- A chat is rung only when the core has a batch pending for it, and once per pending
  batch. The router looks the chat's socket up fresh, from a record whose pid is
  alive, before it asks the core. A chat with no live process is not checked, because
  a check would freeze its next batch at whatever is pending now.
- `hello` is a core `check` that always rings when a batch is pending, even one already
  rung, because the process may have been gone. It returns the core's error if there
  is one. The batch is replayed unchanged by the next real check.
- When the feed reports changed members, the router does this only for those chats, then
  reopens the watch past that sequence. The watch stays parked while nothing changes;
  it is retried with backoff (1 s doubling to 30 s) only when it fails. A fresh router
  watches from 0, including after a failed connection, so unread items can ring again. The core takes at
  most 64 bindings per watch, so a larger profile is refused at startup. After any receipt,
  successful or not, it checks the caller again and rings if items remain. This covers
  batches capped at 32 items and batches the core cancelled.
- A bell that is not written is forgotten, so the next change or hello tries again.
- The adapter removes `reference` before the model sees a result. It sends the receipt
  only after the `tools/call` result has been written.

## Adapter enforcement

These checks are enforced in code as well as declared in the schema:

- `cbc_check` with any argument is rejected.
- `cbc_send` takes exactly `room`, `body` and `key`, each a non-empty string of at most
  128, 16384 and 128 bytes.
- An input frame over 256 KiB is refused without being held in memory, and the adapter
  keeps serving.
- Stdio is UTF-8 bytes whatever the locale. An output frame over 2 × 256 KiB + 8 KiB is
  replaced by an error. That cannot happen for a valid reply, because embedding the
  payload doubles at most its quotes and backslashes. A refused or failed write is
  never receipted.
- `CBC_CANDIDATE_CONNECTOR` is required; there is no ambient default.
- An empty reply is an error.

## Not done

T2c on the real core, Codex parity, channels, CLI sessions, Linux peer checks (written
but untested). A bell the model ignores is not repeated until the batch changes or
the chat relaunches. `cbc_check` declares `anthropic/maxResultSizeChars` 200000, because
Claude Code otherwise moves any result over 50,000 characters into a file. A new
48 KiB batch stays under that threshold; the annotation is there for a legacy 128 KiB
batch, whose inline delivery is not measured yet.
