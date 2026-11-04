use cbc_candidate::{
    bindings::{Role, SessionBinding},
    store::Store,
    Sandbox,
};
use tempfile::tempdir;

#[tokio::test]
async fn a_bound_member_sends_to_a_room_and_only_its_counterpart_receives() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["alice", "bob"] {
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("project", "peer-room", &["alice", "bob"])
        .await
        .unwrap();
    let alice = store
        .bind_member("project", "alice", "laptop", "chat-a")
        .await
        .unwrap();
    let bob = store
        .bind_member("project", "bob", "laptop", "chat-b")
        .await
        .unwrap();
    let seq = store
        .send(&alice, "peer-room", "request-1", "hello")
        .await
        .unwrap();
    assert!(store.check(&alice, 10).await.unwrap().is_none());
    let batch = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].sequence, seq);
    assert_eq!(batch.items[0].body, "hello");
    assert_eq!(batch.items[0].room, "peer-room");
    store.close().await;
}

async fn pair() -> (
    tempfile::TempDir,
    Sandbox,
    Store,
    SessionBinding,
    SessionBinding,
) {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["alice", "bob"] {
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("project", "peer-room", &["alice", "bob"])
        .await
        .unwrap();
    let alice = store
        .bind_member("project", "alice", "laptop", "chat-a")
        .await
        .unwrap();
    let bob = store
        .bind_member("project", "bob", "laptop", "chat-b")
        .await
        .unwrap();
    (temp, sandbox, store, alice, bob)
}

#[tokio::test]
async fn pending_batch_replays_after_restart_until_its_exact_receipt() {
    let (_temp, sandbox, store, alice, bob) = pair().await;
    store
        .send(&alice, "peer-room", "first", "one")
        .await
        .unwrap();
    let first = store.check(&bob, 1).await.unwrap().unwrap();
    store
        .send(&alice, "peer-room", "second", "two")
        .await
        .unwrap();
    let replay = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(
        replay.items, first.items,
        "new sends cannot change a pending batch"
    );
    assert_eq!(
        replay.reference_for_adapter(),
        first.reference_for_adapter()
    );
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let replay = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(replay.items, first.items);
    assert_eq!(
        replay.reference_for_adapter(),
        first.reference_for_adapter()
    );
    store
        .receipt(&bob, first.reference_for_adapter())
        .await
        .unwrap();
    store
        .receipt(&bob, first.reference_for_adapter())
        .await
        .unwrap();
    let next = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].body, "two");
    assert_ne!(next.reference_for_adapter(), first.reference_for_adapter());
    store
        .receipt(&bob, next.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&bob, 10).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn fair_rotation_does_not_let_a_busy_room_starve_other_rooms() {
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    for room in ["a", "b", "c"] {
        store
            .create_room("project", room, &["alice", "bob"])
            .await
            .unwrap();
        for n in 0..3 {
            store
                .send(&alice, room, &format!("{room}-{n}"), "work")
                .await
                .unwrap();
        }
    }
    let first = store.check(&bob, 2).await.unwrap().unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|i| i.room.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    store
        .receipt(&bob, first.reference_for_adapter())
        .await
        .unwrap();
    let second = store.check(&bob, 2).await.unwrap().unwrap();
    assert_eq!(second.items[0].room, "c");
    store
        .receipt(&bob, second.reference_for_adapter())
        .await
        .unwrap();
    store.close().await;
}

#[tokio::test]
async fn stale_sessions_wrong_members_and_wrong_contexts_cannot_receipt_a_batch() {
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    store.send(&alice, "peer-room", "one", "one").await.unwrap();
    let pending = store.check(&bob, 1).await.unwrap().unwrap();
    assert!(store
        .receipt(&alice, pending.reference_for_adapter())
        .await
        .is_err());
    store.create_context("other").await.unwrap();
    store
        .register_member("other", "bob", Role::Participant)
        .await
        .unwrap();
    let other = store
        .bind_member("other", "bob", "laptop", "chat-b")
        .await
        .unwrap();
    assert!(store
        .receipt(&other, pending.reference_for_adapter())
        .await
        .is_err());
    let successor = store
        .bind_member("project", "bob", "new-device", "new-chat")
        .await
        .unwrap();
    assert!(store.check(&bob, 1).await.is_err());
    assert!(store.send(&bob, "peer-room", "old", "bad").await.is_err());
    assert!(store
        .receipt(&bob, pending.reference_for_adapter())
        .await
        .is_err());
    assert!(store
        .receipt(&successor, pending.reference_for_adapter())
        .await
        .is_err());
    let replay = store.check(&successor, 1).await.unwrap().unwrap();
    assert_eq!(replay.items, pending.items);
    assert_ne!(
        replay.reference_for_adapter(),
        pending.reference_for_adapter()
    );
    store
        .receipt(&successor, replay.reference_for_adapter())
        .await
        .unwrap();
    store.close().await;
}

#[tokio::test]
async fn revoked_membership_cancels_a_mixed_batch_without_losing_authorized_items() {
    let (_temp, sandbox, store, alice, bob) = pair().await;
    store
        .create_room("project", "other", &["alice", "bob"])
        .await
        .unwrap();
    store
        .send(&alice, "peer-room", "one", "revoked")
        .await
        .unwrap();
    store.send(&alice, "other", "two", "allowed").await.unwrap();
    let mixed = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(mixed.items.len(), 2);
    store
        .revoke_room_member("project", "peer-room", "bob")
        .await
        .unwrap();
    store
        .create_room("project", "peer-room", &["alice", "bob"])
        .await
        .unwrap();
    assert!(store
        .send(&bob, "peer-room", "bad", "no grant")
        .await
        .is_err());
    assert!(store
        .receipt(&bob, mixed.reference_for_adapter())
        .await
        .is_err());
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let next = store.check(&bob, 10).await.unwrap().unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].body, "allowed");
    assert_ne!(next.reference_for_adapter(), mixed.reference_for_adapter());
    assert!(store
        .receipt(&bob, mixed.reference_for_adapter())
        .await
        .is_err());
    store
        .receipt(&bob, next.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&bob, 10).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn removing_and_reregistering_a_member_does_not_restore_its_old_room_grants() {
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    store
        .send(&alice, "peer-room", "one", "previous grant")
        .await
        .unwrap();
    let pending = store.check(&bob, 32).await.unwrap().unwrap();
    store.remove_member("project", "bob").await.unwrap();
    store
        .register_member("project", "bob", Role::Participant)
        .await
        .unwrap();
    let successor = store
        .bind_member("project", "bob", "device", "successor")
        .await
        .unwrap();
    store
        .create_room("project", "peer-room", &["alice", "bob"])
        .await
        .unwrap();
    assert!(store.check(&successor, 32).await.unwrap().is_none());
    assert!(store
        .receipt(&successor, pending.reference_for_adapter())
        .await
        .is_err());
    assert!(store
        .send(&successor, "peer-room", "bad", "revoked")
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn serialized_byte_budget_bounds_escaped_messages_without_wedging_the_inbox() {
    use cbc_candidate::inbox::{MAX_BATCH_BYTES, MAX_BODY_BYTES};
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    assert!(store
        .send(
            &alice,
            "peer-room",
            "oversized",
            &"a".repeat(MAX_BODY_BYTES + 1)
        )
        .await
        .is_err());
    let body = "\\".repeat(MAX_BODY_BYTES);
    for n in 0..3 {
        store
            .send(&alice, "peer-room", &format!("body-{n}"), &body)
            .await
            .unwrap();
    }
    for _ in 0..3 {
        let batch = store.check(&bob, 32).await.unwrap().unwrap();
        assert_eq!(batch.items.len(), 1);
        assert!(serde_json::to_vec(&batch.items).unwrap().len() <= MAX_BATCH_BYTES);
        assert_eq!(batch.items[0].body, body);
        store
            .receipt(&bob, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    assert!(store.check(&bob, 32).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn close_orders_with_send_and_preserves_the_final_unread_items() {
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    let original = store
        .send(&alice, "peer-room", "before", "before close")
        .await
        .unwrap();
    let (racing, closed) = tokio::join!(
        store.send(&alice, "peer-room", "racing", "racing close"),
        store.close_room(&bob, "peer-room")
    );
    let closed = closed.unwrap();
    if let Ok(sequence) = racing {
        assert!(sequence < closed);
    }
    assert!(store
        .send(&alice, "peer-room", "after", "too late")
        .await
        .is_err());
    assert_eq!(
        store
            .send(&alice, "peer-room", "before", "before close")
            .await
            .unwrap(),
        original
    );
    assert_eq!(store.close_room(&alice, "peer-room").await.unwrap(), closed);
    let batch = store.check(&alice, 32).await.unwrap().unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].kind, "closed");
    assert_eq!(batch.items[0].sequence, closed);
    store
        .receipt(&alice, batch.reference_for_adapter())
        .await
        .unwrap();
    let history = store.check(&bob, 32).await.unwrap().unwrap();
    assert_eq!(history.items[0].sequence, original);
    assert!(history.items.iter().all(|i| i.sequence < closed));
    store.close().await;
}

#[tokio::test]
async fn send_retries_are_idempotent_and_room_history_has_no_twenty_message_cap() {
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    store
        .create_room("project", "other", &["alice", "bob"])
        .await
        .unwrap();
    let first = store
        .send(&alice, "peer-room", "key", "same")
        .await
        .unwrap();
    assert_eq!(
        store
            .send(&alice, "peer-room", "key", "same")
            .await
            .unwrap(),
        first
    );
    assert!(store.send(&alice, "other", "key", "same").await.is_err());
    assert!(store
        .send(&alice, "peer-room", "key", "changed")
        .await
        .is_err());
    for n in 0..40 {
        store
            .send(&alice, "peer-room", &format!("key-{n}"), "ongoing")
            .await
            .unwrap();
    }
    let mut received = 0;
    while let Some(batch) = store.check(&bob, 32).await.unwrap() {
        received += batch.items.len();
        store
            .receipt(&bob, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    assert_eq!(received, 41);
    store.close().await;
}

#[tokio::test]
async fn pathological_json_escaping_is_rejected_but_ordinary_ansi_text_is_allowed() {
    use cbc_candidate::inbox::MAX_BODY_BYTES;
    let (_temp, _sandbox, store, alice, bob) = pair().await;
    let error = store
        .send(
            &alice,
            "peer-room",
            "pathological",
            &"\0".repeat(MAX_BODY_BYTES),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("escaped"));
    store
        .send(
            &alice,
            "peer-room",
            "ansi",
            "\u{1b}[32mtests passed\u{1b}[0m",
        )
        .await
        .unwrap();
    let batch = store.check(&bob, 32).await.unwrap().unwrap();
    assert_eq!(batch.items[0].body, "\u{1b}[32mtests passed\u{1b}[0m");
    store.close().await;
}

#[tokio::test]
async fn older_unread_rows_above_the_new_budget_are_delivered_intact() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection};
    let (_temp, sandbox, store, alice, bob) = pair().await;
    let sequence = store
        .send(&alice, "peer-room", "old", "placeholder")
        .await
        .unwrap();
    store.close().await;
    // Simulate a previously admitted row while no core owns this synthetic database.
    let options = SqliteConnectOptions::new()
        .filename(sandbox.database_path())
        .create_if_missing(false);
    let mut fixture = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let old_body = "\0".repeat(cbc_candidate::inbox::MAX_BODY_BYTES);
    sqlx::query("UPDATE candidate_items SET body=? WHERE sequence=?")
        .bind(&old_body)
        .bind(sequence)
        .execute(&mut fixture)
        .await
        .unwrap();
    fixture.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let batch = store.check(&bob, 32).await.unwrap().unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].body, old_body);
    let bytes = serde_json::to_vec(&batch.items).unwrap().len();
    assert!(bytes > cbc_candidate::inbox::MAX_BATCH_BYTES);
    assert!(bytes <= cbc_candidate::inbox::MAX_BATCH_HARD_BYTES);
    store
        .receipt(&bob, batch.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&bob, 32).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn group_members_receive_the_send_once_without_echoing_to_the_sender() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["alice", "bob", "carol"] {
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("project", "group", &["alice", "bob", "carol"])
        .await
        .unwrap();
    let alice = store
        .bind_member("project", "alice", "laptop", "chat-a")
        .await
        .unwrap();
    store
        .send(&alice, "group", "request-1", "hello group")
        .await
        .unwrap();
    assert!(store.check(&alice, 10).await.unwrap().is_none());
    for member in ["bob", "carol"] {
        let bound = store
            .bind_member("project", member, "laptop", member)
            .await
            .unwrap();
        let batch = store.check(&bound, 10).await.unwrap().unwrap();
        assert_eq!(batch.items.len(), 1);
        assert_eq!(batch.items[0].body, "hello group");
    }
    store.close().await;
}
