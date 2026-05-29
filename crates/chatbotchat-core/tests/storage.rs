use chatbotchat_core::participant::Participant;
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use time::OffsetDateTime;

async fn fresh_storage() -> Storage {
    Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite")
}

fn sample_room() -> Room {
    let now = OffsetDateTime::now_utc();
    Room {
        id: "smoke-test-20260528-1500".into(),
        subject: "smoke test".into(),
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        config: RoomConfig::default(),
        prev_room_id: None,
    }
}

#[tokio::test]
async fn create_then_get_room_round_trips() {
    let storage = fresh_storage().await;
    let room = sample_room();

    storage.create_room(&room).await.expect("create_room ok");

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get_room ok")
        .expect("room exists");

    assert_eq!(fetched.id, room.id);
    assert_eq!(fetched.subject, room.subject);
    assert_eq!(fetched.state, RoomState::Active);
    assert_eq!(fetched.config, room.config);
    assert!(fetched.prev_room_id.is_none());
}

#[tokio::test]
async fn get_room_returns_none_for_missing() {
    let storage = fresh_storage().await;
    let missing = storage
        .get_room("does-not-exist")
        .await
        .expect("get_room ok");
    assert!(missing.is_none());
}

fn sample_participant(room_id: &str, cwd: &str) -> Participant {
    let now = OffsetDateTime::now_utc();
    Participant {
        handle: format!("smoke-test-opus47-{}", &cwd.len()),
        room_id: room_id.into(),
        repo: "smoke-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        joined_at: now,
        last_poll_at: now,
        last_read_seq: 0,
    }
}

#[tokio::test]
async fn participant_create_get_by_tuple_and_list_round_trip() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    let p = sample_participant(&room.id, "/work/a");
    storage.create_participant(&p).await.expect("create ok");

    // Exact tuple round-trips.
    let fetched = storage
        .get_participant_by_tuple(&room.id, &p.repo, &p.model, &p.cwd)
        .await
        .expect("get ok")
        .expect("participant exists");
    assert_eq!(fetched, p);

    // A different cwd is a different tuple — not found.
    let other = storage
        .get_participant_by_tuple(&room.id, &p.repo, &p.model, "/work/b")
        .await
        .expect("get ok");
    assert!(other.is_none());

    // The room lists exactly this one participant.
    let listed = storage.list_participants(&room.id).await.expect("list ok");
    assert_eq!(listed, vec![p]);
}

fn participant_with_handle(room_id: &str, handle: &str, cwd: &str) -> Participant {
    let now = OffsetDateTime::now_utc();
    Participant {
        handle: handle.into(),
        room_id: room_id.into(),
        repo: "smoke-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        joined_at: now,
        last_poll_at: now,
        last_read_seq: 0,
    }
}

#[tokio::test]
async fn messages_seq_is_monotonic_and_filtered_by_recipient_and_cursor() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    let alice = participant_with_handle(&room.id, "smoke-test-opus47-aaaa", "/work/a");
    let bob = participant_with_handle(&room.id, "smoke-test-opus47-bbbb", "/work/b");
    storage
        .create_participant(&alice)
        .await
        .expect("create alice");
    storage.create_participant(&bob).await.expect("create bob");

    let now = OffsetDateTime::now_utc();
    // A broadcast (recipient = None), then a message targeted to alice.
    let m1 = storage
        .create_message(&room.id, &bob.handle, None, "hello all", now)
        .await
        .expect("create m1");
    let m2 = storage
        .create_message(
            &room.id,
            &bob.handle,
            Some(&alice.handle),
            "psst alice",
            now,
        )
        .await
        .expect("create m2");

    // seq is monotonically increasing on insert.
    assert!(
        m2.seq > m1.seq,
        "seq must increase: {} !> {}",
        m2.seq,
        m1.seq
    );
    assert_eq!(m1.recipient, None);
    assert_eq!(m2.recipient, Some(alice.handle.clone()));
    assert_eq!(m1.body, "hello all");

    // From cursor 0, alice's oldest unread is the broadcast m1.
    let next = storage
        .next_unread(&room.id, &alice.handle, 0)
        .await
        .expect("next_unread ok")
        .expect("alice has an unread");
    assert_eq!(next.seq, m1.seq);

    // After consuming past m1, alice's next unread is the targeted m2.
    storage
        .advance_read_cursor(&alice.handle, m1.seq)
        .await
        .expect("advance ok");
    let next2 = storage
        .next_unread(&room.id, &alice.handle, m1.seq)
        .await
        .expect("next_unread ok")
        .expect("alice has the targeted message");
    assert_eq!(next2.seq, m2.seq);

    // The targeted-to-alice message is invisible to a different handle.
    let bob_next = storage
        .next_unread(&room.id, &bob.handle, m1.seq)
        .await
        .expect("next_unread ok");
    assert!(
        bob_next.is_none(),
        "a message addressed to alice must not surface for bob"
    );

    // advance_read_cursor persisted onto the participant row.
    let alice_row = storage
        .get_participant_by_tuple(&room.id, &alice.repo, &alice.model, &alice.cwd)
        .await
        .expect("get ok")
        .expect("alice exists");
    assert_eq!(alice_row.last_read_seq, m1.seq);
}

#[tokio::test]
async fn claim_next_unread_for_unknown_handle_returns_none_without_hanging() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    // A real participant posts a broadcast (seq > 0, recipient = all).
    let p = participant_with_handle(&room.id, "smoke-test-opus47-aaaa", "/work/a");
    storage
        .create_participant(&p)
        .await
        .expect("create participant");
    let now = OffsetDateTime::now_utc();
    storage
        .create_message(&room.id, &p.handle, None, "hi all", now)
        .await
        .expect("create message");

    // Claiming for a handle that is not a participant must return None — not spin
    // forever on a CAS that can never match a non-existent row.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        storage.claim_next_unread(&room.id, "ghost-handle-not-a-participant"),
    )
    .await;
    assert!(
        result.is_ok(),
        "claim_next_unread must not hang for an unknown handle"
    );
    assert!(
        result.unwrap().expect("claim ok").is_none(),
        "an unknown handle has nothing to claim"
    );
}
