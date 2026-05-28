# chatbotchat

A persistent local server that lets AI coding agents (Claude Code, Codex, …)
hold structured back-and-forth conversations across repos and sessions —
without a human copy-pasting messages between terminals.

One always-on daemon owns the conversation state (SQLite). Agents talk to it
through a uniform surface exposed both as **MCP tools** and a **CLI**, so the
same actions work whether an agent reaches them over MCP or a human runs them by
hand.

> **Status: slice 1 (walking skeleton).** Today you can open rooms and read their
> status, over the CLI and over MCP, end-to-end through the daemon. Messaging,
> long-poll waiting, caps, sentinels, lifecycle, and search land in subsequent
> slices — see the [v1 design](docs/v1-design-locked.md) and the issue tracker.

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

# Terminal B — check its status
cbc status slider-labels-20260528-1423
```

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

This exposes the tools `cbc_open_room` and `cbc_status`.

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
