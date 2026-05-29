use crate::identity::{derive_handle, HandleOutcome, JoinIdentity};
use crate::ids;
use crate::message::Message;
use crate::participant::Participant;
use crate::room::{Room, RoomConfig, RoomState};
use crate::storage::{Storage, StorageError};
use crate::waiter::{wait_for_message, Hub, WaitOutcome};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chatbotchat_protocol::{
    ErrorEnvelope, JoinRoomRequest, JoinRoomResponse, MessageView, OpenRoomRequest,
    OpenRoomResponse, ParticipantView, RoomStatus, SendMessageRequest, SendMessageResponse,
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
}

/// Build the application router. This is the integration seam exercised both by
/// in-process `oneshot` tests and the real daemon binary.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rooms", post(open_room))
        .route("/rooms/{id}", get(get_room))
        .route("/rooms/{id}/join", post(join_room))
        .route("/rooms/{id}/messages", post(send_message))
        .route("/rooms/{id}/wait", get(wait_room))
        .with_state(state)
}

async fn open_room(
    State(state): State<AppState>,
    Json(req): Json<OpenRoomRequest>,
) -> Result<(StatusCode, Json<OpenRoomResponse>), ApiError> {
    let now = OffsetDateTime::now_utc();
    let base = ids::room_id(&req.subject, now);

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
            config: RoomConfig::default(),
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
            last_read_seq: 0,
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
    let _ = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let sender = state
        .storage
        .get_participant_by_tuple(&id, &req.repo, &req.model, &req.cwd)
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    let now = OffsetDateTime::now_utc();
    let msg = state
        .storage
        .create_message(&id, &sender.handle, req.to.as_deref(), &req.body, now)
        .await?;
    state.hub.notify(&id);

    Ok((
        StatusCode::CREATED,
        Json(SendMessageResponse { seq: msg.seq }),
    ))
}

/// Long-poll for the next message addressed to the caller (or broadcast). The
/// caller is resolved from the `(repo, model, cwd)` tuple (400 if not a
/// participant). Blocks up to the server cap, then returns `paused_by_timeout`.
async fn wait_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(req): Query<WaitRequest>,
) -> Result<Json<WaitResponse>, ApiError> {
    let _ = state
        .storage
        .get_room(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let caller = state
        .storage
        .get_participant_by_tuple(&id, &req.repo, &req.model, &req.cwd)
        .await?
        .ok_or_else(|| ApiError::BadRequest("not a participant of this room; join first".into()))?;

    let outcome = wait_for_message(
        &state.storage,
        &state.hub,
        &id,
        &caller.handle,
        caller.last_read_seq,
        state.wait_cap,
    )
    .await?;

    Ok(Json(match outcome {
        WaitOutcome::Message(m) => WaitResponse::Message {
            message: message_view(&m)?,
        },
        WaitOutcome::PausedByTimeout => WaitResponse::Timeout {
            status: "paused_by_timeout".to_string(),
        },
    }))
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
