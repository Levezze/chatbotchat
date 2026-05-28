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
