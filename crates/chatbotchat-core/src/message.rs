use time::OffsetDateTime;

/// A single `msg` posted to a room. `seq` is the monotonic primary key (SQLite
/// rowid) and doubles as the long-poll read cursor. `recipient == None` is a
/// broadcast to all participants; `Some(handle)` is targeted delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub seq: i64,
    pub room_id: String,
    pub sender: String,
    pub recipient: Option<String>,
    pub body: String,
    pub created_at: OffsetDateTime,
}
