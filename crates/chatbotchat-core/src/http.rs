use crate::identity::{derive_handle, HandleOutcome, JoinIdentity};
use crate::ids;
use crate::participant::Participant;
use crate::room::{Room, RoomConfig, RoomState};
use crate::storage::{Storage, StorageError};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chatbotchat_protocol::{
    ErrorEnvelope, JoinRoomRequest, JoinRoomResponse, OpenRoomRequest, OpenRoomResponse,
    ParticipantView, RoomStatus,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Shared state injected into every handler. Cloneable: `Storage` wraps a
/// connection pool behind an `Arc`-like handle.
#[derive(Clone)]
pub struct AppState {
    storage: Storage,
}

impl AppState {
    pub fn new(storage: Storage) -> Self {
        AppState { storage }
    }
}

/// Build the application router. This is the integration seam exercised both by
/// in-process `oneshot` tests and the real daemon binary.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rooms", post(open_room))
        .route("/rooms/{id}", get(get_room))
        .route("/rooms/{id}/join", post(join_room))
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
        return Ok(join_response(p.handle, true, &room));
    }

    const MAX_ATTEMPTS: u32 = 64;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let existing = state.storage.list_participants(&id).await?;
        let handle = match derive_handle(&ident, &existing, rand_sess_candidates()) {
            // Reused shouldn't occur after the fast-path lookup, but if a
            // concurrent join inserted the same tuple, honor it as resumed.
            HandleOutcome::Reused(h) => return Ok(join_response(h, true, &room)),
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
        };

        match state.storage.create_participant(&participant).await {
            Ok(()) => return Ok(join_response(handle, false, &room)),
            Err(e) if e.is_unique_violation() && attempt < MAX_ATTEMPTS => {
                // Either a concurrent join took our tuple (refetch → resume) or
                // the random sess collided (refetch returns None → retry).
                if let Some(p) = state
                    .storage
                    .get_participant_by_tuple(&id, &ident.repo, &ident.model, &ident.cwd)
                    .await?
                {
                    return Ok(join_response(p.handle, true, &room));
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
) -> (StatusCode, Json<JoinRoomResponse>) {
    (
        StatusCode::CREATED,
        Json(JoinRoomResponse {
            handle,
            resumed,
            room_state: room.state.as_str().to_string(),
            // Messages land in slice 3; until then the room has none.
            recent_messages: Vec::new(),
        }),
    )
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
