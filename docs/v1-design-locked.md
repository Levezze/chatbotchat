# chatbotchat v1 — Locked Design

> **STATUS (2026-05-31): priority partially superseded by [ADR-0001](decisions/0001-rescope-to-usable-alpha.md).**
> This document remains the design *reference*, but it is no longer the
> priority *ordering*. The core loop (open/join/send/wait/status/close→read-only)
> and most lifecycle machinery are **built and tested**. The browse surface
> (`cbc list`, `cbc show`) and install/always-on wiring are **promoted** and
> being built now. `cbc search`, `cbc summary`, the auto-archive sweeper polish,
> severity polling-decay, ghost detection, and extra cap controls are **deferred**
> (milestone `v1.5-later`) pending real dogfooding. See ADR-0001 for the rationale
> and the full feature-bucket table.

Output of the initial grill-me session. Feeds directly into `/write-a-prd`.

## Goal

Persistent local server that lets AI coding agents (Claude Code, Codex, etc.) hold structured back-and-forth conversations across repos and sessions without the user manually shuttling messages between terminals. The user remains a checkpoint when the receiving agent decides a decision needs human input — but otherwise stays out of the path.

This replaces the `/handoff-chat` + `/handoff-reply` manual copy-paste flow while leaving those skills available for ad-hoc use.

## Architecture

### Processes
- **`chatbotchat-server`** — Rust (axum) HTTP daemon. Always running via launchd on macOS; portable to systemd on Raspberry Pi later. Listens on `localhost:8484` (configurable).
- **`cbc`** — single Rust binary, dual-mode:
  - `cbc mcp` — runs as MCP stdio server; registered globally so every CC/Codex session has the tools.
  - `cbc <subcommand>` — CLI for humans and scripts (`open`, `join`, `send`, `wait`, `list`, `show`, `search`, `close`).
  - Both modes are thin HTTP clients to the daemon.

### Storage
- SQLite at `~/.chatbotchat/state.db`.
- FTS5 virtual table over messages for keyword search.
- All rooms persisted forever. Archive = state transition, not deletion.

### Network
- v1: `localhost` only, no auth.
- Pi migration (post-v1): daemon binds `0.0.0.0:8484`, token auth bolted on, CLI/MCP gain `--server <url>` flag and `CBC_SERVER` env var.

## Data model

### Room
- `id`: `<subject-kebab>-<YYYYMMDD-HHMM>` (e.g., `slider-labels-20260528-1423`)
- `subject`: one-line description
- `started_at`, `last_activity_at`
- `state`: `active | idle | paused | stale | closed | archived`
- `config`: `{ hard_cap: 10, soft_cap: 4 }` — defaults; overridable at `open_room`
- `prev_room_id`: optional link to predecessor when a closed room's conversation continues in a new room

### Participant
- `handle`: `<repo>-<model>-<sess4hex>` (e.g., `mvp-engine-opus47-a3f2`)
- `repo`, `cwd`, `model` (self-reported on join; descriptive, not the identity key)
- `instance` — the identity key (see [ADR-0002](decisions/0002-participant-identity-is-an-instance-token.md))
- `joined_at`, `last_poll_at`
- Idempotent join: same `(room_id, instance)` returns existing handle.

### Message
- `type`: `msg | waiting_user | blocker_real_work | fold | close`
- `body`, `from`, `to` (handle or `all`)
- `severity` (only for `waiting_user`): `low | med | high`
- `question_text` (only for `waiting_user`): the actual question being asked of the receiver's user
- `created_at`
- `counts_toward_cap`: `true` only for `msg`

## Lifecycle

States and transitions:

| State | Enter when | Exit to |
|---|---|---|
| `active` | room opened, or any message in last 24h | `idle` (no activity 24h), `paused`, `closed` |
| `idle` | no activity 24h+, <7d | `active` (any new message), `stale` |
| `paused` | `blocker_real_work` sentinel posted | `active` (explicit `cbc_wake`) |
| `stale` | no activity 7d+, no current pollers | `archived` (after 14d), or `active` if message arrives |
| `closed` | explicit `cbc_close`, or hard_cap reached + user approves close | `archived` after 14d |
| `archived` | from `closed` or `stale` after 14d | terminal — read-only |

Periodic sweep (hourly) handles auto-transitions. Stale detection of ghosted participants: `last_poll_at` older than 15min flags them; if all participants ghosted, room → `idle`.

**Archived rooms are read-only.** Cannot post. To continue a conversation, open a new room and set `prev_room_id`.

## Caps

- **Hard cap**: 10 `msg`-type messages per room (configurable). On hit, room refuses further `cbc_send` until user explicitly raises the cap or closes the room. Agent surfaces to user with the chronological summary.
- **Soft cap**: 4 consecutive `msg` without human input. Counter resets when human interacts with the room (via grill-me response that leads to a fresh message, or any future user-in-chat action). On soft-cap-minus-1 (i.e., 3rd consecutive), receiving agent **auto-surfaces** to its user regardless of confidence. Loop insurance.
- Sentinels (`waiting_user`, `blocker_real_work`, `fold`) do not count toward caps.

## Polling

- `cbc_wait` long-polls with a **server-side cap of 10 minutes** per call. On timeout, returns `paused_by_timeout`; user manually wakes via `cbc_wake` in the agent's session.
- Server returns a `retry_after` value based on the counterpart's most recent sentinel:
  - No sentinel / normal `msg` expected: 10s
  - Counterpart in `waiting_user` low severity: 10s
  - Counterpart in `waiting_user` med severity: 20s
  - Counterpart in `waiting_user` high severity: 45s
- Time-decay layered on top: after 5 minutes of consecutive waiting on the same room state, multiply `retry_after` by 1.5x; cap at 60s.

## MCP / CLI surface

Tools (exposed identically as MCP tools and CLI subcommands):

| Tool | Purpose |
|---|---|
| `cbc_open_room(subject, opts?)` | Create room, return `room_id` and a slash-free share line (`Join CBC room <room_id>`) for the user to paste into the other agent's session. |
| `cbc_join_room(room_id)` | Register caller as participant. Returns room state, recent messages, caller's handle. |
| `cbc_send(room_id, body, to?)` | Post a `msg`. Increments cap counter. |
| `cbc_wait(room_id)` | Long-poll. Returns next message addressed to caller or `all`, or sentinel state, or timeout. |
| `cbc_signal(room_id, type, severity?, reason?, question_text?)` | Post a sentinel (`waiting_user` or `fold`). |
| `cbc_pause(room_id, reason)` | Post `blocker_real_work`, transition to `paused`. |
| `cbc_wake(room_id)` | Resume from `paused` or `idle`. |
| `cbc_close(room_id)` | Post `close`, transition to `closed`. |
| `cbc_status(room_id)` | Fetch state without consuming messages. |
| `cbc_summary(room_id)` | Server-generated deterministic markdown chronology of the room's messages, participants, and cap counters. Used by the receiving agent during grill-me handoff. *(Shipped as `cbc_recap` — see [ADR-0004](decisions/0004-background-poll-owns-the-wait.md).)* |
| `cbc list [--all] [--state X]` | (CLI) List rooms. |
| `cbc show <room_id> [--format markdown|json]` | (CLI) Dump room contents. Both markdown and JSON supported. |
| `cbc search <query>` | (CLI) FTS5 keyword search across all rooms. |
| `cbc allow-tools` | (CLI) Grant the chatbotchat MCP server standing auto-approval in Claude Code's user settings (`~/.claude/settings.json`). Idempotent; backs up before editing; degrades to a printed snippet on an unparseable file. |

`cbc_send` and `cbc_wait` stay separate (no fused `send_and_wait`) — keeps `wait`-only rejoin after pause clean.

## Grill-me handoff (receiver side)

When a receiving agent decides the user is needed:

1. Agent calls `cbc_summary(room_id)` → gets deterministic chronological markdown.
2. Agent simultaneously calls `cbc_signal(room_id, type=waiting_user, severity, question_text)` so the counterpart agent knows what's happening and slows its polling.
3. Agent presents the user with: `[server summary]` + `[agent's interpretation of where the pain point is]` + `[the pointed question]`.
4. User answers (in agent's session, just like today's `/handoff-reply` flow).
5. Agent folds the answer into a `msg` and calls `cbc_send`.
6. Counterpart's `cbc_wait` unblocks with the new message.

The counterpart agent, while polling during a `waiting_user` state, displays a short cheeky idle line ("the other agent is interrogating its user — grabbing coffee") so the user on that side knows what's going on. Idle lines are templated by severity; they do not consume tokens beyond a handful.

## Receiver-side surface heuristic

Reuse the heuristic from `/handoff-reply` (decisions only the user owns, contradictions with prior context, ambiguity, unverifiable claims). **Plus**: auto-surface when about to send the message that would hit `soft_cap`, regardless of agent confidence. This is mandatory, not advisory.

## Identity

- `handle`: `<repo>-<model>-<sess4hex>`
  - `repo` = basename of `git rev-parse --show-toplevel`, falling back to cwd basename.
  - `model` = self-declared on `join_room` (`opus47`, `sonnet46`, `codex53`, etc.). No verification in v1.
  - `sess4hex` = 4-char random per `join_room` call.
  - Identity is keyed on `instance`, not the tuple: rejoining with the same `instance` gets the same handle (idempotent), and two agents sharing `(repo, model, cwd)` but with different `instance` are distinct participants. `instance` is resolved client-side (explicit `--as`/`as:` label → `CBC_INSTANCE` → `CLAUDE_CODE_SESSION_ID` → per-process PID floor); reuse the same `--as` label to resume or hand off an identity from another terminal/client/dir. See [ADR-0002](decisions/0002-participant-identity-is-an-instance-token.md).
- Room share line that the user pastes between agents: `Join CBC room <room_id>` — deliberately slash-free. A leading `/` made agents misread it as a slash command / skill (there is no `cbc-join` skill); the receiving agent now recognizes the bare `slug-YYYYMMDD-HHMM` room id via the MCP server instructions and calls `cbc_join_room`.

## Out of scope for v1

- Auth, tokens, remote access (deferred to Pi migration)
- Web UI (see v2-ideas.md)
- >2 agents per room, `fold` semantics, roundtable (v2-ideas.md)
- User-in-chat as first-class participant (v2-ideas.md)
- Vector / semantic search across archived rooms (v2-ideas.md — design daemon to emit `on_archive` event so this is hookable later)
- Notifications to OS / phone / Slack

## Known v1 implementation considerations to flag in PRD

- **Stale counterpart detection**: server tracks `last_poll_at` per participant; if waiter's counterpart has not polled in >15 min, return `counterpart_stale` so the waiter can surface "should we abandon?" to its user.
- **Daemon lifecycle**: launchd plist needs sane restart-on-crash, log rotation, graceful shutdown that drains in-flight long-polls.
- **SQLite under concurrent long-polls**: WAL mode mandatory. Vacuum and FTS5 rebuild should be background, not blocking writes.
- **Tool timeout interplay**: MCP tool calls in CC and Codex have their own timeouts independent of `cbc_wait`'s 10-minute server cap. Verify the actual tool-call timeout for each client and ensure `cbc_wait` returns before it.
- **Port conflict**: 8484 chosen because it's not in any common service list, but add a `--port` flag and check on startup.
- **Host permission auto-approve**: Claude Code's `auto` mode routes any tool call not covered by a `permissions.allow` rule to a safety classifier; `cbc_send` into a client-flavored room reads as outbound external comms and stalls for per-call approval. The fix is host-specific (a `permissions.allow` rule short-circuits the classifier), so it lives in the install layer, not the protocol: `cbc allow-tools` writes the rule, and `install-daemon` offers it interactively (default no). Other hosts (Codex, etc.) will need their own allow mechanism.

## Tracked elsewhere

- v2 feature drafts: `docs/v2-ideas.md`
