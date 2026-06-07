use crate::identity::{derive_handle, HandleOutcome, JoinIdentity};
use crate::ids;
use crate::lifecycle::{self, LifecycleEvent};
use crate::message::{Message, MessageType, Severity};
use crate::participant::Participant;
use crate::room::{Room, RoomConfig, RoomState};
use crate::storage::{RoomSummaryRow, Storage, StorageError};
use crate::waiter::{backoff_secs, wait_for_message, Hub, WaitOutcome};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chatbotchat_protocol::{
    ErrorEnvelope, JoinRoomRequest, JoinRoomResponse, LifecycleRequest, LifecycleResponse,
    MessageView, OpenRoomRequest, OpenRoomResponse, ParticipantView, RoomStatus, RoomSummary,
    RoomTranscript, SendMessageRequest, SendMessageResponse, SignalRequest, SignalResponse,
    WaitRequest, WaitResponse,
};
use std::sync::Arc;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Default server-side long-poll cap (per the locked design: 10 minutes).
const DEFAULT_WAIT_CAP: Duration = Duration::from_secs(600);

/// Shared state injected into every handler. Cloneable: `Storage` wraps a
/// connection pool behind an `Arc`-like handle, and the `Hub` is shared via
/// `Arc`.
#[derive(Clone)]
pub struct AppState {
    storage: Storage,
    hub: Arc<Hub>,
    /// Server-side cap for a single `wait` long-poll.
    wait_cap: Duration,
}

impl AppState {
    pub fn new(storage: Storage) -> Self {
        AppState {
            storage,
            hub: Arc::new(Hub::new()),
            wait_cap: DEFAULT_WAIT_CAP,
        }
    }

    /// Construct with an explicit long-poll cap. Lets tests exercise the
    /// timeout path without parking for the full 10 minutes.
    pub fn with_wait_cap(storage: Storage, wait_cap: Duration) -> Self {
        AppState {
            storage,
            hub: Arc::new(Hub::new()),
            wait_cap,
        }
    }

    /// A clone of the storage handle (`Storage` is `Clone`). The daemon needs it
    /// to spawn the background sweeper alongside the router; the field itself
    /// stays private to this module.
    pub fn storage(&self) -> Storage {
        self.storage.clone()
    }
}

/// Build the application router. This is the integration seam exercised both by
/// in-process `oneshot` tests and the real daemon binary.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rooms", post(open_room).get(list_rooms))
        .route("/rooms/{id}", get(get_room))
        .route("/rooms/{id}/transcript", get(transcript))
        .route("/rooms/{id}/join", post(join_room))
        .route("/rooms/{id}/messages", post(send_message))
        .route("/rooms/{id}/signals", post(signal_room))
        .route("/rooms/{id}/wait", get(wait_room))
        .route("/rooms/{id}/close", post(close_room))
        .route("/rooms/{id}/pause", post(pause_room))
        .route("/rooms/{id}/wake", post(wake_room))
        .with_state(state)
}

async fn open_room(
    State(state): State<AppState>,
    Json(req): Json<OpenRoomRequest>,
) -> Result<(StatusCode, Json<OpenRoomResponse>), ApiError> {
    let now = OffsetDateTime::now_utc();
    let base = ids::room_id(&req.subject, now);

    // Open-time cap overrides layer over the defaults; omitted opts keep them.
    let defaults = RoomConfig::default();
    let config = RoomConfig {
        hard_cap: req.hard_cap.unwrap_or(defaults.hard_cap),
        soft_cap: req.soft_cap.unwrap_or(defaults.soft_cap),
        close_quorum: defaults.close_quorum,
    };

    // Reject pathological caps rather than minting a degenerate room. A hard_cap
    // of 0 would accept no messages; a soft_cap below 2 has no valid surface
    // threshold (surface fires on the soft_cap-1 th consecutive autonomous turn,
    // so it needs soft_cap >= 2). We deliberately do NOT require soft_cap <=
    // hard_cap: a low hard_cap with the default soft_cap is a legitimate
    // "soft cap effectively off" config, not an error.
    if config.hard_cap < 1 || config.soft_cap < 2 {
        return Err(ApiError::BadRequest(format!(
            "invalid caps: hard_cap must be >= 1 and soft_cap >= 2 \
             (got hard_cap={}, soft_cap={})",
            config.hard_cap, config.soft_cap
        )));
    }

    // The base id is only minute-granular, so two opens of the same subject in
    // one minute would collide on the primary key. Disambiguate by suffixing
    // `-2`, `-3`, … and retrying; the DB UNIQUE constraint makes this race-free
    // even under concurrent opens.
    const MAX_ATTEMPTS: u32 = 64;
    let mut attempt = 0u32;
    let room_id = loop {
        attempt += 1;
        let candidate = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        let room = Room {
            id: candidate.clone(),
            subject: req.subject.clone(),
            started_at: now,
            last_activity_at: now,
            state: RoomState::Active,
            state_changed_at: now,
            config,
            prev_room_id: req.prev_room_id.clone(),
        };
        match state.storage.create_room(&room).await {
            Ok(()) => break candidate,
            Err(e) if e.is_unique_violation() && attempt < MAX_ATTEMPTS => continue,
            Err(e) => return Err(e.into()),
        }
    };

    let resp = OpenRoomResponse {
        share_line: ids::share_line(&room_id),
        room_id,
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn get_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoomStatus>, ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let participants = state.storage.list_participants(&id).await?;
    Ok(Json(room_to_status(&room, &participants)?))
}

/// Query params for `GET /rooms`. `state` deserializes from the same snake_case
/// names the wire uses (`active`, `closed`, …); an unknown value fails the
/// extractor and axum returns 400 — no hand-rolled validation. Both default so a
/// bare `GET /rooms` is valid (no filter, archived hidden).
#[derive(Debug, serde::Deserialize)]
struct ListRoomsQuery {
    #[serde(default)]
    state: Option<RoomState>,
    #[serde(default)]
    all: bool,
}

/// `GET /rooms` — the browse list. Rooms newest-first, each with its live
/// participant count. `?state=X` filters to one state; `?all=true` includes
/// archived; the default hides archived. Backs `cbc list`.
async fn list_rooms(
    State(state): State<AppState>,
    Query(q): Query<ListRoomsQuery>,
) -> Result<Json<Vec<RoomSummary>>, ApiError> {
    let rows = state.storage.list_rooms(q.state, q.all).await?;
    let summaries = rows
        .iter()
        .map(room_summary)
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(summaries))
}

/// `GET /rooms/:id/transcript` — the full room for `cbc show`: metadata, caps and
/// their current counters, participants, and every message oldest-first. Separate
/// from `GET /rooms/:id` (which serves the lighter `RoomStatus`) so neither view
/// constrains the other.
///
/// This is a deliberate, opt-in *peek* at the whole log: it does NOT advance any
/// participant's read cursor and is NOT a consumption path. Agents consume their
/// inbox exclusively via `wait` (which advances `last_read_seq`); a `show` here is
/// for humans/debugging and intentionally overlaps the inbox. (The automatic
/// join `recent_messages` window, by contrast, is bounded to the joiner's cursor
/// so it never double-surfaces unread inbox messages.)
async fn transcript(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoomTranscript>, ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let participants = state.storage.list_participants(&id).await?;
    let messages = state.storage.all_messages(&id).await?;
    let hard_cap_count = state.storage.count_capped_messages(&id).await?;
    // The soft-cap run is evaluated at the room's latest message (high-water seq).
    let high_water = state.storage.current_seq(&id).await?;
    let soft_cap_consecutive = state.storage.consecutive_msg_count(&id, high_water).await?;
    Ok(Json(room_to_transcript(
        &room,
        &participants,
        &messages,
        hard_cap_count,
        soft_cap_consecutive,
    )?))
}

/// Register the caller as a participant. Idempotent on `(room_id, repo, model,
/// cwd)`: the same tuple always resolves to the same handle. A fresh tuple mints
/// `<repo>-<model>-<sess4hex>`, retrying on the (astronomically rare) sess
/// collision via the UNIQUE constraint, mirroring `open_room`'s retry.
async fn join_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<JoinRoomRequest>,
) -> Result<(StatusCode, Json<JoinRoomResponse>), ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // A newly-minted participant starts reading from "now": its cursor is the
    // room's current high-water seq, so `wait` only delivers post-join traffic.
    //
    // `recent_messages` is bounded to the joiner's *effective cursor* (resolved
    // per branch below), NOT the latest 50. This is the Bug-B fix: on a resume the
    // preserved cursor can sit behind the room head, and an unbounded recency
    // window would re-surface unread inbox messages (seq > cursor) that `wait`
    // will also deliver — so the agent acts on them twice. Bounding at the cursor
    // makes the inbox (seq > cursor) the exclusive province of `wait`. A fresh
    // participant's cursor is `start_seq`, so its window is the full backlog
    // (unchanged behavior).
    let start_seq = state.storage.current_seq(&id).await?;

    // Optional display nickname (distinct from identity). A blank/whitespace value
    // is treated as "not supplied" so it neither sets nor clears anything.
    let nickname = normalize_nickname(req.nickname);

    let ident = JoinIdentity {
        instance: effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        repo: req.repo,
        model: req.model,
        cwd: req.cwd,
    };

    // Fast path: an existing participant with this instance resumes its handle. A
    // re-join that supplies a nickname updates it (identity is untouched).
    if let Some(p) = state
        .storage
        .get_participant_by_instance(&id, &ident.instance)
        .await?
    {
        if nickname.is_some() {
            state
                .storage
                .set_nickname(&p.handle, nickname.as_deref())
                .await?;
        }
        let recent = recent_message_views_up_to(&state.storage, &id, p.last_read_seq).await?;
        return Ok(join_response(p.handle, true, &room, recent));
    }

    const MAX_ATTEMPTS: u32 = 64;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let existing = state.storage.list_participants(&id).await?;
        let handle = match derive_handle(&ident, &existing, rand_sess_candidates()) {
            // Reused shouldn't occur after the fast-path lookup, but if a
            // concurrent join inserted the same instance, honor it as resumed —
            // bounding recent to that row's cursor (fall back to start_seq if the
            // race row vanished).
            HandleOutcome::Reused(h) => {
                if nickname.is_some() {
                    state.storage.set_nickname(&h, nickname.as_deref()).await?;
                }
                let cursor = state
                    .storage
                    .get_participant_by_instance(&id, &ident.instance)
                    .await?
                    .map(|p| p.last_read_seq)
                    .unwrap_or(start_seq);
                let recent = recent_message_views_up_to(&state.storage, &id, cursor).await?;
                return Ok(join_response(h, true, &room, recent));
            }
            HandleOutcome::Created(h) => h,
        };

        let now = OffsetDateTime::now_utc();
        let participant = Participant {
            handle: handle.clone(),
            room_id: id.clone(),
            repo: ident.repo.clone(),
            model: ident.model.clone(),
            cwd: ident.cwd.clone(),
            instance: ident.instance.clone(),
            joined_at: now,
            last_poll_at: now,
            last_read_seq: start_seq,
            nickname: nickname.clone(),
            wants_close_at: None,
        };

        match state.storage.create_participant(&participant).await {
            Ok(()) => {
                // Fresh participant: cursor = start_seq, so the window is the full
                // pre-join backlog and the inbox is strictly seq > start_seq.
                let recent = recent_message_views_up_to(&state.storage, &id, start_seq).await?;
                return Ok(join_response(handle, false, &room, recent));
            }
            Err(e) if e.is_unique_violation() && attempt < MAX_ATTEMPTS => {
                // Either a concurrent join took our instance (refetch → resume)
                // or the random sess collided (refetch returns None → retry).
                if let Some(p) = state
                    .storage
                    .get_participant_by_instance(&id, &ident.instance)
                    .await?
                {
                    let recent =
                        recent_message_views_up_to(&state.storage, &id, p.last_read_seq).await?;
                    return Ok(join_response(p.handle, true, &room, recent));
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// The identity key for a request. An explicit, non-empty `instance` (resolved
/// client-side from an `as` label / harness session id / per-process nonce) wins.
/// A legacy caller that sends none gets one synthesized from the tuple, so its
/// identity is the same `(repo, model, cwd)`-equivalent it always had — the exact
/// expression migration 0009 backfills existing rows with.
fn effective_instance(repo: &str, model: &str, cwd: &str, instance: &str) -> String {
    if instance.is_empty() {
        format!("{repo}\n{model}\n{cwd}")
    } else {
        instance.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_instance_passes_through_an_explicit_value() {
        assert_eq!(
            effective_instance("mvp-api", "opus48", "/work/mvp", "concierge"),
            "concierge"
        );
    }

    #[test]
    fn effective_instance_synthesis_matches_the_migration_backfill_expression() {
        // Load-bearing: a legacy/old-binary client sends empty instance, so the
        // server must synthesize the SAME string migration 0009 backfilled
        // existing rows with — `repo || char(10) || model || char(10) || cwd`
        // (char(10) == '\n') — or those participants fail to resume. Pin the
        // exact bytes so a future edit to the Rust side can't drift silently.
        assert_eq!(
            effective_instance("mvp-api", "opus48", "/work/mvp", ""),
            "mvp-api\nopus48\n/work/mvp"
        );
    }

    /// Defense-in-depth for the same coupling, from the SQL side: execute the
    /// REAL migration 0009 text against a seeded pre-0009 row and assert the
    /// backfilled `instance` byte-equals the server-side empty-instance
    /// synthesis. The literal pin above guards the Rust side; this guards the
    /// migration's `char(10)` backfill expression, which that literal can't see.
    /// Also confirms the table recreate preserves `last_read_seq`.
    #[tokio::test]
    async fn migration_0009_backfill_matches_effective_instance_synthesis() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        // Pre-0009 schema: rooms (FK target) + the old tuple-keyed participants.
        for ddl in [
            "CREATE TABLE rooms (id TEXT PRIMARY KEY)",
            "CREATE TABLE participants (\
                 handle TEXT PRIMARY KEY, \
                 room_id TEXT NOT NULL REFERENCES rooms(id), \
                 repo TEXT NOT NULL, model TEXT NOT NULL, cwd TEXT NOT NULL, \
                 joined_at TEXT NOT NULL, last_poll_at TEXT NOT NULL, \
                 last_read_seq INTEGER NOT NULL DEFAULT 0, \
                 UNIQUE (room_id, repo, model, cwd))",
            "INSERT INTO rooms (id) VALUES ('r1')",
            "INSERT INTO participants \
                 (handle, room_id, repo, model, cwd, joined_at, last_poll_at, last_read_seq) \
                 VALUES ('mvp-api-opus48-088a','r1','mvp-api','opus48','/work/mvp','t0','t0',7)",
        ] {
            sqlx::query(ddl)
                .execute(&pool)
                .await
                .expect("seed pre-0009");
        }

        // Apply the actual migration. `raw_sql` runs the whole multi-statement
        // script (and tolerates its leading comment block) the way the migrator
        // would — a naive split on ';' would feed the comment prose to the parser.
        let sql = include_str!("../migrations/0009_participant_instance.sql");
        sqlx::raw_sql(sql)
            .execute(&pool)
            .await
            .expect("run migration 0009");

        let (instance, last_read): (String, i64) = sqlx::query_as(
            "SELECT instance, last_read_seq FROM participants WHERE handle = 'mvp-api-opus48-088a'",
        )
        .fetch_one(&pool)
        .await
        .expect("the seeded row survives the recreate");

        assert_eq!(
            instance,
            effective_instance("mvp-api", "opus48", "/work/mvp", ""),
            "0009 backfill must byte-match the empty-instance synthesis so legacy clients resume"
        );
        assert_eq!(
            last_read, 7,
            "last_read_seq must survive the table recreate"
        );
    }
}

fn join_response(
    handle: String,
    resumed: bool,
    room: &Room,
    recent_messages: Vec<MessageView>,
) -> (StatusCode, Json<JoinRoomResponse>) {
    (
        StatusCode::CREATED,
        Json(JoinRoomResponse {
            handle,
            resumed,
            room_state: room.state.as_str().to_string(),
            recent_messages,
        }),
    )
}

/// The most recent messages in a room, as wire views (oldest-first), bounded to
/// `seq <= max_seq`. Callers pass a joiner's read cursor so the join "recent
/// context" never overlaps the inbox (`wait` delivers seq > cursor); a fresh
/// joiner passes its high-water cursor to recover the full backlog.
const RECENT_MESSAGE_LIMIT: i64 = 50;

async fn recent_message_views_up_to(
    storage: &Storage,
    room_id: &str,
    max_seq: i64,
) -> Result<Vec<MessageView>, ApiError> {
    let msgs = storage
        .recent_messages_up_to(room_id, max_seq, RECENT_MESSAGE_LIMIT)
        .await?;
    msgs.iter().map(message_view).collect()
}

/// Post a `msg` to a room. The sender is resolved from the `(repo, model, cwd)`
/// tuple — the caller must already be a participant (400 otherwise). `to`
/// omitted broadcasts to all. After the insert commits, ring the room so any
/// parked waiters re-query.
async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Write guard: a paused/closed/archived room takes no new traffic. This is an
    // additive pre-check, separate from `create_message_capped`'s atomic cap gate
    // (a locked design constraint — the guard must not fold state into that SQL).
    // Consequently it is a non-atomic pre-read: a `close`/`pause` landing between
    // this check and the insert can admit one trailing message onto a room that
    // just became non-writable. Accepted for v1 — the message is logged, no state
    // is corrupted, and the next `wait` returns the terminal status.
    reject_unless_writable(&room)?;

    let sender = state
        .storage
        .get_participant_by_instance(
            &id,
            &effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    // A targeted `to` must be a real participant, else the message would be an
    // undeliverable orphan (excluded from broadcast, matched by no one) while the
    // sender got a success — a silent black hole. Reject it instead.
    if let Some(to) = req.to.as_deref() {
        let participants = state.storage.list_participants(&id).await?;
        if !participants.iter().any(|p| p.handle == to) {
            return Err(ApiError::BadRequest(
                "recipient is not a participant of this room".into(),
            ));
        }
    }

    // Hard cap: once the room holds `hard_cap` cap-counting messages, refuse
    // further sends. Bounded agent talk with a human in the loop is the point —
    // this is the runaway-token backstop. The gate is enforced inside the insert
    // (one atomic SQL statement), so concurrent sends cannot slip past the cap.
    // The escape hatch — raising the cap (#5) or closing the room (#7) — lands in
    // a later slice; here we only enforce-and-reject, with no room-state change.
    let now = OffsetDateTime::now_utc();
    let msg = state
        .storage
        .create_message_capped(
            &id,
            &sender.handle,
            req.to.as_deref(),
            &req.body,
            now,
            req.from_human,
            room.config.hard_cap as i64,
        )
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "hard cap reached ({} messages); raise the cap or close the room",
                room.config.hard_cap
            ))
        })?;

    // A conversational message is a deterministic "continue, don't close": it
    // cancels any pending close proposal in the room (consensus close). The
    // counterpart's next wait then delivers this message instead of the
    // `close_proposed` status. Signals (waiting_user etc.) deliberately do NOT
    // clear — stepping away is not "keep talking".
    state.storage.clear_close_votes(&id).await?;

    // Activity bookkeeping. The timestamp bump drives the sweeper's idle/stale
    // timing on every msg; and a msg landing on an `idle`/`stale` room revives it
    // to `active` (a `Message` transition). The state write is guarded to those
    // two states so an active room does not log a spurious active→active row.
    state.storage.touch_last_activity(&id, now).await?;
    if matches!(room.state, RoomState::Idle | RoomState::Stale) {
        let to = lifecycle::transition(room.state, LifecycleEvent::Message)
            .expect("Message is a legal transition from idle/stale");
        state
            .storage
            .update_room_state(&id, room.state, to, now, None)
            .await?;
    }

    state.hub.notify(&id);

    Ok((
        StatusCode::CREATED,
        Json(SendMessageResponse { seq: msg.seq }),
    ))
}

/// Post a sentinel (out-of-band signal) to a room. The sender is resolved from
/// the `(repo, model, cwd)` tuple (400 if not a participant). Sentinels are
/// uncapped — they route through `create_message_typed`, never the capped gate —
/// and are always broadcast. Accepted here: `waiting_user`, `fold`, and
/// `blocker_real_work` (the latter also drives a `Pause`); `close` is the close
/// endpoint's, and the conversation `msg` belongs on /messages. After the insert
/// commits, ring the room so any parked waiters re-query.
async fn signal_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SignalRequest>,
) -> Result<(StatusCode, Json<SignalResponse>), ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Write guard: a paused/closed/archived room takes no new signals (same gate
    // as `send_message`). `blocker_real_work` on active/idle passes here and the
    // Pause is decided below.
    reject_unless_writable(&room)?;

    let sender = state
        .storage
        .get_participant_by_instance(
            &id,
            &effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    // Only sentinel types are valid on this endpoint. `close` is the close
    // endpoint's; the conversation `msg` belongs on /messages.
    let msg_type = match MessageType::parse(&req.signal_type) {
        Some(t @ (MessageType::WaitingUser | MessageType::Fold | MessageType::BlockerRealWork)) => {
            t
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "unsupported signal type '{}'; expected waiting_user, fold or blocker_real_work",
                req.signal_type
            )))
        }
    };

    // Per-type field rules, checked on field *presence* (not emptiness) so a
    // stray empty string can't slip a non-NULL value past the invariant. Done
    // before parsing the severity value, so a `fold` carrying any severity is
    // rejected as "fold takes no severity" rather than "invalid severity".
    // `waiting_user` is the question-carrying sentinel (needs both severity and a
    // non-empty question); `fold` is a bare marker; `blocker_real_work` carries
    // only an optional free-text `reason` (no severity, no question).
    let (severity, body): (Option<Severity>, &str) = match msg_type {
        MessageType::WaitingUser => {
            let s = req.severity.as_deref().ok_or_else(|| {
                ApiError::BadRequest("waiting_user requires a severity (low|med|high)".into())
            })?;
            let severity = Severity::parse(s)
                .ok_or_else(|| ApiError::BadRequest(format!("invalid severity '{s}'")))?;
            if req.question_text.as_deref().is_none_or(|q| q.is_empty()) {
                return Err(ApiError::BadRequest(
                    "waiting_user requires a non-empty question_text".into(),
                ));
            }
            (Some(severity), "")
        }
        MessageType::Fold => {
            if req.severity.is_some() {
                return Err(ApiError::BadRequest("fold takes no severity".into()));
            }
            if req.question_text.is_some() {
                return Err(ApiError::BadRequest("fold takes no question_text".into()));
            }
            (None, "")
        }
        MessageType::BlockerRealWork => {
            if req.severity.is_some() {
                return Err(ApiError::BadRequest(
                    "blocker_real_work takes no severity".into(),
                ));
            }
            if req.question_text.is_some() {
                return Err(ApiError::BadRequest(
                    "blocker_real_work takes no question_text".into(),
                ));
            }
            // The reason rides in the message body so the counterpart can read it.
            (None, req.reason.as_deref().unwrap_or(""))
        }
        // The match above only admits the three sentinel types.
        _ => unreachable!("signal type already gated to waiting_user|fold|blocker_real_work"),
    };

    // For `blocker_real_work`, confirm the `Pause` is legal from the room's
    // current state — an illegal origin (e.g. `stale`) is a 409 with nothing
    // written.
    let pause_to = if msg_type == MessageType::BlockerRealWork {
        Some(
            lifecycle::transition(room.state, LifecycleEvent::Pause).map_err(|_| {
                ApiError::Conflict(format!("cannot Pause from {}", room.state.as_str()))
            })?,
        )
    } else {
        None
    };

    let now = OffsetDateTime::now_utc();

    // Apply the pause *before* persisting the sentinel. The CAS is the real
    // gate: if a concurrent transition moved the room off `room.state` between
    // our read and here, it returns `false` and we 409 with nothing written —
    // never a 201 on an unpaused room, and never an orphaned blocker message.
    // The reason is recorded in the transition's audit row (`events.detail`).
    if let Some(to) = pause_to {
        let changed = state
            .storage
            .update_room_state(&id, room.state, to, now, req.reason.as_deref())
            .await?;
        if !changed {
            return Err(ApiError::Conflict(
                "room state changed concurrently; retry".into(),
            ));
        }
    }

    let msg = state
        .storage
        .create_message_typed(
            &id,
            &sender.handle,
            None,
            body,
            now,
            msg_type,
            severity,
            req.question_text.as_deref(),
        )
        .await?;

    state.hub.notify(&id);

    Ok((StatusCode::CREATED, Json(SignalResponse { seq: msg.seq })))
}

/// Close a room by CONSENSUS. The caller must be a participant. A close is a
/// *vote*: the room transitions to `closed` only once a quorum of *live*
/// participants (per `RoomConfig::close_quorum`, default all live) have voted.
/// Until then the vote is recorded and the response is `close_proposed`
/// (`votes`/`needed`); the counterpart learns of it via a computed
/// `close_proposed` wait status and either votes too (agree) or sends a message
/// (which clears all votes — "continue"). A dead/ghost participant never counts
/// toward quorum, so a lone live agent closes immediately. `force` (the
/// human/CLI escape hatch) bypasses consensus. Closing an already-terminal room
/// → 409; a non-participant → 400.
async fn close_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LifecycleRequest>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Idempotency: an already-terminal room takes no further close (preserves the
    // 409-on-double-close contract).
    if matches!(room.state, RoomState::Closed | RoomState::Archived) {
        return Err(ApiError::Conflict(format!(
            "cannot close from {}",
            room.state.as_str()
        )));
    }

    let closer = state
        .storage
        .get_participant_by_instance(
            &id,
            &effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    let now = OffsetDateTime::now_utc();

    // Human-only escape hatch: close unilaterally, no vote.
    if req.force {
        close_now(&state, &id, room.state, now).await?;
        return Ok(Json(close_response("closed", RoomState::Closed, None)));
    }

    // Record the vote; voting also proves the voter is live.
    state.storage.touch_last_poll(&closer.handle, now).await?;
    state
        .storage
        .set_close_vote(&closer.handle, Some(now))
        .await?;

    // Count votes among LIVE participants only (a ghost neither blocks a close nor
    // counts toward quorum — symmetric with `counterpart_is_stale`'s `>` boundary).
    let participants = state.storage.list_participants(&id).await?;
    let live = participants
        .iter()
        .filter(|p| now - p.last_poll_at <= lifecycle::GHOST_AFTER)
        .count();
    let votes = participants
        .iter()
        .filter(|p| p.wants_close_at.is_some() && now - p.last_poll_at <= lifecycle::GHOST_AFTER)
        .count();
    let needed = room.config.close_quorum.needed(live);

    if votes >= needed {
        close_now(&state, &id, room.state, now).await?;
        Ok(Json(close_response("closed", RoomState::Closed, None)))
    } else {
        // Wake any parked counterpart so its wait recomputes and surfaces
        // `close_proposed` immediately.
        state.hub.notify(&id);
        Ok(Json(close_response(
            "close_proposed",
            room.state,
            Some((votes as u32, needed as u32)),
        )))
    }
}

/// Apply the actual Active/Idle/Stale→Closed transition (CAS), clear all pending
/// close votes, and wake waiters. Shared by force-close and quorum-met close. A
/// lost CAS race is success iff the room ended up terminal anyway.
async fn close_now(
    state: &AppState,
    id: &str,
    from: RoomState,
    now: OffsetDateTime,
) -> Result<(), ApiError> {
    let to = lifecycle::transition(from, LifecycleEvent::Close)
        .map_err(|_| ApiError::Conflict(format!("cannot close from {}", from.as_str())))?;
    let changed = state
        .storage
        .update_room_state(id, from, to, now, None)
        .await?;
    if !changed {
        let cur = state
            .storage
            .get_room(id)
            .await?
            .ok_or(ApiError::NotFound)?;
        if !matches!(cur.state, RoomState::Closed | RoomState::Archived) {
            return Err(ApiError::Conflict(
                "room state changed concurrently; retry".into(),
            ));
        }
    }
    state.storage.clear_close_votes(id).await?;
    state.hub.notify(id);
    Ok(())
}

/// Build a close response. `state` is the room's state to report (the new
/// `closed`, or the unchanged current state for a pending proposal); `progress`
/// is `Some((votes, needed))` for a proposal, `None` for a completed close.
fn close_response(
    status: &str,
    state: RoomState,
    progress: Option<(u32, u32)>,
) -> LifecycleResponse {
    LifecycleResponse {
        state: state.as_str().to_string(),
        status: Some(status.to_string()),
        votes: progress.map(|(v, _)| v),
        needed: progress.map(|(_, n)| n),
    }
}

/// Explicitly pause a room (the durable park). The caller must be a participant.
/// Drives a `Pause` transition; the optional `reason` is recorded in the audit
/// row's `detail`. `Err` (e.g. already paused, or from `stale`) → 409.
async fn pause_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LifecycleRequest>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    let detail = req.reason.clone();
    apply_transition(&state, &id, &req, LifecycleEvent::Pause, detail.as_deref()).await
}

/// Explicitly wake a paused (or idle) room back to active. The caller must be a
/// participant. Drives a `Wake` transition; `Err` (e.g. already active) → 409.
async fn wake_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LifecycleRequest>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    apply_transition(&state, &id, &req, LifecycleEvent::Wake, None).await
}

/// Resolve the room + caller, apply `event` through the pure state machine, and
/// persist the transition with a CAS write. Shared by the close/pause/wake
/// endpoints. `detail` is recorded in the `events.detail` audit row (the pause
/// reason). Two failure modes both map to 409: an illegal transition from the
/// room's current state, and a CAS miss (the state changed under us).
async fn apply_transition(
    state: &AppState,
    id: &str,
    req: &LifecycleRequest,
    event: LifecycleEvent,
    detail: Option<&str>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    let room = state
        .storage
        .get_room(id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Lifecycle ops are participant-driven — uniform with send/signal/wait.
    state
        .storage
        .get_participant_by_instance(
            id,
            &effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    let to = lifecycle::transition(room.state, event).map_err(|_| {
        ApiError::Conflict(format!("cannot {event:?} from {}", room.state.as_str()))
    })?;

    let now = OffsetDateTime::now_utc();
    let changed = state
        .storage
        .update_room_state(id, room.state, to, now, detail)
        .await?;
    if !changed {
        return Err(ApiError::Conflict(
            "room state changed concurrently; retry".into(),
        ));
    }
    state.hub.notify(id);

    Ok(Json(LifecycleResponse {
        state: to.as_str().to_string(),
        status: None,
        votes: None,
        needed: None,
    }))
}

/// Reject a write (send/signal) when the room is not accepting traffic. A
/// `paused`/`closed`/`archived` room is non-writable: a `paused` room is waiting
/// on an explicit wake, and `closed`/`archived` are terminal. `active`/`idle`/
/// `stale` accept writes (a `msg` revives idle/stale → active downstream).
fn reject_unless_writable(room: &Room) -> Result<(), ApiError> {
    match room.state {
        RoomState::Active | RoomState::Idle | RoomState::Stale => Ok(()),
        RoomState::Paused | RoomState::Closed | RoomState::Archived => {
            Err(ApiError::Conflict(format!(
                "room is {}; it is not accepting messages",
                room.state.as_str()
            )))
        }
    }
}

/// Long-poll for the next message addressed to the caller (or broadcast). The
/// caller is resolved from the `(repo, model, cwd)` tuple (400 if not a
/// participant). Blocks up to the server cap, then returns `paused_by_timeout`.
async fn wait_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(req): Query<WaitRequest>,
) -> Result<Json<WaitResponse>, ApiError> {
    let room = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let caller = state
        .storage
        .get_participant_by_instance(
            &id,
            &effective_instance(&req.repo, &req.model, &req.cwd, &req.instance),
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    // State entry gate: a paused/closed/archived room never long-polls, but it
    // must still DRAIN. A zero-cap claim delivers any already-unread message
    // (one per call) before reporting the terminal status — otherwise closing or
    // pausing a room strands every message a counterpart posted but the caller
    // had not yet read (the "agents never see the latest messages" bug). The
    // delivered message carries a `room_state` hint so the receiver knows it
    // cannot simply reply. Only once the inbox is empty do we report the state.
    // Zero-cap `wait_for_message` never parks (it claims-then-checks-deadline),
    // so this stays a fast return and the slice-5b park-path invariant is intact.
    if matches!(
        room.state,
        RoomState::Paused | RoomState::Closed | RoomState::Archived
    ) {
        return Ok(Json(
            match wait_for_message(
                &state.storage,
                &state.hub,
                &id,
                &caller.handle,
                Duration::ZERO,
            )
            .await?
            {
                WaitOutcome::Message(m) => WaitResponse::Message {
                    message: message_view(&m)?,
                    surface_to_user: false,
                    retry_after: None,
                    room_state: Some(room.state.as_str().to_string()),
                },
                WaitOutcome::PausedByTimeout => WaitResponse::Timeout {
                    status: room.state.as_str().to_string(),
                    retry_after: None,
                },
            },
        ));
    }

    // Sole-participant gate: when no counterpart has joined yet, do not long-poll.
    // The opener silently blocking here — waiting for someone who has not even been
    // told the room id — is the original "it just polls, I have to hit escape" bug.
    // Return immediately with the non-terminal `awaiting_counterpart` so the agent
    // surfaces the room id to its user and ends its turn; it resumes the wait once
    // the other agent joins. Distinct from `counterpart_stale` (a counterpart
    // joined, then went silent past GHOST_AFTER). Carries no hint — there is no
    // counterpart to back off behind.
    if !has_counterpart(&state.storage, &id, &caller.handle).await? {
        return Ok(Json(WaitResponse::Timeout {
            status: "awaiting_counterpart".to_string(),
            retry_after: None,
        }));
    }

    // Decide the park duration up front from the counterpart's current signal —
    // ghost detection (slice 6c) layered on the polling backoff (slice 5b):
    //
    // - Active `waiting_user` (`active_sentinel_backoff` = `Some`): the
    //   counterpart explicitly said "I'm away consulting my user", so shorten the
    //   long-poll to the severity-scaled, time-decayed backoff and park. An
    //   explicit away-signal is NOT a ghost, so this case *suppresses* ghost
    //   detection (the locked 6c decision).
    // - Otherwise, if the counterpart (the other participant — 2-agent v1) has
    //   gone *silently* dark past `GHOST_AFTER`: stop waiting on it. A zero-cap
    //   `wait_for_message` still claims a ready message before checking the
    //   deadline (so a queued message is delivered), but never parks — a dark
    //   counterpart yields `counterpart_stale` at once rather than a full-cap
    //   timeout (AC #5).
    // - Otherwise park for the full cap.
    // The park is bounded by the server cap and, when the caller supplied one
    // (the MCP path), the per-call `max_wait_secs` — whichever is shorter. This
    // lets `cbc_wait` return before a client's tool-call timeout without changing
    // the server-wide cap the CLI relies on. The backoff branch tightens this
    // further (min with the backoff); the ghost/stale branch parks zero regardless.
    let full_cap = match req.max_wait_secs {
        Some(secs) => state.wait_cap.min(Duration::from_secs(secs as u64)),
        None => state.wait_cap,
    };
    // Which status a *timeout* (no message claimed) should report, decided up
    // front from the counterpart's state. Precedence: an active away-signal
    // (backoff) → a dark counterpart (`counterpart_stale`) → a pending close
    // proposal (`close_proposed`) → the plain `paused_by_timeout`. The stale and
    // close-proposed arms park zero (deliver a ready message at once, else report
    // the status immediately); the backoff arm shortens the park; the normal arm
    // parks the full cap.
    let backoff = active_sentinel_backoff(&state.storage, &id, &caller.handle).await?;
    let (outcome, timeout_status) = if let Some(secs) = backoff {
        // min(server cap, per-call max, backoff): the per-call cap (MCP path) must
        // still win when it is shorter than the backoff, or an MCP `cbc_wait`
        // would overshoot its cap behind a counterpart's away-signal.
        let cap = full_cap.min(Duration::from_secs(secs as u64));
        (
            wait_for_message(&state.storage, &state.hub, &id, &caller.handle, cap).await?,
            TimeoutStatus::PausedByTimeout,
        )
    } else if counterpart_is_stale(&state.storage, &id, &caller.handle).await? {
        (
            wait_for_message(
                &state.storage,
                &state.hub,
                &id,
                &caller.handle,
                Duration::ZERO,
            )
            .await?,
            TimeoutStatus::CounterpartStale,
        )
    } else if close_proposed(&state.storage, &id, &caller.handle).await? {
        (
            wait_for_message(
                &state.storage,
                &state.hub,
                &id,
                &caller.handle,
                Duration::ZERO,
            )
            .await?,
            TimeoutStatus::CloseProposed,
        )
    } else {
        (
            wait_for_message(&state.storage, &state.hub, &id, &caller.handle, full_cap).await?,
            TimeoutStatus::PausedByTimeout,
        )
    };

    // Re-derive the hint from the post-wake state (the slice-5b two-read
    // invariant): the *cap* above used the pre-park reading, but a pause can
    // begin or clear while parked, so the *response hint* is re-read now —
    // otherwise a sentinel delivered on wake would lose its hint, or a msg that
    // cleared the pause would carry a stale one. The `counterpart_stale` arm
    // deliberately carries no hint: the counterpart is gone, not paused, so there
    // is nothing to back off behind.
    //
    // Precedence: an explicit `waiting_user` sentinel (counterpart consulting its
    // human) is the more specific state and wins; otherwise fall back to the
    // inferred *busy* hint (counterpart read my latest and is composing a reply).
    // The `ghosted` arm below overrides this to `None`, so a busy hint never leaks
    // into a `counterpart_stale` response — which keeps ghost behaviour unchanged.
    let retry_after = match active_sentinel_backoff(&state.storage, &id, &caller.handle).await? {
        Some(secs) => Some(secs),
        None => counterpart_busy_backoff(&state.storage, &id, &caller.handle).await?,
    };

    Ok(Json(match outcome {
        WaitOutcome::Message(m) => {
            // Soft-cap signal, computed on read: the count of consecutive
            // autonomous turns at this delivery. Surface when it reaches the
            // (soft_cap − 1)th — pull the user in before the agents circle. The
            // count is read after the claim; rows are immutable and `seq`
            // monotonic, so the count at delivery equals the count at send.
            let consecutive = state.storage.consecutive_msg_count(&id, m.seq).await?;
            let surface_to_user = consecutive == room.config.soft_cap as i64 - 1;
            WaitResponse::Message {
                message: message_view(&m)?,
                surface_to_user,
                retry_after,
                room_state: None,
            }
        }
        WaitOutcome::PausedByTimeout => match timeout_status {
            // `counterpart_stale` and `close_proposed` carry no backoff hint: the
            // peer is gone, or it is the caller's turn to decide on the close —
            // neither is "stay quiet, a reply is coming".
            TimeoutStatus::CounterpartStale => WaitResponse::Timeout {
                status: "counterpart_stale".to_string(),
                retry_after: None,
            },
            TimeoutStatus::CloseProposed => WaitResponse::Timeout {
                status: "close_proposed".to_string(),
                retry_after: None,
            },
            TimeoutStatus::PausedByTimeout => WaitResponse::Timeout {
                status: "paused_by_timeout".to_string(),
                retry_after,
            },
        },
    }))
}

/// The status a `wait` timeout (no message claimed) should report, chosen from
/// the counterpart's state before the park.
#[derive(Clone, Copy)]
enum TimeoutStatus {
    PausedByTimeout,
    CounterpartStale,
    CloseProposed,
}

/// True when at least one participant other than `handle` has joined the room
/// (2-agent v1: the counterpart exists). False for a lone opener, which gates the
/// `awaiting_counterpart` short-circuit in the wait path.
async fn has_counterpart(
    storage: &Storage,
    room_id: &str,
    handle: &str,
) -> Result<bool, StorageError> {
    Ok(storage
        .list_participants(room_id)
        .await?
        .iter()
        .any(|p| p.handle != handle))
}

/// True when a *live* other participant has voted to close (consensus close) and
/// the caller has not yet voted — i.e. there is a pending close proposal awaiting
/// the caller's decision. Counts only live voters (a ghost's stale vote never
/// proposes), symmetric with `counterpart_is_stale`'s liveness boundary. When the
/// caller has already voted there is nothing to surface to it (it is the one
/// waiting on the others), so this is `false`.
async fn close_proposed(
    storage: &Storage,
    room_id: &str,
    handle: &str,
) -> Result<bool, StorageError> {
    let now = OffsetDateTime::now_utc();
    let participants = storage.list_participants(room_id).await?;
    let caller_voted = participants
        .iter()
        .any(|p| p.handle == handle && p.wants_close_at.is_some());
    if caller_voted {
        return Ok(false);
    }
    Ok(participants.iter().any(|p| {
        p.handle != handle
            && p.wants_close_at.is_some()
            && now - p.last_poll_at <= lifecycle::GHOST_AFTER
    }))
}

/// True when the caller has at least one other participant AND *every* other has
/// gone dark (no poll within `GHOST_AFTER`) — i.e. there is no live counterpart
/// left to wait on. A room with no other participant yet is never stale (returns
/// `false`), so a lone waiter parks normally rather than being told an absent
/// other side is gone. Uses the strict `>` boundary, matching
/// `lifecycle::no_live_poller`.
///
/// `.all` (not `.any`) over the others is load-bearing for the 2-live-max +
/// ghosts model: an agent that churned identity or died leaves an inert ghost
/// row behind while its live replacement keeps polling. A single stale ghost must
/// NOT make the wait short-circuit to `counterpart_stale` (which tells the caller
/// to stop polling) while a live counterpart is still here. Only when *all* of
/// them are dark is the conversation genuinely over.
async fn counterpart_is_stale(
    storage: &Storage,
    room_id: &str,
    handle: &str,
) -> Result<bool, StorageError> {
    let now = OffsetDateTime::now_utc();
    let others: Vec<_> = storage
        .list_participants(room_id)
        .await?
        .into_iter()
        .filter(|p| p.handle != handle)
        .collect();
    Ok(!others.is_empty()
        && others
            .iter()
            .all(|p| now - p.last_poll_at > lifecycle::GHOST_AFTER))
}

/// The polling backoff (seconds) implied by the counterpart's *current* state,
/// or `None` when no pause is active. Reads the latest row in the room from
/// anyone but `handle` (two-agent v1) of *any* type: only a `waiting_user`
/// counts, so a later `msg` from that sender self-supersedes the pause and
/// clears the backoff. A corrupt sentinel missing its severity yields `None`
/// rather than panicking the wait path.
///
/// Ghost-robustness (2-live-max model): this reads "latest from any other" and
/// is left intentionally unchanged. Under seq-monotonicity a live replacement
/// always posts *after* the churned/dead agent's final message, so a dead ghost
/// is frozen seq-behind and cannot become the latest-from-other (nor mask a live
/// sentinel) once the replacement has spoken. The only residual is a transient
/// false-pause if the replacement is silent while the ghost's old `waiting_user`
/// is briefly the latest — self-decaying, and it only ever over-parks (never a
/// `counterpart_stale` abort, since a live counterpart still exists). Holding
/// this invariant is what keeps the change surgical; true 3-way live coordination
/// is a future version.
async fn active_sentinel_backoff(
    storage: &Storage,
    room_id: &str,
    handle: &str,
) -> Result<Option<u32>, StorageError> {
    Ok(
        match storage.latest_message_from_other(room_id, handle).await? {
            Some(m) if m.msg_type == MessageType::WaitingUser => m.severity.map(|sev| {
                let elapsed = (OffsetDateTime::now_utc() - m.created_at).whole_seconds();
                backoff_secs(sev, elapsed)
            }),
            _ => None,
        },
    )
}

/// The polling backoff (seconds) implied by a *busy* counterpart — one that has
/// **read my latest message and not yet replied** — or `None` when the ball is
/// not in the counterpart's court. This is the silent-compose case the
/// `waiting_user` sentinel can't cover: an agent composing a long autonomous
/// reply emits no signal, but the server can still infer it is engaged from
/// `last_read_seq`, which is durably advanced when a message is claimed and does
/// not decay with polling.
///
/// Busy ⟺ the room's latest message is the caller's *and* the counterpart's read
/// cursor has reached it. It self-clears the moment the counterpart sends (the
/// latest message is then theirs) or the caller sends something new the
/// counterpart has not yet read. Reuses the `waiting_user` decay curve at a fixed
/// `Med` severity (no agent-supplied severity exists for an inferred state),
/// measuring elapsed from the unanswered message's `created_at`, so the backoff
/// grows the longer the counterpart stays silent. Unlike `active_sentinel_backoff`
/// this never shortens the park — the caller only attaches it as the response
/// hint, so a busy wait keeps its full long-poll (delivering instantly on the
/// reply) while telling the waiter to space out its empty re-polls.
async fn counterpart_busy_backoff(
    storage: &Storage,
    room_id: &str,
    handle: &str,
) -> Result<Option<u32>, StorageError> {
    let Some(latest) = storage.room_latest_message(room_id).await? else {
        return Ok(None);
    };
    // The counterpart spoke last (or nobody has) — the ball is in my court, not
    // theirs. Not busy. In 2-agent v1 this is redundant with the read-cursor guard
    // below (a participant can never claim its own message, so a non-caller latest
    // sender always fails the cursor check too); it stays as an explicit statement
    // of intent and a safeguard if claim semantics or participant count change.
    if latest.sender != handle {
        return Ok(None);
    }
    // Has *any* other participant read my latest? We ask "anyone" rather than
    // picking "the counterpart": `list_participants` is `ORDER BY joined_at`, so a
    // `.find(first non-caller)` deterministically selects the OLDEST row — which,
    // in the 2-live-max + ghosts model, is exactly the dead ghost (it joined
    // before its live replacement). Its frozen low cursor would mask the live
    // replacement that actually read my message. `.any` over the read cursors
    // sidesteps per-row selection and is correct for the live pair. Note we cannot
    // instead select by poll-freshness: a counterpart silently composing a long
    // reply is also not polling, so freshness can't tell it from a ghost — the
    // read cursor is the durable signal.
    //
    // Known v1 limitation (unchanged): if I send a *second* message while the
    // counterpart is still composing a reply to the first, `last_read_seq` falls
    // behind my new latest and busy reads as false until it catches up. Accepted
    // residual of the ghosts model: a dead ghost whose frozen cursor happens to be
    // `>= latest.seq` yields a false busy *hint* — never a `counterpart_stale`
    // abort, self-decaying, and hint-only (it never shortens the park).
    let participants = storage.list_participants(room_id).await?;
    let someone_read_my_latest = participants
        .iter()
        .any(|p| p.handle != handle && p.last_read_seq >= latest.seq);
    if !someone_read_my_latest {
        return Ok(None);
    }
    let elapsed = (OffsetDateTime::now_utc() - latest.created_at).whole_seconds();
    Ok(Some(backoff_secs(Severity::Med, elapsed)))
}

fn message_view(m: &Message) -> Result<MessageView, ApiError> {
    Ok(MessageView {
        seq: m.seq,
        from: m.sender.clone(),
        to: m.recipient.clone(),
        body: m.body.clone(),
        created_at: m
            .created_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        msg_type: m.msg_type.as_str().to_string(),
        severity: m.severity.map(|s| s.as_str().to_string()),
        question_text: m.question_text.clone(),
    })
}

/// An effectively-infinite stream of 4-char lowercase hex sess candidates.
fn rand_sess_candidates() -> impl Iterator<Item = String> {
    use rand::Rng;
    std::iter::repeat_with(|| {
        let n: u16 = rand::thread_rng().gen();
        format!("{n:04x}")
    })
}

fn participant_view(p: &Participant) -> Result<ParticipantView, ApiError> {
    Ok(ParticipantView {
        handle: p.handle.clone(),
        repo: p.repo.clone(),
        model: p.model.clone(),
        cwd: p.cwd.clone(),
        joined_at: p
            .joined_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        nickname: p.nickname.clone(),
    })
}

/// Trim a supplied nickname, mapping blank/whitespace-only to `None` so it is
/// treated as "not supplied" (neither sets nor clears the stored value).
fn normalize_nickname(nickname: Option<String>) -> Option<String> {
    nickname
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

fn room_to_status(room: &Room, participants: &[Participant]) -> Result<RoomStatus, ApiError> {
    let participants = participants
        .iter()
        .map(participant_view)
        .collect::<Result<Vec<_>, ApiError>>()?;

    Ok(RoomStatus {
        id: room.id.clone(),
        subject: room.subject.clone(),
        state: room.state.as_str().to_string(),
        started_at: room
            .started_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        last_activity_at: room
            .last_activity_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        participants,
    })
}

fn room_summary(row: &RoomSummaryRow) -> Result<RoomSummary, ApiError> {
    Ok(RoomSummary {
        room_id: row.room.id.clone(),
        state: row.room.state.as_str().to_string(),
        subject: row.room.subject.clone(),
        last_activity_at: row
            .room
            .last_activity_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        participant_count: row.participant_count,
    })
}

fn room_to_transcript(
    room: &Room,
    participants: &[Participant],
    messages: &[Message],
    hard_cap_count: i64,
    soft_cap_consecutive: i64,
) -> Result<RoomTranscript, ApiError> {
    let participants = participants
        .iter()
        .map(participant_view)
        .collect::<Result<Vec<_>, ApiError>>()?;
    let messages = messages
        .iter()
        .map(message_view)
        .collect::<Result<Vec<_>, ApiError>>()?;

    Ok(RoomTranscript {
        id: room.id.clone(),
        subject: room.subject.clone(),
        state: room.state.as_str().to_string(),
        started_at: room
            .started_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        last_activity_at: room
            .last_activity_at
            .format(&Rfc3339)
            .map_err(|e| ApiError::Internal(e.to_string()))?,
        hard_cap: room.config.hard_cap,
        soft_cap: room.config.soft_cap,
        hard_cap_count,
        soft_cap_consecutive,
        participants,
        messages,
    })
}

/// Handler error type. Maps cleanly onto HTTP status codes with a uniform
/// `ErrorEnvelope` body.
#[derive(Debug)]
enum ApiError {
    NotFound,
    BadRequest(String),
    /// The request conflicts with the room's current state and retrying won't
    /// clear it — e.g. the hard cap is reached (the user must raise the cap or
    /// close the room). Maps to 409, not 429: it is not a transient rate limit.
    Conflict(String),
    Internal(String),
}

impl From<StorageError> for ApiError {
    fn from(e: StorageError) -> Self {
        ApiError::Internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "room not found".to_string()),
            // The message is caller-facing and safe (our own text, no internals).
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            // Likewise caller-facing and safe.
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message),
            ApiError::Internal(detail) => {
                // Log the real cause server-side; never leak DB/internal text to
                // the caller (table names, constraints, file paths, etc.).
                tracing::error!(error = %detail, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(ErrorEnvelope::new(message))).into_response()
    }
}
