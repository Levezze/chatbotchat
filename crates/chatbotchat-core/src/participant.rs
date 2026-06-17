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
    /// Pending vote to close the room (consensus close). `Some(ts)` means this
    /// participant has called `cbc_close`; the room closes once a quorum of live
    /// participants have voted. Cleared (set to `None`) only for the SENDER when
    /// *they* send a conversational message — a deterministic "I'll keep talking",
    /// retracting just their own vote, not the counterpart's (clearing room-wide
    /// here was the consensus-close deadlock). `None` means no pending vote.
    pub wants_close_at: Option<OffsetDateTime>,
    /// Pending vote to extend the message cap (consensus extend, `cbc_extend`).
    /// `Some(ts)` means this participant voted to raise the hard cap by +20; the
    /// cap bumps once a quorum of live participants have voted, at which point all
    /// extend votes clear. Like `wants_close_at`, a conversational message clears it
    /// only for the SENDER — a landed message means the room had cap room, so the
    /// sender did not need the extend (a correct implicit self-decline; a send
    /// refused at the cap wall is a 409 and never lands, so it cannot clear). `None`
    /// means no pending vote.
    pub wants_extend_at: Option<OffsetDateTime>,
}
