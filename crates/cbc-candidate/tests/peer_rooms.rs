//! Core-bound identity (T-25) and one active room per pair (T-26). Real SQLite only.
use cbc_candidate::{
    bindings::{IdentityEventKind, Role, SessionBinding},
    enrollment::{ConnectionKind, NativeHost, SessionEvidence},
    store::Store,
    Sandbox,
};
use tempfile::{tempdir, TempDir};

async fn fixture() -> (TempDir, Sandbox, Store) {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8985,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["worker", "peer"] {
        store
            .register_member("project", member, Role::FeatureLead)
            .await
            .unwrap();
    }
    for scope in ["contract-a", "contract-b"] {
        store
            .authorize_connection("project", "worker", "peer", ConnectionKind::Peer, scope)
            .await
            .unwrap();
    }
    (temp, sandbox, store)
}

async fn bound(store: &Store, member: &str) -> SessionBinding {
    store
        .bind_member("project", member, "device", &format!("{member}-session"))
        .await
        .unwrap()
}

async fn open(store: &Store, who: &SessionBinding, to: &str, scope: &str, key: &str) -> String {
    store
        .connect_members(who, to, ConnectionKind::Peer, scope, key, "")
        .await
        .unwrap()
        .room
}

async fn drain_kinds(store: &Store, who: &SessionBinding) -> Vec<(String, String, String)> {
    let mut seen = Vec::new();
    while let Some(batch) = store.check(who, 32).await.unwrap() {
        seen.extend(
            batch
                .items
                .iter()
                .map(|i| (i.room.clone(), i.kind.clone(), i.body.clone())),
        );
        store
            .receipt(who, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    seen
}

#[tokio::test]
async fn one_active_room_per_pair_whatever_the_scope() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let first = open(&store, &worker, "peer", "contract-a", "k1").await;
    let again = store
        .connect_members(
            &peer,
            "worker",
            ConnectionKind::Peer,
            "contract-b",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert_eq!(again.room, first, "a second open joins the existing room");
    assert!(again.reused);
    store.close().await;
}

#[tokio::test]
async fn a_close_records_actor_and_reason_and_the_other_member_sees_why() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let room = open(&store, &worker, "peer", "contract-a", "k1").await;
    drain_kinds(&store, &peer).await;
    store
        .close_room(&peer, &room, "contract agreed")
        .await
        .unwrap();
    let replay = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert!(replay.closed);
    let closure = replay.closure.expect("closure is recorded");
    assert_eq!(closure.by, "peer");
    assert_eq!(closure.reason, "contract agreed");
    let seen = drain_kinds(&store, &worker).await;
    assert!(seen.contains(&(room, "closed".into(), "contract agreed".into())));
    store.close().await;
}

#[tokio::test]
async fn a_close_needs_a_short_reason_and_a_refused_close_changes_nothing() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let room = open(&store, &worker, "peer", "contract-a", "k1").await;
    for bad in ["", "   ", "line\u{7}bell", &"x".repeat(201)] {
        assert!(
            store.close_room(&worker, &room, bad).await.is_err(),
            "{bad:?}"
        );
    }
    let still = open(&store, &worker, "peer", "contract-a", "k2").await;
    assert_eq!(still, room, "the room stays open after refused closes");
    store.close().await;
}

#[tokio::test]
async fn reopening_after_a_close_creates_a_replacement_that_supersedes_it() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let old = open(&store, &worker, "peer", "contract-a", "k1").await;
    store.close_room(&worker, &old, "stale").await.unwrap();
    let replacement = store
        .connect_members(
            &peer,
            "worker",
            ConnectionKind::Peer,
            "contract-a",
            "k2",
            "",
        )
        .await
        .unwrap();
    assert_ne!(replacement.room, old);
    assert!(!replacement.closed && !replacement.reused);
    assert_eq!(replacement.supersedes.as_deref(), Some(old.as_str()));
    assert_eq!(replacement.generation, 2);
    // The other member's open is idempotent on the replacement.
    let joined = open(&store, &worker, "peer", "contract-b", "k3").await;
    assert_eq!(joined, replacement.room);
    // A replay of the original request still answers with the original, closed room.
    let replay = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert_eq!(replay.room, old);
    assert!(replay.closed);
    // Each close frees the pair for exactly one replacement at a time.
    store
        .close_room(&peer, &replacement.room, "done")
        .await
        .unwrap();
    let third = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k4",
            "",
        )
        .await
        .unwrap();
    assert_eq!(third.generation, 3);
    assert_eq!(third.supersedes.as_deref(), Some(replacement.room.as_str()));
    store.close().await;
}

#[tokio::test]
async fn concurrent_reopens_after_a_close_create_one_replacement() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let old = open(&store, &worker, "peer", "contract-a", "k1").await;
    store.close_room(&worker, &old, "stale").await.unwrap();
    let (a, b) = tokio::join!(
        store.connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k2",
            ""
        ),
        store.connect_members(
            &peer,
            "worker",
            ConnectionKind::Peer,
            "contract-b",
            "k2",
            ""
        )
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.room, b.room);
    assert_ne!(a.reused, b.reused);
    store.close().await;
}

#[tokio::test]
async fn retiring_a_member_closes_its_rooms_and_the_identity_cannot_come_back() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let room = open(&store, &worker, "peer", "contract-a", "k1").await;
    drain_kinds(&store, &peer).await;
    let proof = SessionEvidence::from_trusted_adapter(
        NativeHost::Claude,
        "device",
        "worker-next",
        "synthetic-host-evidence",
    )
    .unwrap();
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();

    store
        .retire_member("project", "worker", "session ended")
        .await
        .unwrap();

    let seen = drain_kinds(&store, &peer).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, room);
    assert_eq!(seen[0].1, "closed");
    assert!(store.send(&worker, &room, "k9", "late").await.is_err());
    assert!(store
        .register_member("project", "worker", Role::FeatureLead)
        .await
        .is_err());
    assert!(store
        .bind_member("project", "worker", "device", "worker-other")
        .await
        .is_err());
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    assert!(store
        .retire_member("project", "worker", "again")
        .await
        .is_err());
    let events = store.identity_events("project", "worker").await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, IdentityEventKind::Retired);
    assert_eq!(last.reason.as_deref(), Some("session ended"));
    store.close().await;
}

#[tokio::test]
async fn a_handoff_rebinds_the_same_identity_and_keeps_its_rooms() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = |session: &str| {
        SessionEvidence::from_trusted_adapter(
            NativeHost::Claude,
            "device",
            session,
            "synthetic-host-evidence",
        )
        .unwrap()
    };
    let first = store
        .issue_enrollment_invite("project", "worker", &proof("worker-1"), 60)
        .await
        .unwrap();
    let worker = store
        .consume_enrollment_invite(first.expose_token_for_adapter(), &proof("worker-1"))
        .await
        .unwrap();
    let peer = bound(&store, "peer").await;
    let room = open(&store, &worker, "peer", "contract-a", "k1").await;

    let next = store
        .issue_enrollment_invite("project", "worker", &proof("worker-2"), 60)
        .await
        .unwrap();
    let handed = store
        .consume_enrollment_invite(next.expose_token_for_adapter(), &proof("worker-2"))
        .await
        .unwrap();

    assert!(store
        .send(&worker, &room, "k2", "old session")
        .await
        .is_err());
    store
        .send(&handed, &room, "k3", "same identity")
        .await
        .unwrap();
    assert_eq!(
        open(&store, &peer, "worker", "contract-a", "k4").await,
        room
    );
    let kinds: Vec<IdentityEventKind> = store
        .identity_events("project", "worker")
        .await
        .unwrap()
        .iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        [IdentityEventKind::Enrolled, IdentityEventKind::Rebound]
    );
    store.close().await;
}

/// A store opened over a database written by migrations 0001-0007 and seeded with `rows`,
/// so the 0008 upgrade runs on real pre-T-26 data.
async fn v7_store(port: u16, rows: &[&str]) -> (TempDir, Sandbox, Store) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        port,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let v7 = temp.path().join("migrations-v7");
    std::fs::create_dir(&v7).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.as_str() < "0008" {
            std::fs::copy(&path, v7.join(name)).unwrap();
        }
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(sandbox.database_path())
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(v7.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    for statement in rows {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;
    std::fs::set_permissions(
        sandbox.database_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    (temp, sandbox, store)
}

/// A database written before T-26 could hold one open room per scope for a pair. The newest
/// open one becomes the pair's active room; the others stay open and usable; a closed one is
/// superseded by the next open.
#[tokio::test]
async fn a_v7_database_keeps_its_rooms_and_gets_one_active_room_per_pair() {
    let (_temp, _sandbox, store) = v7_store(
        8986,
        &[
            "INSERT INTO candidate_contexts VALUES ('project')",
            "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','worker','feature_lead'),('project','peer','feature_lead'),('project','third','feature_lead')",
            "INSERT INTO candidate_route_grants(context_id,first_member,second_member,kind,scope_ref) VALUES ('project','peer','worker','peer','contract-a'),('project','peer','worker','peer','contract-b'),('project','third','worker','peer','contract-c')",
            "INSERT INTO candidate_rooms(context_id,room_id,closed) VALUES ('project','old-a',0),('project','old-b',0),('project','old-c',1)",
            "INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES ('project','old-a','peer'),('project','old-a','worker'),('project','old-b','peer'),('project','old-b','worker'),('project','old-c','third'),('project','old-c','worker')",
            "INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES ('project','old-a','worker','message','a'),('project','old-b','worker','message','b'),('project','old-c','worker','message','c')",
            "INSERT INTO candidate_connections VALUES ('project','peer','worker','peer','contract-a','old-a',1),('project','peer','worker','peer','contract-b','old-b',2),('project','third','worker','peer','contract-c','old-c',3)",
        ],
    )
    .await;
    let worker = bound(&store, "worker").await;
    let joined = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        joined.room, "old-b",
        "the newest open room is the active one"
    );
    assert!(joined.reused);
    assert_eq!(joined.scope_ref, "contract-b");
    store
        .send(&worker, "old-a", "k2", "still open")
        .await
        .unwrap();
    store.close_room(&worker, "old-b", "done").await.unwrap();
    let next = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k3",
            "",
        )
        .await
        .unwrap();
    assert_eq!(next.generation, 2);
    assert_eq!(next.supersedes.as_deref(), Some("old-b"));
    let replaced = store
        .connect_members(
            &worker,
            "third",
            ConnectionKind::Peer,
            "contract-c",
            "k4",
            "",
        )
        .await
        .unwrap();
    assert_eq!(replaced.supersedes.as_deref(), Some("old-c"));
    assert!(!replaced.closed);
    store.close().await;
}

/// A v7 room whose grant or membership was revoked stays open in the rooms table, but it is not
/// usable, so the upgrade never makes it the pair's active room.
#[tokio::test]
async fn a_v7_revoked_room_never_becomes_the_active_room() {
    let (_temp, _sandbox, store) = v7_store(
        8987,
        &[
            "INSERT INTO candidate_contexts VALUES ('project')",
            "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','worker','feature_lead'),('project','peer','feature_lead')",
            "INSERT INTO candidate_route_grants(context_id,first_member,second_member,kind,scope_ref,active) VALUES ('project','peer','worker','peer','contract-a',1),('project','peer','worker','peer','contract-b',0),('project','peer','worker','peer','contract-c',1)",
            "INSERT INTO candidate_rooms(context_id,room_id,closed) VALUES ('project','old-a',0),('project','old-b',0),('project','old-c',0)",
            "INSERT INTO candidate_room_members(context_id,room_id,member_id,active) VALUES ('project','old-a','peer',1),('project','old-a','worker',1),('project','old-b','peer',1),('project','old-b','worker',1),('project','old-c','peer',1),('project','old-c','worker',0)",
            "INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES ('project','old-a','worker','message','a'),('project','old-b','worker','message','b'),('project','old-c','worker','message','c')",
            "INSERT INTO candidate_connections VALUES ('project','peer','worker','peer','contract-a','old-a',1),('project','peer','worker','peer','contract-b','old-b',2),('project','peer','worker','peer','contract-c','old-c',3)",
        ],
    )
    .await;
    let worker = bound(&store, "worker").await;
    let joined = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        joined.room, "old-a",
        "the newest usable room is the active one"
    );
    assert!(joined.reused);
    store.close().await;
}

/// After the upgrade, the room that replaces the pair's active room supersedes that room, not a
/// newer one that was already closed before the upgrade.
#[tokio::test]
async fn a_replacement_after_the_upgrade_supersedes_the_room_it_replaces() {
    let (_temp, _sandbox, store) = v7_store(
        8988,
        &[
            "INSERT INTO candidate_contexts VALUES ('project')",
            "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','worker','feature_lead'),('project','peer','feature_lead')",
            "INSERT INTO candidate_route_grants(context_id,first_member,second_member,kind,scope_ref) VALUES ('project','peer','worker','peer','contract-a'),('project','peer','worker','peer','contract-b')",
            "INSERT INTO candidate_rooms(context_id,room_id,closed) VALUES ('project','old-a',0),('project','old-b',1)",
            "INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES ('project','old-a','peer'),('project','old-a','worker'),('project','old-b','peer'),('project','old-b','worker')",
            "INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES ('project','old-a','worker','message','a'),('project','old-b','worker','message','b')",
            "INSERT INTO candidate_connections VALUES ('project','peer','worker','peer','contract-a','old-a',1),('project','peer','worker','peer','contract-b','old-b',2)",
        ],
    )
    .await;
    let worker = bound(&store, "worker").await;
    store.close_room(&worker, "old-a", "done").await.unwrap();
    let next = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "k1",
            "",
        )
        .await
        .unwrap();
    assert_eq!(next.generation, 2);
    assert_eq!(next.supersedes.as_deref(), Some("old-a"));
    store.close().await;
}

/// Revoking the grant a room was opened under drops it as the pair's active room, so a pair
/// authorized under another scope gets a replacement rather than being stuck for good.
#[tokio::test]
async fn a_revoked_room_never_blocks_the_pair() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let old = open(&store, &worker, "peer", "contract-a", "k1").await;
    store
        .revoke_connection(
            "project",
            "worker",
            "peer",
            ConnectionKind::Peer,
            "contract-a",
        )
        .await
        .unwrap();
    let next = store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-b",
            "k2",
            "",
        )
        .await
        .unwrap();
    assert_ne!(next.room, old, "the revoked room is never restored");
    assert_eq!(next.supersedes.as_deref(), Some(old.as_str()));
    assert!(store.send(&worker, &old, "k3", "revoked").await.is_err());
    store.close().await;
}

/// A removed member that is registered and authorized again gets a new room with the pair.
#[tokio::test]
async fn a_removed_member_never_blocks_the_pair() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let old = open(&store, &worker, "peer", "contract-a", "k1").await;
    store.remove_member("project", "worker").await.unwrap();
    store
        .register_member("project", "worker", Role::FeatureLead)
        .await
        .unwrap();
    store
        .authorize_connection(
            "project",
            "worker",
            "peer",
            ConnectionKind::Peer,
            "contract-a",
        )
        .await
        .unwrap();
    let back = bound(&store, "worker").await;
    let next = open(&store, &back, "peer", "contract-a", "k2").await;
    assert_ne!(next, old);
    store.close().await;
}

/// A retirement note fits the same one-line cap as any close reason.
#[tokio::test]
async fn a_retirement_reason_fits_the_close_reason_cap() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
    let room = open(&store, &worker, "peer", "contract-a", "k1").await;
    drain_kinds(&store, &peer).await;
    assert!(store
        .retire_member("project", "worker", &"x".repeat(200))
        .await
        .is_err());
    store
        .retire_member("project", "worker", &"x".repeat(150))
        .await
        .unwrap();
    let seen = drain_kinds(&store, &peer).await;
    assert_eq!(seen[0].0, room);
    assert!(seen[0].2.chars().count() <= 200);
    store.close().await;
}
