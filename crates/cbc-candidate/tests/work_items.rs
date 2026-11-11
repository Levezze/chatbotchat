//! Work items (T-07, T-09): one owner, atomic multi-field revisions, the HITL flag, and one
//! outbox event per revision delivered through the inbox. Real SQLite only.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    enrollment::ConnectionKind,
    facts::ItemClass,
    store::Store,
    work::{Actor, Stage, WorkOutcome, WorkPatch},
    Sandbox,
};
use serde_json::Value;
use tempfile::{tempdir, TempDir};

struct Case {
    _temp: TempDir,
    _sandbox: Sandbox,
    store: Store,
    worker: SessionBinding,
    lead: SessionBinding,
    other: SessionBinding,
    report_room: String,
}

async fn fixture() -> Case {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8989,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for (member, role) in [
        ("worker", Role::FeatureLead),
        ("lead", Role::RepoLead),
        ("other", Role::FeatureLead),
    ] {
        store
            .register_member("project", member, role)
            .await
            .unwrap();
    }
    let bind = |m: &'static str| {
        let store = &store;
        async move {
            store
                .bind_member("project", m, "device", &format!("{m}-session"))
                .await
                .unwrap()
        }
    };
    let worker = bind("worker").await;
    let lead = bind("lead").await;
    let other = bind("other").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature",
        )
        .await
        .unwrap();
    let report_room = store
        .connect_members(&worker, "lead", ConnectionKind::Report, "feature", "r1", "")
        .await
        .unwrap()
        .room;
    let case = Case {
        _temp: temp,
        _sandbox: sandbox,
        store,
        worker,
        lead,
        other,
        report_room,
    };
    // Start every test from an empty inbox: the introduction is not a work event.
    drain(&case.store, &case.worker).await;
    drain(&case.store, &case.lead).await;
    case
}

/// Every work event a member receives, as (room, parsed body). Receipts everything it reads.
async fn drain(store: &Store, who: &SessionBinding) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    while let Some(batch) = store.check(who, 32).await.unwrap() {
        for item in &batch.items {
            if item.kind == "work_event" {
                assert_eq!(item.sender, "cbc", "a work event is server information");
                events.push((item.room.clone(), serde_json::from_str(&item.body).unwrap()));
            }
        }
        store
            .receipt(who, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    events
}

fn patch() -> WorkPatch {
    WorkPatch::default()
}

#[tokio::test]
async fn registering_a_work_item_records_revision_one_and_wakes_the_lead() {
    let case = fixture().await;
    let item = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert_eq!(item.owner, "worker");
    assert_eq!(item.revision, 1);
    assert_eq!(item.stage, Stage::Planning);
    assert!(
        !item.hitl,
        "HITL starts off; only the user or a repo lead turns it on"
    );

    let events = drain(&case.store, &case.lead).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, case.report_room);
    let body = &events[0].1;
    assert_eq!(body["item"], "feat-login");
    assert_eq!(body["revision"], 1);
    assert_eq!(body["actor"], "worker");
    assert_eq!(body["changed"], serde_json::json!(["registered"]));
    assert!(
        drain(&case.store, &case.worker).await.is_empty(),
        "the actor is not woken by its own change"
    );
}

#[tokio::test]
async fn a_patch_commits_one_revision_and_one_event_for_all_its_fields() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    drain(&case.store, &case.lead).await;

    let outcome = case
        .store
        .update_work_item(
            &case.worker,
            "feat-login",
            1,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Implementing),
                next_action: Some("write the session tests".into()),
                branch: Some(Some("feat/login".into())),
                ..patch()
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome, WorkOutcome::Applied { revision: 2 });

    let item = case
        .store
        .work_item("project", "feat-login")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.revision, 2);
    assert_eq!(item.stage, Stage::Implementing);
    assert_eq!(item.next_action, "write the session tests");
    assert_eq!(item.branch.as_deref(), Some("feat/login"));

    let events = drain(&case.store, &case.lead).await;
    assert_eq!(events.len(), 1, "one event per revision, not per field");
    assert_eq!(
        events[0].1["changed"],
        serde_json::json!(["branch", "next_action", "stage"])
    );
    assert_eq!(events[0].1["stage"], "implementing");

    let revisions = case
        .store
        .work_revisions("project", "feat-login")
        .await
        .unwrap();
    assert_eq!(
        revisions.iter().map(|r| r.revision).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(revisions[1].actor, Actor::Member("worker".into()));
}

#[tokio::test]
async fn a_stale_expected_revision_changes_nothing() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
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

    let error = case
        .store
        .update_work_item(
            &case.worker,
            "feat-login",
            1,
            "u-2",
            &WorkPatch {
                stage: Some(Stage::InReview),
                ..patch()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revision conflict"), "{error}");
    let item = case
        .store
        .work_item("project", "feat-login")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.revision, 2);
    assert_eq!(item.stage, Stage::Implementing);
    assert!(drain(&case.store, &case.lead).await.is_empty());
}

#[tokio::test]
async fn a_patch_that_changes_nothing_records_nothing_and_wakes_nobody() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    drain(&case.store, &case.lead).await;

    let outcome = case
        .store
        .update_work_item(
            &case.worker,
            "feat-login",
            1,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Planning),
                summary: Some("Login flow".into()),
                ..patch()
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome, WorkOutcome::NoOp { revision: 1 });
    assert_eq!(
        case.store
            .work_revisions("project", "feat-login")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(drain(&case.store, &case.lead).await.is_empty());

    let empty = case
        .store
        .update_work_item(&case.worker, "feat-login", 1, "u-2", &patch())
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("empty patch"), "{empty}");
}

#[tokio::test]
async fn a_replayed_key_returns_the_first_result_and_a_reused_key_is_refused() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    let first = WorkPatch {
        stage: Some(Stage::Implementing),
        ..patch()
    };
    let applied = case
        .store
        .update_work_item(&case.worker, "feat-login", 1, "u-1", &first)
        .await
        .unwrap();
    let replayed = case
        .store
        .update_work_item(&case.worker, "feat-login", 1, "u-1", &first)
        .await
        .unwrap();
    assert_eq!(applied, replayed, "a lost reply is retried safely");
    assert_eq!(
        case.store
            .work_revisions("project", "feat-login")
            .await
            .unwrap()
            .len(),
        2
    );

    let reused = case
        .store
        .update_work_item(
            &case.worker,
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
    assert!(
        reused.to_string().contains("idempotency key conflicts"),
        "{reused}"
    );

    let again = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert_eq!(
        again.revision, 2,
        "a replayed registration returns the one item as it is now, never a second item"
    );
    let duplicate = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-2")
        .await
        .unwrap_err();
    assert!(
        duplicate.to_string().contains("already registered"),
        "{duplicate}"
    );
}

#[tokio::test]
async fn only_a_feature_lead_registers_and_only_the_owner_patches() {
    let case = fixture().await;
    let refused = case
        .store
        .register_work_item(&case.lead, "lead-item", "Not mine to own", "reg-1")
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("not authorized"), "{refused}");

    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    for who in [&case.lead, &case.other] {
        let error = case
            .store
            .update_work_item(
                who,
                "feat-login",
                1,
                "u-1",
                &WorkPatch {
                    stage: Some(Stage::Abandoned),
                    ..patch()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not authorized"), "{error}");
    }
    let unknown = case
        .store
        .update_work_item(
            &case.worker,
            "missing",
            1,
            "u-2",
            &WorkPatch {
                stage: Some(Stage::Abandoned),
                ..patch()
            },
        )
        .await
        .unwrap_err();
    assert!(
        unknown.to_string().contains("unknown work item"),
        "{unknown}"
    );
}

/// T-07: the user or a repo lead turns HITL on; only the user turns it off; the owner cannot
/// change it either way.
#[tokio::test]
async fn hitl_is_set_by_the_lead_or_the_user_and_cleared_only_by_the_user() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    drain(&case.store, &case.lead).await;

    for on in [true, false] {
        let error = case
            .store
            .set_work_hitl(&case.worker, "feat-login", on, &format!("w-{on}"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not authorized"), "{error}");
    }

    let on = case
        .store
        .set_work_hitl(&case.lead, "feat-login", true, "h-1")
        .await
        .unwrap();
    assert_eq!(on, WorkOutcome::Applied { revision: 2 });
    let events = drain(&case.store, &case.worker).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["changed"], serde_json::json!(["hitl"]));
    assert_eq!(events[0].1["hitl"], true);
    assert_eq!(events[0].1["actor"], "lead");

    let off = case
        .store
        .set_work_hitl(&case.lead, "feat-login", false, "h-2")
        .await
        .unwrap_err();
    assert!(off.to_string().contains("only the user"), "{off}");

    let cleared = case
        .store
        .owner_set_work_hitl("project", "feat-login", false, "user-1")
        .await
        .unwrap();
    assert_eq!(cleared, WorkOutcome::Applied { revision: 3 });
    let revisions = case
        .store
        .work_revisions("project", "feat-login")
        .await
        .unwrap();
    assert_eq!(revisions[2].actor, Actor::Owner);
    assert_eq!(revisions[2].changed, vec!["hitl".to_string()]);
    // The user is not a room member, so both members hear about the user's change.
    assert_eq!(drain(&case.store, &case.worker).await.len(), 1);
    assert_eq!(drain(&case.store, &case.lead).await.len(), 1);

    let again = case
        .store
        .owner_set_work_hitl("project", "feat-login", false, "user-2")
        .await
        .unwrap();
    assert_eq!(again, WorkOutcome::NoOp { revision: 3 });

    let content = case
        .store
        .update_work_item(
            &case.worker,
            "feat-login",
            3,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Implementing),
                ..patch()
            },
        )
        .await
        .unwrap();
    assert_eq!(content, WorkOutcome::Applied { revision: 4 });
    assert!(
        !case
            .store
            .work_item("project", "feat-login")
            .await
            .unwrap()
            .unwrap()
            .hitl,
        "a content patch never touches HITL"
    );
}

/// The record never waits on transport: with no report room the revision still commits, and
/// the item view shows it was not delivered.
#[tokio::test]
async fn a_revision_commits_without_a_report_room_and_is_marked_undelivered() {
    let case = fixture().await;
    let item = case
        .store
        .register_work_item(&case.other, "feat-other", "No lead yet", "reg-1")
        .await
        .unwrap();
    assert_eq!(item.revision, 1);
    let revisions = case
        .store
        .work_revisions("project", "feat-other")
        .await
        .unwrap();
    assert_eq!(revisions.len(), 1);
    assert!(revisions[0].delivered_to.is_empty());

    let delivered = case
        .store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert_eq!(delivered.revision, 1);
    let revisions = case
        .store
        .work_revisions("project", "feat-login")
        .await
        .unwrap();
    assert_eq!(revisions[0].delivered_to, vec![case.report_room.clone()]);
}

/// Work events ride the inbox, so the #113 PR-A delivery facts cover them with no second path.
#[tokio::test]
async fn a_work_event_has_delivery_facts_like_any_other_item() {
    let case = fixture().await;
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    let revisions = case
        .store
        .work_revisions("project", "feat-login")
        .await
        .unwrap();
    let sequence = revisions[0].event_sequences[0];
    drain(&case.store, &case.lead).await;
    let timeline = case
        .store
        .delivery_timeline("project", sequence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(timeline.class, Some(ItemClass::WorkEvent));
    assert_eq!(timeline.recipients.len(), 1);
    assert_eq!(timeline.recipients[0].member, "lead");
    assert!(timeline.recipients[0].expected);
    assert!(timeline.recipients[0].receipted.is_some());
}

#[tokio::test]
async fn text_fields_are_bounded() {
    let case = fixture().await;
    for (summary, needle) in [
        ("", "summary"),
        ("  ", "summary"),
        (&"x".repeat(501) as &str, "summary"),
    ] {
        let error = case
            .store
            .register_work_item(&case.worker, "feat-login", summary, "reg-x")
            .await
            .unwrap_err();
        assert!(error.to_string().contains(needle), "{error}");
    }
    case.store
        .register_work_item(&case.worker, "feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    for (bad, needle) in [
        (
            WorkPatch {
                branch: Some(Some("feat/a\nb".into())),
                ..patch()
            },
            "branch",
        ),
        (
            WorkPatch {
                blocked_operation: Some(Some("x".repeat(201))),
                ..patch()
            },
            "blocked_operation",
        ),
        (
            WorkPatch {
                next_action: Some("x".repeat(501)),
                ..patch()
            },
            "next_action",
        ),
    ] {
        let error = case
            .store
            .update_work_item(&case.worker, "feat-login", 1, "u-bad", &bad)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(needle), "{error}");
    }
    assert_eq!(
        case.store
            .work_item("project", "feat-login")
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}

/// 0009 rebuilds the delivery-facts table to admit work events. Every fact recorded before the
/// upgrade survives it unchanged.
#[tokio::test]
async fn the_upgrade_keeps_every_delivery_fact_recorded_before_it() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8990,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let v8 = temp.path().join("migrations-v8");
    std::fs::create_dir(&v8).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.as_str() < "0009" {
            std::fs::copy(&path, v8.join(name)).unwrap();
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
    sqlx::migrate::Migrator::new(v8.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let instance = "0123456789abcdef0123456789abcdef";
    for statement in [
        "INSERT INTO candidate_contexts(context_id) VALUES ('project')".to_string(),
        "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','a','feature_lead'),('project','b','repo_lead')".into(),
        "INSERT INTO candidate_rooms(context_id,room_id) VALUES ('project','r')".into(),
        "INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES ('project','r','a'),('project','r','b')".into(),
        "INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES ('project','r','a','message','hello')".into(),
        format!("INSERT INTO candidate_core_instances(instance_id,started_wall_ms) VALUES ('{instance}',1)"),
        format!("INSERT INTO candidate_item_facts(sequence,context_id,room_id,item_class,committed_instance,committed_mono_us,committed_wall_ms) VALUES (1,'project','r','participant','{instance}',5,1000)"),
        format!("INSERT INTO candidate_recipient_facts(sequence,member_id,expected,receipted_instance,receipted_mono_us,receipted_wall_ms) VALUES (1,'b',1,'{instance}',9,1004)"),
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
    let timeline = store
        .delivery_timeline("project", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(timeline.class, Some(ItemClass::Participant));
    assert_eq!(timeline.committed.as_ref().unwrap().wall_ms, 1000);
    assert_eq!(timeline.recipients.len(), 1);
    assert_eq!(timeline.recipients[0].member, "b");
    assert!(timeline.recipients[0].expected);
    assert_eq!(
        timeline.recipients[0].receipted.as_ref().unwrap().wall_ms,
        1004
    );
}
