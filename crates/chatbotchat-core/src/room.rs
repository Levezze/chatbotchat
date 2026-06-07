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

/// How many *live* participants must vote to close before a room actually
/// closes (consensus close). `All` (the default, and exact for the 2-agent
/// world) requires every live participant; `Majority` requires strictly more
/// than half — reserved for the future N-way version. Dead/ghost rows never
/// count toward the denominator, so a lone live agent whose counterpart has gone
/// dark closes immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseQuorum {
    #[default]
    All,
    Majority,
}

impl CloseQuorum {
    /// Votes needed to close, given the number of live participants. Never
    /// exceeds `live` and is at least 1 (a lone live agent can always close).
    pub fn needed(self, live: usize) -> usize {
        let n = match self {
            CloseQuorum::All => live,
            CloseQuorum::Majority => live / 2 + 1,
        };
        n.max(1)
    }
}

/// Per-room caps and policy. Cap defaults come from the locked design (hard cap
/// 10 messages, soft cap 4 consecutive without human input); `close_quorum`
/// defaults to `All`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomConfig {
    pub hard_cap: u32,
    pub soft_cap: u32,
    /// Close-consensus policy. `#[serde(default)]` so rooms persisted before this
    /// field existed deserialize to `All`.
    #[serde(default)]
    pub close_quorum: CloseQuorum,
}

impl Default for RoomConfig {
    fn default() -> Self {
        RoomConfig {
            hard_cap: 10,
            soft_cap: 4,
            close_quorum: CloseQuorum::All,
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
