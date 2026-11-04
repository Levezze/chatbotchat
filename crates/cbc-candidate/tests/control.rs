//! Candidate-only owner/member socket paths. No native app or daily CBC is contacted.
use cbc_candidate::{
    bindings::SessionBinding,
    connector::{LocalClient, LocalConnector, LocalOwnerClient},
    enrollment::ConnectionKind,
    owner::ProvisionSpec,
    settings::PolicyPatch,
    store::Store,
    Sandbox,
};

#[tokio::test]
async fn live_owner_provisioning_discovery_and_settings_share_the_running_core() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-control-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("synthetic-daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let socket = connector.path().to_owned();
    let owner_key = std::fs::read_to_string(sandbox.root().join("owner.key")).unwrap();
    let owner_key = owner_key.trim_end();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    let owner = LocalOwnerClient::new(&socket, owner_key);
    assert!(LocalOwnerClient::new(&socket, "not-an-owner-key")
        .settings()
        .await
        .is_err());

    // This is explicit trusted synthetic setup. Evidence labels do not prove native identity.
    let spec: ProvisionSpec = serde_json::from_value(serde_json::json!({
        "context":"test", "device":"fixture-device",
        "members":[
            {"member":"lead","role":"repo_lead","host":"codex","session":"lead-native",
             "evidence_ref":"synthetic-owned-session","repository":"repo","label":"repo lead","summary":"coordination"},
            {"member":"feature","role":"feature_lead","host":"claude","session":"feature-native",
             "evidence_ref":"synthetic-owned-session","repository":"repo","label":"feature lead","summary":"one assignment"}
        ],
        "grants":[{"requester":"feature","target":"lead","kind":"report","scope_ref":"assignment-one"}]
    })).unwrap();
    let profile = owner.provision(spec).await.unwrap();
    assert_eq!(profile["bindings"].as_array().unwrap().len(), 2);
    assert_eq!(profile["targets"].as_array().unwrap().len(), 2);
    let binding = |name: &str| {
        let b = profile["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["member"].as_str() == Some(name))
            .unwrap();
        SessionBinding::from_token(
            b["context"].as_str().unwrap(),
            b["member"].as_str().unwrap(),
            b["device"].as_str().unwrap(),
            b["session"].as_str().unwrap(),
            b["epoch"].as_i64().unwrap(),
            b["token"].as_str().unwrap(),
        )
        .unwrap()
    };
    let lead_binding = binding("lead");
    let feature_binding = binding("feature");
    let lead = LocalClient::new(&socket, &lead_binding);
    let feature = LocalClient::new(&socket, &feature_binding);
    let directory = feature.discover(32).await.unwrap();
    assert_eq!(directory.len(), 1);
    assert_eq!(directory[0].member, "lead");
    let connected = feature
        .connect(
            "lead",
            ConnectionKind::Report,
            "assignment-one",
            "connect-once",
            "",
        )
        .await
        .unwrap();
    assert!(
        feature
            .connect(
                "lead",
                ConnectionKind::Report,
                "assignment-one",
                "connect-once",
                ""
            )
            .await
            .unwrap()
            .reused
    );
    let message = feature
        .send(&connected.room, "one", "disposable integration marker")
        .await
        .unwrap();
    let inbox = lead.check().await.unwrap().unwrap();
    assert!(inbox.items.iter().any(|item| item.sequence == message));
    lead.receipt(&inbox).await.unwrap();
    assert!(lead.check().await.unwrap().is_none());

    let initial = owner.settings().await.unwrap();
    let mut update = initial.settings.clone();
    update.members.entry("test".into()).or_default().insert(
        "feature".into(),
        PolicyPatch {
            retry_initial_ms: Some(125),
            retry_max_ms: Some(500),
        },
    );
    let changed = owner
        .update_settings(&initial.revision, update)
        .await
        .unwrap();
    assert_ne!(initial.revision, changed.revision);
    assert_eq!(
        feature.settings().await.unwrap().policy.retry_initial_ms,
        125
    );
    assert_eq!(lead.settings().await.unwrap().policy.retry_initial_ms, 1000);
    assert!(owner
        .update_settings(&initial.revision, initial.settings)
        .await
        .is_err());
    std::fs::write(
        sandbox.root().join("runtime-settings.json"),
        b"not valid JSON",
    )
    .unwrap();
    let retained = feature.settings().await.unwrap();
    assert!(!retained.valid);
    assert_eq!(retained.policy.retry_initial_ms, 125);
    assert_eq!(retained.revision, changed.revision);

    // Ordinary member namespace cannot smuggle an owner operation into the socket request.
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut raw = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let bytes = serde_json::to_vec(&serde_json::json!({"v":1,"auth":{
        "context":"test","member":"feature","device":"fixture-device","session":"feature-native",
        "epoch":feature_binding.epoch(),"token":feature_binding.expose_token_for_adapter()},
        "request":{"op":"provision","spec":{}}}))
    .unwrap();
    raw.write_all(&[bytes, b"\n".to_vec()].concat())
        .await
        .unwrap();
    let mut reply = String::new();
    BufReader::new(raw).read_line(&mut reply).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&reply).unwrap()["type"],
        "error"
    );
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();

    // Reopening recovers last-good policy and the same credential/receipt history.
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.current_member(&feature_binding).await.is_ok());
    assert!(store.check(&lead_binding, 32).await.unwrap().is_none());
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    assert_eq!(
        feature.settings().await.unwrap().policy.retry_initial_ms,
        125
    );
    // Owner revocation fences the exact binding while the core and a shared watch are live.
    // A rejected stale CAS must not invalidate any other member.
    let watch_socket = socket.clone();
    let watch_lead = binding("lead");
    let watch_feature = binding("feature");
    let waiting = tokio::spawn(async move {
        LocalClient::new(&watch_socket, &watch_lead)
            .watch_members(&[&watch_feature], i64::MAX)
            .await
    });
    tokio::task::yield_now().await;
    assert!(owner
        .revoke_binding("test", "feature", feature_binding.epoch() + 1)
        .await
        .is_err());
    assert!(feature.settings().await.is_ok());
    assert_eq!(
        owner
            .revoke_binding("test", "feature", feature_binding.epoch())
            .await
            .unwrap(),
        feature_binding.epoch() + 1
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(feature.check().await.is_err());
    assert!(feature
        .send(&connected.room, "revoked", "must never be stored")
        .await
        .is_err());
    assert!(feature.discover(32).await.is_err());
    assert!(lead.check().await.unwrap().is_none());
    assert!(owner
        .revoke_binding("test", "feature", feature_binding.epoch())
        .await
        .is_err());
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
}
