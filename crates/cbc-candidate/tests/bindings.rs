use cbc_candidate::{
    bindings::{Role, SessionBinding},
    store::Store,
    Sandbox,
};
use tempfile::tempdir;

#[tokio::test]
async fn epoch_exhaustion_reserves_a_final_revocation_and_never_wraps() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["unbind", "remove", "revoke"] {
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
    }
    store.close().await;
    // Boundary fault injection into this closed disposable candidate, never a daily database.
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(sandbox.database_path())
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE candidate_members SET epoch = ?")
        .bind(i64::MAX - 2)
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    for member in ["unbind", "remove", "revoke"] {
        let last = store
            .bind_member("project", member, "laptop", "last")
            .await
            .unwrap();
        assert_eq!(last.epoch(), i64::MAX - 1);
        assert!(store
            .bind_member("project", member, "mini", "too-late")
            .await
            .is_err());
        assert!(store.current_member(&last).await.is_ok());
        if member == "unbind" {
            store.unbind_member("project", member).await.unwrap();
        } else if member == "revoke" {
            assert_eq!(
                store
                    .revoke_binding("project", member, last.epoch())
                    .await
                    .unwrap(),
                i64::MAX
            );
            assert!(store
                .revoke_binding("project", member, last.epoch())
                .await
                .is_err());
            assert!(store
                .revoke_binding("project", member, i64::MAX)
                .await
                .is_err());
        } else {
            store.remove_member("project", member).await.unwrap();
        }
        assert!(store.current_member(&last).await.is_err());
        assert!(store
            .bind_member("project", member, "laptop", "after-revoke")
            .await
            .is_err());
        assert!(store.unbind_member("project", member).await.is_err());
        assert!(store.remove_member("project", member).await.is_err());
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
        assert!(store
            .bind_member("project", member, "laptop", "new")
            .await
            .is_err());
    }
    store.close().await;
}

#[tokio::test]
async fn exact_epoch_revocation_is_single_use_and_cannot_revoke_a_newer_binding() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    for expected in [-1, 0, 1, i64::MAX] {
        assert!(store
            .revoke_binding("project", "lead", expected)
            .await
            .is_err());
    }
    assert!(store.revoke_binding("project", "absent", 1).await.is_err());
    assert!(store.revoke_binding("project", " \n", 1).await.is_err());
    let old = store
        .bind_member("project", "lead", "laptop", "old")
        .await
        .unwrap();
    assert_eq!(old.epoch(), 1);
    assert!(store.revoke_binding("project", "lead", 2).await.is_err());
    assert!(store.current_member(&old).await.is_ok());
    let (first, second) = tokio::join!(
        store.revoke_binding("project", "lead", old.epoch()),
        store.revoke_binding("project", "lead", old.epoch()),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(first.or(second).unwrap(), old.epoch() + 1);
    assert!(store.current_member(&old).await.is_err());
    assert!(store
        .revoke_binding("project", "lead", old.epoch() + 1)
        .await
        .is_err());
    let next = store
        .bind_member("project", "lead", "mini", "next")
        .await
        .unwrap();
    assert_eq!(next.epoch(), old.epoch() + 2);
    for expected in [old.epoch(), old.epoch() + 1] {
        assert!(store
            .revoke_binding("project", "lead", expected)
            .await
            .is_err());
    }
    assert_eq!(
        store.current_member(&next).await.unwrap().epoch,
        next.epoch()
    );
    store.remove_member("project", "lead").await.unwrap();
    assert!(store
        .revoke_binding("project", "lead", next.epoch() + 1)
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn revocation_cancels_pending_transport_without_erasing_room_access_or_unread_items() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for member in ["sender", "receiver"] {
        store
            .register_member("project", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("project", "room", &["sender", "receiver"])
        .await
        .unwrap();
    let sender = store
        .bind_member("project", "sender", "laptop", "sender")
        .await
        .unwrap();
    let receiver = store
        .bind_member("project", "receiver", "laptop", "old")
        .await
        .unwrap();
    let sequence = store
        .send(&sender, "room", "once", "unread marker")
        .await
        .unwrap();
    let old_batch = store.check(&receiver, 32).await.unwrap().unwrap();
    assert_eq!(old_batch.items[0].sequence, sequence);
    let revoked = store
        .revoke_binding("project", "receiver", receiver.epoch())
        .await
        .unwrap();
    assert_eq!(revoked, receiver.epoch() + 1);
    assert!(store
        .receipt(&receiver, old_batch.reference_for_adapter())
        .await
        .is_err());
    assert!(store.check(&receiver, 32).await.is_err());
    assert!(store.current_member(&sender).await.is_ok());
    let next = store
        .bind_member("project", "receiver", "mini", "new")
        .await
        .unwrap();
    assert_eq!(next.epoch(), revoked + 1);
    assert!(store
        .receipt(&next, old_batch.reference_for_adapter())
        .await
        .is_err());
    let recovered = store.check(&next, 32).await.unwrap().unwrap();
    assert_ne!(
        recovered.reference_for_adapter(),
        old_batch.reference_for_adapter()
    );
    assert_eq!(recovered.items[0].sequence, sequence);
    store
        .receipt(&next, recovered.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&next, 32).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn binding_survives_reopen_and_a_later_rebind_still_fences_it() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let old = store
        .bind_member("project", "lead", "laptop", "old")
        .await
        .unwrap();
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.current_member(&old).await.is_ok());
    let next = store
        .bind_member("project", "lead", "mini", "new")
        .await
        .unwrap();
    assert!(next.epoch() > old.epoch());
    assert!(store.current_member(&old).await.is_err());
    assert!(store.current_member(&next).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn concurrent_binds_receive_distinct_epochs_and_only_the_latest_is_current() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.bind_member("project", "lead", "laptop", "a"),
        store.bind_member("project", "lead", "mini", "b"),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.epoch(), b.epoch());
    assert_ne!(a.expose_token_for_adapter(), b.expose_token_for_adapter());
    for binding in [&a, &b] {
        assert_eq!(
            store.current_member(binding).await.is_ok(),
            binding.epoch() == a.epoch().max(b.epoch())
        );
    }
    store.close().await;
}

#[tokio::test]
async fn credentials_are_redacted_and_never_persist_as_plaintext() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let binding = store
        .bind_member("project", "lead", "laptop", "chat")
        .await
        .unwrap();
    store.unbind_member("project", "lead").await.unwrap();
    let error = store.current_member(&binding).await.unwrap_err();
    let rendered = format!("{binding:?} {binding} {error:?}");
    let secret = binding.expose_token_for_adapter();
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(secret.strip_prefix("cbcc1_").unwrap()));
    // Inspect only this disposable candidate's database/WAL, both before and after close.
    for name in ["candidate.sqlite3", "candidate.sqlite3-wal"] {
        let bytes = std::fs::read(sandbox.root().join(name)).unwrap();
        for needle in [secret, secret.strip_prefix("cbcc1_").unwrap()] {
            assert!(!bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes()));
        }
    }
    store.close().await;
    let bytes = std::fs::read(sandbox.database_path()).unwrap();
    for needle in [secret, secret.strip_prefix("cbcc1_").unwrap()] {
        assert!(!bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes()));
    }
}

#[tokio::test]
async fn unknown_or_malformed_binding_claims_fail_without_echoing_secrets() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store
        .register_member("absent", "lead", Role::RepoLead)
        .await
        .is_err());
    store.create_context("project").await.unwrap();
    assert!(store
        .bind_member("project", "absent", "laptop", "chat")
        .await
        .is_err());
    assert!(store.unbind_member("project", "absent").await.is_err());
    assert!(store.remove_member("project", "absent").await.is_err());
    for token in ["secret-to-not-echo", "cbcc1_short", "cbcc1_zz"] {
        let error =
            SessionBinding::from_token("project", "lead", "laptop", "chat", 1, token).unwrap_err();
        assert!(!format!("{error:?}").contains(token));
    }
    assert!(SessionBinding::from_token(
        "project",
        "lead",
        "laptop",
        "chat",
        0,
        &format!("cbcc1_{}", "00".repeat(32))
    )
    .is_err());
    assert!(store
        .register_member("project", " \n", Role::RepoLead)
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn removal_blocks_binding_and_reregistration_never_reuses_an_epoch() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let old = store
        .bind_member("project", "lead", "laptop", "old")
        .await
        .unwrap();
    store.remove_member("project", "lead").await.unwrap();
    assert!(store.current_member(&old).await.is_err());
    assert!(store
        .bind_member("project", "lead", "mini", "new")
        .await
        .is_err());
    assert!(store
        .register_member("project", "lead", Role::ProjectLead)
        .await
        .is_err());
    store
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    assert!(store.current_member(&old).await.is_err());
    let next = store
        .bind_member("project", "lead", "mini", "new")
        .await
        .unwrap();
    assert!(next.epoch() > old.epoch());
    assert!(store.current_member(&next).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn unbinding_revokes_the_session_without_removing_the_member() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let old = store
        .bind_member("project", "lead", "laptop", "old")
        .await
        .unwrap();
    store.unbind_member("project", "lead").await.unwrap();
    assert!(store.current_member(&old).await.is_err());
    let next = store
        .bind_member("project", "lead", "mini", "new")
        .await
        .unwrap();
    assert!(next.epoch() > old.epoch());
    assert!(store.current_member(&next).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn a_registered_member_receives_a_scoped_device_session_binding() {
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
        .register_member("project", "api-feature", Role::FeatureLead)
        .await
        .unwrap();
    let binding = store
        .bind_member("project", "api-feature", "laptop", "native-chat-a")
        .await
        .unwrap();
    let current = store.current_member(&binding).await.unwrap();
    assert_eq!(
        (
            current.context_id.as_str(),
            current.member_id.as_str(),
            current.role,
            current.device_id.as_str(),
            current.native_session_id.as_str(),
            current.epoch
        ),
        (
            "project",
            "api-feature",
            Role::FeatureLead,
            "laptop",
            "native-chat-a",
            1
        )
    );
    store.close().await;
}

#[tokio::test]
async fn a_successor_binding_fences_the_previous_native_session() {
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
        .register_member("project", "api-feature", Role::FeatureLead)
        .await
        .unwrap();
    let old = store
        .bind_member("project", "api-feature", "laptop", "old-chat")
        .await
        .unwrap();
    let successor = store
        .bind_member("project", "api-feature", "mini", "new-chat")
        .await
        .unwrap();
    assert!(successor.epoch() > old.epoch());
    assert!(store.current_member(&old).await.is_err());
    assert_eq!(
        store
            .current_member(&successor)
            .await
            .unwrap()
            .native_session_id,
        "new-chat"
    );
    store.close().await;
}

#[tokio::test]
async fn credentials_do_not_cross_contexts_members_devices_or_native_sessions() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    for context in ["project-a", "project-b"] {
        store.create_context(context).await.unwrap();
        store
            .register_member(context, "same-name", Role::Participant)
            .await
            .unwrap();
    }
    store
        .register_member("project-a", "other-member", Role::Participant)
        .await
        .unwrap();
    let a = store
        .bind_member("project-a", "same-name", "laptop", "chat")
        .await
        .unwrap();
    let b = store
        .bind_member("project-b", "same-name", "laptop", "chat")
        .await
        .unwrap();
    for (context, member, device, session) in [
        ("project-b", "same-name", "laptop", "chat"),
        ("project-a", "other-member", "laptop", "chat"),
        ("project-a", "same-name", "other-device", "chat"),
        ("project-a", "same-name", "laptop", "other-chat"),
    ] {
        let forged = SessionBinding::from_token(
            context,
            member,
            device,
            session,
            a.epoch(),
            a.expose_token_for_adapter(),
        )
        .unwrap();
        assert!(store.current_member(&forged).await.is_err());
    }
    let forged = SessionBinding::from_token(
        "project-a",
        "same-name",
        "laptop",
        "chat",
        a.epoch(),
        &format!("cbcc1_{}", "00".repeat(32)),
    )
    .unwrap();
    assert!(store.current_member(&forged).await.is_err());
    assert!(store.current_member(&a).await.is_ok());
    assert!(store.current_member(&b).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn duplicate_registration_preserves_the_binding_and_rejects_role_changes() {
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
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    let binding = store
        .bind_member("project", "lead", "laptop", "chat")
        .await
        .unwrap();
    store
        .register_member("project", "lead", Role::RepoLead)
        .await
        .unwrap();
    assert!(store
        .register_member("project", "lead", Role::ProjectLead)
        .await
        .is_err());
    assert_eq!(store.current_member(&binding).await.unwrap().epoch, 1);
    store.close().await;
}

#[tokio::test]
async fn real_v1_database_upgrades_without_losing_its_context() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/store-v1.sqlite3"
        ),
        sandbox.database_path(),
    )
    .unwrap();
    std::fs::set_permissions(
        sandbox.database_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.context_exists("v1-upgrade-kept").await.unwrap());
    store
        .register_member("v1-upgrade-kept", "new-member", Role::Participant)
        .await
        .unwrap();
    let binding = store
        .bind_member("v1-upgrade-kept", "new-member", "laptop", "chat")
        .await
        .unwrap();
    assert!(store.current_member(&binding).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn real_v2_database_preserves_binding_epochs_and_adds_the_domain_role() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/store-v2.sqlite3"
        ),
        sandbox.database_path(),
    )
    .unwrap();
    std::fs::set_permissions(
        sandbox.database_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let successor = store
        .bind_member("v2-upgrade-kept", "lead", "new-device", "new-chat")
        .await
        .unwrap();
    assert_eq!(successor.epoch(), 2);
    assert_eq!(
        store.current_member(&successor).await.unwrap().role,
        Role::RepoLead
    );
    store
        .register_member("v2-upgrade-kept", "domain", Role::DomainLead)
        .await
        .unwrap();
    let domain = store
        .bind_member("v2-upgrade-kept", "domain", "device", "domain-chat")
        .await
        .unwrap();
    assert_eq!(
        store.current_member(&domain).await.unwrap().role,
        Role::DomainLead
    );
    store.close().await;
}
