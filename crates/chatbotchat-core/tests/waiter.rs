use chatbotchat_core::participant::Participant;
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use chatbotchat_core::waiter::{wait_for_message, Hub, WaitOutcome};
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;

async fn storage_with_room() -> (Storage, String) {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    let now = OffsetDateTime::now_utc();
    let room = Room {
        id: "wait-test-20260528-1500".into(),
        subject: "wait test".into(),
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        config: RoomConfig::default(),
        prev_room_id: None,
    };
    storage.create_room(&room).await.expect("create room");
    (storage, room.id)
}

async fn join(storage: &Storage, room_id: &str, handle: &str, cwd: &str) {
    let now = OffsetDateTime::now_utc();
    let p = Participant {
        handle: handle.into(),
        room_id: room_id.into(),
        repo: "wait-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        joined_at: now,
        last_poll_at: now,
        last_read_seq: 0,
    };
    storage
        .create_participant(&p)
        .await
        .expect("create participant");
}

#[tokio::test]
async fn wait_returns_existing_unread_immediately() {
    let handle = "wait-test-opus47-aaaa";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, handle, "/work/a").await;

    let now = OffsetDateTime::now_utc();
    let m = storage
        .create_message(&room_id, "someone-else", None, "already here", now)
        .await
        .expect("create message");

    let hub = Hub::new();
    let outcome = wait_for_message(&storage, &hub, &room_id, handle, 0, Duration::from_secs(5))
        .await
        .expect("wait ok");

    match outcome {
        WaitOutcome::Message(got) => {
            assert_eq!(got.seq, m.seq);
            assert_eq!(got.body, "already here");
        }
        WaitOutcome::PausedByTimeout => panic!("expected the already-present message, got timeout"),
    }

    // The cursor advanced to the consumed message.
    let row = storage
        .get_participant_by_tuple(&room_id, "wait-test", "opus47", "/work/a")
        .await
        .expect("get ok")
        .expect("participant exists");
    assert_eq!(row.last_read_seq, m.seq);
}

#[tokio::test]
async fn wait_parks_then_returns_when_a_message_arrives() {
    let handle = "wait-test-opus47-aaaa";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, handle, "/work/a").await;

    let hub = Arc::new(Hub::new());

    // Park a waiter with nothing yet to read.
    let waiter = {
        let storage = storage.clone();
        let hub = hub.clone();
        let room_id = room_id.clone();
        let handle = handle.to_string();
        tokio::spawn(async move {
            wait_for_message(&storage, &hub, &room_id, &handle, 0, Duration::from_secs(5)).await
        })
    };

    // Let it park, then post a message and ring the room.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let now = OffsetDateTime::now_utc();
    storage
        .create_message(&room_id, "someone-else", None, "arrived late", now)
        .await
        .expect("create message");
    hub.notify(&room_id);

    let outcome = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("waiter resolved before the test deadline")
        .expect("waiter task did not panic")
        .expect("wait ok");

    match outcome {
        WaitOutcome::Message(m) => assert_eq!(m.body, "arrived late"),
        WaitOutcome::PausedByTimeout => panic!("waiter should have woken on the new message"),
    }
}

#[tokio::test]
async fn wait_times_out_when_no_message_arrives() {
    let handle = "wait-test-opus47-aaaa";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, handle, "/work/a").await;

    let hub = Hub::new();
    let cap = Duration::from_millis(80);
    let start = tokio::time::Instant::now();
    let outcome = wait_for_message(&storage, &hub, &room_id, handle, 0, cap)
        .await
        .expect("wait ok");
    let elapsed = start.elapsed();

    assert!(
        matches!(outcome, WaitOutcome::PausedByTimeout),
        "expected a timeout when nothing arrives"
    );
    assert!(
        elapsed >= cap,
        "should have parked for at least the cap: {elapsed:?} < {cap:?}"
    );
}

#[tokio::test]
async fn wait_ignores_a_message_addressed_to_another_handle() {
    let me = "wait-test-opus47-aaaa";
    let other = "wait-test-opus47-bbbb";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, me, "/work/a").await;
    join(&storage, &room_id, other, "/work/b").await;

    let hub = Hub::new();
    let now = OffsetDateTime::now_utc();
    storage
        .create_message(&room_id, "someone-else", Some(other), "for other only", now)
        .await
        .expect("create message");
    hub.notify(&room_id);

    // `me` polling with a short cap must NOT receive a message addressed to `other`.
    let outcome = wait_for_message(&storage, &hub, &room_id, me, 0, Duration::from_millis(80))
        .await
        .expect("wait ok");
    assert!(
        matches!(outcome, WaitOutcome::PausedByTimeout),
        "a message targeted at another handle must not wake this caller"
    );
}

#[tokio::test]
async fn wait_does_not_return_the_callers_own_message() {
    let handle = "wait-test-opus47-aaaa";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, handle, "/work/a").await;

    let hub = Hub::new();
    let now = OffsetDateTime::now_utc();
    // The caller posts their OWN broadcast.
    storage
        .create_message(&room_id, handle, None, "my own words", now)
        .await
        .expect("create message");
    hub.notify(&room_id);

    // `wait` is the inbox, not the log: it must not echo the sender's own message
    // back, or A's wait-for-B's-reply would return A's own post and break the loop.
    let outcome = wait_for_message(
        &storage,
        &hub,
        &room_id,
        handle,
        0,
        Duration::from_millis(80),
    )
    .await
    .expect("wait ok");
    assert!(
        matches!(outcome, WaitOutcome::PausedByTimeout),
        "a participant must not receive its own message"
    );
}

#[tokio::test]
async fn broadcast_wakes_concurrent_waiters_for_distinct_handles() {
    let a = "wait-test-opus47-aaaa";
    let b = "wait-test-opus47-bbbb";
    let (storage, room_id) = storage_with_room().await;
    join(&storage, &room_id, a, "/work/a").await;
    join(&storage, &room_id, b, "/work/b").await;

    let hub = Arc::new(Hub::new());
    let spawn_waiter = |handle: &str| {
        let storage = storage.clone();
        let hub = hub.clone();
        let room_id = room_id.clone();
        let handle = handle.to_string();
        tokio::spawn(async move {
            wait_for_message(&storage, &hub, &room_id, &handle, 0, Duration::from_secs(5)).await
        })
    };
    let wa = spawn_waiter(a);
    let wb = spawn_waiter(b);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let now = OffsetDateTime::now_utc();
    storage
        .create_message(&room_id, "someone-else", None, "to everyone", now)
        .await
        .expect("create message");
    hub.notify(&room_id);

    for w in [wa, wb] {
        let outcome = tokio::time::timeout(Duration::from_secs(2), w)
            .await
            .expect("waiter resolved before deadline")
            .expect("waiter task did not panic")
            .expect("wait ok");
        match outcome {
            WaitOutcome::Message(m) => assert_eq!(m.body, "to everyone"),
            WaitOutcome::PausedByTimeout => panic!("a broadcast must wake every waiter"),
        }
    }
}
