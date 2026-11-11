//! Canonical requests (T-10, T-12), R3: one row per question, an owner who answers and a
//! callback who is woken, append-only answers that record their channel. Real SQLite only.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::{
    bindings::SessionBinding,
    enrollment::ConnectionKind,
    facts::ItemClass,
    requests::{AnswerChannel, RequestOwner, RequestState},
    store::Store,
    Sandbox,
};
use serde_json::Value;
use tempfile::tempdir;
use work_fixture::{database_before, fixture, release, Case};

/// Every request event a member receives, as (room, parsed body). Receipts everything.
async fn requests_for(store: &Store, who: &SessionBinding) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    while let Some(batch) = store.check(who, 32).await.unwrap() {
        for item in &batch.items {
            if item.kind == "request_event" {
                assert_eq!(item.sender, "cbc", "a request event is server information");
                events.push((item.room.clone(), serde_json::from_str(&item.body).unwrap()));
            }
        }
        store
            .receipt(who, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    events
}

fn lead() -> RequestOwner {
    RequestOwner::Member("lead".into())
}

/// Open a request and return the ID the server gave it.
async fn ask(case: &Case, who: &SessionBinding, owner: &RequestOwner, key: &str) -> String {
    case.store
        .open_request(who, owner, None, "Which branch?", key)
        .await
        .unwrap()
        .request
}

fn message(error: anyhow::Error) -> String {
    error.to_string()
}

/// A question for the user needs no room. The user's answer on the owner route is the
/// authenticated one, and it wakes the asker and nobody else.
#[tokio::test]
async fn the_users_answer_on_the_owner_route_wakes_the_asker() {
    let case = fixture().await;
    let opened = case
        .store
        .open_request(&case.worker, &RequestOwner::User, None, "Ship it?", "k-1")
        .await
        .unwrap();
    assert_eq!(opened.state, RequestState::Open);
    assert_eq!(opened.callback, "worker");
    assert!(opened.request.starts_with("req-") && opened.request.len() == 36);
    assert!(requests_for(&case.store, &case.lead).await.is_empty());

    let answer = case
        .store
        .owner_answer_request("project", &opened.request, "Yes.", "u-1")
        .await
        .unwrap();
    assert_eq!(answer.channel, AnswerChannel::Owner);
    assert_eq!(answer.actor, None);
    let request = case
        .store
        .request("project", &opened.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.state, RequestState::Answered);

    let events = requests_for(&case.store, &case.worker).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, case.report_room);
    assert_eq!(events[0].1["request"], opened.request);
    assert_eq!(events[0].1["event"], "answered");
    assert_eq!(events[0].1["channel"], "owner");
    assert_eq!(events[0].1["answer"], answer.id);
    assert_eq!(events[0].1["body"], "Yes.");
    assert!(requests_for(&case.store, &case.lead).await.is_empty());

    // The user answers once. A second answer, under a new key, is refused.
    let again = case
        .store
        .owner_answer_request("project", &opened.request, "No.", "u-2")
        .await
        .unwrap_err();
    assert!(message(again).contains("already answered"));
}

/// A member can only relay what the user said; a relay never counts as the user's answer.
/// The user can still answer after it, and once the user has, nobody relays again.
#[tokio::test]
async fn a_relay_is_recorded_as_one_and_never_locks_the_user_out() {
    let case = fixture().await;
    let id = ask(&case, &case.worker, &RequestOwner::User, "k-1").await;
    let relayed = case
        .store
        .answer_request(&case.worker, &id, "The user said yes in chat.", "r-1")
        .await
        .unwrap();
    assert_eq!(relayed.channel, AnswerChannel::Relayed);
    assert_eq!(relayed.actor.as_deref(), Some("worker"));
    assert_eq!(
        case.store
            .request("project", &id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RequestState::Relayed,
        "relayed, not answered"
    );
    assert!(
        requests_for(&case.store, &case.worker).await.is_empty(),
        "the relaying asker is not woken by its own relay"
    );

    case.store
        .owner_answer_request("project", &id, "Yes.", "u-1")
        .await
        .unwrap();
    assert_eq!(
        case.store
            .request("project", &id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RequestState::Answered
    );
    let refused = case
        .store
        .answer_request(&case.lead, &id, "The user said no.", "r-2")
        .await
        .unwrap_err();
    assert!(message(refused).contains("already answered"));
    let answers = case.store.request_answers("project", &id).await.unwrap();
    assert_eq!(
        answers.iter().map(|a| a.channel).collect::<Vec<_>>(),
        vec![AnswerChannel::Relayed, AnswerChannel::Owner]
    );
}

/// A relay by someone other than the asker wakes the asker.
#[tokio::test]
async fn a_relay_by_another_member_wakes_the_asker() {
    let case = fixture().await;
    let id = ask(&case, &case.worker, &RequestOwner::User, "k-1").await;
    case.store
        .answer_request(&case.lead, &id, "The user said yes.", "r-1")
        .await
        .unwrap();
    let events = requests_for(&case.store, &case.worker).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["channel"], "relayed");
}

/// A question for a member goes to that member in the pair's room and only that member
/// answers it, once. Nobody relays for a member, and nobody else can tell it exists.
#[tokio::test]
async fn a_member_owned_request_is_answered_by_its_owner_only() {
    let case = fixture().await;
    let id = ask(&case, &case.worker, &lead(), "k-1").await;
    let opened = requests_for(&case.store, &case.lead).await;
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].0, case.report_room);
    assert_eq!(opened[0].1["event"], "opened");
    assert_eq!(opened[0].1["request"], id);
    assert_eq!(opened[0].1["question"], "Which branch?");
    assert_eq!(opened[0].1["callback"], "worker");

    let hidden = message(
        case.store
            .answer_request(&case.other, &id, "main", "o-1")
            .await
            .unwrap_err(),
    );
    let missing = message(
        case.store
            .answer_request(&case.other, "req-missing", "main", "o-2")
            .await
            .unwrap_err(),
    );
    assert_eq!(hidden, missing);
    assert!(message(
        case.store
            .answer_request(&case.worker, &id, "main", "w-1")
            .await
            .unwrap_err()
    )
    .contains("not authorized"));
    assert!(case
        .store
        .owner_answer_request("project", &id, "main", "u-1")
        .await
        .is_err());
    let answer = case
        .store
        .answer_request(&case.lead, &id, "feat/login", "l-1")
        .await
        .unwrap();
    assert_eq!(answer.channel, AnswerChannel::Member);
    assert_eq!(answer.actor.as_deref(), Some("lead"));
    let events = requests_for(&case.store, &case.worker).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["channel"], "member");

    let second = case
        .store
        .answer_request(&case.lead, &id, "main after all", "l-2")
        .await
        .unwrap_err();
    assert!(message(second).contains("already answered"));
}

/// A member-owned request is refused when the pair has no active room, rather than stored
/// where nobody will see it.
#[tokio::test]
async fn a_member_owned_request_needs_the_pairs_active_room() {
    let case = fixture().await;
    let refused = case
        .store
        .open_request(&case.other, &lead(), None, "Anyone?", "k-1")
        .await
        .unwrap_err();
    assert!(message(refused).contains("no active room"));
    assert!(case
        .store
        .open_requests("project")
        .await
        .unwrap()
        .is_empty());

    case.store
        .close_room(&case.worker, &case.report_room, "rotating")
        .await
        .unwrap();
    assert!(case
        .store
        .open_request(&case.worker, &lead(), None, "Still there?", "k-2")
        .await
        .is_err());
}

/// Keys belong to their actor. A retry returns the first result and adds nothing; the same
/// key for a different question is refused. Two members' keys never collide.
#[tokio::test]
async fn a_retried_key_returns_the_first_result() {
    let case = fixture().await;
    let first = case
        .store
        .open_request(&case.worker, &lead(), None, "Which branch?", "k-1")
        .await
        .unwrap();
    let again = case
        .store
        .open_request(&case.worker, &lead(), None, "Which branch?", "k-1")
        .await
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(requests_for(&case.store, &case.lead).await.len(), 1);
    assert!(message(
        case.store
            .open_request(&case.worker, &lead(), None, "Another?", "k-1")
            .await
            .unwrap_err()
    )
    .contains("idempotency key"));
    let other = case
        .store
        .open_request(
            &case.other,
            &RequestOwner::User,
            None,
            "Which branch?",
            "k-1",
        )
        .await
        .unwrap();
    assert_ne!(other.request, first.request);

    let answer = case
        .store
        .answer_request(&case.lead, &first.request, "main", "l-1")
        .await
        .unwrap();
    let retried = case
        .store
        .answer_request(&case.lead, &first.request, "main", "l-1")
        .await
        .unwrap();
    assert_eq!(retried, answer);
    assert_eq!(requests_for(&case.store, &case.worker).await.len(), 1);
    assert_eq!(
        case.store
            .request_answers("project", &first.request)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Only the asker withdraws, with a reason, and only while unanswered. The owner is told, and
/// nobody answers afterwards.
#[tokio::test]
async fn the_asker_withdraws_and_the_owner_is_told() {
    let case = fixture().await;
    let id = ask(&case, &case.worker, &lead(), "k-1").await;
    requests_for(&case.store, &case.lead).await;
    let hidden = message(
        case.store
            .withdraw_request(&case.lead, &id, "not mine")
            .await
            .unwrap_err(),
    );
    let missing = message(
        case.store
            .withdraw_request(&case.lead, "req-missing", "not mine")
            .await
            .unwrap_err(),
    );
    assert_eq!(hidden, missing);
    let withdrawn = case
        .store
        .withdraw_request(&case.worker, &id, "found it")
        .await
        .unwrap();
    assert_eq!(withdrawn.state, RequestState::Withdrawn);
    assert_eq!(withdrawn.withdraw_reason.as_deref(), Some("found it"));
    let events = requests_for(&case.store, &case.lead).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["event"], "withdrawn");
    assert_eq!(events[0].1["reason"], "found it");
    assert!(message(
        case.store
            .answer_request(&case.lead, &id, "main", "l-1")
            .await
            .unwrap_err()
    )
    .contains("request withdrawn: found it"));

    let answered = ask(&case, &case.worker, &RequestOwner::User, "k-2").await;
    case.store
        .owner_answer_request("project", &answered, "Yes.", "u-1")
        .await
        .unwrap();
    assert!(message(
        case.store
            .withdraw_request(&case.worker, &answered, "too late")
            .await
            .unwrap_err()
    )
    .contains("answered"));
}

/// Retirement withdraws every open request the member asked or owns, with the reason.
#[tokio::test]
async fn retirement_withdraws_what_the_member_asked_and_what_it_owns() {
    let case = fixture().await;
    let asked_user = ask(&case, &case.worker, &RequestOwner::User, "k-1").await;
    let asked_lead = ask(&case, &case.worker, &lead(), "k-2").await;
    let owned = ask(
        &case,
        &case.lead,
        &RequestOwner::Member("worker".into()),
        "k-3",
    )
    .await;
    let unrelated = ask(&case, &case.other, &RequestOwner::User, "k-4").await;
    case.store
        .retire_member("project", "worker", "done")
        .await
        .unwrap();
    for id in [&asked_user, &asked_lead, &owned] {
        let request = case.store.request("project", id).await.unwrap().unwrap();
        assert_eq!(request.state, RequestState::Withdrawn, "{id}");
        assert_eq!(request.withdraw_reason.as_deref(), Some("retired: done"));
    }
    assert_eq!(
        case.store
            .open_requests("project")
            .await
            .unwrap()
            .iter()
            .map(|r| r.request.clone())
            .collect::<Vec<_>>(),
        vec![unrelated]
    );
}

/// Answered, delivered and applied are separate facts. Only the asker marks an answer applied,
/// and marking it twice keeps the first time.
#[tokio::test]
async fn only_the_asker_marks_an_answer_applied() {
    let case = fixture().await;
    let id = ask(&case, &case.worker, &RequestOwner::User, "k-1").await;
    let answer = case
        .store
        .owner_answer_request("project", &id, "Yes.", "u-1")
        .await
        .unwrap();
    assert_eq!(answer.applied_wall_ms, None);
    assert!(case
        .store
        .mark_answer_applied(&case.lead, &id, answer.id)
        .await
        .is_err());
    let applied = case
        .store
        .mark_answer_applied(&case.worker, &id, answer.id)
        .await
        .unwrap();
    let first = applied.applied_wall_ms.expect("applied");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let again = case
        .store
        .mark_answer_applied(&case.worker, &id, answer.id)
        .await
        .unwrap();
    assert_eq!(again.applied_wall_ms, Some(first));
    assert!(case
        .store
        .mark_answer_applied(&case.worker, &id, answer.id + 100)
        .await
        .is_err());
}

/// A request can name a work item, which must exist.
#[tokio::test]
async fn a_request_names_an_existing_work_item_or_none() {
    let case = fixture().await;
    let refused = case
        .store
        .open_request(
            &case.worker,
            &RequestOwner::User,
            Some("missing"),
            "?",
            "k-1",
        )
        .await
        .unwrap_err();
    assert!(message(refused).contains("unknown work item"));
    case.store
        .register_work_item(&case.worker, "feat-login", "Login", "reg-1")
        .await
        .unwrap();
    let request = case
        .store
        .open_request(
            &case.worker,
            &RequestOwner::User,
            Some("feat-login"),
            "?",
            "k-2",
        )
        .await
        .unwrap();
    assert_eq!(request.item.as_deref(), Some("feat-login"));
}

/// A request event is a delivery-fact class of its own.
#[tokio::test]
async fn a_request_event_has_delivery_facts() {
    let case = fixture().await;
    ask(&case, &case.worker, &lead(), "k-1").await;
    let sequence = {
        let batch = case.store.check(&case.lead, 32).await.unwrap().unwrap();
        let item = batch
            .items
            .iter()
            .find(|item| item.kind == "request_event")
            .unwrap();
        item.sequence
    };
    let timeline = case
        .store
        .delivery_timeline("project", sequence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(timeline.class, Some(ItemClass::RequestEvent));
    assert_eq!(
        timeline
            .recipients
            .iter()
            .map(|r| r.member.as_str())
            .collect::<Vec<_>>(),
        vec!["lead"]
    );
}

/// 0011 rebuilds the delivery-facts table again. A work-event fact recorded at schema 10
/// survives unchanged.
#[tokio::test]
async fn the_upgrade_keeps_every_delivery_fact_recorded_before_it() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8993,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let pool = database_before(&sandbox, temp.path(), "0011").await;
    let instance = "0123456789abcdef0123456789abcdef";
    for statement in [
        "INSERT INTO candidate_contexts(context_id) VALUES ('project')".to_string(),
        "INSERT INTO candidate_members(context_id,member_id,role) VALUES ('project','a','feature_lead'),('project','b','repo_lead')".into(),
        "INSERT INTO candidate_rooms(context_id,room_id) VALUES ('project','r')".into(),
        "INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES ('project','r','a'),('project','r','b')".into(),
        "INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES ('project','r','a','message','{}')".into(),
        format!("INSERT INTO candidate_core_instances(instance_id,started_wall_ms) VALUES ('{instance}',1)"),
        format!("INSERT INTO candidate_item_facts(sequence,context_id,room_id,item_class,committed_instance,committed_mono_us,committed_wall_ms) VALUES (1,'project','r','work_event','{instance}',5,1000)"),
    ] {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    release(pool, &sandbox).await;
    let store = Store::open(&sandbox).await.unwrap();
    let timeline = store
        .delivery_timeline("project", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(timeline.class, Some(ItemClass::WorkEvent));
    assert_eq!(timeline.committed.unwrap().wall_ms, 1000);
}

/// Each rebuild renames a table and drops the old copy. Nothing in the final schema may still
/// refer to a dropped copy, or a later write through it fails.
#[tokio::test]
async fn no_schema_object_refers_to_a_dropped_table() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8994,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let pool = database_before(&sandbox, temp.path(), "~").await;
    let stale: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE sql LIKE '%\\_v%' ESCAPE '\\' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(stale.is_empty(), "{stale:?}");
    let violations: Vec<(String,)> =
        sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    release(pool, &sandbox).await;
}

/// A peer can be asked too, in the pair's peer room.
#[tokio::test]
async fn a_peer_is_asked_in_the_peer_room() {
    let case = fixture().await;
    case.store
        .authorize_connection("project", "worker", "other", ConnectionKind::Peer, "shared")
        .await
        .unwrap();
    let room = case
        .store
        .connect_members(
            &case.worker,
            "other",
            ConnectionKind::Peer,
            "shared",
            "p-1",
            "",
        )
        .await
        .unwrap()
        .room;
    ask(
        &case,
        &case.worker,
        &RequestOwner::Member("other".into()),
        "k-1",
    )
    .await;
    let events = requests_for(&case.store, &case.other).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, room);
}
