use crate::ids;
use crate::room::{Room, RoomConfig, RoomState};
use crate::storage::{Storage, StorageError};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chatbotchat_protocol::{ErrorEnvelope, OpenRoomRequest, OpenRoomResponse, RoomStatus};
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
        .with_state(state)
}

async fn open_room(
    State(state): State<AppState>,
    Json(req): Json<OpenRoomRequest>,
) -> Result<(StatusCode, Json<OpenRoomResponse>), ApiError> {
    let now = OffsetDateTime::now_utc();
    let id = ids::room_id(&req.subject, now);

    let room = Room {
        id: id.clone(),
        subject: req.subject,
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        config: RoomConfig::default(),
        prev_room_id: None,
    };

    state.storage.create_room(&room).await?;

    let resp = OpenRoomResponse {
        share_line: ids::share_line(&id),
        room_id: id,
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
    Ok(Json(room_to_status(&room)?))
}

fn room_to_status(room: &Room) -> Result<RoomStatus, ApiError> {
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
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(ErrorEnvelope::new(message))).into_response()
    }
}
