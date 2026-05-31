use crate::room::RoomState;
use time::OffsetDateTime;

/// What an [`Event`] row records. Every lifecycle transition writes one row; a
/// transition *into* `archived` is recorded as [`EventKind::Archive`] (the
/// `on_archive` hook seam) rather than [`EventKind::Transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Transition,
    Archive,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Transition => "transition",
            EventKind::Archive => "archive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "transition" => EventKind::Transition,
            "archive" => EventKind::Archive,
            _ => return None,
        })
    }
}

/// A row from the append-only `events` log. Read-only history; never mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    pub room_id: String,
    pub kind: EventKind,
    pub from_state: Option<RoomState>,
    pub to_state: Option<RoomState>,
    pub detail: Option<String>,
    pub at: OffsetDateTime,
}
