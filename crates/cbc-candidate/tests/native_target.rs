//! Synthetic trusted-adapter evidence, not a claim of native application placement.
use cbc_candidate::{
    bindings::Role,
    enrollment::{NativeHost, SessionEvidence},
    native_target::NativeTarget,
    store::Store,
    Sandbox,
};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt};
use tempfile::{tempdir, TempDir};

fn target_json() -> Value {
    json!({"v":1,"profile":"desktop","repository":"repo","worktree":"/workspace",
        "cwd":"/workspace/one","saved_project":{"status":"not_exposed"},
        "saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"synthetic-ui",
            "project_id":"synthetic-project-id","environment_id":"synthetic-environment-id",
            "project_label":"Disposable project","environment_label":"This computer"}})
}

fn target() -> NativeTarget {
    serde_json::from_value(target_json()).unwrap()
}

fn evidence(target: Option<NativeTarget>) -> SessionEvidence {
    let proof = SessionEvidence::from_trusted_adapter(
        NativeHost::Claude,
        "device",
        "native",
        "synthetic-process",
    )
    .unwrap();
    match target {
        Some(target) => proof.with_target(target).unwrap(),
        None => proof,
    }
}

async fn fixture() -> (TempDir, Sandbox, Store) {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    store
        .register_member("project", "worker", Role::FeatureLead)
        .await
        .unwrap();
    (temp, sandbox, store)
}

#[test]
fn configured_target_shape_bounds_and_lexical_paths_are_strict() {
    target().validate().unwrap();
    assert_eq!(serde_json::to_value(target()).unwrap(), target_json());
    for field in ["project_id", "environment_id"] {
        for value in [Value::Null, json!(7)] {
            let mut changed = target_json();
            changed["placement"][field] = value;
            assert!(serde_json::from_value::<NativeTarget>(changed).is_err());
        }
        for value in ["", "  ", "bad\n", &"x".repeat(129)] {
            let mut changed = target_json();
            changed["placement"][field] = json!(value);
            assert!(serde_json::from_value::<NativeTarget>(changed)
                .unwrap()
                .validate()
                .is_err());
        }
    }
    let mut valid = target_json();
    valid["cwd"] = json!("/a/future/device/path");
    valid["worktree"] = Value::Null;
    serde_json::from_value::<NativeTarget>(valid)
        .unwrap()
        .validate()
        .unwrap();
    // Each lexical path fits its own byte bound, while JSON escaping exceeds
    // the total record bound. This must fail before an invitation is persisted.
    let mut oversized = target_json();
    let escaped_path = format!("/{}", "\"".repeat(4095));
    oversized["cwd"] = json!(escaped_path);
    oversized["worktree"] = oversized["cwd"].clone();
    assert!(serde_json::from_value::<NativeTarget>(oversized)
        .unwrap()
        .validate()
        .is_err());
    for (field, value) in [
        ("v", json!(2)),
        ("cwd", json!("relative")),
        ("cwd", json!("/workspace/../other")),
        ("cwd", json!("/workspace/./one")),
        ("cwd", json!("/workspace//one")),
        ("cwd", json!("/workspace/one/")),
        ("cwd", json!("/workspace-elsewhere/one")),
        ("cwd", json!(format!("/{}", "x".repeat(4096)))),
        ("worktree", json!("/elsewhere")),
        ("repository", json!("x".repeat(129))),
    ] {
        let mut changed = target_json();
        changed[field] = value;
        assert!(
            serde_json::from_value::<NativeTarget>(changed)
                .unwrap()
                .validate()
                .is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("profile", json!("headless")),
        ("saved_project", json!({"status":"configured"})),
        (
            "saved_environment",
            json!({"status":"not_exposed","id":"invented"}),
        ),
        (
            "saved_environment",
            json!({"status":"not_exposed","id":null}),
        ),
        ("saved_project", json!({"status":"configured","id":null})),
        (
            "placement",
            json!({"evidence_ref":"ui","permissions":"all"}),
        ),
        (
            "placement",
            json!({"evidence_ref":"ui","project_label":null}),
        ),
        ("unexpected", json!(true)),
    ] {
        let mut changed = target_json();
        changed[field] = value;
        assert!(
            serde_json::from_value::<NativeTarget>(changed).is_err(),
            "{field}"
        );
    }
    let mut missing = target_json();
    missing.as_object_mut().unwrap().remove("worktree");
    assert!(serde_json::from_value::<NativeTarget>(missing).is_err());
    for id in ["", "  ", "bad\n", &"x".repeat(129)] {
        let mut changed = target_json();
        changed["saved_project"] = json!({"status":"configured","id":id});
        assert!(serde_json::from_value::<NativeTarget>(changed)
            .unwrap()
            .validate()
            .is_err());
    }
}

#[tokio::test]
async fn invite_rejects_each_changed_target_and_cannot_be_downgraded_to_legacy() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = evidence(Some(target()));
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &evidence(None))
        .await
        .is_err());
    for (field, value) in [
        ("cwd", json!("/workspace/two")),
        ("worktree", json!(null)),
        ("repository", json!("other")),
        ("profile", json!("cli")),
        (
            "saved_project",
            json!({"status":"configured","id":"project-other"}),
        ),
        (
            "saved_environment",
            json!({"status":"configured","id":"environment-other"}),
        ),
        ("placement", json!({"evidence_ref":"different-observation"})),
    ] {
        let mut changed = target_json();
        changed[field] = value;
        let changed = evidence(Some(serde_json::from_value(changed).unwrap()));
        assert!(
            store
                .consume_enrollment_invite(invite.expose_token_for_adapter(), &changed)
                .await
                .is_err(),
            "{field}"
        );
    }
    for field in ["project_id", "environment_id"] {
        let mut changed = target_json();
        changed["placement"][field] = json!("different-audited-id");
        let changed = evidence(Some(serde_json::from_value(changed).unwrap()));
        assert!(
            store
                .consume_enrollment_invite(invite.expose_token_for_adapter(), &changed)
                .await
                .is_err(),
            "{field}"
        );
    }
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    assert_eq!(store.native_target(&binding).await.unwrap(), Some(target()));
    assert_eq!(
        store
            .binding_activity(&binding)
            .await
            .unwrap()
            .unwrap()
            .native_target,
        Some(target())
    );
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn target_invite_and_exact_epoch_survive_restart_without_leaking_to_later_bindings() {
    let (_temp, sandbox, store) = fixture().await;
    let proof = evidence(Some(target()));
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    assert_eq!(store.native_target(&binding).await.unwrap(), Some(target()));
    let new_binding = store
        .bind_member("project", "worker", "device", "replacement-native")
        .await
        .unwrap();
    assert!(store.native_target(&binding).await.is_err());
    assert_eq!(store.binding_activity(&binding).await.unwrap(), None);
    assert_eq!(store.native_target(&new_binding).await.unwrap(), None);
    let legacy_activity = store.binding_activity(&new_binding).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(legacy_activity).unwrap()["native_target"],
        Value::Null
    );
    store.close().await;
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(sandbox.database_path())
            .read_only(true),
    )
    .await
    .unwrap();
    let epochs: Vec<i64> = sqlx::query_scalar("SELECT epoch FROM candidate_native_targets")
        .fetch_all(&mut db)
        .await
        .unwrap();
    assert_eq!(epochs, vec![binding.epoch()]);
    db.close().await.unwrap();
}

#[tokio::test]
async fn revoked_target_invite_cannot_rotate_binding_and_legacy_invites_stay_legacy() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = evidence(Some(target()));
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    store.unbind_member("project", "worker").await.unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    let legacy = evidence(None);
    let invite = store
        .issue_enrollment_invite("project", "worker", &legacy, 60)
        .await
        .unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &legacy)
        .await
        .unwrap();
    assert_eq!(store.native_target(&binding).await.unwrap(), None);
    store.close().await;
}

#[tokio::test]
async fn native_target_migration_keeps_real_legacy_history_and_does_not_invent_placement() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
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
        .bind_member("v4-upgrade-kept", "lead", "fixture-device", "lead-native")
        .await
        .unwrap();
    assert_eq!(store.native_target(&lead).await.unwrap(), None);
    let mut sequences = Vec::new();
    while let Some(batch) = store.check(&lead, 32).await.unwrap() {
        sequences.extend(batch.items.iter().map(|item| item.sequence));
        store
            .receipt(&lead, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    assert_eq!(sequences.len(), 100);
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    store.close().await;
}

#[tokio::test]
async fn failed_target_persistence_rolls_back_binding_rotation_and_invite_consumption() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let (_temp, sandbox, store) = fixture().await;
    let proof = evidence(Some(target()));
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    store.close().await;
    let options = SqliteConnectOptions::new().filename(sandbox.database_path());
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("ALTER TABLE candidate_native_targets RENAME TO temporarily_unavailable_targets")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    store.close().await;
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("ALTER TABLE temporarily_unavailable_targets RENAME TO candidate_native_targets")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    assert_eq!(binding.epoch(), 1);
    assert_eq!(store.native_target(&binding).await.unwrap(), Some(target()));
    store.close().await;
}

#[test]
fn status_without_explicit_native_target_is_unsupported_instead_of_legacy() {
    use cbc_candidate::bindings::BindingActivity;
    let mut status = json!({"last_receipted_sequence":0,"first_unread_sequence":null});
    assert!(serde_json::from_value::<BindingActivity>(status.clone()).is_err());
    status["native_target"] = Value::Null;
    assert_eq!(
        serde_json::from_value::<BindingActivity>(status)
            .unwrap()
            .native_target,
        None
    );
}

#[tokio::test]
async fn private_binding_status_wire_reports_authoritative_target_or_explicit_legacy_null() {
    use cbc_candidate::{bindings::SessionBinding, connector::LocalConnector};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
    };
    async fn status(
        path: &std::path::Path,
        member: &str,
        session: &str,
        binding: &SessionBinding,
    ) -> Value {
        let mut connection = UnixStream::connect(path).await.unwrap();
        let request = json!({"v":1,"auth":{"context":"project","member":member,"device":"device",
            "session":session,"epoch":binding.epoch(),"token":binding.expose_token_for_adapter()},
            "request":{"op":"binding_status"}});
        connection
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        BufReader::new(connection)
            .read_line(&mut line)
            .await
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let temp = tempfile::Builder::new()
        .prefix("cbc-target-wire-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["worker", "legacy"] {
        store
            .register_member("project", member, Role::FeatureLead)
            .await
            .unwrap();
    }
    let proof = evidence(Some(target()));
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    let legacy = store
        .bind_member("project", "legacy", "device", "legacy-native")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    let configured = status(&path, "worker", "native", &binding).await;
    assert_eq!(configured["type"], "binding_status");
    assert_eq!(configured["native_target"], target_json());
    let legacy = status(&path, "legacy", "legacy-native", &legacy).await;
    assert_eq!(legacy["type"], "binding_status");
    assert!(legacy.as_object().unwrap().contains_key("native_target"));
    assert_eq!(legacy["native_target"], Value::Null);
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn target_status_corruption_and_storage_failure_are_errors_not_legacy_or_empty() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    for fault in [
        "UPDATE candidate_native_targets SET target_json='not-json'",
        "UPDATE candidate_native_targets SET target_json='{}'",
        "UPDATE candidate_native_targets SET target_digest=zeroblob(32)",
        "UPDATE candidate_native_targets SET device_id='different-device'",
        "DELETE FROM candidate_native_targets",
        "ALTER TABLE candidate_native_targets RENAME TO unavailable_targets",
    ] {
        let (_temp, sandbox, store) = fixture().await;
        let proof = evidence(Some(target()));
        let invite = store
            .issue_enrollment_invite("project", "worker", &proof, 60)
            .await
            .unwrap();
        let binding = store
            .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
            .await
            .unwrap();
        store.close().await;
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(sandbox.database_path()),
        )
        .await
        .unwrap();
        sqlx::query(fault).execute(&mut db).await.unwrap();
        db.close().await.unwrap();
        let store = Store::open(&sandbox).await.unwrap();
        assert!(store.current_member(&binding).await.is_ok());
        assert!(store.binding_activity(&binding).await.is_err(), "{fault}");
        assert!(store.native_target(&binding).await.is_err(), "{fault}");
        store.close().await;
    }
}
