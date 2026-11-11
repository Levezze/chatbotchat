//! R3b: requests over the local connector. Members reach them through their binding, the user
//! through the owner route, and every committed request event wakes a parked watcher.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    connector::{LocalClient, LocalConnector, LocalOwnerClient},
    enrollment::ConnectionKind,
    requests::{AnswerChannel, RequestOwner, RequestState, MAX_REQUEST_PAGE},
    store::Store,
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
        .prefix("cbc-req-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8995,
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
        async move { store.bind_member("project", m, "device", m).await.unwrap() }
    };
    let worker = bind("worker").await;
    let lead = bind("lead").await;
    let other = bind("other").await;
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
    for who in [&case.worker, &case.lead] {
        drain(&LocalClient::new(&case.socket, who)).await;
    }
    case
}

/// Kinds of every item a member reads, receipting as it goes.
async fn drain(client: &LocalClient<'_>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Some(batch) = client.check().await.unwrap() {
        for item in &batch.items {
            seen.push(item.kind.clone());
        }
        client.receipt(&batch).await.unwrap();
    }
    seen
}

async fn stop(case: Running) {
    case.stop.send(()).unwrap();
    case.task.await.unwrap().unwrap();
}

/// Park a watch for `who`, run `act`, and require that it wakes the watcher.
async fn wakes<F: std::future::Future>(watcher: &LocalClient<'_>, act: F, what: &str) {
    let mut parked = Box::pin(watcher.watch(&[], 0));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut parked)
            .await
            .is_err(),
        "nothing to wake for before {what}"
    );
    act.await;
    tokio::time::timeout(Duration::from_secs(3), parked)
        .await
        .unwrap_or_else(|_| panic!("{what} wakes the parked watcher"))
        .unwrap();
    assert_eq!(drain(watcher).await, vec!["request_event".to_string()]);
}

fn lead_owner() -> RequestOwner {
    RequestOwner::Member("lead".into())
}

#[tokio::test]
async fn a_member_request_round_trips_and_each_event_wakes_its_recipient() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);

    let mut opened = None;
    wakes(
        &lead,
        async {
            opened = Some(
                worker
                    .open_request(&lead_owner(), None, "Which branch?", "k-1")
                    .await
                    .unwrap(),
            );
        },
        "an open to a member",
    )
    .await;
    let opened = opened.unwrap();
    assert_eq!(opened.state, RequestState::Open);
    let retried = worker
        .open_request(&lead_owner(), None, "Which branch?", "k-1")
        .await
        .unwrap();
    assert_eq!(retried.request, opened.request, "a retried key replays");
    assert_eq!(
        lead.pending_requests("", 10).await.unwrap().len(),
        1,
        "the owner sees it pending"
    );

    let mut answer = None;
    wakes(
        &worker,
        async {
            answer = Some(
                lead.answer_request(&opened.request, "main", "a-1")
                    .await
                    .unwrap(),
            );
        },
        "a member answer",
    )
    .await;
    let answer = answer.unwrap();
    assert_eq!(answer.channel, AnswerChannel::Member);
    assert_eq!(
        worker.request(&opened.request).await.unwrap().state,
        RequestState::Answered
    );
    let applied = worker
        .mark_answer_applied(&opened.request, answer.id)
        .await
        .unwrap();
    assert!(applied.applied_wall_ms.is_some());
    let answers = worker
        .request_answers(&opened.request, 0, 10)
        .await
        .unwrap();
    assert_eq!(answers, vec![applied]);
    assert!(worker.pending_requests("", 10).await.unwrap().is_empty());
    stop(case).await;
}

#[tokio::test]
async fn a_user_request_wakes_the_asker_on_a_relay_and_on_the_owner_answer() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);
    let owner = LocalOwnerClient::new(&case.socket, &case.owner_key);
    let opened = worker
        .open_request(&RequestOwner::User, None, "Ship it?", "k-1")
        .await
        .unwrap();
    assert_eq!(
        owner.pending_requests("project", "", 10).await.unwrap(),
        vec![opened.clone()]
    );
    assert_eq!(
        owner.request("project", &opened.request).await.unwrap(),
        opened
    );

    wakes(
        &worker,
        async {
            let relay = lead
                .answer_request(&opened.request, "The user said yes.", "r-1")
                .await
                .unwrap();
            assert_eq!(relay.channel, AnswerChannel::Relayed);
        },
        "a relay",
    )
    .await;
    wakes(
        &worker,
        async {
            let answer = owner
                .answer_request("project", &opened.request, "Yes.", "u-1")
                .await
                .unwrap();
            assert_eq!(answer.channel, AnswerChannel::Owner);
        },
        "the user's answer",
    )
    .await;
    let answers = owner
        .request_answers("project", &opened.request, 0, 10)
        .await
        .unwrap();
    assert_eq!(
        answers.iter().map(|a| a.channel).collect::<Vec<_>>(),
        vec![AnswerChannel::Relayed, AnswerChannel::Owner]
    );
    assert!(LocalOwnerClient::new(&case.socket, "not-an-owner-key")
        .answer_request("project", &opened.request, "No.", "u-2")
        .await
        .is_err());
    stop(case).await;
}

#[tokio::test]
async fn a_withdrawal_wakes_the_member_owner_and_hidden_requests_stay_hidden() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);
    let other = LocalClient::new(&case.socket, &case.other);
    let opened = worker
        .open_request(&lead_owner(), None, "Which branch?", "k-1")
        .await
        .unwrap();
    drain(&lead).await;

    assert!(other.request(&opened.request).await.is_err());
    assert!(other.request_answers(&opened.request, 0, 10).await.is_err());
    assert!(other
        .withdraw_request(&opened.request, "mine now")
        .await
        .is_err());
    assert!(other.pending_requests("", 10).await.unwrap().is_empty());

    wakes(
        &lead,
        async {
            let withdrawn = worker
                .withdraw_request(&opened.request, "moot")
                .await
                .unwrap();
            assert_eq!(withdrawn.state, RequestState::Withdrawn);
        },
        "a withdrawal",
    )
    .await;
    assert!(lead
        .answer_request(&opened.request, "main", "a-1")
        .await
        .is_err());
    stop(case).await;
}

/// The largest page of the largest requests and answers fits the reply limit, on both routes.
#[tokio::test]
async fn a_full_page_of_the_largest_requests_and_answers_fits_a_reply() {
    let case = running().await;
    let worker = LocalClient::new(&case.socket, &case.worker);
    let lead = LocalClient::new(&case.socket, &case.lead);
    let owner = LocalOwnerClient::new(&case.socket, &case.owner_key);
    let largest = "\u{1F600}".repeat(4000);
    let mut first = None;
    for n in 0..MAX_REQUEST_PAGE {
        let opened = worker
            .open_request(&RequestOwner::User, None, &largest, &format!("k-{n}"))
            .await
            .unwrap();
        first.get_or_insert(opened.request);
    }
    let first = first.unwrap();
    for n in 0..MAX_REQUEST_PAGE {
        lead.answer_request(&first, &largest, &format!("r-{n}"))
            .await
            .unwrap();
    }
    let mine = worker.pending_requests("", 1000).await.unwrap();
    assert_eq!(mine.len(), MAX_REQUEST_PAGE as usize);
    assert!(mine.iter().all(|r| r.question == largest));
    assert_eq!(
        owner
            .pending_requests("project", "", 1000)
            .await
            .unwrap()
            .len(),
        MAX_REQUEST_PAGE as usize
    );
    let answers = worker.request_answers(&first, 0, 1000).await.unwrap();
    assert_eq!(answers.len(), MAX_REQUEST_PAGE as usize);
    assert!(answers.iter().all(|a| a.body == largest));
    assert_eq!(
        owner
            .request_answers("project", &first, 0, 1000)
            .await
            .unwrap()
            .len(),
        MAX_REQUEST_PAGE as usize
    );
    stop(case).await;
}

/// The owner of a request is `"user"` or `{"member": id}` on the wire, nothing else.
#[test]
fn a_request_owner_on_the_wire_is_user_or_one_member() {
    assert_eq!(
        serde_json::to_string(&RequestOwner::User).unwrap(),
        r#""user""#
    );
    assert_eq!(
        serde_json::to_string(&lead_owner()).unwrap(),
        r#"{"member":"lead"}"#
    );
    assert_eq!(
        serde_json::from_str::<RequestOwner>(r#"{"member":"lead"}"#).unwrap(),
        lead_owner()
    );
    assert!(serde_json::from_str::<RequestOwner>(r#""owner""#).is_err());
    assert!(serde_json::from_str::<RequestOwner>(r#"{"member":"a","extra":1}"#).is_err());
}
