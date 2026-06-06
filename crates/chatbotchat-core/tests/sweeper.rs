//! Sweeper integration tests. The hourly sweeper's testable seam is
//! `sweep_once(storage, now)`: it reads every non-archived room, derives the
//! single time-based transition implied by `now` (via the pure
//! `compute_time_event`), and applies it with the conditional `update_room_state`
//! CAS. Time is injected as `now` so a test can cross any threshold without
//! sleeping.

use chatbotchat_core::event::EventKind;
use chatbotchat_core::participant::Participant;
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use chatbotchat_core::sweeper::sweep_once;
use time::{Duration, OffsetDateTime};

async fn fresh_storage() -> Storage {
    Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite")
}

/// Join a participant whose last poll is `last_poll_at`. Backdating it past
/// `GHOST_AFTER` (15 min) makes `no_live_poller` true, so the `idle → stale`
/// step exercises the liveness check for real rather than passing vacuously on
/// an empty room.
async fn join_with_poll(
    storage: &Storage,
    room_id: &str,
    handle: &str,
    cwd: &str,
    last_poll_at: OffsetDateTime,
) {
    let p = Participant {
        handle: handle.into(),
        room_id: room_id.into(),
        repo: "sweep-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        instance: handle.into(),
        joined_at: last_poll_at,
        last_poll_at,
        last_read_seq: 0,
        nickname: None,
    };
    storage
        .create_participant(&p)
        .await
        .expect("create participant");
}

/// A room in `state` whose `last_activity_at` and `state_changed_at` are both
/// `entered_at`. The sweeper reads both columns, so anchoring them lets a test
/// place a room exactly at a chosen age relative to `now`.
fn room_at(id: &str, state: RoomState, entered_at: OffsetDateTime) -> Room {
    Room {
        id: id.into(),
        subject: "sweep test".into(),
        started_at: entered_at,
        last_activity_at: entered_at,
        state,
        state_changed_at: entered_at,
        config: RoomConfig::default(),
        prev_room_id: None,
    }
}

#[tokio::test]
async fn sweep_advances_an_overdue_active_room_to_idle() {
    let storage = fresh_storage().await;
    let now = OffsetDateTime::now_utc();

    // Active, last activity 25h ago — past IDLE_AFTER (24h).
    let room = room_at(
        "sweep-idle-20260530-1200",
        RoomState::Active,
        now - Duration::hours(25),
    );
    storage.create_room(&room).await.expect("create room");

    let outcomes = sweep_once(&storage, now).await.expect("sweep ok");

    assert_eq!(outcomes.len(), 1, "the one overdue room transitions");
    assert_eq!(outcomes[0].room_id, room.id);
    assert_eq!(outcomes[0].from, RoomState::Active);
    assert_eq!(outcomes[0].to, RoomState::Idle);

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get ok")
        .expect("room exists");
    assert_eq!(
        fetched.state,
        RoomState::Idle,
        "the sweep must persist the new state"
    );
}

#[tokio::test]
async fn successive_sweeps_step_a_room_active_to_idle_to_stale_to_archived() {
    let storage = fresh_storage().await;
    let entered = OffsetDateTime::now_utc();
    let room = room_at("sweep-chain-20260530-1200", RoomState::Active, entered);
    storage.create_room(&room).await.expect("create room");

    // Two participants that stopped polling at room start — past GHOST_AFTER for
    // the whole sweep window, so the idle→stale liveness gate is real (not
    // vacuously true on an empty room).
    join_with_poll(&storage, &room.id, "sweep-test-opus47-aaaa", "/a", entered).await;
    join_with_poll(&storage, &room.id, "sweep-test-opus47-bbbb", "/b", entered).await;

    let step = |from: RoomState, to: RoomState, now: OffsetDateTime| {
        let storage = &storage;
        let room_id = room.id.clone();
        async move {
            let outcomes = sweep_once(storage, now).await.expect("sweep ok");
            assert_eq!(outcomes.len(), 1, "exactly one room steps per tick");
            assert_eq!(outcomes[0].from, from);
            assert_eq!(outcomes[0].to, to);
            let state = storage
                .get_room(&room_id)
                .await
                .expect("get ok")
                .expect("exists")
                .state;
            assert_eq!(state, to, "state persisted after {from:?}→{to:?}");
        }
    };

    // active → idle: 24h+ since last activity.
    step(
        RoomState::Active,
        RoomState::Idle,
        entered + Duration::hours(25),
    )
    .await;
    // idle → stale: 7d+ inactivity AND no live poller.
    step(
        RoomState::Idle,
        RoomState::Stale,
        entered + Duration::days(8),
    )
    .await;
    // stale → archived: 14d+ in `stale`, anchored to state_changed_at (the
    // previous tick's `now`), so 8d + 14d = 22d.
    step(
        RoomState::Stale,
        RoomState::Archived,
        entered + Duration::days(8) + Duration::days(14) + Duration::minutes(1),
    )
    .await;

    // Exactly one `archive` row landed — guards against a double-log (the sweeper
    // must NOT insert its own archive event; `update_room_state` already writes
    // the Archive-kind row when `to == Archived`).
    let events = storage.list_events(&room.id).await.expect("events ok");
    let archive_rows = events
        .iter()
        .filter(|e| e.kind == EventKind::Archive)
        .count();
    assert_eq!(
        archive_rows, 1,
        "exactly one archive event for one archival"
    );
}

#[tokio::test]
async fn sweep_archives_a_closed_room_past_the_archive_window() {
    let storage = fresh_storage().await;
    let now = OffsetDateTime::now_utc();
    // Closed 15 days ago — past ARCHIVE_AFTER (14d), anchored to state_changed_at.
    let room = room_at(
        "sweep-closed-20260515-1200",
        RoomState::Closed,
        now - Duration::days(15),
    );
    storage.create_room(&room).await.expect("create room");

    let outcomes = sweep_once(&storage, now).await.expect("sweep ok");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].from, RoomState::Closed);
    assert_eq!(outcomes[0].to, RoomState::Archived);
    assert_eq!(
        storage
            .get_room(&room.id)
            .await
            .expect("get ok")
            .expect("exists")
            .state,
        RoomState::Archived
    );

    // closed→archived must also land exactly one archive event (the on_archive
    // hook, AC #8) — same `update_room_state` path as stale→archived.
    let events = storage.list_events(&room.id).await.expect("events ok");
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == EventKind::Archive)
            .count(),
        1,
        "an archive of a closed room writes one archive event"
    );
}

#[tokio::test]
async fn sweep_idles_an_active_room_whose_pollers_have_all_gone_stale() {
    // GhostAllStale: an active room with RECENT activity (well under IDLE_AFTER)
    // but every participant polling-dark past GHOST_AFTER still idles. This drives
    // the liveness branch of compute_time_event through the sweeper, distinct from
    // the 24h-inactivity branch covered above.
    let storage = fresh_storage().await;
    let now = OffsetDateTime::now_utc();
    // last activity only 30 min ago — NOT past IDLE_AFTER (24h), so only the
    // all-pollers-stale path can idle this room.
    let room = room_at(
        "sweep-ghost-20260530-1200",
        RoomState::Active,
        now - Duration::minutes(30),
    );
    storage.create_room(&room).await.expect("create room");
    join_with_poll(
        &storage,
        &room.id,
        "sweep-test-opus47-aaaa",
        "/a",
        now - Duration::minutes(20),
    )
    .await;
    join_with_poll(
        &storage,
        &room.id,
        "sweep-test-opus47-bbbb",
        "/b",
        now - Duration::minutes(20),
    )
    .await;

    let outcomes = sweep_once(&storage, now).await.expect("sweep ok");

    assert_eq!(outcomes.len(), 1, "all-pollers-stale should idle the room");
    assert_eq!(outcomes[0].from, RoomState::Active);
    assert_eq!(outcomes[0].to, RoomState::Idle);
}

#[tokio::test]
async fn a_recently_active_room_is_not_idled() {
    let storage = fresh_storage().await;
    let now = OffsetDateTime::now_utc();
    // Active, last activity only 1h ago — well under IDLE_AFTER. A sweep between
    // ticks must leave it untouched: the sweeper reads `last_activity_at` live,
    // so fresh activity cancels a would-be idle.
    let room = room_at(
        "sweep-fresh-20260530-1200",
        RoomState::Active,
        now - Duration::hours(1),
    );
    storage.create_room(&room).await.expect("create room");

    let outcomes = sweep_once(&storage, now).await.expect("sweep ok");

    assert!(outcomes.is_empty(), "a fresh room must not transition");
    assert_eq!(
        storage
            .get_room(&room.id)
            .await
            .expect("get ok")
            .expect("exists")
            .state,
        RoomState::Active
    );
}
