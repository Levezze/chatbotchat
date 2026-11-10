use cbc_candidate::{
    bindings::{Role, SessionBinding},
    connector::{LocalClient, LocalConnector},
    enrollment::ConnectionKind,
    inbox::Item,
    store::Store,
    Sandbox,
};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

struct Case {
    _temp: tempfile::TempDir,
    sandbox: Sandbox,
    store: Store,
    lead: SessionBinding,
    child: SessionBinding,
    peer: SessionBinding,
    report_room: String,
    peer_room: String,
}

async fn fixture() -> Case {
    let temp = tempfile::Builder::new()
        .prefix("cbc-notices-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18485,
        &[temp.path().join("synthetic-daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for (member, role) in [
        ("lead", Role::RepoLead),
        ("child", Role::FeatureLead),
        ("peer", Role::FeatureLead),
    ] {
        store
            .register_member("project", member, role)
            .await
            .unwrap();
    }
    let lead = store
        .bind_member("project", "lead", "device", "lead-native")
        .await
        .unwrap();
    let child = store
        .bind_member("project", "child", "device", "child-native")
        .await
        .unwrap();
    let peer = store
        .bind_member("project", "peer", "device", "peer-native")
        .await
        .unwrap();
    store
        .authorize_connection(
            "project",
            "child",
            "lead",
            ConnectionKind::Report,
            "feature",
        )
        .await
        .unwrap();
    store
        .authorize_connection("project", "child", "peer", ConnectionKind::Peer, "contract")
        .await
        .unwrap();
    let report_room = store
        .connect_members(
            &child,
            "lead",
            ConnectionKind::Report,
            "feature",
            "open-report",
            "",
        )
        .await
        .unwrap()
        .room;
    let peer_room = store
        .connect_members(
            &child,
            "peer",
            ConnectionKind::Peer,
            "contract",
            "open-peer",
            "one shared contract",
        )
        .await
        .unwrap()
        .room;
    Case {
        _temp: temp,
        sandbox,
        store,
        lead,
        child,
        peer,
        report_room,
        peer_room,
    }
}

async fn drain(store: &Store, binding: &SessionBinding) -> Vec<Item> {
    let mut all = Vec::new();
    while let Some(batch) = store.check(binding, 32).await.unwrap() {
        all.extend(batch.items.clone());
        store
            .receipt(binding, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    all
}

fn notices(items: &[Item]) -> Vec<&Item> {
    items.iter().filter(|i| i.kind == "room_notice").collect()
}

#[tokio::test]
async fn report_counts_exclude_introductions_retries_and_notices_and_never_stop_sends() {
    let c = fixture().await;
    for n in 1..100 {
        let key = format!("message-{n}");
        let sequence = c
            .store
            .send(&c.child, &c.report_room, &key, "ordinary update")
            .await
            .unwrap();
        assert_eq!(
            sequence,
            c.store
                .send(&c.child, &c.report_room, &key, "ordinary update")
                .await
                .unwrap()
        );
    }
    assert!(notices(&drain(&c.store, &c.lead).await).is_empty());
    c.store
        .send(&c.child, &c.report_room, "message-100", "still moving")
        .await
        .unwrap();
    let parent = drain(&c.store, &c.lead).await;
    let notice = notices(&parent);
    assert_eq!(notice.len(), 1);
    assert_eq!(notice[0].sender, "cbc");
    assert!(notice[0]
        .body
        .starts_with("Info: 100 messages. No action required."));
    assert!(notices(&drain(&c.store, &c.child).await).is_empty());
    // The parent receives a notice even when its own send crosses the threshold.
    for n in 101..=200 {
        c.store
            .send(
                &c.lead,
                &c.report_room,
                &format!("message-{n}"),
                "parent coordination",
            )
            .await
            .unwrap();
    }
    let parent = drain(&c.store, &c.lead).await;
    assert_eq!(notices(&parent).len(), 1);
    assert!(notices(&parent)[0].body.starts_with("Info: 200 messages."));
    assert!(notices(&drain(&c.store, &c.child).await).is_empty());
    c.store
        .send(
            &c.child,
            &c.report_room,
            "message-201",
            "no extension or approval",
        )
        .await
        .unwrap();
    assert_eq!(drain(&c.store, &c.lead).await.len(), 1);
    c.store.close().await;
}

#[tokio::test]
async fn peer_notice_reaches_both_peers_including_the_sender_but_no_other_room_member() {
    let c = fixture().await;
    c.store
        .send_with_notice_interval(&c.child, &c.peer_room, "one", "one", 2)
        .await
        .unwrap();
    let sequence = c
        .store
        .send_with_notice_interval(&c.peer, &c.peer_room, "two", "two", 2)
        .await
        .unwrap();
    assert_eq!(
        sequence,
        c.store
            .send_with_notice_interval(&c.peer, &c.peer_room, "two", "two", 2)
            .await
            .unwrap()
    );
    for binding in [&c.child, &c.peer] {
        let items = drain(&c.store, binding).await;
        assert_eq!(notices(&items).len(), 1);
        assert_eq!(notices(&items)[0].room, c.peer_room);
        assert!(notices(&items)[0].body.starts_with("Info: 2 messages."));
    }
    assert!(notices(&drain(&c.store, &c.lead).await).is_empty());
    c.store
        .send_with_notice_interval(&c.peer, &c.peer_room, "three", "three", 2)
        .await
        .unwrap();
    assert!(notices(&drain(&c.store, &c.child).await).is_empty());
    c.store.close().await;
}

#[tokio::test]
async fn restart_preserves_frozen_notice_batch_and_later_unfrozen_notices_coalesce() {
    let Case {
        _temp,
        sandbox,
        store,
        lead,
        child,
        report_room,
        ..
    } = fixture().await;
    for n in 1..=2 {
        store
            .send_with_notice_interval(&child, &report_room, &format!("message-{n}"), "update", 2)
            .await
            .unwrap();
    }
    let frozen = store.check(&lead, 32).await.unwrap().unwrap();
    assert_eq!(notices(&frozen.items).len(), 1);
    for n in 3..=4 {
        store
            .send_with_notice_interval(&child, &report_room, &format!("message-{n}"), "update", 2)
            .await
            .unwrap();
    }
    assert_eq!(
        store.check(&lead, 32).await.unwrap().unwrap().items,
        frozen.items
    );
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let replay = store.check(&lead, 32).await.unwrap().unwrap();
    assert_eq!(replay.items, frozen.items);
    assert_eq!(
        replay.reference_for_adapter(),
        frozen.reference_for_adapter()
    );
    store
        .receipt(&lead, replay.reference_for_adapter())
        .await
        .unwrap();
    store
        .receipt(&lead, replay.reference_for_adapter())
        .await
        .unwrap();
    let next = drain(&store, &lead).await;
    assert_eq!(notices(&next).len(), 1);
    assert!(notices(&next)[0].body.starts_with("Info: 4 messages."));
    for n in 5..=8 {
        store
            .send_with_notice_interval(&child, &report_room, &format!("message-{n}"), "update", 2)
            .await
            .unwrap();
    }
    let recent = drain(&store, &lead).await;
    assert_eq!(notices(&recent).len(), 1);
    assert!(notices(&recent)[0].body.starts_with("Info: 8 messages."));
    // An older receipt remains idempotent after its informational notice is no longer latest.
    store
        .receipt(&lead, frozen.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&lead, 32).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn policy_changes_apply_on_new_sends_without_backfilling_or_accepting_invalid_counts() {
    let c = fixture().await;
    for n in 1..=3 {
        c.store
            .send(&c.child, &c.report_room, &format!("message-{n}"), "update")
            .await
            .unwrap();
    }
    assert!(notices(&drain(&c.store, &c.lead).await).is_empty());
    assert!(c
        .store
        .send_with_notice_interval(&c.child, &c.report_room, "four", "four", 0)
        .await
        .is_err());
    c.store
        .send_with_notice_interval(&c.child, &c.report_room, "four", "four", 2)
        .await
        .unwrap();
    let at_four = drain(&c.store, &c.lead).await;
    assert_eq!(at_four.iter().filter(|i| i.kind == "message").count(), 1);
    assert!(notices(&at_four)[0].body.starts_with("Info: 4 messages."));
    c.store
        .send_with_notice_interval(&c.child, &c.report_room, "five", "five", 3)
        .await
        .unwrap();
    assert!(notices(&drain(&c.store, &c.lead).await).is_empty());
    c.store
        .send_with_notice_interval(&c.child, &c.report_room, "six", "six", 3)
        .await
        .unwrap();
    let at_six = drain(&c.store, &c.lead).await;
    assert_eq!(notices(&at_six).len(), 1);
    assert!(notices(&at_six)[0].body.starts_with("Info: 6 messages."));
    c.store.close().await;
}

#[tokio::test]
async fn report_policy_owner_is_verified_and_notices_obey_binding_and_room_revocation() {
    let c = fixture().await;
    for binding in [&c.child, &c.lead] {
        assert_eq!(
            c.store
                .room_notice_policy_owner(binding, &c.report_room)
                .await
                .unwrap(),
            Some(("lead".into(), "repo_lead".into()))
        );
    }
    assert!(c
        .store
        .room_notice_policy_owner(&c.child, &c.peer_room)
        .await
        .unwrap()
        .is_none());
    assert!(c
        .store
        .room_notice_policy_owner(&c.peer, &c.report_room)
        .await
        .is_err());
    c.store
        .send_with_notice_interval(&c.child, &c.report_room, "one", "one", 1)
        .await
        .unwrap();
    let pending = c.store.check(&c.lead, 32).await.unwrap().unwrap();
    assert_eq!(notices(&pending.items).len(), 1);
    assert!(c
        .store
        .receipt(&c.child, pending.reference_for_adapter())
        .await
        .is_err());
    assert!(c
        .store
        .binding_activity(&c.child)
        .await
        .unwrap()
        .unwrap()
        .first_unread_sequence
        .is_none());
    c.store
        .revoke_room_member("project", &c.report_room, "lead")
        .await
        .unwrap();
    assert!(c
        .store
        .receipt(&c.lead, pending.reference_for_adapter())
        .await
        .is_err());
    assert!(c.store.check(&c.lead, 32).await.unwrap().is_none());
    c.store
        .revoke_binding("project", "child", c.child.epoch())
        .await
        .unwrap();
    assert!(c
        .store
        .room_notice_policy_owner(&c.child, &c.report_room)
        .await
        .is_err());
    c.store.close().await;
}

#[tokio::test]
async fn shared_watch_routes_a_self_send_parent_notice_without_exposing_it_to_the_child() {
    let c = fixture().await;
    let mut sequence = 0;
    for n in 1..=100 {
        sequence = c
            .store
            .send(
                &c.lead,
                &c.report_room,
                &format!("parent-{n}"),
                "parent update",
            )
            .await
            .unwrap();
    }
    let connector = LocalConnector::bind(&c.sandbox, c.store).unwrap();
    let socket = connector.path().to_owned();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    let changed = tokio::time::timeout(
        Duration::from_secs(2),
        LocalClient::new(&socket, &c.lead).watch_members(&[&c.child], sequence),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].member, "lead");
    assert!(changed[0].sequence > sequence);
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        LocalClient::new(&socket, &c.child).watch_members(&[], sequence)
    )
    .await
    .is_err());
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn closure_drains_the_final_notice_without_extending_or_reopening_a_room() {
    let c = fixture().await;
    c.store
        .send_with_notice_interval(&c.child, &c.report_room, "one", "one", 1)
        .await
        .unwrap();
    c.store
        .close_room(&c.child, &c.report_room, "done")
        .await
        .unwrap();
    let items = drain(&c.store, &c.lead).await;
    assert_eq!(notices(&items).len(), 1);
    assert!(items.iter().any(|i| i.kind == "closed"));
    assert!(c
        .store
        .send(&c.child, &c.report_room, "two", "closed room")
        .await
        .is_err());
    assert!(c
        .store
        .send(&c.child, &c.report_room, "one", "one")
        .await
        .is_ok());
    assert!(c.store.check(&c.lead, 32).await.unwrap().is_none());
    c.store.close().await;
}

#[tokio::test]
async fn actual_v4_upgrade_counts_99_participant_sends_without_counting_connection_introductions() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-v4-upgrade-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        18485,
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
    let child = store
        .bind_member(
            "v4-upgrade-kept",
            "child",
            "fixture-device",
            "child-new-native",
        )
        .await
        .unwrap();
    let lead = store
        .bind_member(
            "v4-upgrade-kept",
            "lead",
            "fixture-device",
            "lead-new-native",
        )
        .await
        .unwrap();
    assert_eq!(child.epoch(), 2);
    assert_eq!(lead.epoch(), 2);
    let room = store
        .connect_members(
            &child,
            "lead",
            ConnectionKind::Report,
            "fixture-feature",
            "new-connect",
            "",
        )
        .await
        .unwrap();
    assert!(room.reused);
    let before = drain(&store, &lead).await;
    assert_eq!(before.len(), 100); // 99 normal sends plus one deterministic introduction.
    assert!(notices(&before).is_empty());
    store
        .send(
            &child,
            &room.room,
            "new-message-100",
            "first post-upgrade message",
        )
        .await
        .unwrap();
    let after = drain(&store, &lead).await;
    assert_eq!(after.len(), 2);
    assert!(notices(&after)[0].body.starts_with("Info: 100 messages."));
    store.close().await;
}
