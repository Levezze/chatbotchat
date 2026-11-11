//! The T-27 waits mapping (R3): every §6 wait cause of the trial protocol has a declared
//! source, and the causes the records already carry are derived from them, never guessed.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::{
    enrollment::ConnectionKind,
    requests::RequestOwner,
    waits::{Derivation, WaitCause},
};
use work_fixture::fixture;

/// The cause labels in the protocol's §6 table, in order.
fn protocol_causes() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/design/working-trial-protocol.md");
    let text = std::fs::read_to_string(path).unwrap();
    let mut rows = text
        .lines()
        .skip_while(|line| line.trim() != "| Cause | Source |")
        .skip(2);
    let mut causes = Vec::new();
    for line in rows.by_ref() {
        let Some(row) = line.trim().strip_prefix('|') else {
            break;
        };
        causes.push(row.split('|').next().unwrap().trim().to_owned());
    }
    assert!(!causes.is_empty(), "the §6 cause table was not found");
    causes
}

/// The code and the protocol name the same causes, so a trial report can't silently drop or
/// invent one.
#[test]
fn the_causes_match_the_protocol_table() {
    assert_eq!(
        WaitCause::ALL
            .iter()
            .map(|cause| cause.label().to_owned())
            .collect::<Vec<_>>(),
        protocol_causes()
    );
}

/// A cause is only claimed derivable where a record carries it today. The rest say what is
/// missing, so the report shows `unknown` rather than a guess.
#[test]
fn each_cause_states_whether_the_records_carry_it() {
    for cause in WaitCause::ALL {
        let derivation = cause.derivation();
        match cause {
            WaitCause::User | WaitCause::ChapterMaster | WaitCause::Delivery => {
                assert_eq!(derivation, Derivation::Known, "{cause:?}")
            }
            // C2: B1's activity facts give receipt-to-first-activity; whether anything else
            // blocked the member still needs holds (#114).
            WaitCause::HumanReview | WaitCause::Primarch | WaitCause::ReadyIdle => {
                assert!(
                    matches!(derivation, Derivation::Partial { .. }),
                    "{cause:?}"
                )
            }
            WaitCause::Resource | WaitCause::Ci => {
                assert!(
                    matches!(derivation, Derivation::Unknown { .. }),
                    "{cause:?}"
                )
            }
        }
    }
}

/// An asker waits on whoever owns its open request, by the owner's role, and every member waits
/// on the delivery of what it has not receipted. Answered and relayed requests are not waits.
#[tokio::test]
async fn open_requests_and_unread_items_are_classified() {
    let case = fixture().await;
    case.store
        .authorize_connection("project", "worker", "other", ConnectionKind::Peer, "shared")
        .await
        .unwrap();
    case.store
        .connect_members(
            &case.worker,
            "other",
            ConnectionKind::Peer,
            "shared",
            "p-1",
            "",
        )
        .await
        .unwrap();
    let mut ids = std::collections::HashMap::new();
    for (name, owner) in [
        ("for-user", RequestOwner::User),
        ("for-lead", RequestOwner::Member("lead".into())),
        ("for-peer", RequestOwner::Member("other".into())),
        ("relayed", RequestOwner::User),
        ("answered", RequestOwner::Member("lead".into())),
    ] {
        let request = case
            .store
            .open_request(&case.worker, &owner, None, "?", name)
            .await
            .unwrap()
            .request;
        ids.insert(request, name);
    }
    let id = |name: &str| ids.iter().find(|(_, n)| **n == name).unwrap().0.clone();
    case.store
        .answer_request(&case.worker, &id("relayed"), "the user said yes", "r-1")
        .await
        .unwrap();
    case.store
        .answer_request(&case.lead, &id("answered"), "yes", "a-1")
        .await
        .unwrap();
    // The worker reads everything it was sent, so only its requests remain.
    while let Some(batch) = case.store.check(&case.worker, 32).await.unwrap() {
        case.store
            .receipt(&case.worker, batch.reference_for_adapter())
            .await
            .unwrap();
    }

    let mut worker = case
        .store
        .waits_now("project", "worker")
        .await
        .unwrap()
        .into_iter()
        .map(|wait| (ids[&wait.request.unwrap()], wait.cause))
        .collect::<Vec<_>>();
    worker.sort();
    assert_eq!(
        worker,
        vec![
            ("for-lead", Some(WaitCause::Primarch)),
            ("for-peer", Some(WaitCause::ChapterMaster)),
            ("for-user", Some(WaitCause::User)),
        ]
    );

    // The lead was told of two requests and has read neither.
    let lead = case.store.waits_now("project", "lead").await.unwrap();
    assert_eq!(lead.len(), 2, "{lead:?}");
    for wait in &lead {
        assert_eq!(wait.cause, Some(WaitCause::Delivery));
        assert!(wait.sequence.is_some() && wait.since_wall_ms.is_some());
    }
}
