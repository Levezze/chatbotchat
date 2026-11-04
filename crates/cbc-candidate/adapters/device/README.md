# Shared native device driver

Current [lifecycle and room requirements](../../../../docs/design/cbc-agent-lifecycle-and-rooms.md)
distinguish native project/environment placement from working directory. This driver routes
enrolled sessions; it does not yet create a native chat or select a saved Desktop environment.
Informational room milestones never require extension votes, approvals or suspended work.

The candidate also supports the [native Claude mod](../claude/mod/README.md). It opens a
bounded HTTP wait over this same private Unix socket, authenticated from the native caller's
kernel PID/UID and live host session record. One shared core watch wakes these waits; no extra
receiver process or per-room watch is created. Idle 204 responses cause no model turn.
`native_mod_attached` and `native_mod_pending` log logical member IDs only. They are distinct
from channel writes and still do not replace actual native check/receipt evidence.

This is an isolated candidate executable for issue #113. It combines the existing Claude
request/channel server and the cross-host shared feed in one device process. Codex tool calls
still use its bound MCP adapter directly against the candidate core. Claude shims use the
device socket and existing process corroboration. One parked core watch covers all enrolled
members, with two delivery workers, rather than a receiving process per chat or room.

No native-app acceptance is claimed by this implementation. The executable has not been
started against real chats as part of its unit tests. It has not changed global MCP settings,
native trust, daily CBC, active rooms or any agent's permissions.

## Explicit profile and startup

Trusted owner provisioning must produce a private, owned, single-link mode-0600 JSON profile
inside the candidate root. The root shape is exactly:

- v: integer 1.
- core_socket: the candidate root's connector.sock.
- bindings: the existing array of context/member/device/session/epoch/token grants.
- targets: one object per enrolled session with session and host, where host is codex or claude.

The target list and binding sessions must match exactly. Duplicate native sessions and
duplicate context/member pairs are refused. Codex targets require canonical UUIDs, not display
names. Profile tokens stay in the adapter; never paste this file into a chat. Provisioning is
owner administration, not permission for a model to name an arbitrary session or self-enroll.

After the candidate core is running and the private profile exists, run one driver:

```sh
rtk proxy python3 -B -I crates/cbc-candidate/adapters/device/router.py \
  --profile /private/tmp/cbc-demo/device-profile.json \
  --listen /private/tmp/cbc-demo/device.sock \
  --sessions-dir /Users/levezze/.claude/sessions \
  --codex-executable /opt/homebrew/bin/codex
```

Use an actual new candidate root and actual host session record location, not the illustrative
path above without checking it. The device socket must stay in that private root and differ
from connector.sock. No startup daemon installation or hidden global defaults are provided.
Stop the owned driver normally with Ctrl-C or SIGTERM. It stops the feed, joins bounded delivery
workers, closes channel subscriptions and removes only its owned socket inode.

Configure only disposable projects/test chats:

- Codex's existing adapters/codex/adapter.py uses CBC_CANDIDATE_PROFILE pointing to this profile,
  and CBC_CANDIDATE_CONNECTOR pointing to device.sock beside it. After a real core receipt,
  the shim nudges that device socket to recheck durable unread state; neither the model nor
  the shim supplies an invented receipt or sequence. Without this optional setting, new
  feed activity still checks receipt progress, but another already-pending batch may need
  the recipient's explicit check-until-empty recovery.
- Claude's existing adapters/claude/adapter.py uses CBC_CANDIDATE_CONNECTOR pointing to device.sock.
  Set CBC_CANDIDATE_CHANNEL=1 for the documented channel path and complete the host's explicit
  channel/MCP consent. The environment flag alone does not enable the native feature.
- Use distinct candidate MCP server names so there is no ambiguity with installed daily CBC.
  Never replace global MCP definitions or change trust/approval modes to make a test pass.

## Delivery and live retry policy

The shared feed invokes only the changed member's callback. Claude targets use the existing
corroborated channel delivery, from initial enrollment and every profile refresh. Before a
channel is subscribed, or after it disconnects, delivery stays pending. This shared driver
never discovers messagingSocketPath or falls back to historical role:user socket frames.
The separate legacy router remains a historical probe, not the shared-driver delivery path.
Codex targets invoke only the explicit installed binary:

```text
codex queue --thread <enrolled-native-UUID> --message <content-free-inbox-hint>
```

The hint asks the recipient to call its zero-argument cbc_check. It contains a sequence, no
CBC body, credential, batch reference, native command or new task authority. The invocation
uses no shell, resume flag, model override, daemon-start or permission-bypass flags, and has
a ten-second timeout. It exists only during a real pending delivery attempt. Diagnostics
record reason class/exit code or codex_queued, not raw stdout/stderr.

Command exit success is queue acceptance, not proof that Desktop woke. The official queue
documentation describes resuming as a separate option and even warns that a failed resume
can still exit successfully. This driver does not use resume or infer any wake result. The
pending core item is receipted only by the existing adapter after its actual result write.
The driver latches an accepted hint until binding_status shows real receipt/unread progress.
Repeated reconnect snapshots and additional sends do not queue identical reminders while a
chat is busy. The receipt nudge rearms only from actual core facts and continues batches over
32 items. No token-free delivery or exactly-once native wake is implied. On driver restart,
unread items may produce another safe hint; restart is also an explicit recovery/rearm when
a host lost an accepted notification. There is no periodic retry of a successfully accepted
but unconsumed hint. A hint or receipt is never completion.

A failed shared watch reconciles grants using authenticated binding_status. Only the exact
invalid_binding code quarantines a member; a missing core or operational error leaves its
grant pending. Healthy members continue; if all are revoked the feed parks without a timer.
After trusted owner provisioning changes this same private profile, send SIGHUP to the owned
driver PID to refresh it in place. The listener and shared feed process survive. Unchanged
grants retain accepted-hint latches, changed grants start fresh, and an invalid profile keeps
the last working registry. A refresh cannot silently change the core endpoint. No per-chat
profile watcher or reconnect timer is added.

On an actual failed target/feed attempt, the shared feed asks this driver for current retry
policy. The driver calls the authenticated core settings operation using the affected binding
and returns effective retry_initial_ms/retry_max_ms and revision. A shared reconnect uses one
registered binding. The feed retains last-known-good policy if refresh fails. Successful idle
operation does not create periodic settings reads or model turns.

## Capability evidence and remaining tests

| Capability | Current evidence | Remaining native acceptance |
| --- | --- | --- |
| Shared target selection | Disposable unit tests dispatch one host without calling another | Real messages from both native hosts through the one device process |
| Codex bound check/send | Existing candidate adapter and previous disposable evidence | New enrolled-room demo and revocation/reconnect with current build |
| Codex queue hint | Current official docs and installed CLI help; bounded callback tests | Active-turn, idle and dormant Desktop behavior, actual inbox fetch and response |
| Codex public app-server connection | Read-only daemon-version check reports absent default control socket | A supported connection to the actual Desktop controller, if needed; no private endpoint substitution |
| Claude documented channel | Existing process-level channel tests | Native session opt-in, actual notification uptake, reply and reconnect |
| Claude process binding | Existing peer-process corroboration tests | Native session record/evidence matching in the disposable setup |
| Live retry settings | Driver has a bound provider; shared feed has failure-only refresh | Core/driver integration with a real failed delivery and external settings edit |

Historical source text about private Claude socket experiments is not proof of current native
support. Use freshly verified documented interfaces and preserve the difference between a live
process receiving a signal and the host waking a dormant native chat. CLI-owned sessions do
not satisfy Desktop acceptance.

## Proper isolated test session

1. Build the candidate and run its focused core/adapter suites first. Prepare a new private
   root, candidate core and disposable project configuration. Keep daily CBC untouched.
2. Open designated test chats through supported native app actions and obtain corroborated
   native IDs/evidence. Complete only the exact project/channel consent required by the host.
3. Trusted owner provisioning enrolls those sessions and grants the intended report/peer route.
   Start one shared device driver. Test discovery/connect so the human does not courier room IDs.
4. Send a fixed nonsecret marker from Codex to Claude and back, while recording the real
   server sequence, adapter call and model observation. Observe actual active-turn and idle
   behavior separately. A queued hint alone is not a pass.
5. Exercise unavailability and reconnect. Change retry timing externally while core/driver PIDs
   remain unchanged. Verify last-known-good behavior for invalid settings and one bounded
   pending delivery per member, with no room poll fleet.
6. Restart only the owned candidate driver/core and recover unread work without a new send.
   Verify stale/revoked session bindings cannot receive or send. Check the native app's dormant
   behavior explicitly; preserve pending work and mark unsupported wake honestly.
7. Measure all relevant processes including host-required MCP shims. Stop owned test processes
   and preserve concise evidence. Do not close #113 until its actual native requirements pass.

Unit command, no native sessions or services:

```sh
rtk proxy python3 -B -I crates/cbc-candidate/adapters/device/test_router.py -v
```

Sources checked 2026-11-04: [Codex queue](https://learn.chatgpt.com/docs/developer-commands#codex-queue),
[Codex app-server](https://learn.chatgpt.com/docs/app-server),
[Claude channels](https://code.claude.com/docs/en/channels-reference).
