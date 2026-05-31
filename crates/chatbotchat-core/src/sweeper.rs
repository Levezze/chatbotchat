//! Time-based room sweeper. The daemon runs [`run_sweeper`] on an hourly tick;
//! the per-tick work lives in [`sweep_once`], which is the unit-testable seam
//! (time is injected as `now`, so a test crosses any threshold without
//! sleeping).
//!
//! The sweep is a thin orchestration over already-tested pieces: the pure
//! [`crate::lifecycle::compute_time_event`] decides *whether* and *which* single
//! transition a room is due, and the conditional [`Storage::update_room_state`]
//! CAS applies it (and writes the `events` audit row — including the
//! `EventKind::Archive` row that *is* the `on_archive` hook). The sweeper itself
//! holds no transition policy and writes no events directly.

use crate::lifecycle::{self, compute_time_event};
use crate::room::RoomState;
use crate::storage::{Storage, StorageError};
use std::time::Duration as StdDuration;
use time::OffsetDateTime;
use tokio::time::{interval_at, Instant};

/// One transition the sweep applied, for the return vec / logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepOutcome {
    pub room_id: String,
    pub from: RoomState,
    pub to: RoomState,
}

/// Run one sweep pass at logical time `now`: for every non-archived room, apply
/// the single time-based transition it is due (if any) and return what changed.
///
/// One transition per room per tick — `compute_time_event` returns at most one
/// event, so a room steps `active → idle → stale → archived` across successive
/// ticks, each landing its own audit row. The `update_room_state` write is a
/// conditional CAS: if an explicit close/wake raced this sweep and moved the
/// room out from under us, the precondition misses and we skip it (a benign
/// no-op, not an error).
pub async fn sweep_once(
    storage: &Storage,
    now: OffsetDateTime,
) -> Result<Vec<SweepOutcome>, StorageError> {
    let rooms = storage.list_rooms_for_sweep().await?;
    let mut outcomes = Vec::new();

    for room in rooms {
        let polls: Vec<OffsetDateTime> = storage
            .list_participants(&room.id)
            .await?
            .into_iter()
            .map(|p| p.last_poll_at)
            .collect();

        let Some(event) = compute_time_event(
            room.state,
            room.last_activity_at,
            room.state_changed_at,
            &polls,
            now,
        ) else {
            continue;
        };

        // The event came from `compute_time_event`, which only ever emits events
        // legal from the room's current state, so `transition` cannot error here;
        // treat an unexpected error defensively by skipping the room.
        let Ok(to) = lifecycle::transition(room.state, event) else {
            continue;
        };

        let changed = storage
            .update_room_state(&room.id, room.state, to, now, None)
            .await?;
        if changed {
            outcomes.push(SweepOutcome {
                room_id: room.id,
                from: room.state,
                to,
            });
        }
    }

    Ok(outcomes)
}

/// Hourly sweep loop driven by the wall clock. Spawned by the daemon at startup.
///
/// `interval_at` is anchored one period out so the first tick fires after an
/// hour, NOT at t=0 — a freshly booted daemon has nothing aged, and skipping the
/// boot sweep keeps daemon integration tests from running a real pass on
/// startup. Untested thin loop; coverage is via [`sweep_once`].
pub async fn run_sweeper(storage: Storage) {
    let period = StdDuration::from_secs(3600);
    let mut ticker = interval_at(Instant::now() + period, period);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_once(&storage, OffsetDateTime::now_utc()).await {
            tracing::warn!("sweep_once failed: {e}");
        }
    }
}
