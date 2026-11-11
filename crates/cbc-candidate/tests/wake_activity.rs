//! #113 B1: wake attempts the device driver reports, the first CBC activity after a receipt, and
//! the recovery flag the core derives from both. Real SQLite, and the real connector socket for
//! the routes; no mocks.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    connector::{LocalClient, LocalConnector, LocalOwnerClient},
    facts::{Basis, ItemTimeline, RecipientTimeline, Stage, WakeKind, WakeOutcome, WakeReport},
    settings::PolicyPatch,
    store::Store,
    Sandbox,
};
use std::time::Duration;

struct Case {
    _temp: tempfile::TempDir,
    sandbox: Sandbox,
    store: Store,
    alice: SessionBinding,
    bob: SessionBinding,
}

async fn pair() -> Case {
    let temp = tempfile::Builder::new()
        .prefix("cbc-wake-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18487,
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

fn wake(through: i64, outcome: WakeOutcome, failure: Option<&str>, recovery: bool) -> WakeReport {
    WakeReport {
        through,
        kind: WakeKind::ClaudeChannel,
        outcome,
        failure: failure.map(str::to_owned),
        recovery,
        driver_wall_ms: now_ms(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn accepted(through: i64) -> WakeReport {
    wake(through, WakeOutcome::Accepted, None, false)
}

async fn bob_in(store: &Store, sequence: i64) -> RecipientTimeline {
    let timeline: ItemTimeline = store
        .delivery_timeline("project", sequence)
        .await
        .unwrap()
        .expect("item exists");
    timeline
        .recipients
        .into_iter()
        .find(|r| r.member == "bob")
        .expect("bob is a recipient")
}

const INTERVAL_MS: i64 = 150;

async fn due(c: &Case) -> bool {
    c.store.recovery_due(&c.bob, INTERVAL_MS, 2).await.unwrap()
}

async fn read_all(store: &Store, binding: &SessionBinding) {
    while let Some(batch) = store.check(binding, 10).await.unwrap() {
        store
            .receipt(binding, batch.reference_for_adapter())
            .await
            .unwrap();
    }
}

/// One hint covers every unreceipted item of the member up to `through`, and only those. The
/// first dispatch and the first accepted wake are kept; later attempts change nothing.
#[tokio::test]
async fn a_wake_marks_each_item_it_covers_once() {
    let c = pair().await;
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    let two = c
        .store
        .send(&c.alice, "peer-room", "k2", "two")
        .await
        .unwrap();
    let three = c
        .store
        .send(&c.alice, "peer-room", "k3", "three")
        .await
        .unwrap();
    assert_eq!(
        c.store.record_wake(&c.bob, &accepted(two)).await.unwrap(),
        2
    );
    let first = bob_in(&c.store, one).await;
    assert!(first.dispatched.is_some());
    assert_eq!(first.dispatched, first.woken);
    let to_wake = first.to_wake.clone().expect("commit and wake are known");
    assert_eq!(to_wake.basis, Basis::Exact);
    assert!(to_wake.micros >= 0);
    assert!(bob_in(&c.store, two).await.woken.is_some());
    assert!(
        bob_in(&c.store, three).await.dispatched.is_none(),
        "beyond the hint"
    );

    assert_eq!(
        c.store.record_wake(&c.bob, &accepted(three)).await.unwrap(),
        3
    );
    assert_eq!(bob_in(&c.store, one).await, first, "first occurrence wins");
    assert!(bob_in(&c.store, three).await.woken.is_some());

    read_all(&c.store, &c.bob).await;
    assert_eq!(
        c.store.record_wake(&c.bob, &accepted(three)).await.unwrap(),
        0,
        "receipted items are not woken"
    );
    c.store.record_activity(&c.bob, "send").await.unwrap();
    let stages: Vec<Stage> = c
        .store
        .delivery_events("project", one)
        .await
        .unwrap()
        .iter()
        .map(|e| e.stage)
        .collect();
    for stage in [
        Stage::DispatchStarted,
        Stage::NativeWakeAccepted,
        Stage::PostReceiptActivity,
    ] {
        assert!(stages.contains(&stage), "{stage:?} in {stages:?}");
    }
    assert_eq!(
        c.store
            .record_wake(&c.alice, &accepted(three))
            .await
            .unwrap(),
        0,
        "a sender is not a recipient of its own items"
    );
    c.store.close().await;
}

/// The driver sends reports from a queue, so one can reach the core after the member has
/// already receipted what it covered. The attempt still covers every item that was unread when
/// the driver made it, and nothing receipted before that.
#[tokio::test]
async fn a_report_that_arrives_after_the_receipt_still_covers_its_items() {
    let c = pair().await;
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    let attempted = accepted(one);
    tokio::time::sleep(Duration::from_millis(5)).await;
    read_all(&c.store, &c.bob).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let late = accepted(one);
    assert_eq!(
        c.store.record_wake(&c.bob, &late).await.unwrap(),
        0,
        "receipted before this attempt"
    );
    assert!(bob_in(&c.store, one).await.dispatched.is_none());
    assert_eq!(c.store.record_wake(&c.bob, &attempted).await.unwrap(), 1);
    let bob = bob_in(&c.store, one).await;
    assert!(bob.woken.is_some());
    assert!(bob.receipted.is_some());
    c.store.close().await;
}

/// An activity request that was already waiting for the store when a receipt committed did not
/// start after that receipt, so it is not post-receipt activity.
#[tokio::test]
async fn activity_that_started_before_the_receipt_is_not_post_receipt() {
    let c = pair().await;
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    let started = c.store.request_start();
    read_all(&c.store, &c.bob).await;
    c.store
        .record_activity_since(&c.bob, "send", &started)
        .await
        .unwrap();
    assert!(bob_in(&c.store, one).await.active.is_none());
    c.store.record_activity(&c.bob, "send").await.unwrap();
    assert!(bob_in(&c.store, one).await.active.is_some());
    c.store.close().await;
}

/// A failed attempt is a dispatch, never a wake. The outcome and its failure class must agree.
#[tokio::test]
async fn a_failed_wake_is_a_dispatch_without_a_wake() {
    let c = pair().await;
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    let failed = wake(one, WakeOutcome::Failed, Some("channel_closed"), false);
    assert_eq!(c.store.record_wake(&c.bob, &failed).await.unwrap(), 1);
    let bob = bob_in(&c.store, one).await;
    assert!(bob.dispatched.is_some());
    assert!(bob.woken.is_none());
    assert!(bob.to_wake.is_none());

    for bad in [
        wake(one, WakeOutcome::Failed, None, false),
        wake(one, WakeOutcome::Accepted, Some("timeout"), false),
        wake(one, WakeOutcome::Failed, Some(""), false),
        wake(one, WakeOutcome::Failed, Some("has space"), false),
        wake(one, WakeOutcome::Failed, Some(&"x".repeat(65)), false),
        wake(0, WakeOutcome::Accepted, None, false),
    ] {
        assert!(c.store.record_wake(&c.bob, &bad).await.is_err(), "{bad:?}");
    }
    assert!(
        bob_in(&c.store, one).await.woken.is_none(),
        "refused reports record nothing"
    );
    c.store.close().await;
}

/// The first non-transport operation after a receipt marks each receipted item once. Activity
/// before the receipt marks nothing.
#[tokio::test]
async fn the_first_activity_after_a_receipt_is_recorded_once() {
    let c = pair().await;
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    c.store.record_activity(&c.bob, "work").await.unwrap();
    read_all(&c.store, &c.bob).await;
    assert!(
        bob_in(&c.store, one).await.active.is_none(),
        "activity before the receipt"
    );
    let two = c
        .store
        .send(&c.alice, "peer-room", "k2", "two")
        .await
        .unwrap();

    c.store.record_activity(&c.bob, "send").await.unwrap();
    let first = bob_in(&c.store, one).await;
    assert_eq!(first.active_operation.as_deref(), Some("send"));
    let to_activity = first
        .to_activity
        .clone()
        .expect("commit and activity known");
    assert_eq!(to_activity.basis, Basis::Exact);
    assert!(to_activity.micros >= first.to_receipt.as_ref().unwrap().micros);
    assert!(
        bob_in(&c.store, two).await.active.is_none(),
        "not yet receipted"
    );

    c.store.record_activity(&c.bob, "request").await.unwrap();
    assert_eq!(bob_in(&c.store, one).await, first, "first occurrence wins");
    read_all(&c.store, &c.bob).await;
    c.store.record_activity(&c.bob, "request").await.unwrap();
    assert_eq!(
        bob_in(&c.store, two).await.active_operation.as_deref(),
        Some("request")
    );
    assert!(c.store.record_activity(&c.bob, "Send!").await.is_err());
    c.store.close().await;
}

/// Due only with an unreceipted item older than the interval, no wake attempt within the
/// interval, and recovery attempts left for the oldest such item.
#[tokio::test]
async fn recovery_is_due_only_for_old_unreceipted_items_with_attempts_left() {
    let c = pair().await;
    assert!(!due(&c).await, "nothing unread");
    let one = c
        .store
        .send(&c.alice, "peer-room", "k1", "one")
        .await
        .unwrap();
    assert!(!due(&c).await, "too recent");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(due(&c).await);

    c.store.record_wake(&c.bob, &accepted(one)).await.unwrap();
    assert!(!due(&c).await, "a wake within the interval");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(due(&c).await, "an ordinary wake uses no recovery attempt");

    for attempt in 1..=2 {
        c.store
            .record_wake(&c.bob, &wake(one, WakeOutcome::Accepted, None, true))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(due(&c).await, attempt < 2, "attempt {attempt}");
    }
    assert!(
        !c.store.recovery_due(&c.bob, INTERVAL_MS, 0).await.unwrap(),
        "zero attempts turns recovery off"
    );
    read_all(&c.store, &c.bob).await;
    assert!(!due(&c).await, "everything receipted");
    c.store.close().await;
}

/// The routes: the driver reports a wake with the member's binding, `binding_status` carries the
/// recovery flag from the member's live settings, and only non-transport operations count as
/// activity. The facts outlive the core.
#[tokio::test]
async fn the_connector_reports_wakes_flags_recovery_and_counts_activity() {
    let c = pair().await;
    let connector = LocalConnector::bind(&c.sandbox, c.store).unwrap();
    let owner_token = std::fs::read_to_string(c.sandbox.root().join("owner.key")).unwrap();
    let owner_token = owner_token.trim_end().to_owned();
    let path = connector.path().to_owned();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    let owner = LocalOwnerClient::new(&path, &owner_token);
    let initial = owner.settings().await.unwrap();
    let mut update = initial.settings.clone();
    update.members.entry("project".into()).or_default().insert(
        "bob".into(),
        PolicyPatch {
            recovery_after_ms: Some(1000),
            ..PolicyPatch::default()
        },
    );
    owner
        .update_settings(&initial.revision, update)
        .await
        .unwrap();

    let alice = LocalClient::new(&path, &c.alice);
    let bob = LocalClient::new(&path, &c.bob);
    let one = alice.send("peer-room", "k1", "one").await.unwrap();
    assert!(!bob.binding_status().await.unwrap().recovery_due);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(bob.binding_status().await.unwrap().recovery_due);
    assert_eq!(
        bob.report_wake(&wake(one, WakeOutcome::Accepted, None, true))
            .await
            .unwrap(),
        1
    );
    assert!(!bob.binding_status().await.unwrap().recovery_due);

    let batch = bob.check().await.unwrap().unwrap();
    bob.receipt(&batch).await.unwrap();
    assert!(bob.check().await.unwrap().is_none());
    bob.binding_status().await.unwrap();
    bob.settings().await.unwrap();
    bob.report_wake(&accepted(one)).await.unwrap();
    let transport_only = bob.discover(10).await;
    let _ = stop.send(());
    task.await.unwrap().unwrap();

    let store = Store::open(&c.sandbox).await.unwrap();
    let after = bob_in(&store, one).await;
    assert!(after.woken.is_some());
    assert!(transport_only.is_ok());
    assert_eq!(
        after.active_operation.as_deref(),
        Some("discover"),
        "check, receipt, status, settings and wake reports are transport"
    );
    store.close().await;
}
