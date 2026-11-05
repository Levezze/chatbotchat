use cbc_candidate::{
    bindings::SessionBinding,
    enrollment::ConnectionKind,
    native_target::{ExecutionProfile, SavedSelection},
    owner::{OwnerControl, ProvisionSpec},
    store::Store,
    Sandbox,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
};
use tempfile::{tempdir, TempDir};

fn fixture() -> (TempDir, Sandbox) {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    (temp, sandbox)
}

fn spec() -> ProvisionSpec {
    serde_json::from_value(json!({
        "context":"demo", "device":"laptop", "members":[
            {"member":"codex-owner","role":"repo_lead","host":"codex","session":"codex-native-session",
             "evidence_ref":"trusted-host-read-1","repository":"api","label":"api owner","summary":"Disposable native delivery", "target":target("api")},
            {"member":"claude-owner","role":"repo_lead","host":"claude","session":"claude-native-session",
             "evidence_ref":"trusted-host-hook-2","repository":"client","label":"client owner","summary":"Disposable native delivery", "target":target("client")}
        ], "grants":[{"requester":"codex-owner","target":"claude-owner","kind":"peer","scope_ref":"demo-contract"}]
    })).unwrap()
}

fn target(repository: &str) -> Value {
    json!({"v":1,"profile":"unknown","repository":repository,"worktree":null,
        "cwd":format!("/fixtures/{repository}"),"saved_project":{"status":"not_exposed"},
        "saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"synthetic-ui-observation"}})
}

fn binding(value: &Value) -> SessionBinding {
    SessionBinding::from_token(
        value["context"].as_str().unwrap(),
        value["member"].as_str().unwrap(),
        value["device"].as_str().unwrap(),
        value["session"].as_str().unwrap(),
        value["epoch"].as_i64().unwrap(),
        value["token"].as_str().unwrap(),
    )
    .unwrap()
}

#[test]
fn private_owner_key_survives_reopen_and_authentication_is_separate_from_member_credentials() {
    let (_temp, sandbox) = fixture();
    let owner = OwnerControl::open(&sandbox).unwrap();
    let bytes = fs::read(owner.key_path()).unwrap();
    assert_eq!(bytes.len(), 71);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let token = std::str::from_utf8(&bytes[..70]).unwrap();
    assert!(owner.authenticate(token));
    assert!(!owner.authenticate(""));
    assert!(!owner.authenticate(&"x".repeat(70)));
    assert!(!owner.authenticate(&format!("{token}\n")));
    let restarted = OwnerControl::open(&sandbox).unwrap();
    assert!(restarted.authenticate(token));
    assert_eq!(fs::read(restarted.key_path()).unwrap(), bytes);
    assert_eq!(
        fs::metadata(owner.key_path()).unwrap().mode() & 0o777,
        0o600
    );
}

#[test]
fn unsafe_or_corrupt_owner_keys_are_not_replaced() {
    use std::os::unix::fs::symlink;
    let (temp, sandbox) = fixture();
    let outside = temp.path().join("outside-key");
    fs::write(&outside, b"outside data").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    let key = sandbox.root().join("owner.key");
    symlink(&outside, &key).unwrap();
    assert!(OwnerControl::open(&sandbox).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"outside data");
    fs::remove_file(&key).unwrap();
    fs::hard_link(&outside, &key).unwrap();
    assert!(OwnerControl::open(&sandbox).is_err());
    fs::remove_file(&key).unwrap();
    fs::write(&key, b"bad-key").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(OwnerControl::open(&sandbox).is_err());
    assert_eq!(fs::read(&key).unwrap(), b"bad-key");
}

#[tokio::test]
async fn live_store_provision_returns_private_profile_and_authorized_discovery_without_second_store(
) {
    let (_temp, sandbox) = fixture();
    let owner = OwnerControl::open(&sandbox).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let profile = owner.provision(&store, spec()).await.unwrap();
    let fields: Vec<_> = profile
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields.len(), 4);
    for field in ["v", "core_socket", "bindings", "targets"] {
        assert!(profile.get(field).is_some());
    }
    assert_eq!(profile["v"], 2);
    assert_eq!(
        profile["core_socket"],
        sandbox
            .root()
            .join("connector.sock")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        profile["targets"],
        json!([
            {"session":"codex-native-session","host":"codex","target":target("api")},
            {"session":"claude-native-session","host":"claude","target":target("client")}
        ])
    );
    let codex = binding(&profile["bindings"][0]);
    let claude = binding(&profile["bindings"][1]);
    assert!(store.current_member(&codex).await.is_ok());
    assert!(store.current_member(&claude).await.is_ok());
    assert_eq!(
        serde_json::to_value(store.native_target(&codex).await.unwrap().unwrap()).unwrap(),
        target("api")
    );
    assert!(!owner.authenticate(codex.expose_token_for_adapter()));
    let discovery = store.discover_connections(&codex, 64).await.unwrap();
    assert_eq!(discovery.len(), 1);
    assert_eq!(discovery[0].member, "claude-owner");
    let route = store
        .connect_members(
            &codex,
            "claude-owner",
            ConnectionKind::Peer,
            "demo-contract",
            "connect-1",
            "Need the contract details",
        )
        .await
        .unwrap();
    assert!(!route.closed);
    assert!(!route.reused);
    assert!(store.check(&claude, 32).await.unwrap().is_some());
    store.close().await;
}

#[tokio::test]
async fn duplicate_provision_is_refused_without_rotating_existing_binding() {
    let (_temp, sandbox) = fixture();
    let owner = OwnerControl::open(&sandbox).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let profile = owner.provision(&store, spec()).await.unwrap();
    let original = binding(&profile["bindings"][0]);
    assert!(owner.provision(&store, spec()).await.is_err());
    assert!(store.current_member(&original).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn invalid_manifest_is_rejected_before_context_or_member_creation() {
    let (_temp, sandbox) = fixture();
    let owner = OwnerControl::open(&sandbox).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let mut invalid = spec();
    invalid.grants[0].target = "unknown-member".into();
    assert!(owner.provision(&store, invalid).await.is_err());
    assert!(!store.context_exists("demo").await.unwrap());
    let mut invalid = spec();
    invalid.members[1].role = "feature_lead".into();
    assert!(owner.provision(&store, invalid).await.is_err());
    assert!(!store.context_exists("demo").await.unwrap());
    let mut invalid = spec();
    invalid.members[1].session = invalid.members[0].session.clone();
    assert!(owner.provision(&store, invalid).await.is_err());
    assert!(!store.context_exists("demo").await.unwrap());
    let mut invalid = spec();
    invalid.members[0].summary = "x".repeat(161);
    assert!(owner.provision(&store, invalid).await.is_err());
    assert!(!store.context_exists("demo").await.unwrap());
    store.close().await;
}

#[tokio::test]
async fn unavailable_adapter_constraints_are_rejected_before_mutation_but_audit_ids_are_retained() {
    let (_temp, sandbox) = fixture();
    let owner = OwnerControl::open(&sandbox).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    for profile in [ExecutionProfile::Desktop, ExecutionProfile::Cli] {
        let mut unsupported = spec();
        unsupported.members[0].target.profile = profile;
        let error = owner.provision(&store, unsupported).await.unwrap_err();
        assert!(error.to_string().contains("Codex execution profile"));
        assert!(!store.context_exists("demo").await.unwrap());
    }
    for member in [0, 1] {
        for project in [true, false] {
            let mut unsupported = spec();
            let selection = SavedSelection::Configured {
                id: "synthetic-saved-id".into(),
            };
            if project {
                unsupported.members[member].target.saved_project = selection;
            } else {
                unsupported.members[member].target.saved_environment = selection;
            }
            let error = owner.provision(&store, unsupported).await.unwrap_err();
            assert!(error
                .to_string()
                .contains("unsupported by current adapters"));
            assert!(!store.context_exists("demo").await.unwrap());
        }
    }
    let mut supported = spec();
    supported.members[0].target.placement.project_id = Some("synthetic-observed-project".into());
    supported.members[0].target.placement.environment_id =
        Some("synthetic-observed-environment".into());
    supported.members[1].target.profile = ExecutionProfile::Desktop;
    let profile = owner.provision(&store, supported).await.unwrap();
    assert_eq!(
        profile["targets"][0]["target"]["placement"]["project_id"],
        "synthetic-observed-project"
    );
    assert_eq!(
        profile["targets"][0]["target"]["placement"]["environment_id"],
        "synthetic-observed-environment"
    );
    assert_eq!(profile["targets"][1]["target"]["profile"], "desktop");
    store.close().await;
}

#[tokio::test]
async fn owner_cannot_provision_a_different_candidate_store() {
    let (_first_temp, first) = fixture();
    let (_second_temp, second) = fixture();
    let owner = OwnerControl::open(&first).unwrap();
    let store = Store::open(&second).await.unwrap();
    assert!(owner.provision(&store, spec()).await.is_err());
    assert!(!store.context_exists("demo").await.unwrap());
    store.close().await;
}

#[test]
fn manifest_schema_and_summary_bounds_are_strict() {
    let mut missing = serde_json::to_value(spec()).unwrap();
    missing["members"][0]
        .as_object_mut()
        .unwrap()
        .remove("target");
    assert!(serde_json::from_value::<ProvisionSpec>(missing).is_err());
    let mut mismatched = spec();
    mismatched.members[0].target.repository = "wrong-repository".into();
    assert!(mismatched.validate().is_err());
    let mut value = serde_json::to_value(spec()).unwrap();
    value["permissions"] = json!("allow everything");
    assert!(serde_json::from_value::<ProvisionSpec>(value).is_err());
    let mut value = serde_json::to_value(spec()).unwrap();
    value["members"][0]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ProvisionSpec>(value).is_err());
    let mut invalid = spec();
    invalid.members[0].summary = "x".repeat(601);
    assert!(invalid.validate().is_err());
    let mut invalid = spec();
    invalid.members[0].role = "domain_lead".into();
    assert!(invalid.validate().is_err());
    let mut invalid = spec();
    let reversed = serde_json::from_value(json!({"requester":"claude-owner","target":"codex-owner","kind":"peer","scope_ref":"demo-contract"})).unwrap();
    invalid.grants.push(reversed);
    assert!(invalid.validate().is_err());
}
