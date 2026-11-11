//! Durable first-occurrence delivery facts (#113 PR-A). Real SQLite only; no mocks.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    enrollment::ConnectionKind,
    facts::{Basis, ItemClass, ItemTimeline, RecipientTimeline, Stage},
    store::Store,
    Sandbox,
};
use std::{fs, os::unix::fs::PermissionsExt};

struct Case {
    _temp: tempfile::TempDir,
    sandbox: Sandbox,
    store: Store,
    alice: SessionBinding,
    bob: SessionBinding,
}

async fn pair() -> Case {
    let temp = tempfile::Builder::new()
        .prefix("cbc-facts-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18486,
        &[temp.path().join("synthetic-daily/state.db")],
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
    Case {
        _temp: temp,
        sandbox,
        store,
        alice,
        bob,
    }
}

async fn timeline(store: &Store, sequence: i64) -> ItemTimeline {
    store
        .delivery_timeline("project", sequence)
        .await
        .unwrap()
        .expect("item exists")
}

fn recipient<'a>(timeline: &'a ItemTimeline, member: &str) -> &'a RecipientTimeline {
    timeline
        .recipients
        .iter()
        .find(|r| r.member == member)
        .unwrap_or_else(|| panic!("no recipient fact for {member}"))
}

fn members(timeline: &ItemTimeline) -> Vec<&str> {
    timeline
        .recipients
        .iter()
        .map(|r| r.member.as_str())
        .collect()
}

#[tokio::test]
async fn send_check_receipt_records_each_stage_once_with_exact_same_instance_intervals() {
    let c = pair().await;
    let sequence = c
        .store
        .send(&c.alice, "peer-room", "k1", "hello")
        .await
        .unwrap();
    let committed = timeline(&c.store, sequence).await;
    assert_eq!(committed.class, Some(ItemClass::Participant));
    assert_eq!(committed.room, "peer-room");
    assert!(committed.committed.is_some());
    assert_eq!(
        members(&committed),
        ["bob"],
        "the sender is never a recipient"
    );
    let bob = recipient(&committed, "bob");
    assert!(bob.expected);
    assert!(bob.offered.is_none() && bob.receipted.is_none());

    let batch = c.store.check(&c.bob, 10).await.unwrap().unwrap();
    c.store
        .receipt(&c.bob, batch.reference_for_adapter())
        .await
        .unwrap();
    let done = timeline(&c.store, sequence).await;
    let bob = recipient(&done, "bob");
    assert_eq!(bob.offer_count, 1);
    let to_offer = bob.to_offer.as_ref().unwrap();
    let to_receipt = bob.to_receipt.as_ref().unwrap();
    assert_eq!(to_offer.basis, Basis::Exact);
    assert_eq!(to_receipt.basis, Basis::Exact);
    assert!(to_offer.micros >= 0 && to_receipt.micros >= to_offer.micros);
    c.store.close().await;
}

#[tokio::test]
async fn retries_keep_the_first_occurrence_and_count_reoffers() {
    let c = pair().await;
    let sequence = c
        .store
        .send(&c.alice, "peer-room", "k1", "hello")
        .await
        .unwrap();
    let first = timeline(&c.store, sequence).await.committed.unwrap();
    // An idempotent resend commits nothing new.
    c.store
        .send(&c.alice, "peer-room", "k1", "hello")
        .await
        .unwrap();
    assert_eq!(timeline(&c.store, sequence).await.committed.unwrap(), first);

    let batch = c.store.check(&c.bob, 10).await.unwrap().unwrap();
    let offered = recipient(&timeline(&c.store, sequence).await, "bob")
        .offered
        .clone()
        .unwrap();
    // Re-offering the same pending batch is a retry, not a new first offer.
    let again = c.store.check(&c.bob, 10).await.unwrap().unwrap();
    assert_eq!(again.items, batch.items);
    let after = timeline(&c.store, sequence).await;
    let bob = recipient(&after, "bob");
    assert_eq!(bob.offer_count, 2);
    assert_eq!(bob.offered.as_ref().unwrap(), &offered);

    c.store
        .receipt(&c.bob, batch.reference_for_adapter())
        .await
        .unwrap();
    let receipted = recipient(&timeline(&c.store, sequence).await, "bob")
        .receipted
        .clone()
        .unwrap();
    c.store
        .receipt(&c.bob, batch.reference_for_adapter())
        .await
        .unwrap();
    assert_eq!(
        recipient(&timeline(&c.store, sequence).await, "bob")
            .receipted
            .as_ref()
            .unwrap(),
        &receipted
    );
    c.store.close().await;
}

#[tokio::test]
async fn a_restart_between_stages_reports_an_estimate_not_an_exact_interval() {
    let c = pair().await;
    let sequence = c
        .store
        .send(&c.alice, "peer-room", "k1", "hello")
        .await
        .unwrap();
    let before = c.store.instance_id().to_owned();
    c.store.close().await;
    let store = Store::open(&c.sandbox).await.unwrap();
    assert_ne!(
        store.instance_id(),
        before,
        "each open is a new core instance"
    );
    let bob = store
        .bind_member("project", "bob", "laptop", "chat-b-restarted")
        .await
        .unwrap();
    let batch = store.check(&bob, 10).await.unwrap().unwrap();
    store
        .receipt(&bob, batch.reference_for_adapter())
        .await
        .unwrap();
    let done = timeline(&store, sequence).await;
    let fact = recipient(&done, "bob");
    assert_eq!(fact.to_offer.as_ref().unwrap().basis, Basis::Estimate);
    assert_eq!(fact.to_receipt.as_ref().unwrap().basis, Basis::Estimate);
    store.close().await;
}

#[tokio::test]
async fn a_revoked_receipt_records_nothing_and_keeps_the_recipient_in_the_denominator() {
    let c = pair().await;
    let sequence = c
        .store
        .send(&c.alice, "peer-room", "k1", "hello")
        .await
        .unwrap();
    let batch = c.store.check(&c.bob, 10).await.unwrap().unwrap();
    c.store
        .revoke_room_member("project", "peer-room", "bob")
        .await
        .unwrap();
    assert!(c
        .store
        .receipt(&c.bob, batch.reference_for_adapter())
        .await
        .is_err());
    let after = timeline(&c.store, sequence).await;
    assert_eq!(
        members(&after),
        ["bob"],
        "commit-time snapshot survives revocation"
    );
    let bob = recipient(&after, "bob");
    assert!(bob.offered.is_some());
    assert!(bob.receipted.is_none() && bob.to_receipt.is_none());
    c.store.close().await;
}

#[tokio::test]
async fn system_items_are_classified_and_notices_snapshot_their_own_recipients() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-facts-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18486,
        &[temp.path().join("synthetic-daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for (member, role) in [("child", Role::FeatureLead), ("peer", Role::FeatureLead)] {
        store
            .register_member("project", member, role)
            .await
            .unwrap();
    }
    let child = store
        .bind_member("project", "child", "device", "child-native")
        .await
        .unwrap();
    let peer = store
        .bind_member("project", "peer", "device", "peer-native")
        .await
        .unwrap();
    store
        .authorize_connection("project", "child", "peer", ConnectionKind::Peer, "contract")
        .await
        .unwrap();
    let room = store
        .connect_members(
            &child,
            "peer",
            ConnectionKind::Peer,
            "contract",
            "open",
            "a contract",
        )
        .await
        .unwrap()
        .room;
    let intro = drain_sequences(&store, &peer).await;
    assert_eq!(intro.len(), 1);
    let intro_timeline = timeline(&store, intro[0]).await;
    assert_eq!(intro_timeline.class, Some(ItemClass::Introduction));
    // Members exist before the introduction commits, so its audience is the other member.
    assert_eq!(members(&intro_timeline), ["peer"]);

    let sent = store
        .send_with_notice_interval(&child, &room, "one", "one", 1)
        .await
        .unwrap();
    let notice = sent + 1;
    let notice_timeline = timeline(&store, notice).await;
    assert_eq!(notice_timeline.class, Some(ItemClass::Notice));
    // A peer notice reaches both peers, the triggering sender included.
    assert_eq!(members(&notice_timeline), ["child", "peer"]);
    assert_eq!(
        timeline(&store, sent).await.class,
        Some(ItemClass::Participant)
    );

    let closed = store.close_room(&peer, &room, "done").await.unwrap();
    let closure = timeline(&store, closed).await;
    assert_eq!(closure.class, Some(ItemClass::Closure));
    assert_eq!(members(&closure), ["child"]);
    store.close().await;
}

async fn drain_sequences(store: &Store, binding: &SessionBinding) -> Vec<i64> {
    let mut all = Vec::new();
    while let Some(batch) = store.check(binding, 32).await.unwrap() {
        all.extend(batch.items.iter().map(|i| i.sequence));
        store
            .receipt(binding, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    all
}

#[tokio::test]
async fn items_committed_before_the_facts_migration_report_unknown_not_an_error() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-facts-v4-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18486,
        &[temp.path().join("synthetic-daily/state.db")],
    )
    .unwrap();
    fs::write(
        sandbox.database_path(),
        include_bytes!("fixtures/store-v4.sqlite3"),
    )
    .unwrap();
    fs::set_permissions(sandbox.database_path(), fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let lead = store
        .bind_member(
            "v4-upgrade-kept",
            "lead",
            "fixture-device",
            "lead-new-native",
        )
        .await
        .unwrap();
    let mut old = Vec::new();
    while let Some(batch) = store.check(&lead, 32).await.unwrap() {
        old.extend(batch.items.iter().map(|i| i.sequence));
        store
            .receipt(&lead, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    let item = store
        .delivery_timeline("v4-upgrade-kept", old[0])
        .await
        .unwrap()
        .expect("pre-migration item still exists");
    assert!(item.committed.is_none() && item.class.is_none());
    let fact = recipient(&item, "lead");
    assert!(
        !fact.expected,
        "no commit-time snapshot exists for an old item"
    );
    assert!(fact.offered.is_some() && fact.receipted.is_some());
    assert!(fact.to_offer.is_none() && fact.to_receipt.is_none());
    assert!(store
        .delivery_timeline("v4-upgrade-kept", i64::MAX)
        .await
        .unwrap()
        .is_none());
    store.close().await;
}

#[tokio::test]
async fn events_are_versioned_and_carry_no_body_or_batch_reference() {
    let c = pair().await;
    let body = "a body that must never appear in an event";
    let sequence = c
        .store
        .send(&c.alice, "peer-room", "k1", body)
        .await
        .unwrap();
    let batch = c.store.check(&c.bob, 10).await.unwrap().unwrap();
    c.store
        .receipt(&c.bob, batch.reference_for_adapter())
        .await
        .unwrap();
    let events = c.store.delivery_events("project", sequence).await.unwrap();
    let stages: Vec<Stage> = events.iter().map(|e| e.stage).collect();
    assert_eq!(
        stages,
        [Stage::Committed, Stage::InboxOffered, Stage::Receipted]
    );
    let json = serde_json::to_string(&events).unwrap();
    assert!(!json.contains(body));
    assert!(!json.contains(batch.reference_for_adapter()));
    let first: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(first[0]["schema"], 1);
    assert_eq!(first[1]["stage"], "inbox_offered");
    assert_eq!(first[1]["member"], "bob");
    assert!(first[1]["batch_tag"]
        .as_str()
        .is_some_and(|t| t.len() == 16));
    c.store.close().await;
}

#[tokio::test]
async fn a_batch_offered_before_the_migration_still_records_its_receipt() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection};
    let Case {
        _temp,
        sandbox,
        store,
        alice,
        bob,
    } = pair().await;
    let sequence = store.send(&alice, "peer-room", "k1", "old").await.unwrap();
    let batch = store.check(&bob, 10).await.unwrap().unwrap();
    store.close().await;
    // A batch offered before migration 0007 has no fact rows; the receipt arrives after upgrade.
    let options = SqliteConnectOptions::new()
        .filename(sandbox.database_path())
        .create_if_missing(false);
    let mut fixture = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    for table in ["candidate_recipient_facts", "candidate_item_facts"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE sequence=?"))
            .bind(sequence)
            .execute(&mut fixture)
            .await
            .unwrap();
    }
    fixture.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store
        .receipt(&bob, batch.reference_for_adapter())
        .await
        .unwrap();
    let item = timeline(&store, sequence).await;
    let fact = recipient(&item, "bob");
    assert!(!fact.expected);
    assert!(fact.offered.is_none(), "the old offer was never observed");
    assert!(
        fact.receipted.is_some(),
        "the observed receipt must not be lost"
    );
    store.close().await;
}

#[tokio::test]
async fn events_are_ordered_by_when_they_happened_not_by_member() {
    let c = pair().await;
    c.store
        .register_member("project", "carol", Role::Participant)
        .await
        .unwrap();
    c.store
        .create_room("project", "trio", &["alice", "bob", "carol"])
        .await
        .unwrap();
    let carol = c
        .store
        .bind_member("project", "carol", "laptop", "chat-c")
        .await
        .unwrap();
    let sequence = c.store.send(&c.alice, "trio", "k1", "hi").await.unwrap();
    // Carol sorts after Bob by name but checks and receipts first.
    for member in [&carol, &c.bob] {
        let batch = c.store.check(member, 10).await.unwrap().unwrap();
        c.store
            .receipt(member, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    let events = c.store.delivery_events("project", sequence).await.unwrap();
    let order: Vec<(Stage, Option<&str>)> = events
        .iter()
        .map(|e| (e.stage, e.member.as_deref()))
        .collect();
    assert_eq!(
        order,
        [
            (Stage::Committed, None),
            (Stage::InboxOffered, Some("carol")),
            (Stage::Receipted, Some("carol")),
            (Stage::InboxOffered, Some("bob")),
            (Stage::Receipted, Some("bob")),
        ]
    );
    c.store.close().await;
}
