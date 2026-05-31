use crate::identity::{derive_handle, HandleOutcome, JoinIdentity};
use crate::ids;
use crate::lifecycle::{self, LifecycleEvent};
use crate::message::{Message, MessageType, Severity};
use crate::participant::Participant;
use crate::room::{Room, RoomConfig, RoomState};
use crate::storage::{Storage, StorageError};
use crate::waiter::{backoff_secs, wait_for_message, Hub, WaitOutcome};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chatbotchat_protocol::{
    ErrorEnvelope, JoinRoomRequest, JoinRoomResponse, LifecycleRequest, LifecycleResponse,
    MessageView, OpenRoomRequest, OpenRoomResponse, ParticipantView, RoomStatus,
    SendMessageRequest, SendMessageResponse, SignalRequest, SignalResponse, WaitRequest,
    WaitResponse,
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
}

/// Build the application router. This is the integration seam exercised both by
/// in-process `oneshot` tests and the real daemon binary.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rooms", post(open_room))
        .route("/rooms/{id}", get(get_room))
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
            prev_room_id: None,
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

    // The room's recent messages (log view), surfaced to the joiner.
    let recent = recent_message_views(&state.storage, &id).await?;

    // A newly-minted participant starts reading from "now": its cursor is the
    // room's current high-water seq, so `wait` only delivers post-join traffic
    // (the pre-join backlog lives in `recent` above, not the inbox).
    let start_seq = state.storage.current_seq(&id).await?;

    let ident = JoinIdentity {
        repo: req.repo,
        model: req.model,
        cwd: req.cwd,
    };

    // Fast path: an existing matching participant resumes its handle.
    if let Some(p) = state
        .storage
        .get_participant_by_tuple(&id, &ident.repo, &ident.model, &ident.cwd)
        .await?
    {
        return Ok(join_response(p.handle, true, &room, recent));
    }

    const MAX_ATTEMPTS: u32 = 64;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let existing = state.storage.list_participants(&id).await?;
        let handle = match derive_handle(&ident, &existing, rand_sess_candidates()) {
            // Reused shouldn't occur after the fast-path lookup, but if a
            // concurrent join inserted the same tuple, honor it as resumed.
            HandleOutcome::Reused(h) => return Ok(join_response(h, true, &room, recent.clone())),
            HandleOutcome::Created(h) => h,
        };

        let now = OffsetDateTime::now_utc();
        let participant = Participant {
            handle: handle.clone(),
            room_id: id.clone(),
            repo: ident.repo.clone(),
            model: ident.model.clone(),
            cwd: ident.cwd.clone(),
            joined_at: now,
            last_poll_at: now,
            last_read_seq: start_seq,
        };

        match state.storage.create_participant(&participant).await {
            Ok(()) => return Ok(join_response(handle, false, &room, recent.clone())),
            Err(e) if e.is_unique_violation() && attempt < MAX_ATTEMPTS => {
                // Either a concurrent join took our tuple (refetch → resume) or
                // the random sess collided (refetch returns None → retry).
                if let Some(p) = state
                    .storage
                    .get_participant_by_tuple(&id, &ident.repo, &ident.model, &ident.cwd)
                    .await?
                {
                    return Ok(join_response(p.handle, true, &room, recent.clone()));
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
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

/// The most recent messages in a room, as wire views (oldest-first).
const RECENT_MESSAGE_LIMIT: i64 = 50;

async fn recent_message_views(
    storage: &Storage,
    room_id: &str,
) -> Result<Vec<MessageView>, ApiError> {
    let msgs = storage
        .recent_messages(room_id, RECENT_MESSAGE_LIMIT)
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
    // additive pre-check, separate from `create_message_capped`'s atomic cap gate.
    reject_unless_writable(&room)?;

    let sender = state
        .storage
        .get_participant_by_tuple(&id, &req.repo, &req.model, &req.cwd)
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
        .get_participant_by_tuple(&id, &req.repo, &req.model, &req.cwd)
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
    // current state *before* persisting the sentinel — a `stale`-room 409 must
    // not leave an orphaned blocker message behind.
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

    // Apply the pause, recording the reason in the transition's audit row. (A
    // concurrent transition can make the CAS a no-op; the sentinel still stands.)
    if let Some(to) = pause_to {
        state
            .storage
            .update_room_state(&id, room.state, to, now, req.reason.as_deref())
            .await?;
    }

    state.hub.notify(&id);

    Ok((StatusCode::CREATED, Json(SignalResponse { seq: msg.seq })))
}

/// Explicitly close a room. The caller must be a participant. Drives a `Close`
/// lifecycle transition; `Err` (e.g. already closed) → 409.
async fn close_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LifecycleRequest>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    apply_transition(&state, &id, &req, LifecycleEvent::Close, None).await
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
        .get_participant_by_tuple(id, &req.repo, &req.model, &req.cwd)
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
        .get_participant_by_tuple(&id, &req.repo, &req.model, &req.cwd)
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    // State entry gate: a paused/closed/archived room never long-polls. Return
    // its state immediately so the caller stops waiting — only an explicit wake
    // (paused) or a human (closed/archived) clears it, so polling can't help and
    // there is no retry_after hint. This early return never enters the park path,
    // so the slice-5b two-read backoff invariant below is untouched.
    if matches!(
        room.state,
        RoomState::Paused | RoomState::Closed | RoomState::Archived
    ) {
        return Ok(Json(WaitResponse::Timeout {
            status: room.state.as_str().to_string(),
            retry_after: None,
        }));
    }

    // Polling backoff (slice 5b). If the counterpart is parked behind an active
    // `waiting_user` sentinel, shorten this long-poll to the severity-scaled,
    // time-decayed backoff and hand that hint back so the LLM consumer — which
    // has no `sleep()` of its own — stays quiet.
    //
    // The state is read twice on purpose. The park duration can only be decided
    // up front, so the *cap* uses the pre-park reading. But a pause can clear (or
    // a new one can begin) while we are parked, so the *response hint* is
    // re-derived from the post-wake state — otherwise a sentinel delivered on
    // wake would lose its hint, or a msg that cleared the pause would carry a
    // stale one. `wait_cap` is the absolute ceiling; a sentinel only ever
    // shortens it (in prod wait_cap = 600s ≥ backoff ≤ 60s, so the min is always
    // the backoff; the min exists so the `with_wait_cap` test seam keeps parking
    // tests fast).
    let effective_cap = match active_sentinel_backoff(&state.storage, &id, &caller.handle).await? {
        Some(secs) => state.wait_cap.min(Duration::from_secs(secs as u64)),
        None => state.wait_cap,
    };

    let outcome = wait_for_message(
        &state.storage,
        &state.hub,
        &id,
        &caller.handle,
        effective_cap,
    )
    .await?;

    let retry_after = active_sentinel_backoff(&state.storage, &id, &caller.handle).await?;

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
            }
        }
        WaitOutcome::PausedByTimeout => WaitResponse::Timeout {
            status: "paused_by_timeout".to_string(),
            retry_after,
        },
    }))
}

/// The polling backoff (seconds) implied by the counterpart's *current* state,
/// or `None` when no pause is active. Reads the latest row in the room from
/// anyone but `handle` (two-agent v1) of *any* type: only a `waiting_user`
/// counts, so a later `msg` from that sender self-supersedes the pause and
/// clears the backoff. A corrupt sentinel missing its severity yields `None`
/// rather than panicking the wait path.
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

fn room_to_status(room: &Room, participants: &[Participant]) -> Result<RoomStatus, ApiError> {
    let participants = participants
        .iter()
        .map(|p| {
            Ok(ParticipantView {
                handle: p.handle.clone(),
                repo: p.repo.clone(),
                model: p.model.clone(),
                cwd: p.cwd.clone(),
                joined_at: p
                    .joined_at
                    .format(&Rfc3339)
                    .map_err(|e| ApiError::Internal(e.to_string()))?,
            })
        })
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
