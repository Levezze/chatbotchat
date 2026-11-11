//! C1: trial evidence. The running core freezes its database into a snapshot file on the
//! owner's request; the manifest and the report read only such snapshots, never the live
//! database, so the same snapshot always gives the same output.
use cbc_candidate::{
    connector::{LocalConnector, LocalOwnerClient},
    evidence::{canonical_json, Evidence, ManifestInput},
    owner::ProvisionSpec,
    store::Store,
    Sandbox,
};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

struct Core {
    temp: tempfile::TempDir,
    sandbox: Sandbox,
    socket: std::path::PathBuf,
    owner_key: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    running: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Core {
    async fn start() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("cbc-evidence-")
            .tempdir_in("/tmp")
            .unwrap();
        let sandbox = Sandbox::initialize(
            &temp.path().join("c"),
            8985,
            &[temp.path().join("synthetic-daily/state.db")],
        )
        .unwrap();
        let store = Store::open(&sandbox).await.unwrap();
        let connector = LocalConnector::bind(&sandbox, store).unwrap();
        let socket = connector.path().to_owned();
        let owner_key = std::fs::read_to_string(sandbox.root().join("owner.key"))
            .unwrap()
            .trim_end()
            .to_owned();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let running = tokio::spawn(connector.run(async {
            let _ = stopped.await;
        }));
        let core = Self {
            temp,
            sandbox,
            socket,
            owner_key,
            stop: Some(stop),
            running: Some(running),
        };
        let target = serde_json::json!({"v":1,"profile":"unknown","repository":"repo","worktree":null,
            "cwd":"/fixtures/repo","saved_project":{"status":"not_exposed"},
            "saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"synthetic-ui-observation"}});
        let spec: ProvisionSpec = serde_json::from_value(serde_json::json!({
            "context":"test", "device":"fixture-device",
            "members":[
                {"member":"lead","role":"repo_lead","host":"codex","session":"lead-native",
                 "evidence_ref":"synthetic-owned-session","repository":"repo","label":"repo lead","summary":"coordination","target":target},
                {"member":"feature","role":"feature_lead","host":"claude","session":"feature-native",
                 "evidence_ref":"synthetic-owned-session","repository":"repo","label":"feature lead","summary":"one assignment","target":target}
            ],
            "grants":[{"requester":"feature","target":"lead","kind":"report","scope_ref":"assignment-one"}]
        }))
        .unwrap();
        core.owner().provision(spec).await.unwrap();
        core
    }

    fn owner(&self) -> LocalOwnerClient<'_> {
        LocalOwnerClient::new(&self.socket, &self.owner_key)
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.running.take().unwrap().await.unwrap().unwrap();
    }
}

fn sha256(path: &std::path::Path) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(std::fs::read(path).unwrap()).to_vec()
}

#[tokio::test]
async fn a_snapshot_is_a_frozen_private_copy_that_later_writes_do_not_change() {
    let core = Core::start().await;
    let first = core.owner().snapshot().await.unwrap();
    core.owner()
        .retire_member("test", "feature", "trial seat ended")
        .await
        .unwrap();
    let second = core.owner().snapshot().await.unwrap();
    assert_ne!(first.file, second.file, "a snapshot never reuses a file");
    for snapshot in [&first, &second] {
        let meta = std::fs::symlink_metadata(&snapshot.file).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert!(snapshot
            .file
            .starts_with(core.sandbox.root().join("snapshots")));
    }

    let digest = sha256(&first.file);
    let before = Evidence::open(&core.sandbox, &first.file).await.unwrap();
    let after = Evidence::open(&core.sandbox, &second.file).await.unwrap();
    let retired = |seats: &[cbc_candidate::evidence::Seat]| {
        seats
            .iter()
            .find(|seat| seat.member == "feature")
            .unwrap()
            .retired
    };
    assert!(!retired(&before.seats().await.unwrap()));
    assert!(retired(&after.seats().await.unwrap()));
    before.close().await;
    after.close().await;
    assert_eq!(
        sha256(&first.file),
        digest,
        "reading evidence changes nothing"
    );
    core.stop().await;
}

#[tokio::test]
async fn evidence_opens_only_a_candidate_snapshot() {
    let core = Core::start().await;
    let snapshots = core.sandbox.root().join("snapshots");
    std::fs::create_dir_all(&snapshots).unwrap();
    // Named like a snapshot, so only the database brand check can refuse it.
    let junk = snapshots.join("snapshot-0-junk.sqlite3");
    std::fs::write(&junk, b"not a database at all, just bytes").unwrap();
    std::fs::set_permissions(&junk, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Evidence::open(&core.sandbox, &junk).await.is_err());
    // The live database is never evidence: it changes under the reader.
    assert!(Evidence::open(&core.sandbox, &core.sandbox.database_path())
        .await
        .is_err());
    core.stop().await;
}

#[tokio::test]
async fn the_manifest_is_canonical_lists_seats_and_carries_no_credentials() {
    let core = Core::start().await;
    let snapshot = core.owner().snapshot().await.unwrap();
    let settings = core.owner().settings().await.unwrap();
    let evidence = Evidence::open(&core.sandbox, &snapshot.file).await.unwrap();
    let input = ManifestInput {
        trial: 1,
        versions: BTreeMap::from([
            ("os".into(), "26.1".into()),
            ("claude".into(), "2.1".into()),
        ]),
        notes: BTreeMap::from([("skills_commit".into(), "abc".into())]),
        settings,
    };
    let manifest = evidence.manifest(&snapshot, &input).await.unwrap();
    evidence.close().await;
    let bytes = canonical_json(&manifest);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        !text.contains("credential"),
        "no credential field or digest"
    );
    assert!(!text.contains(&core.owner_key));

    // Sorted keys at every level, whatever order the value was built in.
    fn sorted(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                map.keys().zip(map.keys().skip(1)).all(|(a, b)| a < b) && map.values().all(sorted)
            }
            serde_json::Value::Array(items) => items.iter().all(sorted),
            _ => true,
        }
    }
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(sorted(&parsed));
    assert_eq!(
        canonical_json(&parsed),
        bytes,
        "canonical form is a fixed point"
    );

    assert_eq!(parsed["trial"], 1);
    assert_eq!(parsed["schema_version"], cbc_candidate::SCHEMA_VERSION);
    // The build is the one the core that took the snapshot reports, not the caller's.
    assert_eq!(
        parsed["build"],
        serde_json::to_value(&snapshot.build).unwrap()
    );
    assert_eq!(snapshot.build.binary_sha256.len(), 64);
    assert_eq!(parsed["snapshot"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(parsed["versions"]["claude"], "2.1");
    assert_eq!(parsed["notes"]["skills_commit"], "abc");
    assert!(parsed["settings"]["revision"].is_string());
    let seats = parsed["seats"].as_array().unwrap();
    assert_eq!(seats.len(), 2);
    let lead = seats.iter().find(|s| s["member"] == "lead").unwrap();
    assert_eq!(lead["role"], "repo_lead");
    assert_eq!(lead["host"], "codex");
    assert_eq!(lead["session"], "lead-native");
    // The manifest is written once, before the go, and never edited: the user's approval
    // tally is a separate file the report reads, so the manifest carries no slot for it.
    assert!(parsed.get("approval_prompts").is_none());
    core.stop().await;
}

#[tokio::test]
async fn the_manifest_command_writes_a_new_file_beside_its_snapshot_only() {
    use cbc_candidate::evidence::{record_manifest, ManifestArgs};
    let core = Core::start().await;
    let args = || ManifestArgs {
        trial: 2,
        versions: BTreeMap::new(),
        notes: BTreeMap::new(),
    };
    let snapshots = core.sandbox.root().join("snapshots");
    let outside = core.sandbox.root().join("manifest.json");
    assert!(
        record_manifest(&core.sandbox, &core.owner(), &outside, args())
            .await
            .is_err()
    );
    assert!(!outside.exists());

    let out = snapshots.join("trial-2.json");
    let snapshot = record_manifest(&core.sandbox, &core.owner(), &out, args())
        .await
        .unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(
        std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(canonical_json(&parsed), bytes);
    assert_eq!(parsed["snapshot"]["file"], snapshot.file.to_str().unwrap());
    let digest: String = sha256(&snapshot.file)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(parsed["snapshot"]["sha256"], digest);

    // The root given on the command line may be an alias of the sandbox root (on macOS,
    // /tmp is /private/tmp): the same directory, so the same rule applies.
    let alias = core.temp.path().join("c/snapshots/trial-3.json");
    assert_ne!(alias.parent().unwrap(), snapshots.as_path());
    record_manifest(&core.sandbox, &core.owner(), &alias, args())
        .await
        .unwrap();
    assert!(snapshots.join("trial-3.json").is_file());
    // An alias still has to name the snapshot directory itself.
    let beside = core.temp.path().join("c/trial-4.json");
    assert!(
        record_manifest(&core.sandbox, &core.owner(), &beside, args())
            .await
            .is_err()
    );

    // Never overwritten, and a refused run leaves the existing record as it was.
    assert!(record_manifest(&core.sandbox, &core.owner(), &out, args())
        .await
        .is_err());
    assert_eq!(std::fs::read(&out).unwrap(), bytes);
    core.stop().await;
}
