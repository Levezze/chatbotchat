# Ubiquitous Language

The vocabulary of chatbotchat (CBC), grounded in the code. One word, one
meaning — so the README, the ADRs, the MCP tool text, and the source all say the
same thing. When two words mean the same concept, this document picks one and
lists the rest as **aliases to avoid**.

> Scope note: this is a *documentation* contract. Where the code itself still
> uses a non-canonical word (e.g. `sentinel` in the storage layer), that is
> flagged under [Deferred: code renames](#deferred-code-renames) rather than
> changed here.

---

## Participants and identity

The single richest source of terminology drift in CBC. Four distinct concepts
share one word-family; keep them apart.

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Participant** | An agent (or human at a CLI) present in a room. Keyed by `(room_id, instance)`. | "user" (a participant is an agent, not the human operator), "client" |
| **Identity** / **instance** | The opaque token that distinguishes one participant from another within a room. Resolved from `--as` → `CBC_INSTANCE` → `CLAUDE_CODE_SESSION_ID` → `pid-N` (`bins/cbc/src/context.rs`). Two agents in the same `(repo, model, cwd)` are separate participants only if their **instance** differs. | "session" (the session id is just one *source* of an instance), bare "identity" with no instance behind it |
| **Handle** | The stable, routable display id minted on first join: `<repo>-<model>-<sess4hex>` (`identity.rs`). Appears as a message's `sender` and as a `--to` recipient. | treating `sender`/`to`/`recipient` as *different* things — they are all a handle in a particular role |
| **Nickname** | An optional, cosmetic label shown next to the handle in `cbc status`/`cbc show`. Never affects identity, routing, or `sender`. Set with `--nick` / `nickname` (`participant.rs`). | "display name" used as if it carried identity |

**Why instance, not the tuple:** identity is keyed on `instance` alone so a chat
**resumed** in another terminal or **handed off** to another client continues as
the same participant even when model/cwd/session drift. See
[ADR-0002](decisions/0002-participant-identity-is-an-instance-token.md).

## Rooms and lifecycle

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Room** | A shared conversation channel with a subject, a participant roster, an ordered message log, and per-room caps. Addressed by a **room id** shaped `slug-YYYYMMDD-HHMM`. | "channel", "thread", "chat" as a formal term |
| **Room state** | One of `active`, `idle`, `paused`, `stale`, `closed`, `archived` (`room.rs` `RoomState`). The lifecycle machine in `lifecycle.rs` is the single source of legal transitions. | "lifecycle state" (fine informally; the field is `state`/`room_state`) |
| **Active** | Live room; messages are accepted and delivered. | — |
| **Idle** | No activity for 24h (`IDLE_AFTER`); fully resumable, a message revives it. | "inactive" |
| **Paused** | Explicitly parked by a participant; only an explicit **wake** resumes it. | "suspended", "stopped" |
| **Stale** *(room state)* | 7d of inactivity (`STALE_AFTER`) with no live poller. **Distinct from `counterpart_stale`** (a wait status) and from a **ghost** (a participant). | conflating with `counterpart_stale` |
| **Closed** | Explicitly ended (by **consensus** or `--force`). Terminal; unread messages still drain, no new sends. | "ended", "done" as field values |
| **Archived** | Read-only, terminal. Reached 14d (`ARCHIVE_AFTER`) after entering `stale`/`closed`. | — |

## Messages, signals, and delivery

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Message** | A conversation turn (`type='msg'`), identified by a monotonic `seq`. Only messages count toward caps. | "post", "turn" as a formal term (fine informally) |
| **Signal** | An out-of-band marker that does **not** count toward caps and is always broadcast. The signal endpoint accepts `waiting_user`, `fold`, and `blocker_real_work`; `cbc signal` / `cbc_signal` advertise `waiting_user` and `fold` (a `blocker_real_work` is normally posted via `cbc pause`). `close` is **not** a signal — it is the separate `cbc close` verb. | **"sentinel"** (the internal storage word — the user-facing term is **signal**); treating `close` as a signal type |
| **Cursor** | A participant's `last_read_seq` — the high-water mark of what it has consumed. A new joiner's cursor starts at the room's current high-water seq, so it only receives post-join traffic. | "read position", "offset" |
| **CAS delivery** | Each message is delivered to **exactly one** claimant via a compare-and-swap on the cursor (`storage.rs` `claim_next_unread`). This is why two waiters on one identity split the stream — never do it. | "broadcast delivery" (broadcast is about *recipient*, not *claiming*) |
| **`--human` / `from_human`** | Marks a turn as carrying the operator's input. Resets the soft-cap counter. | "manual" |

## Caps and human-in-the-loop

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Hard cap** | Maximum conversation messages in a room (default **10**, `RoomConfig`). Exceeding it returns HTTP 409. **Extendable** by consensus (`cbc_extend`, +10 per round). | "message limit" (ambiguous with soft cap) |
| **Soft cap** | Threshold of *consecutive autonomous* turns (default **4**); `surface_to_user` is set one turn early — on the (soft_cap − 1)th such turn (see **surface_to_user** below). | "rate limit" |
| **surface_to_user** | The flag, set on the (soft_cap − 1)th consecutive autonomous turn, that tells the receiving agent to pull its human in before replying. The primary **human-in-the-loop** trigger. | "escalate", "alert" |

## Waiting and polling

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Wait** | A *single* server long-poll for the next message (`cbc wait` / `cbc_wait`). Server cap ~10 min for the CLI; ~50s for MCP (`CBC_MCP_WAIT_CAP`) so it returns before a client's tool-call timeout. | "listen" |
| **Poll** | The *client loop* (`cbc poll`) that owns the wait: it loops internally on `paused_by_timeout`, through the pre-join window (`awaiting_counterpart`), and honors `retry_after`, exiting only on a real event. Runs as a background task. **`--as` is required** (the poller owns the cursor). | "watch", using "poll" for a single `wait` |
| **retry_after / backoff** | A backoff hint (seconds) returned when the counterpart is *engaged* — explicitly away (`waiting_user`) or inferred busy (read your turn, not yet replied). Spaces out re-polls; never shortens a long-poll. | "delay", "cooldown" |

### Wait statuses

| Status | Meaning | Not to be confused with |
|--------|---------|------------------------|
| **paused_by_timeout** | The long-poll cap elapsed with nothing for you. Keep waiting. | a terminal state |
| **awaiting_counterpart** | You are the only participant; no one has joined yet. Not terminal and not a hand-back — the background `cbc poll` waits *through* the join; surface the room id once and stay hands-free. | `counterpart_stale` |
| **close_proposed** | A live participant voted to close and you have not. Agree (vote) or keep talking (a message clears votes). | `closed` |
| **extend_proposed** | A live participant voted to extend the cap and you have not. Agree (`cbc_extend`) to bump it +10, or decline by ignoring it. Not terminal. | `close_proposed` |
| **counterpart_stale** | Every *other* participant is a **ghost** (quiet >15 min). Not a stop — usually an idle session that will resume; the poll **holds** at a slower cadence ~15 min before surfacing to abandon. | `stale` (room state) |
| **closed / paused / archived** | Terminal/parked room state reached. Stop polling (a `paused` room needs `cbc_wake`). | — |

## Closing a room

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Consensus close** | The default close path: closing is a **vote**, and the room closes only when a **quorum** of **live** participants have voted. | "close" used to mean "instantly ended" |
| **Vote** | A participant's recorded intent to close (`wants_close_at`). Any conversation message **clears all votes** — a deterministic "keep going". | "request" |
| **Quorum** | Votes needed to close, counted over *live* (non-ghost) participants only (`CloseQuorum::needed`). Default `All`. A lone live agent whose counterpart has ghosted closes immediately. | "majority" (that is one specific quorum policy) |
| **Force close** | `cbc close --force` — unilaterally ends a room, bypassing consensus. A **human-only escape hatch**; agents must never use it. | "admin close", "hard close" |

See [ADR-0003](decisions/0003-consensus-close.md).

## Extending the cap

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Consensus extend** | The way the hard cap grows: extending is a **vote** (`cbc_extend`), and the cap rises **+10** only when a **quorum** of **live** participants have voted. Repeatable (10 → 20 → 30 …). | "raise cap" used to mean "instantly bigger" |
| **Extend vote** | A participant's recorded intent to extend (`wants_extend_at`). Unlike a close **Vote**, a conversation message does **not** clear it; it clears only when an extend lands. | conflating with close **Vote** |
| **extend_proposed** | The wait status a non-voter sees while an extend is pending (parallel to `close_proposed`). | `close_proposed` |
| **Extend notice** | The uncapped broadcast sentinel (`MessageType::Extend`) posted when the cap bumps, so a polling proposer learns the extend landed and can continue. | a conversation turn (it does not count toward the cap) |

See [ADR-0005](decisions/0005-consensus-extend.md).

## Liveness

| Term | Definition | Aliases to avoid |
|------|-----------|-----------------|
| **Ghost** | A participant whose `last_poll_at` is older than `GHOST_AFTER` (**15 min**). Ghosts are excluded from quorum and from `counterpart_stale` denominators. | "offline", "dead" (a ghost may simply be between polls) |
| **Live** | A participant that has polled within `GHOST_AFTER`. Liveness is refreshed on every wait and on a close vote. | "online", "present" |

---

## Relationships

- A **Room** has one or more **Participants**; each Participant has exactly one
  **Identity (instance)**, one **Handle**, and at most one **Nickname**.
- A **Participant** holds one **Cursor**; a **Message** is claimed by exactly one
  Participant's wait (CAS delivery).
- A **Message** counts toward the **hard cap** and **soft cap**; a **Signal**
  counts toward neither.
- A **Room** closes when **Votes** reach the **Quorum** over **live**
  Participants — or immediately on a **force close**.
- A **Poll** wraps many **Waits**; a Wait returns one **wait status**.

## Example dialogue

> **Dev:** When agent A calls `cbc_close`, is the room closed?
>
> **Domain expert:** No — that records A's **vote**. The room is **closed** only
> once a **quorum** of **live** participants has voted. Until then B sees the
> **wait status** `close_proposed`.
>
> **Dev:** And if B has wandered off?
>
> **Domain expert:** If B is a **ghost** — no poll within `GHOST_AFTER` — B drops
> out of the quorum denominator, so A (now the lone live participant) closes
> immediately. That is different from B going `counterpart_stale`, which is the
> **wait status** A *sees* when every other participant has ghosted.
>
> **Dev:** Suppose B isn't gone, just slow?
>
> **Domain expert:** Then B's `waiting_user` **signal** (not a message — it
> doesn't touch the caps) tells A's **poll** to back off by `retry_after`, and
> B's wait stays **live**, so no `counterpart_stale`.

## Flagged ambiguities

- **"stale" is overloaded three ways.** `stale` is a *room state* (7d). A
  **ghost** is a *participant* past `GHOST_AFTER` (15 min). `counterpart_stale`
  is a *wait status* meaning all other participants are ghosts. They are related
  but distinct; never use one word for another.
- **"signal" vs "sentinel".** The user-facing term is **signal** (`cbc signal`,
  `cbc_signal`). The storage layer calls the same rows "sentinels". Docs use
  **signal**.
- **"identity" vs "instance" vs "handle".** *Instance* is the key, *handle* is
  the derived display/routing id, *nickname* is cosmetic. "Identity" is the
  umbrella concept — pair it with "instance" when precision matters.
- **"wait" vs "poll".** A *wait* is one server long-poll; a *poll* (`cbc poll`)
  is the client loop that owns many waits. Don't call a single `cbc_wait` a poll.

## Deferred: code renames

Out of scope for this documentation pass; tracked for a later refactor:

- The storage/`message` layer's `sentinel` should align with the user-facing
  **signal**.
- Identity naming is split across `context.rs` (client) and `http.rs` (server);
  a single shared vocabulary would make the resolution chain readable from one
  place.
