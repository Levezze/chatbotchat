//! Pure room-lifecycle state machine. No I/O. Both the request handlers
//! (explicit close/pause/wake, activity) and the hourly sweeper (time-based
//! transitions) consult this module: handlers map an operation to a
//! [`LifecycleEvent`] and call [`transition`]; the sweeper derives the event
//! from elapsed time via [`compute_time_event`] and then calls [`transition`].
//!
//! The transition table is the single source of truth for which `(state, event)`
//! pairs are legal; forbidden pairs return [`IllegalTransition`]. See the slice-6
//! plan / issue #7 for the table.

use crate::room::RoomState;
use time::{Duration, OffsetDateTime};

/// An event that may drive a room from one [`RoomState`] to another.
///
/// `Pause`/`Wake`/`Close` come from explicit operations; `Message` from a `msg`
/// landing (activity); `Idle`/`Stale`/`Archive`/`GhostAllStale` are computed by
/// the sweeper from elapsed time and poller liveness ([`compute_time_event`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// A conversation `msg` was posted — the room is active.
    Message,
    /// Explicit pause (`cbc_pause` or `blocker_real_work` signal).
    Pause,
    /// Explicit resume (`cbc_wake`).
    Wake,
    /// Explicit close (`cbc_close`).
    Close,
    /// 24h with no activity → idle.
    Idle,
    /// 7d total inactivity with no live pollers → stale.
    Stale,
    /// 14d in `stale` or `closed` → archived.
    Archive,
    /// Every participant has stopped polling → idle.
    GhostAllStale,
}

/// Returned when an event is not legal from the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal lifecycle transition: cannot apply {event:?} from {from:?}")]
pub struct IllegalTransition {
    pub from: RoomState,
    pub event: LifecycleEvent,
}

/// Apply `event` to `state`, returning the next state or [`IllegalTransition`].
///
/// This is the full transition table. Every `(state, event)` pair is decided
/// here; anything not listed as legal is an error.
pub fn transition(state: RoomState, event: LifecycleEvent) -> Result<RoomState, IllegalTransition> {
    use LifecycleEvent as E;
    use RoomState as S;

    let next = match (state, event) {
        // Active: live room.
        (S::Active, E::Message) => S::Active,
        (S::Active, E::Pause) => S::Paused,
        (S::Active, E::Close) => S::Closed,
        (S::Active, E::Idle) => S::Idle,
        (S::Active, E::GhostAllStale) => S::Idle,

        // Idle: quiet but resumable.
        (S::Idle, E::Message) => S::Active,
        (S::Idle, E::Pause) => S::Paused,
        (S::Idle, E::Wake) => S::Active,
        (S::Idle, E::Close) => S::Closed,
        (S::Idle, E::Stale) => S::Stale,

        // Paused: durable park; only an explicit wake (or close) leaves it.
        (S::Paused, E::Wake) => S::Active,
        (S::Paused, E::Close) => S::Closed,

        // Stale: ghosted/old; a message revives it, otherwise it archives.
        (S::Stale, E::Message) => S::Active,
        (S::Stale, E::Close) => S::Closed,
        (S::Stale, E::Archive) => S::Archived,

        // Closed: explicitly ended; only the archive sweep acts on it.
        (S::Closed, E::Archive) => S::Archived,

        // Archived: terminal, read-only.

        // Everything else is forbidden.
        _ => return Err(IllegalTransition { from: state, event }),
    };
    Ok(next)
}

/// How long with no activity before `active` → `idle`.
pub const IDLE_AFTER: Duration = Duration::hours(24);
/// Total inactivity before `idle` → `stale` (when no pollers are live).
pub const STALE_AFTER: Duration = Duration::days(7);
/// How long in `stale`/`closed` before `→ archived`.
pub const ARCHIVE_AFTER: Duration = Duration::days(14);
/// A participant that has not polled within this window is a ghost.
pub const GHOST_AFTER: Duration = Duration::minutes(15);

/// True if every participant is a ghost (no live poller). An empty room counts
/// as having no live poller.
fn no_live_poller(participants_last_poll: &[OffsetDateTime], now: OffsetDateTime) -> bool {
    participants_last_poll
        .iter()
        .all(|&last| now - last > GHOST_AFTER)
}

/// Derive the single time-based [`LifecycleEvent`] (if any) implied by elapsed
/// time and poller liveness. Returns **at most one** event so the sweeper steps
/// one transition per tick, each landing its own audit row.
///
/// `last_activity_at` is read at sweep time, so a message arriving between ticks
/// naturally cancels a pending idle. The `stale`/`closed` → `archived` window is
/// anchored to `state_changed_at` (when the room *entered* that state), not to
/// `last_activity_at`.
pub fn compute_time_event(
    state: RoomState,
    last_activity_at: OffsetDateTime,
    state_changed_at: OffsetDateTime,
    participants_last_poll: &[OffsetDateTime],
    now: OffsetDateTime,
) -> Option<LifecycleEvent> {
    match state {
        RoomState::Active => {
            if now - last_activity_at >= IDLE_AFTER {
                Some(LifecycleEvent::Idle)
            } else if !participants_last_poll.is_empty()
                && no_live_poller(participants_last_poll, now)
            {
                Some(LifecycleEvent::GhostAllStale)
            } else {
                None
            }
        }
        RoomState::Idle => {
            if now - last_activity_at >= STALE_AFTER && no_live_poller(participants_last_poll, now)
            {
                Some(LifecycleEvent::Stale)
            } else {
                None
            }
        }
        RoomState::Stale | RoomState::Closed => {
            if now - state_changed_at >= ARCHIVE_AFTER {
                Some(LifecycleEvent::Archive)
            } else {
                None
            }
        }
        RoomState::Paused | RoomState::Archived => None,
    }
}
