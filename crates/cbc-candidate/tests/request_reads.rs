//! R3b: bounded, scoped reads of requests. A member reads a request it asked, a request it
//! owns, or, as a repo lead, any request; anyone else can't tell it from a missing one. Every
//! list comes in pages small enough for the connector's reply limit. Real SQLite only.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::requests::{RequestOwner, RequestState, MAX_REQUEST_PAGE};
use work_fixture::{fixture, Case};

fn lead() -> RequestOwner {
    RequestOwner::Member("lead".into())
}

async fn ask(case: &Case, owner: &RequestOwner, question: &str, key: &str) -> String {
    case.store
        .open_request(&case.worker, owner, None, question, key)
        .await
        .unwrap()
        .request
}

#[tokio::test]
async fn a_request_is_read_by_its_asker_its_owner_and_a_repo_lead_only() {
    let case = fixture().await;
    let for_lead = ask(&case, &lead(), "Which branch?", "k-1").await;
    let for_user = ask(&case, &RequestOwner::User, "Ship it?", "k-2").await;

    for id in [&for_lead, &for_user] {
        let read = case.store.read_request(&case.worker, id).await.unwrap();
        assert_eq!(&read.request, id, "the asker reads it");
        assert_eq!(
            case.store
                .read_request(&case.lead, id)
                .await
                .unwrap()
                .request,
            *id,
            "the owner, or a repo lead, reads it"
        );
        let hidden = case
            .store
            .read_request(&case.other, id)
            .await
            .unwrap_err()
            .to_string();
        let missing = case
            .store
            .read_request(&case.worker, "req-missing")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(hidden, "unknown request");
        assert_eq!(hidden, missing);
        let hidden_answers = case
            .store
            .read_request_answers(&case.other, id, 0, 10)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(hidden_answers, "unknown request");
    }
}

/// A member owner who is not a repo lead still reads a request it owns.
#[tokio::test]
async fn a_feature_lead_reads_a_request_it_owns() {
    let case = fixture().await;
    case.store
        .authorize_connection(
            "project",
            "worker",
            "other",
            cbc_candidate::enrollment::ConnectionKind::Peer,
            "feature",
        )
        .await
        .unwrap();
    case.store
        .connect_members(
            &case.worker,
            "other",
            cbc_candidate::enrollment::ConnectionKind::Peer,
            "feature",
            "p1",
            "",
        )
        .await
        .unwrap();
    let id = ask(
        &case,
        &RequestOwner::Member("other".into()),
        "Rebase?",
        "k-1",
    )
    .await;
    assert_eq!(
        case.store
            .read_request(&case.other, &id)
            .await
            .unwrap()
            .request,
        id
    );
    let pending = case
        .store
        .pending_requests(&case.other, "", 10)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "an owner sees what it has to answer");
}

/// Pending means open or relayed, the member's own (asked or owned), ordered by ID, in pages
/// of at most MAX_REQUEST_PAGE.
#[tokio::test]
async fn pending_requests_are_the_members_own_in_bounded_pages() {
    let case = fixture().await;
    let mut ids = Vec::new();
    for n in 0..(MAX_REQUEST_PAGE + 3) {
        ids.push(ask(&case, &RequestOwner::User, "Ship it?", &format!("k-{n}")).await);
    }
    ids.sort();
    let first = case
        .store
        .pending_requests(&case.worker, "", 1000)
        .await
        .unwrap();
    assert_eq!(first.len(), MAX_REQUEST_PAGE as usize, "the page is capped");
    assert_eq!(
        first.iter().map(|r| r.request.clone()).collect::<Vec<_>>(),
        ids[..MAX_REQUEST_PAGE as usize]
    );
    let rest = case
        .store
        .pending_requests(&case.worker, &first.last().unwrap().request, 1000)
        .await
        .unwrap();
    assert_eq!(
        rest.iter().map(|r| r.request.clone()).collect::<Vec<_>>(),
        ids[MAX_REQUEST_PAGE as usize..]
    );
    assert_eq!(
        case.store
            .pending_requests(&case.worker, "", 2)
            .await
            .unwrap()
            .len(),
        2,
        "a smaller limit is honoured"
    );

    // Another member's requests, even a repo lead's view of them, are not its pending list.
    assert!(case
        .store
        .pending_requests(&case.other, "", 10)
        .await
        .unwrap()
        .is_empty());
    assert!(case
        .store
        .pending_requests(&case.lead, "", 10)
        .await
        .unwrap()
        .is_empty());

    // Answered and withdrawn requests leave the list; relayed ones stay.
    case.store
        .owner_answer_request("project", &ids[0], "Yes.", "u-1")
        .await
        .unwrap();
    case.store
        .withdraw_request(&case.worker, &ids[1], "moot")
        .await
        .unwrap();
    case.store
        .answer_request(&case.lead, &ids[2], "The user said yes.", "r-1")
        .await
        .unwrap();
    let remaining = case
        .store
        .owner_pending_requests("project", "", 1000)
        .await
        .unwrap();
    assert_eq!(remaining.len(), MAX_REQUEST_PAGE as usize);
    assert_eq!(remaining[0].request, ids[2]);
    assert_eq!(remaining[0].state, RequestState::Relayed);
    let all_remaining = case
        .store
        .owner_pending_requests("project", &remaining.last().unwrap().request, 1000)
        .await
        .unwrap();
    assert_eq!(remaining.len() + all_remaining.len(), ids.len() - 2);
}

/// Answers come in pages by answer ID, so many relays never outgrow a reply.
#[tokio::test]
async fn answers_come_in_bounded_pages_for_the_asker_and_the_owner_route() {
    let case = fixture().await;
    let id = ask(&case, &RequestOwner::User, "Ship it?", "k-1").await;
    for n in 0..(MAX_REQUEST_PAGE + 2) {
        case.store
            .answer_request(&case.lead, &id, &format!("relay {n}"), &format!("r-{n}"))
            .await
            .unwrap();
    }
    let first = case
        .store
        .read_request_answers(&case.worker, &id, 0, 1000)
        .await
        .unwrap();
    assert_eq!(first.len(), MAX_REQUEST_PAGE as usize);
    assert_eq!(first[0].body, "relay 0");
    let rest = case
        .store
        .read_request_answers(&case.worker, &id, first.last().unwrap().id, 1000)
        .await
        .unwrap();
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[1].body, format!("relay {}", MAX_REQUEST_PAGE + 1));
    let owner = case
        .store
        .owner_request_answers("project", &id, 0, 3)
        .await
        .unwrap();
    assert_eq!(
        owner.iter().map(|a| a.id).collect::<Vec<_>>(),
        first[..3].iter().map(|a| a.id).collect::<Vec<_>>()
    );
    assert!(case
        .store
        .owner_request_answers("project", "req-missing", 0, 3)
        .await
        .is_err());
}
