use time::OffsetDateTime;

/// A registered participant in a room. The handle is minted on first join and
/// is stable for a given `(room_id, repo, model, cwd)` tuple — rejoining with
/// the same tuple returns the same handle (idempotent identity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub handle: String,
    pub room_id: String,
    pub repo: String,
    pub model: String,
    pub cwd: String,
    pub joined_at: OffsetDateTime,
    pub last_poll_at: OffsetDateTime,
}
