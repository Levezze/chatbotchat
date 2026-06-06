use time::OffsetDateTime;

/// A registered participant in a room. The handle is minted on first join and
/// is stable for a given `(room_id, instance)` — rejoining with the same
/// instance returns the same handle (idempotent identity). `repo`/`model`/`cwd`
/// are descriptive attributes (they also form the handle prefix); they are no
/// longer part of the identity key, so two distinct agents sharing all three are
/// told apart by their `instance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub handle: String,
    pub room_id: String,
    pub repo: String,
    pub model: String,
    pub cwd: String,
    /// Identity key within a room: an opaque per-agent token (explicit `as`
    /// label, harness session id, or a per-process nonce — resolved client-side;
    /// the server synthesizes one from the tuple for legacy callers that send
    /// none). `UNIQUE (room_id, instance)`.
    pub instance: String,
    pub joined_at: OffsetDateTime,
    pub last_poll_at: OffsetDateTime,
    /// Long-poll read cursor: the `seq` of the highest message this participant
    /// has consumed via `wait`. Starts at 0 (no messages read).
    pub last_read_seq: i64,
    /// Optional human-friendly display label, distinct from identity. Never
    /// affects handle derivation, routing, or `sender` — purely a display alias
    /// (e.g. "concierge-agent") so humans can tell rows apart. `None` renders by
    /// handle.
    pub nickname: Option<String>,
}
