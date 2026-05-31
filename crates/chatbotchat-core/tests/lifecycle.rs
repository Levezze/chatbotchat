use chatbotchat_core::lifecycle::{
    compute_time_event, transition, LifecycleEvent, ARCHIVE_AFTER, GHOST_AFTER, IDLE_AFTER,
    STALE_AFTER,
};
use chatbotchat_core::room::RoomState;
use time::{Duration, OffsetDateTime};

const ALL_STATES: [RoomState; 6] = [
    RoomState::Active,
    RoomState::Idle,
    RoomState::Paused,
    RoomState::Stale,
    RoomState::Closed,
    RoomState::Archived,
];

const ALL_EVENTS: [LifecycleEvent; 8] = [
    LifecycleEvent::Message,
    LifecycleEvent::Pause,
    LifecycleEvent::Wake,
    LifecycleEvent::Close,
    LifecycleEvent::Idle,
    LifecycleEvent::Stale,
    LifecycleEvent::Archive,
    LifecycleEvent::GhostAllStale,
];

/// The expected next state for a `(state, event)` pair, or `None` if the
/// transition is forbidden. This matrix is the spec from the slice-6 plan /
/// issue #7 — written here independently of the implementation so the test
/// pins behavior, not the code's shape. Column order matches `ALL_EVENTS`:
/// Message, Pause, Wake, Close, Idle, Stale, Archive, GhostAllStale.
fn expected(state: RoomState, event: LifecycleEvent) -> Option<RoomState> {
    use LifecycleEvent as E;
    use RoomState as S;
    // (Message,  Pause,    Wake,     Close,     Idle,     Stale,     Archive,    GhostAllStale)
    let row: [Option<RoomState>; 8] = match state {
        S::Active => [
            Some(S::Active), // Message
            Some(S::Paused), // Pause
            None,            // Wake
            Some(S::Closed), // Close
            Some(S::Idle),   // Idle
            None,            // Stale
            None,            // Archive
            Some(S::Idle),   // GhostAllStale
        ],
        S::Idle => [
            Some(S::Active), // Message
            Some(S::Paused), // Pause
            Some(S::Active), // Wake
            Some(S::Closed), // Close
            None,            // Idle
            Some(S::Stale),  // Stale
            None,            // Archive
            None,            // GhostAllStale
        ],
        S::Paused => [
            None,            // Message
            None,            // Pause
            Some(S::Active), // Wake
            Some(S::Closed), // Close
            None,            // Idle
            None,            // Stale
            None,            // Archive
            None,            // GhostAllStale
        ],
        S::Stale => [
            Some(S::Active),   // Message
            None,              // Pause
            None,              // Wake
            Some(S::Closed),   // Close
            None,              // Idle
            None,              // Stale
            Some(S::Archived), // Archive
            None,              // GhostAllStale
        ],
        S::Closed => [
            None,              // Message
            None,              // Pause
            None,              // Wake
            None,              // Close
            None,              // Idle
            None,              // Stale
            Some(S::Archived), // Archive
            None,              // GhostAllStale
        ],
        S::Archived => [None, None, None, None, None, None, None, None],
    };
    let idx = match event {
        E::Message => 0,
        E::Pause => 1,
        E::Wake => 2,
        E::Close => 3,
        E::Idle => 4,
        E::Stale => 5,
        E::Archive => 6,
        E::GhostAllStale => 7,
    };
    row[idx]
}

#[test]
fn transition_table_is_exhaustive() {
    for state in ALL_STATES {
        for event in ALL_EVENTS {
            let got = transition(state, event);
            match expected(state, event) {
                Some(next) => {
                    assert_eq!(got, Ok(next), "expected {state:?} + {event:?} -> {next:?}")
                }
                None => {
                    let err =
                        got.expect_err(&format!("expected {state:?} + {event:?} to be forbidden"));
                    assert_eq!(err.from, state);
                    assert_eq!(err.event, event);
                }
            }
        }
    }
}

// --- compute_time_event boundaries ---

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

#[test]
fn active_idles_only_after_24h_of_inactivity() {
    let n = now();
    // Just under 24h: still active. (No participants → no ghost path.)
    assert_eq!(
        compute_time_event(
            RoomState::Active,
            n - IDLE_AFTER + Duration::minutes(1),
            n,
            &[],
            n
        ),
        None
    );
    // At/after 24h: idle.
    assert_eq!(
        compute_time_event(RoomState::Active, n - IDLE_AFTER, n, &[], n),
        Some(LifecycleEvent::Idle)
    );
}

#[test]
fn active_with_all_ghost_pollers_goes_idle_via_ghost_event() {
    let n = now();
    let stale_poll = n - GHOST_AFTER - Duration::minutes(1);
    let live_poll = n - Duration::minutes(1);
    // Recently active, but every poller is a ghost → GhostAllStale.
    assert_eq!(
        compute_time_event(RoomState::Active, n, n, &[stale_poll, stale_poll], n),
        Some(LifecycleEvent::GhostAllStale)
    );
    // One live poller → not all ghosts → no event.
    assert_eq!(
        compute_time_event(RoomState::Active, n, n, &[stale_poll, live_poll], n),
        None
    );
}

#[test]
fn idle_goes_stale_after_7d_only_when_no_live_poller() {
    let n = now();
    let stale_poll = n - GHOST_AFTER - Duration::minutes(1);
    let live_poll = n - Duration::minutes(1);
    // 7d inactive, no live poller → stale.
    assert_eq!(
        compute_time_event(RoomState::Idle, n - STALE_AFTER, n, &[stale_poll], n),
        Some(LifecycleEvent::Stale)
    );
    // 7d inactive but a live poller present → not stale.
    assert_eq!(
        compute_time_event(RoomState::Idle, n - STALE_AFTER, n, &[live_poll], n),
        None
    );
    // Under 7d → not stale.
    assert_eq!(
        compute_time_event(
            RoomState::Idle,
            n - STALE_AFTER + Duration::hours(1),
            n,
            &[stale_poll],
            n
        ),
        None
    );
}

#[test]
fn stale_and_closed_archive_after_14d_from_state_entry() {
    let n = now();
    // Anchored to state_changed_at, not last_activity_at: last_activity here is
    // recent, but the room entered `stale` 14d ago → archive.
    assert_eq!(
        compute_time_event(RoomState::Stale, n, n - ARCHIVE_AFTER, &[], n),
        Some(LifecycleEvent::Archive)
    );
    assert_eq!(
        compute_time_event(RoomState::Closed, n, n - ARCHIVE_AFTER, &[], n),
        Some(LifecycleEvent::Archive)
    );
    // Under 14d in state → no archive.
    assert_eq!(
        compute_time_event(
            RoomState::Closed,
            n,
            n - ARCHIVE_AFTER + Duration::hours(1),
            &[],
            n
        ),
        None
    );
}

#[test]
fn paused_and_archived_have_no_time_event() {
    let n = now();
    let ancient = n - Duration::days(365);
    assert_eq!(
        compute_time_event(RoomState::Paused, ancient, ancient, &[], n),
        None
    );
    assert_eq!(
        compute_time_event(RoomState::Archived, ancient, ancient, &[], n),
        None
    );
}
