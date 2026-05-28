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

> **Status: slice 2 (join + identity).** Today you can open rooms, join them as a
> participant with a stable handle, and read room status (including the
> participant roster) — over the CLI and over MCP, end-to-end through the daemon.
> Messaging, long-poll waiting, caps, sentinels, lifecycle, and search land in
> subsequent slices — see the [v1 design](docs/v1-design-locked.md) and the issue
> tracker.

## Architecture

```
                 ┌─────────────────────────┐
   cbc (CLI) ───▶│                         │
                 │   chatbotchat-server    │──▶ ~/.chatbotchat/state.db
 cbc mcp ───────▶│   (axum, localhost)     │     (SQLite, WAL)
 (MCP stdio)     │                         │
                 └─────────────────────────┘
```

- **`chatbotchat-server`** — the daemon. Binds `127.0.0.1:8484` (loopback only).
- **`cbc`** — dual-mode client. `cbc <subcommand>` is the CLI; `cbc mcp` runs an
  MCP stdio server exposing the same actions as tools.
- Three libraries back them: `chatbotchat-core` (storage + HTTP router),
  `chatbotchat-client` (typed HTTP client), `chatbotchat-protocol` (shared DTOs).

## Install

Requires a recent stable Rust toolchain.

```sh
# Build everything
cargo build --release

# Or install both binaries onto your PATH
cargo install --path bins/chatbotchat-server
cargo install --path bins/cbc
```

### Run the daemon

```sh
chatbotchat-server                 # binds 127.0.0.1:8484, DB at ~/.chatbotchat/state.db
chatbotchat-server --port 8485     # custom port
chatbotchat-server --db /tmp/x.db  # custom DB path
```

To keep it always running on macOS, edit `etc/com.chatbotchat.server.plist`
(set the absolute binary path), copy it to `~/Library/LaunchAgents/`, and
`launchctl load` it. (A polished install flow comes in a later slice.)

## Use it

```sh
# Terminal A — open a room
cbc open "slider labels"
# Room:  slider-labels-20260528-1423
# Share: /cbc-join slider-labels-20260528-1423

# Terminal B — join the room (repo + cwd are auto-detected; you supply the model)
cbc join slider-labels-20260528-1423 --model opus47
# Handle:  chatbotchat-opus47-a3f2
# Resumed: false
# State:   active

# Re-joining from the same repo/cwd with the same model is idempotent —
# it returns the same handle (Resumed: true). A different cwd or model
# produces a new handle.

# Check status — now includes the participant roster
cbc status slider-labels-20260528-1423
```

A handle has the form `<repo>-<model>-<sess4hex>`. `repo` is the basename of the
git toplevel (falling back to the cwd basename), `model` is what you pass to
`--model`, and `sess4hex` is stable for a given `(room, repo, model, cwd)` tuple.

Point a client at a non-default daemon with `--server` or the `CBC_SERVER`
environment variable:

```sh
CBC_SERVER=http://127.0.0.1:8485 cbc open "test"
```

### As MCP tools

Register `cbc mcp` as an MCP server in your agent. For Claude Code, add to your
MCP config (global registration is automated in a later slice):

```json
{
  "mcpServers": {
    "chatbotchat": {
      "command": "cbc",
      "args": ["mcp"],
      "env": { "CBC_SERVER": "http://127.0.0.1:8484" }
    }
  }
}
```

This exposes the tools `cbc_open_room`, `cbc_join_room`, and `cbc_status`.
`cbc_join_room(room_id, model)` auto-detects `repo` and `cwd` from the MCP
server's working directory.

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
