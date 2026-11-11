//! Work-item follow-ups (R2c): a no-op binds its key, a replayed registration says so, and a
//! report room connected after its child's items changed catches the parent up. Real SQLite only.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::{
    bindings::Role,
    enrollment::ConnectionKind,
    store::Store,
    work::{Stage, WorkOutcome, WorkPatch},
    Sandbox,
};
use tempfile::tempdir;
use work_fixture::{drain, fixture, patch};
/// A no-op still binds its key. Otherwise a delayed retry of an old "HITL off", sent while HITL
/// was already off, would clear a hold the lead set in between: an old request turned into a
/// fresh human decision.
#[tokio::test]
async fn a_noop_binds_its_key_so_a_stale_retry_cannot_clear_a_newer_hold() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    let first = case
        .store
        .owner_set_work_hitl("project", "feat-login", false, "user-off")
        .await
        .unwrap();
    assert_eq!(first, WorkOutcome::NoOp { revision: 1 });
    case.store
        .set_work_hitl(&case.lead, "feat-login", true, "lead-on")
        .await
        .unwrap();
    let retried = case
        .store
        .owner_set_work_hitl("project", "feat-login", false, "user-off")
        .await
        .unwrap();
    assert_eq!(
        retried,
        WorkOutcome::NoOp { revision: 1 },
        "the first outcome"
    );
    let item = case
        .store
        .work_item("project", "feat-login")
        .await
        .unwrap()
        .unwrap();
    assert!(item.hitl, "the lead's hold survives the stale retry");
    assert_eq!(item.revision, 2);
    let reused = case
        .store
        .owner_set_work_hitl("project", "feat-login", true, "user-off")
        .await
        .unwrap_err();
    assert!(reused.to_string().contains("idempotency key"), "{reused}");
}

/// The same holds for a patch: a no-op patch binds its key, and the key can't be reused for a
/// different change.
#[tokio::test]
async fn a_noop_patch_binds_its_key() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    let same = WorkPatch {
        stage: Some(Stage::Planning),
        ..patch()
    };
    assert_eq!(
        case.store
            .update_work_item(&case.worker, "feat-login", 1, "p-1", &same)
            .await
            .unwrap(),
        WorkOutcome::NoOp { revision: 1 }
    );
    assert_eq!(
        case.store
            .update_work_item(&case.worker, "feat-login", 1, "p-1", &same)
            .await
            .unwrap(),
        WorkOutcome::NoOp { revision: 1 },
        "a retry returns the first outcome"
    );
    let different = WorkPatch {
        stage: Some(Stage::Implementing),
        ..patch()
    };
    let error = case
        .store
        .update_work_item(&case.worker, "feat-login", 1, "p-1", &different)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("idempotency key"), "{error}");
    assert_eq!(
        case.store
            .work_item("project", "feat-login")
            .await
            .unwrap()
            .unwrap()
            .stage,
        Stage::Planning
    );
}

/// A replayed registration says it is a replay and shows the current state. It records nothing
/// and wakes nobody.
#[tokio::test]
async fn a_replayed_registration_says_so() {
    let case = fixture().await;
    let first = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.item.revision, 1);
    case.store
        .update_work_item(
            &case.worker,
            "feat-login",
            1,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Implementing),
                ..patch()
            },
        )
        .await
        .unwrap();
    drain(&case.store, &case.lead).await;
    let again = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert!(again.replayed);
    assert_eq!(again.item.revision, 2, "the current state, not revision 1");
    assert!(drain(&case.store, &case.lead).await.is_empty());
}

/// An item registered before its owner's report room exists reaches the lead when the room
/// connects: one current snapshot per open item, none for finished ones, and no intermediate
/// history. A reused room sends nothing again; a replacement room after a close does.
#[tokio::test]
async fn an_item_registered_before_the_report_room_reaches_the_lead_on_connect() {
    let case = fixture().await;
    case.store
        .register_member("project", "late", Role::FeatureLead)
        .await
        .unwrap();
    let late = case
        .store
        .bind_member("project", "late", "device", "late-session")
        .await
        .unwrap();
    for (item, key) in [("feat-a", "a"), ("feat-b", "b"), ("feat-done", "d")] {
        case.store
            .register_work_item(&late, item, "Work", key)
            .await
            .unwrap();
    }
    case.store
        .update_work_item(
            &late,
            "feat-a",
            1,
            "a-1",
            &WorkPatch {
                stage: Some(Stage::Implementing),
                ..patch()
            },
        )
        .await
        .unwrap();
    case.store
        .update_work_item(
            &late,
            "feat-done",
            1,
            "d-1",
            &WorkPatch {
                stage: Some(Stage::Abandoned),
                ..patch()
            },
        )
        .await
        .unwrap();
    case.store
        .authorize_connection("project", "late", "lead", ConnectionKind::Report, "late")
        .await
        .unwrap();
    let room = case
        .store
        .connect_members(&late, "lead", ConnectionKind::Report, "late", "c-1", "")
        .await
        .unwrap()
        .room;

    let mut events = drain(&case.store, &case.lead).await;
    events.sort_by(|a, b| a.1["item"].as_str().cmp(&b.1["item"].as_str()));
    assert_eq!(events.len(), 2, "{events:?}");
    for (event_room, body) in &events {
        assert_eq!(event_room, &room);
        assert_eq!(body["snapshot"], true);
    }
    assert_eq!(events[0].1["item"], "feat-a");
    assert_eq!(events[0].1["revision"], 2);
    assert_eq!(events[0].1["stage"], "implementing");
    assert_eq!(events[1].1["item"], "feat-b");
    assert!(
        drain(&case.store, &late).await.is_empty(),
        "the owner knows its own items"
    );
    // History is the record of change: a catch-up is not a delivery of revision 2.
    let history = case
        .store
        .work_revisions("project", "feat-a")
        .await
        .unwrap();
    assert!(
        history.iter().all(|r| r.delivered_to.is_empty()),
        "{history:?}"
    );

    case.store
        .connect_members(&late, "lead", ConnectionKind::Report, "late", "c-2", "")
        .await
        .unwrap();
    assert!(
        drain(&case.store, &case.lead).await.is_empty(),
        "a reused room"
    );

    case.store
        .close_room(&late, &room, "rotating")
        .await
        .unwrap();
    case.store
        .connect_members(&late, "lead", ConnectionKind::Report, "late", "c-3", "")
        .await
        .unwrap();
    assert_eq!(drain(&case.store, &case.lead).await.len(), 2);
}

/// Upgrading from schema 9 binds every key a revision already used, so a key from before the
/// upgrade is still refused for a different request.
#[tokio::test]
async fn the_upgrade_binds_every_key_used_before_it() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8992,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let v9 = temp.path().join("migrations-v9");
    std::fs::create_dir(&v9).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.as_str() < "0010" {
            std::fs::copy(&path, v9.join(name)).unwrap();
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
    sqlx::migrate::Migrator::new(v9.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let fingerprint = "x'0000000000000000000000000000000000000000000000000000000000000000'";
    for statement in [
        "INSERT INTO candidate_contexts(context_id) VALUES ('project')".to_string(),
        "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','worker','feature_lead')".into(),
        "INSERT INTO candidate_work_items(context_id,item_id,owner_member,summary,stage,revision,created_wall_ms,updated_wall_ms) VALUES ('project','feat-login','worker','Login flow','implementing',2,1,2)".into(),
        format!("INSERT INTO candidate_work_revisions(context_id,item_id,revision,actor_kind,actor_member,request_key,fingerprint,changed,at_wall_ms) VALUES ('project','feat-login',1,'member','worker','reg-1',{fingerprint},'[\"registered\"]',1),('project','feat-login',2,'member','worker','u-1',{fingerprint},'[\"stage\"]',2)"),
    ] {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    pool.close().await;
    std::fs::set_permissions(
        sandbox.database_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    let store = Store::open(&sandbox).await.unwrap();
    let worker = store
        .bind_member("project", "worker", "device", "worker-session")
        .await
        .unwrap();
    let error = store
        .update_work_item(
            &worker,
            "feat-login",
            2,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::InReview),
                ..patch()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("idempotency key"), "{error}");
    assert_eq!(
        store
            .work_item("project", "feat-login")
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
}
