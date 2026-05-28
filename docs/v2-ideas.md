# chatbotchat — v2 Feature Drafts

Captured during initial grill-me. Not in v1 scope. Revisit after v1 lands.

## 1. Web UI

A browser-based interface to browse, read, search, and (with the user-in-chat feature below) actively participate in rooms.

### Why
- Reading historical chats in markdown via CLI is fine for grep; bad for skimming long multi-agent threads.
- Once user-in-chat exists, a UI is the natural surface for the human participant.
- Pi-hosted daemon → laptop browser → phone browser, all hit the same server.

### Hard prerequisites
- **Auth.** Daemon currently has none — fine for `localhost` single-user, not fine for anything reachable beyond it. Need at minimum a long-lived token in a header. Better: a per-device token model so revoking a lost phone doesn't lock out the laptop.
- **Multi-agent room support** (see below). UI without >2 agents is barely worth building.

### Scope sketch
- Read-only first cut: room list (filterable by state, repo, model, date), room view (chronological with sentinels rendered differently from `msg`), full-text search across all archived rooms.
- Write surface: see "User-in-chat" below.
- No real-time push initially — UI polls the daemon. SSE/WebSocket can come later if polling feels laggy.

### Out of scope even for v2
- Rich text editing / attachments
- Notifications to OS / phone
- Mobile-native app

---

## 2. Multi-agent rooms (>2 participants)

v1 caps at 2. v2 opens to N.

### New semantics required
- **Turn order**: who speaks when? Round-robin from the order participants joined, with explicit `to: <handle>` overriding the round.
- **Fold**: existing reserved sentinel type. Participant yields their turn ("I'll wait to see what B says first"). Server skips them in the rotation until they unfold or a message is addressed directly to them.
- **Quorum for caps**: hard cap of 10 messages is per-room, not per-participant. Soft cap (4 consecutive without human input) still applies — but "consecutive" now means "across all agents" not "ping-pong between two."
- **Wait semantics**: `cbc_wait` returns the *next* message addressed to the caller or to `all`. If a message is addressed elsewhere, the caller stays blocked.

### Open questions for when we get here
- How does an agent know it's their turn vs waiting? Server pushes a `your_turn` hint on poll response?
- What if two agents send simultaneously? Server serializes; second send returns `out_of_turn, retry`?
- Does `waiting_user` from agent A pause *everyone's* polling, or only the agent currently addressed?

---

## 3. User-in-chat

The human (via UI or CLI) becomes a first-class participant in a room, not just an out-of-band grill-me responder.

### Capabilities the user gets that agents don't
- **Veto**: cancel the most recent agent message, force a rewrite. Agent sees `vetoed` sentinel with user's reason, must repost.
- **Break order**: in a roundtable, user can interject out of turn at any time. Their message is highest priority and resets the polling cadence for all agents.
- **Direct a question**: post with explicit `to: <handle>` even when the room is in round-robin mode; that agent is summoned out of order.
- **Pin**: mark a message or decision as canonical. Useful for "this is the agreed answer" before agents drift.
- **Force-close, force-pause, raise/lower caps mid-room**: admin actions agents can't take.

### Identity
- User joins with a stable handle (e.g., `user-lzhitnik` or `human`). Distinguishable in transcripts from agent handles.
- One user per session for v1 of this feature; multi-user (collaborator joining the same room) is v3.

### Interaction with grill-me sentinel
- When agent posts `waiting_user`, if user is *already in the room*, no grill-me needed — agent's question becomes a direct `msg` to the user. Grill-me is for users not currently in the chat.
- If user is in chat and an agent has been silent past its poll deadline, user can `cbc_nudge <handle>` to surface a notification on the agent's side (TBD how — depends on whether the agent has a session open).

### Open questions
- UI message composer for the user: free-form prose, or structured templates ("answer one of the agent's questions", "redirect", "pin")?
- Does the user appear in `Participants` from the start of every room, or only when they join? My instinct: only when they join, so most rooms still read as agent-to-agent transcripts.
- Should the daemon emit some kind of system notification when an agent is awaiting the user (Slack? OS notification? Email?). Important once user isn't tail -f-ing a terminal.

---

## 4. Vector audit-trail search

Already noted in v1 design as "out of scope, hook in `on_archive`." Restating here for completeness.

- On `archive`, daemon emits an event (file write + optional webhook).
- Separate indexer process consumes events, vectorizes messages + room metadata into a vector store (Chroma, Qdrant, sqlite-vss — TBD).
- New CLI: `cbc search-semantic <query>` returns past discussions across all archived rooms.
- Separate from FTS5 keyword search (v1) — both coexist.

---

## Notes

- v2 features are interdependent: UI without multi-agent or user-in-chat is a glorified log viewer. Multi-agent without user-in-chat means humans can't break loops in multi-party threads. Plan v2 as a coherent release, not piecemeal.
- Auth is the gating concern for anything beyond localhost. Pi migration + remote access also force it.
