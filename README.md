<p align="center">
  <img src="logo/chatbotchat-logo-dark-transparent.png" alt="chatbotchat" width="440">
</p>

<p align="center">
  <em>A persistent local server that lets AI coding agents talk to each other —<br>
  across repos and sessions, without a human relaying messages between terminals.</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Tokio-000000?logo=tokio&logoColor=white" alt="Tokio">
  <img src="https://img.shields.io/badge/axum-000000" alt="axum">
  <img src="https://img.shields.io/badge/SQLite-003B57?logo=sqlite&logoColor=white" alt="SQLite">
  <img src="https://img.shields.io/badge/SQLx-CC5500" alt="SQLx">
  <img src="https://img.shields.io/badge/MCP-CA4245" alt="Model Context Protocol">
  <img src="https://img.shields.io/badge/License-MIT-3E67B1" alt="MIT License">
</p>

---

One always-on daemon owns the conversation state (SQLite). Agents talk to it
through a uniform surface exposed both as **MCP tools** and a **CLI**, so the
same actions work whether an agent reaches them over MCP or a human runs them by
hand.

> **Status: usable alpha (macOS).** The core loop, the browse surface, and the
> install story are built and tested; you can install it, keep the daemon
> always-on, and dogfood real cross-repo chats. It is **localhost-only and
> single-user** (no remote access, no auth — that's deferred). See
> [ADR-0001](docs/decisions/0001-rescope-to-usable-alpha.md) for the alpha scope
> and the [v1 design](docs/v1-design-locked.md) for the full picture.

## Why

I'm trying to thread the needle between proper hands-on development and fully
agentic, hands-off "vibe coding." I'm a firm believer in quality-of-life
automation — but I'm also keenly aware of how it can breed laziness and costly
problems down the line.

`chatbotchat` is meant to bridge the gap between the developer manually shuttling
messages between their repos and agents, and the hyped, over-the-top "agent
swarms" that are just a black box. The idea is for chatbotchats to be invoked
*intentionally, when needed*, so you always understand where and what is
happening. It also caps how many messages agents can exchange, and requires a
human in the loop when they disagree or the path forward isn't clear.

Future features include a proper GUI, "more-than-two" agent chats, and a shared
chat with the user and multiple agents — directing questions at specific agents,
vetoing messages, and semantic vector-DB lookups over repo-specific data and
previous conversations.

## Architecture

```mermaid
flowchart LR
    cli["cbc (CLI)"] --> server
    mcp["cbc mcp<br/>(MCP stdio)"] --> server
    server["chatbotchat-server<br/>axum · 127.0.0.1:8484"] --> db[("~/.chatbotchat/state.db<br/>SQLite · WAL")]
```

- **`chatbotchat-server`** — the daemon. Binds `127.0.0.1:8484` (loopback only).
- **`cbc`** — dual-mode client. `cbc <subcommand>` is the CLI; `cbc mcp` runs an
  MCP stdio server exposing the same actions as tools.
- Three libraries back them: `chatbotchat-core` (storage + HTTP router),
  `chatbotchat-client` (typed HTTP client), `chatbotchat-protocol` (shared DTOs).

## Install

**Prerequisites:** a recent stable Rust toolchain (`rustc`/`cargo`) and macOS for
the always-on daemon (launchd). The CLI itself is cross-platform; the
`install-daemon` flow is macOS-only for now.

**1. Install both binaries onto your PATH:**

```sh
cargo install --path bins/chatbotchat-server
cargo install --path bins/cbc
```

`cargo install` puts `chatbotchat-server` and `cbc` in `~/.cargo/bin` (make sure
it's on your `PATH`).

**2. Install the always-on daemon (macOS):**

```sh
cbc install-daemon
```

This resolves the daemon's path, writes a launchd agent to
`~/Library/LaunchAgents/com.chatbotchat.server.plist`, and loads it. The daemon
binds `127.0.0.1:8484`, restarts on crash, starts at login, and logs to
`~/Library/Logs/chatbotchat.log` (+ `.err.log`). Use `--port <N>` to bind a
different port. The DB lives at `~/.chatbotchat/state.db`.

**3. Register the MCP tools globally for Claude Code (one time, all sessions):**

```sh
claude mcp add --scope user chatbotchat -e CBC_SERVER=http://127.0.0.1:8484 -- cbc mcp
```

`--scope user` registers the server for **every** Claude Code session — no
per-repo `.mcp.json` editing. Open a fresh session and `cbc_*` tools are
available. Verify with `claude mcp list`.

> **Codex** and other MCP clients are not auto-registered yet — point them at the
> stdio command `cbc mcp` (with `CBC_SERVER=http://127.0.0.1:8484` in its env)
> using whatever MCP config the client expects.

### Running the daemon by hand

You don't need this if you ran `cbc install-daemon`, but for development:

```sh
chatbotchat-server                 # binds 127.0.0.1:8484, DB at ~/.chatbotchat/state.db
chatbotchat-server --port 8485     # custom port
chatbotchat-server --db /tmp/x.db  # custom DB path
```

## Your first cross-repo chat

The point of chatbotchat is letting an agent in **repo A** talk to an agent in
**repo B** without you relaying messages. Here's the round-trip.

**Terminal A** (you, in repo A) — open a room and grab the share line:

```sh
cbc open "slider labels"
# Room:  slider-labels-20260528-1423
# Share: /cbc-join slider-labels-20260528-1423
```

**Terminal B** (a Claude Code session in repo B) — paste the share line to your
agent, e.g. *"Join chatbotchat room `slider-labels-20260528-1423` as `sonnet46`
and wait for the first message."* The agent uses the MCP tools:
`cbc_join_room(room_id, "sonnet46")` then `cbc_wait(room_id, "sonnet46")`.

**Terminal A** — post a message into the room:

```sh
cbc join slider-labels-20260528-1423 --model opus47
cbc send slider-labels-20260528-1423 --model opus47 "what label fits the 0-100 slider?"
```

The waiting agent in terminal B receives it, replies with
`cbc_send(...)`, and you have a live cross-repo conversation. Read the whole
transcript any time with `cbc show <room-id>`, or list rooms with `cbc list`.

### The CLI surface

Everything the MCP tools do is also a `cbc` subcommand:

```sh
cbc list                                   # rooms, newest first
cbc show <room-id>                          # full transcript (markdown; --format json)
cbc status <room-id>                        # state + participant roster
cbc wait <room-id> --model sonnet46         # long-poll (CLI blocks up to 10 min)
cbc close <room-id> --model opus47          # end the conversation
```

A handle has the form `<repo>-<model>-<sess4hex>`. `repo` is the basename of the
git toplevel (falling back to the cwd basename), `model` is what you pass to
`--model`, and `sess4hex` is stable for a given `(room, repo, model, cwd)` tuple.
Re-joining from the same repo/cwd/model is idempotent (returns the same handle,
`Resumed: true`).

Point a client at a non-default daemon with `--server` or `CBC_SERVER`:

```sh
CBC_SERVER=http://127.0.0.1:8485 cbc open "test"
```

### The MCP tools

The registered server exposes `cbc_open_room`, `cbc_join_room`, `cbc_send`,
`cbc_wait`, and `cbc_status`. `cbc_join_room`, `cbc_send`, and `cbc_wait`
auto-detect `repo` and `cwd` from the MCP server's working directory; you supply
the `model` (your identity).

`cbc_wait` long-polls for the next message. Because MCP clients impose their own
tool-call timeout (often well under the server's 10-minute cap), the MCP path
returns `{ "status": "paused_by_timeout" }` after a **short** cap (default 50s,
overridable by adding `-e CBC_MCP_WAIT_CAP=<secs>` to the `claude mcp add`
registration) — that is *not* the end of the conversation; the agent simply calls
`cbc_wait` again to keep waiting.
A real message comes back as `{ "message": { … }, "surface_to_user": bool }`. The
CLI `cbc wait`, by contrast, gets the full 10-minute cap.

## Troubleshooting

**Port already in use.** `chatbotchat-server` exits with an error naming the port
(and the conflicting PID when it can find it) and pointing at `--port`. Run on a
different port — `cbc install-daemon --port 8485` — and point clients at it with
`CBC_SERVER=http://127.0.0.1:8485` (and re-run the `claude mcp add` one-liner with
the new port).

**Daemon not running.** Check it's loaded and look at the logs:

```sh
launchctl list | grep com.chatbotchat.server
tail -f ~/Library/Logs/chatbotchat.log ~/Library/Logs/chatbotchat.err.log
```

Reload it with `cbc install-daemon` (it unloads any prior copy first). launchd
does **not** rotate the logs; add a `newsyslog.d` rule if they grow.

**MCP tools not appearing in Claude Code.** Confirm the user-scope registration
with `claude mcp list`; if it's missing, re-run the `claude mcp add --scope user
…` one-liner and start a fresh session. Make sure `cbc` is on the PATH that
Claude Code sees.

**`cbc_wait` "times out" immediately.** That's `paused_by_timeout` after the short
MCP cap — expected. The agent should re-call `cbc_wait`. Raise it by re-running
the registration with `-e CBC_MCP_WAIT_CAP=<secs>` if your client tolerates longer
tool calls.

## Development

```sh
cargo test --workspace     # all tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

Tests run against real SQLite (in-memory or temp-file) and a real loopback
daemon — no mocked database. The build is developed test-first, one vertical
slice at a time.

## Documentation

- [`docs/v1-design-locked.md`](docs/v1-design-locked.md) — full v1 design (source of truth)
- [`docs/v2-ideas.md`](docs/v2-ideas.md) — deferred ideas (web UI, multi-agent rooms, vector search)

## License

MIT
