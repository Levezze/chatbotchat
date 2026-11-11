//! R2b: work items over the local connector. Members reach them through their binding, the
//! user through the owner route, and a committed work event wakes a parked watcher.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    connector::{LocalClient, LocalConnector, LocalOwnerClient},
    enrollment::ConnectionKind,
    store::Store,
    work::{Stage, WorkOutcome, WorkPatch},
    Sandbox,
};
use std::time::Duration;

struct Running {
    _temp: tempfile::TempDir,
    socket: std::path::PathBuf,
    owner_key: String,
    worker: SessionBinding,
    lead: SessionBinding,
    other: SessionBinding,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn running() -> Running {
    let temp = tempfile::Builder::new()
        .prefix("cbc-work-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8991,
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
    let worker = store
        .bind_member("project", "worker", "device", "w")
        .await
        .unwrap();
    let lead = store
        .bind_member("project", "lead", "device", "l")
        .await
        .unwrap();
    let other = store
        .bind_member("project", "other", "device", "o")
        .await
        .unwrap();
    store
        .authorize_connection("project", "worker", "lead", ConnectionKind::Report, "f")
        .await
        .unwrap();
    store
        .connect_members(&worker, "lead", ConnectionKind::Report, "f", "r1", "")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let socket = connector.path().to_owned();
    let owner_key = std::fs::read_to_string(sandbox.root().join("owner.key"))
        .unwrap()
        .trim_end()
        .to_owned();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    let case = Running {
        _temp: temp,
        socket,
        owner_key,
        worker,
        lead,
        other,
        stop,
        task,
    };
    // Start from empty inboxes: the introduction is not a work event.
    for who in [&case.worker, &case.lead] {
        drain(&LocalClient::new(&case.socket, who)).await;
    }
    case
}

/// Kinds of every item a member reads, receipting as it goes.
async fn drain(client: &LocalClient<'_>) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    while let Some(batch) = client.check().await.unwrap() {
        for item in &batch.items {
            seen.push((item.kind.clone(), item.sender.clone()));
        }
        client.receipt(&batch).await.unwrap();
    }
    seen
}

async fn stop(case: Running) {
    case.stop.send(()).unwrap();
    case.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_feature_lead_drives_its_item_and_a_parked_lead_is_woken() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);

    // The lead's inbox is empty, so a watch from zero parks until something arrives.
    let mut parked = Box::pin(lead.watch(&[], 0));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut parked)
            .await
            .is_err(),
        "nothing to wake for yet"
    );
    let registered = worker
        .register_work_item("feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert_eq!(registered.item.revision, 1);
    assert!(!registered.replayed);
    tokio::time::timeout(Duration::from_secs(3), parked)
        .await
        .expect("registration wakes the parked lead")
        .unwrap();
    assert_eq!(
        drain(&lead).await,
        vec![("work_event".into(), "cbc".into())]
    );

    let applied = worker
        .update_work_item(
            "feat-login",
            1,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Implementing),
                branch: Some(Some("feat/login".into())),
                ..WorkPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(applied, WorkOutcome::Applied { revision: 2 });
    let cleared = worker
        .update_work_item(
            "feat-login",
            2,
            "u-2",
            &WorkPatch {
                branch: Some(None),
                ..WorkPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared, WorkOutcome::Applied { revision: 3 });
    let current = worker.work_item("feat-login").await.unwrap();
    assert_eq!(current.revision, 3);
    assert_eq!(current.stage, Stage::Implementing);
    assert_eq!(
        current.branch, None,
        "null on the wire clears, absence leaves"
    );
    assert_eq!(drain(&lead).await.len(), 2);
    stop(case).await;
}

#[tokio::test]
async fn hitl_authority_holds_over_the_wire() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);
    let owner = LocalOwnerClient::new(&case.socket, &case.owner_key);
    worker
        .register_work_item("feat-login", "Login flow", "reg-1")
        .await
        .unwrap();

    assert!(worker
        .set_work_hitl("feat-login", true, "w-1")
        .await
        .is_err());
    assert_eq!(
        lead.set_work_hitl("feat-login", true, "h-1").await.unwrap(),
        WorkOutcome::Applied { revision: 2 }
    );
    assert!(lead
        .set_work_hitl("feat-login", false, "h-2")
        .await
        .is_err());
    assert_eq!(
        owner
            .set_work_hitl("project", "feat-login", false, "user-1")
            .await
            .unwrap(),
        WorkOutcome::Applied { revision: 3 }
    );
    assert!(LocalOwnerClient::new(&case.socket, "not-an-owner-key")
        .set_work_hitl("project", "feat-login", false, "user-2")
        .await
        .is_err());

    let item = owner.work_item("project", "feat-login").await.unwrap();
    assert!(!item.hitl);
    assert_eq!(item.revision, 3);
    let revisions = owner
        .work_revisions("project", "feat-login", 0, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 3);
    assert_eq!(
        revisions
            .iter()
            .map(|r| r.changed.clone())
            .collect::<Vec<_>>(),
        vec![
            vec!["registered".to_string()],
            vec!["hitl".to_string()],
            vec!["hitl".to_string()]
        ]
    );
    stop(case).await;
}

/// The owner and a repo lead can read an item; another feature lead can't, and can't tell an
/// unknown item from one it may not see.
#[tokio::test]
async fn a_work_item_is_read_by_its_owner_and_a_repo_lead_only() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);
    let other = LocalClient::new(&case.socket, &case.other);
    worker
        .register_work_item("feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    assert_eq!(
        worker.work_item("feat-login").await.unwrap().owner,
        "worker"
    );
    assert_eq!(lead.work_item("feat-login").await.unwrap().owner, "worker");
    let hidden = other.work_item("feat-login").await.unwrap_err().to_string();
    let missing = worker.work_item("missing").await.unwrap_err().to_string();
    assert_eq!(hidden, missing);
    assert!(other
        .update_work_item(
            "feat-login",
            1,
            "u-1",
            &WorkPatch {
                stage: Some(Stage::Abandoned),
                ..WorkPatch::default()
            },
        )
        .await
        .is_err());
    stop(case).await;
}

/// The wire form of a patch keeps leave, clear and set apart and refuses unknown fields.
#[test]
fn a_patch_on_the_wire_keeps_leave_clear_and_set_apart() {
    let leave: WorkPatch = serde_json::from_str(r#"{"stage":"in_review"}"#).unwrap();
    assert_eq!(leave.branch, None);
    let clear: WorkPatch = serde_json::from_str(r#"{"branch":null}"#).unwrap();
    assert_eq!(clear.branch, Some(None));
    let set: WorkPatch = serde_json::from_str(r#"{"branch":"feat/x"}"#).unwrap();
    assert_eq!(set.branch, Some(Some("feat/x".into())));
    assert!(serde_json::from_str::<WorkPatch>(r#"{"hitl":true}"#).is_err());
    for patch in [leave, clear, set] {
        let back: WorkPatch =
            serde_json::from_str(&serde_json::to_string(&patch).unwrap()).unwrap();
        assert_eq!(back, patch);
    }
}

/// History comes in pages of at most 100, so a long-lived item never outgrows the reply limit.
/// Revisions are append-only, so a page can't contradict an item read taken before or after it.
#[tokio::test]
async fn the_owner_reads_revision_history_in_bounded_pages() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let owner = LocalOwnerClient::new(&case.socket, &case.owner_key);
    worker
        .register_work_item("feat-login", "Login flow", "reg-1")
        .await
        .unwrap();
    for n in 0..120 {
        owner
            .set_work_hitl("project", "feat-login", n % 2 == 0, &format!("user-{n}"))
            .await
            .unwrap();
    }
    assert_eq!(
        owner
            .work_item("project", "feat-login")
            .await
            .unwrap()
            .revision,
        121
    );
    let first = owner
        .work_revisions("project", "feat-login", 0, 1000)
        .await
        .unwrap();
    assert_eq!(first.len(), 100, "the page size is capped");
    assert_eq!(first[0].revision, 1);
    assert_eq!(first[99].revision, 100);
    let rest = owner
        .work_revisions("project", "feat-login", 100, 1000)
        .await
        .unwrap();
    assert_eq!(
        rest.iter().map(|r| r.revision).collect::<Vec<_>>(),
        (101..=121).collect::<Vec<_>>()
    );
    assert!(owner
        .work_revisions("project", "feat-login", 121, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(owner.work_item("project", "missing").await.is_err());
    stop(case).await;
}
