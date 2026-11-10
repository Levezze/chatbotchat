use cbc_candidate::{
    bindings::{Role, SessionBinding},
    enrollment::{ConnectionKind, NativeHost, SessionEvidence, MAX_DISCOVERY},
    store::Store,
    Sandbox,
};
use tempfile::{tempdir, TempDir};

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
    for (member, role) in [
        ("lead", Role::RepoLead),
        ("other-lead", Role::RepoLead),
        ("worker", Role::FeatureLead),
        ("peer", Role::FeatureLead),
        ("hidden", Role::FeatureLead),
    ] {
        store
            .register_member("project", member, role)
            .await
            .unwrap();
        store
            .describe_member(
                "project",
                member,
                "repo",
                member,
                "bounded outward description",
            )
            .await
            .unwrap();
    }
    (temp, sandbox, store)
}
fn evidence(host: NativeHost, session: &str) -> SessionEvidence {
    // Synthetic adapter evidence only, not a native-host verification claim.
    SessionEvidence::from_trusted_adapter(host, "device", session, "synthetic-host-evidence")
        .unwrap()
}
async fn bound(store: &Store, member: &str) -> SessionBinding {
    store
        .bind_member("project", member, "device", &format!("{member}-session"))
        .await
        .unwrap()
}

#[tokio::test]
async fn invite_requires_preexisting_member_and_one_use_never_rebinds_on_retry() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = evidence(NativeHost::Codex, "native-chat");
    assert!(store
        .issue_enrollment_invite("project", "absent", &proof, 60)
        .await
        .is_err());
    assert!(store
        .issue_enrollment_invite("project", "worker", &proof, 0)
        .await
        .is_err());
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    let obsolete = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    assert_eq!(
        store.current_member(&binding).await.unwrap().member_id,
        "worker"
    );
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    assert!(store
        .consume_enrollment_invite(obsolete.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    assert_eq!(store.current_member(&binding).await.unwrap().epoch, 1);
    store.close().await;
}

#[tokio::test]
async fn invite_cannot_be_claimed_with_changed_host_device_session_or_evidence() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = evidence(NativeHost::Claude, "native-chat");
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    for changed in [
        evidence(NativeHost::Codex, "native-chat"),
        evidence(NativeHost::Claude, "different-chat"),
        SessionEvidence::from_trusted_adapter(
            NativeHost::Claude,
            "other-device",
            "native-chat",
            "synthetic-host-evidence",
        )
        .unwrap(),
        SessionEvidence::from_trusted_adapter(
            NativeHost::Claude,
            "device",
            "native-chat",
            "unmatched-host-evidence",
        )
        .unwrap(),
    ] {
        assert!(store
            .consume_enrollment_invite(invite.expose_token_for_adapter(), &changed)
            .await
            .is_err());
    }
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    assert!(store.current_member(&binding).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn restart_preserves_invites_and_secrets_are_redacted_and_digest_only() {
    let (_temp, sandbox, store) = fixture().await;
    let proof = evidence(NativeHost::Codex, "native-chat");
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    assert!(!format!("{invite} {invite:?}").contains(invite.expose_token_for_adapter()));
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let binding = store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .unwrap();
    store.close().await;
    for name in ["candidate.sqlite3", "candidate.sqlite3-wal"] {
        if let Ok(data) = std::fs::read(sandbox.root().join(name)) {
            for token in [
                invite.expose_token_for_adapter(),
                binding.expose_token_for_adapter(),
            ] {
                for needle in [token, token.split_once('_').unwrap().1] {
                    assert!(!data
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes()));
                }
            }
        }
    }
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.current_member(&binding).await.is_ok());
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn expired_and_superseded_invitations_cannot_change_current_binding() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let (_temp, sandbox, store) = fixture().await;
    let proof = evidence(NativeHost::Codex, "native-chat");
    let old = bound(&store, "worker").await;
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    store.close().await;
    // Fault injection only into this closed disposable candidate.
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(sandbox.database_path())
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE candidate_enrollment_invites SET expires_at=0")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    assert!(store.current_member(&old).await.is_ok());
    let another = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    let newer = bound(&store, "worker").await;
    assert!(store
        .consume_enrollment_invite(another.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    assert!(store.current_member(&newer).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn concurrent_claims_issue_only_one_current_binding() {
    let (_temp, _sandbox, store) = fixture().await;
    let proof = evidence(NativeHost::Claude, "native-chat");
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        store.consume_enrollment_invite(invite.expose_token_for_adapter(), &proof),
        store.consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let binding = one.or(two).unwrap();
    assert_eq!(store.current_member(&binding).await.unwrap().epoch, 1);
    store.close().await;
}

#[tokio::test]
async fn discovery_exposes_only_granted_same_context_outward_metadata() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let lead = bound(&store, "lead").await;
    assert!(store
        .discover_connections(&worker, MAX_DISCOVERY)
        .await
        .unwrap()
        .is_empty());
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    store
        .authorize_connection(
            "project",
            "worker",
            "peer",
            ConnectionKind::Peer,
            "feature-a",
        )
        .await
        .unwrap();
    store.create_context("another").await.unwrap();
    store
        .register_member("another", "secret", Role::RepoLead)
        .await
        .unwrap();
    store
        .describe_member(
            "another",
            "secret",
            "private-repo",
            "private-label",
            "private-summary",
        )
        .await
        .unwrap();
    let entries = store
        .discover_connections(&worker, MAX_DISCOVERY)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| e.member.as_str())
            .collect::<Vec<_>>(),
        vec!["lead", "peer"]
    );
    assert_eq!(
        store
            .discover_connections(&lead, MAX_DISCOVERY)
            .await
            .unwrap()[0]
            .member,
        "worker"
    );
    let rendered = serde_json::to_string(&entries).unwrap();
    assert!(
        !rendered.contains("private-")
            && !rendered.contains("session")
            && !rendered.contains("epoch")
            && !rendered.contains("cbcc1_")
    );
    assert_eq!(
        store.discover_connections(&worker, 1).await.unwrap().len(),
        1
    );
    assert!(store.discover_connections(&worker, 0).await.is_err());
    assert!(store
        .discover_connections(&worker, MAX_DISCOVERY + 1)
        .await
        .is_err());
    store.unbind_member("project", "worker").await.unwrap();
    assert!(store.discover_connections(&worker, 1).await.is_err());
    store.close().await;
}

#[tokio::test]
async fn maximum_discovery_metadata_stays_within_the_core_reply_bound() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let escaped = "\"".repeat(128);
    assert!(store
        .describe_member("project", "worker", "repo", "worker", &"x".repeat(161))
        .await
        .is_err());
    for index in 0..MAX_DISCOVERY {
        let member = format!("{}{index:02}", "\"".repeat(126));
        store
            .register_member("project", &member, Role::RepoLead)
            .await
            .unwrap();
        store
            .describe_member("project", &member, &escaped, &escaped, &"💬".repeat(128))
            .await
            .unwrap();
        store
            .authorize_connection(
                "project",
                "worker",
                &member,
                ConnectionKind::Report,
                &escaped,
            )
            .await
            .unwrap();
    }
    let entries = store
        .discover_connections(&worker, MAX_DISCOVERY)
        .await
        .unwrap();
    assert_eq!(entries.len(), MAX_DISCOVERY);
    assert!(
        serde_json::to_vec(&entries).unwrap().len() + 1024
            < cbc_candidate::inbox::MAX_BATCH_HARD_BYTES
    );
    store.close().await;
}

#[tokio::test]
async fn report_connection_is_deterministic_and_introduction_is_delivered_once() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let lead = bound(&store, "lead").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    assert!(store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "model-written report intro"
        )
        .await
        .is_err());
    let connected = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    assert!(!connected.reused);
    let retry = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    assert!(retry.reused);
    let parent = store
        .connect_members(
            &lead,
            "worker",
            ConnectionKind::Report,
            "feature-a",
            "parent",
            "",
        )
        .await
        .unwrap();
    assert_eq!(parent.room, connected.room);
    assert_eq!(retry.introduction_sequence, connected.introduction_sequence);
    let batch = store.check(&lead, 32).await.unwrap().unwrap();
    assert_eq!(batch.items.len(), 1);
    let intro: serde_json::Value = serde_json::from_str(&batch.items[0].body).unwrap();
    assert_eq!(intro["event"], "connection_opened");
    assert_eq!(intro["scope_ref"], "feature-a");
    store
        .receipt(&lead, batch.reference_for_adapter())
        .await
        .unwrap();
    assert!(store.check(&lead, 32).await.unwrap().is_none());
    store
        .send(&worker, &connected.room, "work", "ordinary progress")
        .await
        .unwrap();
    assert_eq!(
        store.check(&lead, 32).await.unwrap().unwrap().items[0].body,
        "ordinary progress"
    );
    store.close().await;
}

#[tokio::test]
async fn peer_connection_is_symmetric_under_concurrent_creation_and_keys_conflict() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
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
    let (a, b) = tokio::join!(
        store.connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "first",
            "Please reconcile the shared types."
        ),
        store.connect_members(
            &peer,
            "worker",
            ConnectionKind::Peer,
            "contract-a",
            "first",
            "The types are ready for reconciliation."
        )
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.room, b.room);
    assert_eq!(a.introduction_sequence, b.introduction_sequence);
    assert_ne!(a.reused, b.reused);
    assert!(store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "first",
            "Changed request"
        )
        .await
        .is_err());
    assert!(store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-b",
            "other",
            ""
        )
        .await
        .is_err());
    assert!(store
        .connect_members(
            &worker,
            "hidden",
            ConnectionKind::Peer,
            "contract-a",
            "hidden",
            ""
        )
        .await
        .is_err());
    assert!(store
        .authorize_connection("project", "worker", "lead", ConnectionKind::Peer, "bad")
        .await
        .is_err());
    assert!(store
        .authorize_connection("project", "worker", "peer", ConnectionKind::Report, "bad")
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn introduction_limits_preserve_unicode_and_refuse_oversize_without_side_effects() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let peer = bound(&store, "peer").await;
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
    for bad in ["x".repeat(601), "💬".repeat(600), "hidden\0control".into()] {
        assert!(store
            .connect_members(
                &worker,
                "peer",
                ConnectionKind::Peer,
                "contract-a",
                "new",
                &bad
            )
            .await
            .is_err());
    }
    assert!(store.check(&peer, 32).await.unwrap().is_none());
    let note = "日本語 café ✓";
    store
        .connect_members(
            &worker,
            "peer",
            ConnectionKind::Peer,
            "contract-a",
            "new",
            note,
        )
        .await
        .unwrap();
    let intro: serde_json::Value =
        serde_json::from_str(&store.check(&peer, 32).await.unwrap().unwrap().items[0].body)
            .unwrap();
    assert_eq!(intro["explanation"], note);
    store.close().await;
}

#[tokio::test]
async fn policy_revoke_cancels_discovery_and_access_and_regrant_does_not_restore_room() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let lead = bound(&store, "lead").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    let route = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    let pending = store.check(&lead, 32).await.unwrap().unwrap();
    store
        .revoke_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    assert!(store
        .discover_connections(&worker, MAX_DISCOVERY)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .receipt(&lead, pending.reference_for_adapter())
        .await
        .is_err());
    assert!(store
        .send(&worker, &route.room, "denied", "no access")
        .await
        .is_err());
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    assert!(store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            ""
        )
        .await
        .is_err());
    // T-26 (2026-11-10): an open after the regrant creates a replacement room. It never
    // restores the revoked room, its memberships or its history.
    let replacement = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "next",
            "",
        )
        .await
        .unwrap();
    assert_ne!(replacement.room, route.room);
    assert_eq!(replacement.supersedes.as_deref(), Some(route.room.as_str()));
    assert!(store
        .send(&worker, &route.room, "still-denied", "no access")
        .await
        .is_err());
    store.close().await;
}

#[tokio::test]
async fn removed_member_does_not_recover_old_route_grants_on_registration() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .discover_connections(&worker, MAX_DISCOVERY)
            .await
            .unwrap()
            .len(),
        1
    );
    let proof = evidence(NativeHost::Codex, "replacement");
    let invite = store
        .issue_enrollment_invite("project", "worker", &proof, 60)
        .await
        .unwrap();
    store.remove_member("project", "worker").await.unwrap();
    store
        .register_member("project", "worker", Role::FeatureLead)
        .await
        .unwrap();
    assert!(store
        .consume_enrollment_invite(invite.expose_token_for_adapter(), &proof)
        .await
        .is_err());
    let next = bound(&store, "worker").await;
    assert!(store
        .discover_connections(&next, MAX_DISCOVERY)
        .await
        .unwrap()
        .is_empty());
    store.close().await;
}

#[tokio::test]
async fn connection_survives_restart_and_a_closed_room_is_replaced_not_reopened() {
    let (_temp, sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    let route = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    store.close().await;
    let store = Store::open(&sandbox).await.unwrap();
    let retry = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    assert_eq!(retry.room, route.room);
    store
        .close_room(&worker, &route.room, "done")
        .await
        .unwrap();
    let closed = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    assert!(closed.closed);
    // T-26 (decided 2026-11-10): a new open after a close never reopens the closed room; it
    // creates one replacement for the pair that records which room it supersedes.
    let replacement = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "second",
            "",
        )
        .await
        .unwrap();
    assert_ne!(replacement.room, route.room);
    assert!(!replacement.closed);
    assert_eq!(replacement.supersedes.as_deref(), Some(route.room.as_str()));
    store.close().await;
}

#[tokio::test]
async fn real_v3_database_preserves_room_memberships_and_binding_epochs() {
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
            "/tests/fixtures/store-v3.sqlite3"
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
    assert!(store.context_exists("v3-upgrade-kept").await.unwrap());
    let successor = store
        .bind_member("v3-upgrade-kept", "one", "new-device", "new-native")
        .await
        .unwrap();
    assert_eq!(successor.epoch(), 2);
    store
        .send(
            &successor,
            "original-room",
            "after-upgrade",
            "membership survived",
        )
        .await
        .unwrap();
    let other = store
        .bind_member("v3-upgrade-kept", "two", "new-device", "other-native")
        .await
        .unwrap();
    assert_eq!(other.epoch(), 2);
    assert_eq!(
        store.check(&other, 32).await.unwrap().unwrap().items[0].body,
        "membership survived"
    );
    store.close().await;
}

#[tokio::test]
async fn binding_activity_observes_unread_without_freezing_or_receipting_batches() {
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let lead = bound(&store, "lead").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature-a",
        )
        .await
        .unwrap();
    let route = store
        .connect_members(
            &worker,
            "lead",
            ConnectionKind::Report,
            "feature-a",
            "first",
            "",
        )
        .await
        .unwrap();
    let first = store.binding_activity(&lead).await.unwrap().unwrap();
    assert_eq!(first.last_receipted_sequence, 0);
    assert_eq!(
        first.first_unread_sequence,
        Some(route.introduction_sequence)
    );
    assert_eq!(
        store
            .binding_activity(&worker)
            .await
            .unwrap()
            .unwrap()
            .first_unread_sequence,
        None
    );
    let sequence = store
        .send(&worker, &route.room, "work", "fresh progress")
        .await
        .unwrap();
    let batch = store.check(&lead, 32).await.unwrap().unwrap();
    assert_eq!(batch.items.len(), 2); // The earlier status read did not freeze a one-item batch.
    store
        .receipt(&lead, batch.reference_for_adapter())
        .await
        .unwrap();
    let drained = store.binding_activity(&lead).await.unwrap().unwrap();
    assert_eq!(drained.last_receipted_sequence, sequence);
    assert_eq!(drained.first_unread_sequence, None);
    let next = store
        .send(&worker, &route.room, "more", "later progress")
        .await
        .unwrap();
    assert_eq!(
        store
            .binding_activity(&lead)
            .await
            .unwrap()
            .unwrap()
            .first_unread_sequence,
        Some(next)
    );
    store
        .revoke_room_member("project", &route.room, "lead")
        .await
        .unwrap();
    let revoked = store.binding_activity(&lead).await.unwrap().unwrap();
    assert_eq!(revoked.last_receipted_sequence, sequence);
    assert_eq!(revoked.first_unread_sequence, None);
    store.close().await;
}

#[tokio::test]
async fn invalid_binding_is_typed_and_only_definitive_invalidation_returns_none() {
    use cbc_candidate::bindings::InvalidBinding;
    let (_temp, _sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    let impostor = SessionBinding::from_token(
        "project",
        "worker",
        "different-device",
        "worker-session",
        worker.epoch(),
        worker.expose_token_for_adapter(),
    )
    .unwrap();
    let error = store.current_member(&impostor).await.unwrap_err();
    assert!(error.downcast_ref::<InvalidBinding>().is_some());
    assert_eq!(error.to_string(), "invalid or superseded binding");
    assert_eq!(store.binding_activity(&impostor).await.unwrap(), None);
    assert!(store.binding_activity(&worker).await.unwrap().is_some());
    store.unbind_member("project", "worker").await.unwrap();
    assert_eq!(store.binding_activity(&worker).await.unwrap(), None);
    store.close().await;
}

#[tokio::test]
async fn activity_storage_failures_are_errors_not_revocation_or_empty_success() {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let (_temp, sandbox, store) = fixture().await;
    let worker = bound(&store, "worker").await;
    store.close().await;
    // Operational fault in a closed disposable database only, never the daily installation.
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(sandbox.database_path())
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("ALTER TABLE candidate_receipts RENAME TO receipts_unavailable")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.current_member(&worker).await.is_ok());
    assert!(store.binding_activity(&worker).await.is_err());
    store.close().await;
}
