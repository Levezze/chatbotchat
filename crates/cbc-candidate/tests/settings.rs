use cbc_candidate::{
    settings::{DispatchPolicy, PolicyPatch, RuntimeSettings, RuntimeSettingsService},
    Sandbox,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
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

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn publish_external(root: &Path, settings: &RuntimeSettings) {
    let temporary = root.join("external-settings.tmp");
    write_private(&temporary, &serde_json::to_vec_pretty(settings).unwrap());
    fs::rename(temporary, root.join("runtime-settings.json")).unwrap();
}

#[test]
fn same_running_service_reloads_external_edits_and_keeps_last_good_on_invalid_input() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let first = service.snapshot();
    let mut edited = first.settings.clone();
    edited.defaults.retry_initial_ms = 50;
    edited.defaults.retry_max_ms = 250;
    publish_external(sandbox.root(), &edited);
    let changed = service.refresh();
    assert_ne!(changed.revision, first.revision);
    assert_eq!(changed.settings.defaults.retry_initial_ms, 50);
    assert!(changed.last_error.is_none());
    let path = sandbox.root().join("runtime-settings.json");
    write_private(&path, b"{bad-json");
    let retained = service.refresh();
    assert_eq!(retained.revision, changed.revision);
    assert_eq!(retained.settings, edited);
    assert!(retained.last_error.is_some());
    assert_eq!(fs::read(path).unwrap(), b"{bad-json"); // No silent editor rollback/default reset.
    assert!(service.update(&retained.revision, edited.clone()).is_err());
    publish_external(sandbox.root(), &edited);
    assert!(service.refresh().last_error.is_none());
}

#[test]
fn inheritance_and_explicit_override_removal_preserve_scope() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let mut settings = service.snapshot().settings;
    settings.contexts.insert(
        "api".into(),
        PolicyPatch {
            retry_initial_ms: Some(100),
            retry_max_ms: None,
            room_notice_every_messages: None,
        },
    );
    settings.roles.insert(
        "repo_lead".into(),
        PolicyPatch {
            retry_initial_ms: Some(200),
            retry_max_ms: None,
            room_notice_every_messages: None,
        },
    );
    settings.members.entry("api".into()).or_default().insert(
        "owner".into(),
        PolicyPatch {
            retry_initial_ms: Some(300),
            retry_max_ms: None,
            room_notice_every_messages: None,
        },
    );
    let initial = service.snapshot();
    let snapshot = service.update(&initial.revision, settings.clone()).unwrap();
    assert_eq!(
        snapshot
            .effective("api", "repo_lead", "owner")
            .policy
            .retry_initial_ms,
        300
    );
    assert_eq!(
        snapshot
            .effective("api", "repo_lead", "other")
            .policy
            .retry_initial_ms,
        200
    );
    assert_eq!(
        snapshot
            .effective("api", "feature_lead", "other")
            .policy
            .retry_initial_ms,
        100
    );
    assert_eq!(
        snapshot
            .effective("client", "feature_lead", "owner")
            .policy
            .retry_initial_ms,
        1000
    );
    assert_eq!(
        snapshot.effective("api", "repo_lead", "owner").sources,
        ["defaults", "context", "role", "member"]
    );
    settings.defaults.retry_initial_ms = 400;
    let snapshot = service
        .update(&snapshot.revision, settings.clone())
        .unwrap();
    assert_eq!(
        snapshot
            .effective("client", "feature_lead", "owner")
            .policy
            .retry_initial_ms,
        400
    );
    assert_eq!(
        snapshot
            .effective("api", "repo_lead", "owner")
            .policy
            .retry_initial_ms,
        300
    );
    settings.members.clear();
    let snapshot = service.update(&snapshot.revision, settings).unwrap();
    assert_eq!(
        snapshot
            .effective("api", "repo_lead", "owner")
            .policy
            .retry_initial_ms,
        200
    );
}

#[test]
fn management_cas_observes_other_writers_and_survives_restart() {
    let (_temp, sandbox) = fixture();
    let mut first = RuntimeSettingsService::open(&sandbox).unwrap();
    let mut stale = RuntimeSettingsService::open(&sandbox).unwrap();
    let original = first.snapshot();
    let mut settings = original.settings.clone();
    settings.defaults.retry_initial_ms = 75;
    let updated = first.update(&original.revision, settings.clone()).unwrap();
    let mut conflicting = original.settings;
    conflicting.defaults.retry_initial_ms = 125;
    assert!(stale.update(&original.revision, conflicting).is_err());
    let reopened = RuntimeSettingsService::open(&sandbox).unwrap().snapshot();
    assert_eq!(reopened.revision, updated.revision);
    assert_eq!(reopened.settings, settings);
    assert_eq!(stale.refresh().revision, updated.revision);
    assert_eq!(
        fs::metadata(sandbox.root().join("runtime-settings.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn competing_management_updates_have_one_winner() {
    let (_temp, sandbox) = fixture();
    let first = RuntimeSettingsService::open(&sandbox).unwrap();
    let second = RuntimeSettingsService::open(&sandbox).unwrap();
    let revision = first.snapshot().revision;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = [first, second]
            .into_iter()
            .enumerate()
            .map(|(index, mut service)| {
                let barrier = barrier.clone();
                let revision = revision.clone();
                scope.spawn(move || {
                    let mut changed = RuntimeSettings::default();
                    changed.defaults.retry_initial_ms = 50 + index as u64;
                    barrier.wait();
                    service.update(&revision, changed).is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|value| **value).count(), 1);
}

#[test]
fn validated_checkpoint_recovers_after_invalid_external_edit_but_fresh_corruption_fails() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let mut changed = service.snapshot().settings;
    changed.defaults.retry_max_ms = 2000;
    publish_external(sandbox.root(), &changed);
    let accepted = service.refresh();
    write_private(&sandbox.root().join("runtime-settings.json"), b"not-json");
    let recovered = RuntimeSettingsService::open(&sandbox).unwrap().snapshot();
    assert_eq!(recovered.settings, changed);
    assert_eq!(recovered.revision, accepted.revision);
    assert!(recovered.recovered_last_good);
    assert!(recovered.last_error.is_some());

    let (_other_temp, fresh) = fixture();
    write_private(&fresh.root().join("runtime-settings.json"), b"not-json");
    assert!(RuntimeSettingsService::open(&fresh).is_err());
    assert_eq!(
        fs::read(fresh.root().join("runtime-settings.json")).unwrap(),
        b"not-json"
    );
    assert!(!fresh
        .root()
        .join("runtime-settings.last-good.json")
        .exists());
}

#[test]
fn unknown_fields_invalid_schema_and_composed_policy_do_not_replace_accepted_values() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let first = service.snapshot();
    let mut value = serde_json::to_value(&first.settings).unwrap();
    value["authority"] = serde_json::json!("allow-all");
    write_private(
        &sandbox.root().join("runtime-settings.json"),
        &serde_json::to_vec(&value).unwrap(),
    );
    assert_eq!(service.refresh().revision, first.revision);

    let mut bad = RuntimeSettings {
        schema: 2,
        ..RuntimeSettings::default()
    };
    assert!(bad.validate().is_err());
    bad.schema = 1;
    bad.defaults = DispatchPolicy {
        retry_initial_ms: 100,
        retry_max_ms: 1000,
        room_notice_every_messages: 100,
    };
    bad.contexts.insert(
        "api".into(),
        PolicyPatch {
            retry_initial_ms: Some(500),
            retry_max_ms: None,
            room_notice_every_messages: None,
        },
    );
    bad.roles.insert(
        "repo_lead".into(),
        PolicyPatch {
            retry_initial_ms: None,
            retry_max_ms: Some(200),
            room_notice_every_messages: None,
        },
    );
    assert!(bad.validate().is_err()); // Both individual patches fit defaults; their combination does not.
    bad.roles.clear();
    assert!(bad.validate().is_ok());
    bad.members
        .entry("api".into())
        .or_default()
        .insert("bad\nmember".into(), PolicyPatch::default());
    assert!(bad.validate().is_err());
}

#[test]
fn revision_is_semantic_and_independent_of_json_whitespace_or_key_order() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let first = service.snapshot();
    let reversed = br#"{ "members":{}, "roles":{}, "contexts":{}, "defaults":{"retry_max_ms":30000,"retry_initial_ms":1000}, "schema":1 }"#;
    write_private(&sandbox.root().join("runtime-settings.json"), reversed);
    let unchanged = service.refresh();
    assert_eq!(unchanged.revision, first.revision);
    assert!(unchanged.last_error.is_none());
}

#[test]
fn unsafe_or_oversized_files_cannot_supply_new_policy() {
    use std::os::unix::fs::symlink;
    let (temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let first = service.snapshot();
    let primary = sandbox.root().join("runtime-settings.json");
    let outside = temp.path().join("outside.json");
    write_private(&outside, &serde_json::to_vec(&first.settings).unwrap());
    fs::remove_file(&primary).unwrap();
    symlink(&outside, &primary).unwrap();
    assert!(service.refresh().last_error.is_some());
    assert_eq!(service.snapshot().revision, first.revision);
    assert!(service
        .update(&first.revision, RuntimeSettings::default())
        .is_err());
    assert!(fs::symlink_metadata(&primary)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&primary).unwrap();
    fs::hard_link(&outside, &primary).unwrap();
    assert!(service.refresh().last_error.is_some());
    fs::remove_file(&primary).unwrap();
    write_private(&primary, &vec![b' '; 65537]);
    assert!(service.refresh().last_error.is_some());
    fs::remove_file(&primary).unwrap();
    write_private(&primary, &serde_json::to_vec(&first.settings).unwrap());
    fs::set_permissions(&primary, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(service.refresh().last_error.is_some());
    assert_eq!(service.snapshot().revision, first.revision);
}

#[test]
fn rejected_initial_checkpoint_is_not_replaced_by_defaults() {
    let (_temp, sandbox) = fixture();
    let checkpoint = sandbox.root().join("runtime-settings.last-good.json");
    write_private(&checkpoint, b"{corrupt");
    assert!(RuntimeSettingsService::open(&sandbox).is_err());
    assert!(!sandbox.root().join("runtime-settings.json").exists());
    assert_eq!(fs::read(checkpoint).unwrap(), b"{corrupt");
}

#[test]
fn independent_candidate_roots_have_independent_policy() {
    let (_first_temp, first_root) = fixture();
    let (_second_temp, second_root) = fixture();
    let mut first = RuntimeSettingsService::open(&first_root).unwrap();
    let mut second = RuntimeSettingsService::open(&second_root).unwrap();
    let initial = first.snapshot();
    let mut changed = initial.settings;
    changed.defaults.retry_initial_ms = 35;
    first.update(&initial.revision, changed).unwrap();
    assert_eq!(second.refresh().settings, RuntimeSettings::default());
    assert_ne!(first.snapshot().source, second.snapshot().source);
    let _: PathBuf = first.snapshot().source;
}

#[test]
fn fifo_edits_and_busy_management_lock_return_last_good_without_waiting() {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
        time::Duration,
    };
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let original = service.snapshot();
    let path = sandbox.root().join("runtime-settings.json");
    fs::remove_file(&path).unwrap();
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let snapshot = service.refresh();
        sender.send((service, snapshot)).unwrap();
    });
    let (mut service, retained) = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("FIFO policy read must not wait for a writer");
    worker.join().unwrap();
    assert_eq!(retained.revision, original.revision);
    assert!(retained.last_error.is_some());
    fs::remove_file(&path).unwrap();
    publish_external(sandbox.root(), &original.settings);

    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(sandbox.root().join("runtime-settings.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let retained = service.refresh();
        let rejected = service
            .update(&retained.revision, RuntimeSettings::default())
            .is_err();
        sender.send((retained, rejected)).unwrap();
    });
    let (retained, rejected) = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("management lock contention must not block core");
    worker.join().unwrap();
    assert_eq!(retained.revision, original.revision);
    assert!(retained.last_error.is_some());
    assert!(rejected);
}

#[test]
fn owner_settings_snapshot_roundtrips_without_accepting_unknown_envelope_fields() {
    use cbc_candidate::settings::SettingsSnapshot;
    let (_temp, sandbox) = fixture();
    let snapshot = RuntimeSettingsService::open(&sandbox).unwrap().snapshot();
    let mut encoded = serde_json::to_value(&snapshot).unwrap();
    let parsed: SettingsSnapshot = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(parsed.revision, snapshot.revision);
    assert_eq!(parsed.settings, snapshot.settings);
    encoded["permissions"] = serde_json::json!("unrelated authority");
    assert!(serde_json::from_value::<SettingsSnapshot>(encoded).is_err());
}

#[test]
fn old_settings_files_inherit_the_default_notice_cadence_without_rewriting_primary() {
    let (_temp, sandbox) = fixture();
    let legacy = br#"{"schema":1,"defaults":{"retry_initial_ms":1000,"retry_max_ms":30000},"contexts":{"api":{"retry_initial_ms":500}},"roles":{},"members":{}}"#;
    let path = sandbox.root().join("runtime-settings.json");
    write_private(&path, legacy);
    let snapshot = RuntimeSettingsService::open(&sandbox).unwrap().snapshot();
    assert_eq!(snapshot.settings.defaults.room_notice_every_messages, 100);
    let effective = snapshot.effective("api", "repo_lead", "lead");
    assert_eq!(effective.policy.room_notice_every_messages, 100);
    assert_eq!(effective.policy.retry_initial_ms, 500);
    assert_eq!(fs::read(path).unwrap(), legacy);
}

#[test]
fn live_notice_cadence_updates_preserve_overrides_and_reset_to_inheritance() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let initial = service.snapshot();
    let mut settings = initial.settings;
    settings.defaults.room_notice_every_messages = 200;
    settings.contexts.insert(
        "api".into(),
        PolicyPatch {
            room_notice_every_messages: Some(150),
            ..PolicyPatch::default()
        },
    );
    settings.roles.insert(
        "repo_lead".into(),
        PolicyPatch {
            room_notice_every_messages: Some(125),
            ..PolicyPatch::default()
        },
    );
    settings.members.entry("api".into()).or_default().insert(
        "lead".into(),
        PolicyPatch {
            room_notice_every_messages: Some(50),
            ..PolicyPatch::default()
        },
    );
    let updated = service.update(&initial.revision, settings.clone()).unwrap();
    assert_ne!(initial.revision, updated.revision);
    assert_eq!(
        updated
            .effective("other", "feature_lead", "lead")
            .policy
            .room_notice_every_messages,
        200
    );
    assert_eq!(
        updated
            .effective("api", "feature_lead", "child")
            .policy
            .room_notice_every_messages,
        150
    );
    assert_eq!(
        updated
            .effective("api", "repo_lead", "peer")
            .policy
            .room_notice_every_messages,
        125
    );
    assert_eq!(
        updated
            .effective("api", "repo_lead", "lead")
            .policy
            .room_notice_every_messages,
        50
    );
    assert_eq!(
        updated.effective("api", "repo_lead", "lead").sources,
        ["defaults", "context", "role", "member"]
    );

    // External edits are observed by the same running service, with no restart.
    settings.defaults.room_notice_every_messages = 300;
    publish_external(sandbox.root(), &settings);
    let refreshed = service.refresh();
    assert_eq!(
        refreshed
            .effective("other", "feature_lead", "lead")
            .policy
            .room_notice_every_messages,
        300
    );
    assert_eq!(
        refreshed
            .effective("api", "repo_lead", "lead")
            .policy
            .room_notice_every_messages,
        50
    );
    settings.members.clear();
    let inherited = service.update(&refreshed.revision, settings).unwrap();
    assert_eq!(
        inherited
            .effective("api", "repo_lead", "lead")
            .policy
            .room_notice_every_messages,
        125
    );
    let reopened = RuntimeSettingsService::open(&sandbox).unwrap().snapshot();
    assert_eq!(reopened.revision, inherited.revision);
    assert_eq!(reopened.settings, inherited.settings);
}

#[test]
fn invalid_notice_cadence_and_hidden_invalid_overrides_keep_last_good() {
    let (_temp, sandbox) = fixture();
    let mut service = RuntimeSettingsService::open(&sandbox).unwrap();
    let initial = service.snapshot();
    for invalid in [0, 1_000_001] {
        let mut changed = initial.settings.clone();
        changed.defaults.room_notice_every_messages = invalid;
        assert!(service.update(&initial.revision, changed.clone()).is_err());
        publish_external(sandbox.root(), &changed);
        let retained = service.refresh();
        assert_eq!(retained.revision, initial.revision);
        assert_eq!(retained.settings.defaults.room_notice_every_messages, 100);
        assert!(retained.last_error.is_some());
        publish_external(sandbox.root(), &initial.settings);
        assert!(service.refresh().last_error.is_none());
    }
    let mut changed = initial.settings;
    changed.roles.insert(
        "repo_lead".into(),
        PolicyPatch {
            room_notice_every_messages: Some(0),
            ..PolicyPatch::default()
        },
    );
    changed.members.entry("api".into()).or_default().insert(
        "lead".into(),
        PolicyPatch {
            room_notice_every_messages: Some(100),
            ..PolicyPatch::default()
        },
    );
    assert!(changed.validate().is_err());
}
