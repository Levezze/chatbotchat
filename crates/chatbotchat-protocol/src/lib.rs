use serde::{Deserialize, Serialize};

/// Request body for `POST /rooms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRoomRequest {
    pub subject: String,
}

/// Response body for `POST /rooms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRoomResponse {
    pub room_id: String,
    /// The line the user pastes into the other agent's session to join.
    pub share_line: String,
}

/// Request body for `POST /rooms/:id/join`. `repo` and `cwd` are self-reported
/// by the caller (auto-detected from the shell / MCP working directory); the
/// `(room_id, repo, model, cwd)` tuple keys idempotent identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRoomRequest {
    pub repo: String,
    pub model: String,
    pub cwd: String,
}

/// Wire view of a single message. Placeholder in slice 2 — messages land in
/// slice 3, where this gains fields additively without re-touching
/// `JoinRoomResponse`. Until then `recent_messages` is always empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {}

/// Response body for `POST /rooms/:id/join`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRoomResponse {
    pub handle: String,
    /// `true` when an existing participant matching the tuple was returned;
    /// `false` when a fresh handle was minted. Lets the caller distinguish
    /// resuming an identity from joining anew.
    pub resumed: bool,
    pub room_state: String,
    pub recent_messages: Vec<MessageView>,
}

/// Wire view of a room participant, surfaced in `RoomStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantView {
    pub handle: String,
    pub repo: String,
    pub model: String,
    pub cwd: String,
    pub joined_at: String,
}

/// Response body for `GET /rooms/:id`. The wire-facing view of a room's status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomStatus {
    pub id: String,
    pub subject: String,
    pub state: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub participants: Vec<ParticipantView>,
}

/// Uniform error body for failed requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: String,
}

impl ErrorEnvelope {
    pub fn new(message: impl Into<String>) -> Self {
        ErrorEnvelope {
            error: message.into(),
        }
    }
}
