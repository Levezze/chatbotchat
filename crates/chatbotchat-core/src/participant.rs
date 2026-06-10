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
    /// participants have voted. Cleared (set to `None`) for everyone when any
    /// participant sends a conversational message — a deterministic "continue".
    /// `None` means no pending vote.
    pub wants_close_at: Option<OffsetDateTime>,
    /// Pending vote to extend the message cap (consensus extend, `cbc_extend`).
    /// `Some(ts)` means this participant voted to raise the hard cap by +10; the
    /// cap bumps once a quorum of live participants have voted, at which point all
    /// extend votes clear. Unlike `wants_close_at`, a conversational message does
    /// NOT clear it — wanting to extend and continuing to talk are not opposites,
    /// and clearing on send would wipe a proposal before the counterpart saw it.
    /// `None` means no pending vote.
    pub wants_extend_at: Option<OffsetDateTime>,
}
