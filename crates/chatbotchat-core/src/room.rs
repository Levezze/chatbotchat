use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Lifecycle state of a room. Slice 1 only exercises `Active`; the remaining
/// variants are defined now so the schema and serialization are stable, but
/// transitions between them land in slice 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomState {
    Active,
    Idle,
    Paused,
    Stale,
    Closed,
    Archived,
}

impl RoomState {
    pub fn as_str(self) -> &'static str {
        match self {
            RoomState::Active => "active",
            RoomState::Idle => "idle",
            RoomState::Paused => "paused",
            RoomState::Stale => "stale",
            RoomState::Closed => "closed",
            RoomState::Archived => "archived",
        }
    }
}

/// Per-room caps. Defaults come from the locked design (hard cap 10 messages,
/// soft cap 4 consecutive without human input). Enforcement is slice 4; here we
/// only need the value to persist and round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomConfig {
    pub hard_cap: u32,
    pub soft_cap: u32,
}

impl Default for RoomConfig {
    fn default() -> Self {
        RoomConfig {
            hard_cap: 10,
            soft_cap: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub subject: String,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
    pub state: RoomState,
    /// When the room last entered `state`. Anchors the `stale`/`closed` -> `archived`
    /// window. Set to `started_at` at creation.
    #[serde(with = "time::serde::rfc3339")]
    pub state_changed_at: OffsetDateTime,
    pub config: RoomConfig,
    pub prev_room_id: Option<String>,
}
