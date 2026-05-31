use axum::body::Body;
use axum::http::{Request, StatusCode};
use chatbotchat_core::http::{router, AppState};
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use time::OffsetDateTime;
use tower::ServiceExt; // for `oneshot`

async fn test_router() -> axum::Router {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    router(AppState::new(storage))
}

async fn test_router_with_cap(cap: std::time::Duration) -> axum::Router {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    router(AppState::with_wait_cap(storage, cap))
}

/// Like `test_router`, but also hands back a clone of the `Storage` so a test
/// can read the audit log or arrange a precondition state (`idle`/`stale`/
/// `archived`) that has no 6b HTTP path — those are sweeper-driven (6c), so the
/// test drives them straight through `storage.update_room_state(...)`.
async fn test_router_returning_storage() -> (axum::Router, Storage) {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    (router(AppState::new(storage.clone())), storage)
}

/// A cap-bound router that also returns the `Storage` handle. The short cap means
/// a regressed wait-state gate (one that parked instead of returning the state
/// immediately) fails fast — it returns `paused_by_timeout` after the cap rather
/// than hanging the suite.
async fn test_router_with_cap_returning_storage(
    cap: std::time::Duration,
) -> (axum::Router, Storage) {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    (
        router(AppState::with_wait_cap(storage.clone(), cap)),
        storage,
    )
}

async fn body_json(body: Body) -> Value {
    let bytes = body.collect().await.expect("collect body").to_bytes();
    serde_json::from_slice(&bytes).expect("valid json body")
}

#[tokio::test]
async fn open_room_then_status_round_trips() {
    let app = test_router().await;

    // POST /rooms
    let open_req = Request::builder()
        .method("POST")
        .uri("/rooms")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "subject": "slider labels" }).to_string(),
        ))
        .unwrap();

    let open_resp = app.clone().oneshot(open_req).await.unwrap();
    assert_eq!(open_resp.status(), StatusCode::CREATED);

    let open_body = body_json(open_resp.into_body()).await;
    let room_id = open_body["room_id"].as_str().expect("room_id present");
    assert!(
        room_id.starts_with("slider-labels-"),
        "room id should be kebab subject + timestamp, got {room_id}"
    );
    assert_eq!(
        open_body["share_line"]
            .as_str()
            .expect("share_line present"),
        format!("/cbc-join {room_id}")
    );

    // GET /rooms/:id
    let status_req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();

    let status_resp = app.oneshot(status_req).await.unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);

    let status_body = body_json(status_resp.into_body()).await;
    assert_eq!(status_body["id"].as_str().unwrap(), room_id);
    assert_eq!(status_body["subject"].as_str().unwrap(), "slider labels");
    assert_eq!(status_body["state"].as_str().unwrap(), "active");
}

#[tokio::test]
async fn status_for_missing_room_is_404() {
    let app = test_router().await;
    let req = Request::builder()
        .method("GET")
        .uri("/rooms/nope-20260528-1500")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn open_subject(app: &axum::Router, subject: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/rooms")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "subject": subject }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn open_room_id(app: &axum::Router, subject: &str) -> String {
    let (status, body) = open_subject(app, subject).await;
    assert_eq!(status, StatusCode::CREATED);
    body["room_id"].as_str().expect("room_id").to_string()
}

/// Open a room with optional open-time cap overrides, returning its id.
async fn open_with_caps(
    app: &axum::Router,
    subject: &str,
    hard_cap: Option<u32>,
    soft_cap: Option<u32>,
) -> String {
    let mut payload = json!({ "subject": subject });
    if let Some(h) = hard_cap {
        payload["hard_cap"] = json!(h);
    }
    if let Some(s) = soft_cap {
        payload["soft_cap"] = json!(s);
    }
    let req = Request::builder()
        .method("POST")
        .uri("/rooms")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    body["room_id"].as_str().expect("room_id").to_string()
}

async fn join(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/join"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "repo": repo, "model": model, "cwd": cwd }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn send(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    to: Option<&str>,
    body: &str,
) -> (StatusCode, Value) {
    let mut payload = json!({ "repo": repo, "model": model, "cwd": cwd, "body": body });
    if let Some(to) = to {
        payload["to"] = json!(to);
    }
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

/// Send a `--human` turn: the sender folded its user's input into this message.
async fn send_human(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    body: &str,
) -> (StatusCode, Value) {
    let payload =
        json!({ "repo": repo, "model": model, "cwd": cwd, "body": body, "from_human": true });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn wait(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/rooms/{room_id}/wait?repo={repo}&model={model}&cwd={cwd}"
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[allow(clippy::too_many_arguments)]
async fn signal(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    signal_type: &str,
    severity: Option<&str>,
    question_text: Option<&str>,
) -> (StatusCode, Value) {
    let mut payload = json!({ "repo": repo, "model": model, "cwd": cwd, "type": signal_type });
    if let Some(s) = severity {
        payload["severity"] = json!(s);
    }
    if let Some(q) = question_text {
        payload["question_text"] = json!(q);
    }
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/signals"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn signal_posts_a_waiting_user_sentinel_uncapped() {
    // A room capped at a single `msg`. A participant posts a `waiting_user`
    // sentinel, then a real `msg`. The sentinel must NOT consume the lone cap
    // slot — signals are uncapped — so the real msg is still admitted.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "signal uncapped", Some(1), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    let (status, body) = signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("should I merge to production?"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "signal should be accepted; got {body}"
    );
    assert!(
        body["seq"].as_i64().is_some(),
        "signal returns the assigned seq; got {body}"
    );

    // The single cap slot is still free: a real msg is admitted, proving the
    // sentinel did not count toward the cap.
    let (send_status, _) = send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        None,
        "real turn",
    )
    .await;
    assert_eq!(
        send_status,
        StatusCode::CREATED,
        "a sentinel must not consume cap budget"
    );
}

#[tokio::test]
async fn wait_delivers_a_waiting_user_sentinel_with_its_question() {
    // A signals that it is consulting its user; B's wait must surface the sentinel
    // once, carrying the type, severity, and the question — so B's UX can show
    // "the other agent is asking its user: …".
    let app = test_router().await;
    let room_id = open_room_id(&app, "sentinel delivery").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let (sig_status, _) = signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("should I merge to production?"),
    )
    .await;
    assert_eq!(sig_status, StatusCode::CREATED);

    let (status, body) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(status, StatusCode::OK);
    let m = &body["message"];
    assert_eq!(m["type"].as_str(), Some("waiting_user"), "got {body}");
    assert_eq!(m["severity"].as_str(), Some("high"), "got {body}");
    assert_eq!(
        m["question_text"].as_str(),
        Some("should I merge to production?"),
        "got {body}"
    );
}

#[tokio::test]
async fn wait_delivering_a_fresh_sentinel_carries_the_backoff_hint() {
    // Slice 5b deliver path: A pauses (high). B's very next wait returns the
    // sentinel itself, and that delivery rides the freshly-computed backoff —
    // `high` at n = 0 is the base 45 — so B knows how long to stay quiet.
    let app = test_router().await;
    let room_id = open_room_id(&app, "backoff deliver").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;

    let (status, body) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["message"]["type"].as_str(),
        Some("waiting_user"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "fresh high sentinel ⇒ base backoff 45, got {body}"
    );
}

#[tokio::test]
async fn an_active_sentinel_shortens_a_parked_wait_and_returns_the_hint() {
    // Slice 5b park path. B has already consumed the sentinel, so its next wait
    // has nothing unread and parks — but the counterpart is still paused, so the
    // long-poll is shortened to the backoff and the timeout still carries the
    // hint. We prove the shortening via the `retry_after` field, not wall-clock:
    // `effective_cap = min(wait_cap, backoff)`, so at an 80ms test cap both a
    // sentinel-active and a plain wait return at ~80ms — the field is the only
    // observable difference (see the control test below).
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "backoff park").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;

    // First wait consumes the sentinel (cursor advances past it).
    let (_, first) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(first["message"]["type"].as_str(), Some("waiting_user"));

    // Second wait: nothing new to read, but the pause is still active, so it
    // parks the shortened cap and times out carrying the backoff hint.
    let (status, body) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "an active sentinel keeps handing back the hint on timeout, got {body}"
    );
}

#[tokio::test]
async fn wait_without_an_active_sentinel_omits_retry_after() {
    // The control for the park test: with no counterpart pause, a parked wait
    // times out with no `retry_after` key at all (omitted, not null). This field
    // difference — present vs absent — is what distinguishes a backed-off wait
    // from an ordinary one at the same short test cap.
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "no backoff").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let (status, body) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"].as_str(), Some("paused_by_timeout"));
    assert!(
        body.get("retry_after").is_none(),
        "no active sentinel ⇒ retry_after omitted, got {body}"
    );
}

#[tokio::test]
async fn a_wait_parked_when_a_sentinel_arrives_gains_the_backoff_hint() {
    // Live hot path: B is already long-polling an empty room when A pauses to
    // consult its user. B wakes on the sentinel — and that delivery must carry
    // the backoff hint even though no sentinel existed when B began to park. The
    // hint reflects the state at wake, not at park-start.
    let app = test_router_with_cap(std::time::Duration::from_secs(2)).await;
    let room_id = open_room_id(&app, "parked arrival").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // B parks first — nothing to read yet.
    let waiter = {
        let app = app.clone();
        let room_id = room_id.clone();
        tokio::spawn(async move { wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await })
    };

    // Let B reach the park (past its pre-park state read), then A signals; the
    // hub wakes B and delivers the sentinel.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;

    let (status, body) = waiter.await.expect("waiter task");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["message"]["type"].as_str(),
        Some("waiting_user"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "a sentinel delivered on wake must carry the hint, got {body}"
    );
}

#[tokio::test]
async fn a_wait_parked_when_the_pause_clears_drops_the_stale_hint() {
    // B has already consumed A's sentinel and re-parks while the pause is still
    // active (so the long-poll is shortened). A then resumes with a normal turn,
    // clearing the pause mid-park. B wakes on that msg — and it must NOT carry a
    // stale backoff hint, since the counterpart is no longer paused.
    let app = test_router_with_cap(std::time::Duration::from_secs(2)).await;
    let room_id = open_room_id(&app, "parked clear").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A pauses; B consumes the sentinel (cursor advances past it), pause active.
    signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;
    let (_, first) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(first["message"]["type"].as_str(), Some("waiting_user"));

    // B re-parks: nothing new to read, pause still active so the cap shortens.
    let waiter = {
        let app = app.clone();
        let room_id = room_id.clone();
        tokio::spawn(async move { wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await })
    };

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // A resumes with a normal turn — the later msg self-supersedes the pause.
    send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        None,
        "back, resuming",
    )
    .await;

    let (status, body) = waiter.await.expect("waiter task");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["message"]["body"].as_str(),
        Some("back, resuming"),
        "got {body}"
    );
    assert!(
        body.get("retry_after").is_none(),
        "a pause cleared during the park must not leave a stale hint, got {body}"
    );
}

#[tokio::test]
async fn signal_validation_rejects_malformed_sentinels() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "signal validation").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    let sig = |ty: &'static str, sev: Option<&'static str>, q: Option<&'static str>| {
        let app = app.clone();
        let room_id = room_id.clone();
        async move { signal(&app, &room_id, "repo-a", "opus47", "/work/a", ty, sev, q).await }
    };

    // waiting_user requires both severity and question_text.
    let (s, _) = sig("waiting_user", None, Some("q?")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "waiting_user needs a severity");
    let (s, _) = sig("waiting_user", Some("high"), None).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "waiting_user needs a question_text"
    );
    let (s, _) = sig("waiting_user", Some("urgent"), Some("q?")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "severity must be low|med|high");

    // fold carries neither severity nor question_text.
    let (s, _) = sig("fold", Some("high"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "fold takes no severity");
    let (s, _) = sig("fold", None, Some("q?")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "fold takes no question_text");

    // blocker_real_work carries neither severity nor question_text (only an
    // optional `reason`); presence of either is rejected on its own merits — it
    // is a *valid* signal type, so these 400s are field-rule failures, not
    // "unsupported type".
    let (s, _) = sig("blocker_real_work", Some("high"), None).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "blocker_real_work takes no severity"
    );
    let (s, _) = sig("blocker_real_work", None, Some("q?")).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "blocker_real_work takes no question_text"
    );

    // The conversation `msg` and the `close` lifecycle op are not signals.
    // (blocker_real_work IS a valid signal type as of 6b, so it is not in this
    // list — it is exercised by its own field-rule and happy-path tests.)
    for ty in ["msg", "close", "bogus"] {
        let (s, _) = sig(ty, Some("high"), Some("q?")).await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{ty} is not a valid signal type"
        );
    }

    // fold rejects an *empty-string* question too — "carries neither" is about
    // presence, not non-emptiness; an empty string must not slip through and
    // persist a non-NULL question_text on a fold row.
    let (s, _) = sig("fold", None, Some("")).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "fold must reject question_text even when empty"
    );

    // The happy paths still work: waiting_user with both, fold with neither.
    let (s, _) = sig("waiting_user", Some("high"), Some("q?")).await;
    assert_eq!(
        s,
        StatusCode::CREATED,
        "a complete waiting_user is accepted"
    );
    let (s, _) = sig("fold", None, None).await;
    assert_eq!(s, StatusCode::CREATED, "a bare fold is accepted");
}

#[tokio::test]
async fn signal_to_missing_room_is_404() {
    let app = test_router().await;
    let (status, _) = signal(
        &app,
        "nope-20260528-1500",
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("high"),
        Some("q?"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signal_from_non_participant_is_rejected() {
    // The room exists but the caller never joined — signalling must be refused,
    // mirroring the send path. (Identity is the (repo, model, cwd) tuple.)
    let app = test_router().await;
    let room_id = open_room_id(&app, "signal non participant").await;
    let (status, _) = signal(
        &app,
        &room_id,
        "repo-ghost",
        "opus47",
        "/work/ghost",
        "waiting_user",
        Some("high"),
        Some("q?"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn send_then_wait_round_trips_a_message() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "send wait").await;

    // Two participants in the room.
    let (_, a) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    let a_handle = a["handle"].as_str().expect("a handle").to_string();
    join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;

    // A posts a broadcast message.
    let (s_send, send_body) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "hello room",
    )
    .await;
    assert_eq!(s_send, StatusCode::CREATED);
    assert!(
        send_body["seq"].as_i64().is_some(),
        "send returns the assigned seq, got {send_body}"
    );

    // B waits and receives it immediately (already unread).
    let (s_wait, wait_body) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(s_wait, StatusCode::OK);
    assert_eq!(wait_body["message"]["body"].as_str(), Some("hello room"));
    assert_eq!(
        wait_body["message"]["from"].as_str(),
        Some(a_handle.as_str())
    );

    // A tuple that never joined cannot send or wait.
    let (s_send_np, _) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/never-joined",
        None,
        "nope",
    )
    .await;
    assert_eq!(s_send_np, StatusCode::BAD_REQUEST);
    let (s_wait_np, _) = wait(&app, &room_id, "mvp-engine", "opus47", "/never-joined").await;
    assert_eq!(s_wait_np, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn send_to_unknown_recipient_is_rejected_and_targeted_delivery_works() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "targeted").await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    let (_, b) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    let b_handle = b["handle"].as_str().expect("b handle").to_string();

    // A `to` that is not a participant of the room is rejected (no silent orphan).
    let (s_bad, _) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        Some("no-such-handle"),
        "hi?",
    )
    .await;
    assert_eq!(s_bad, StatusCode::BAD_REQUEST);

    // A valid targeted message is delivered to that participant.
    let (s_ok, _) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        Some(&b_handle),
        "hi B",
    )
    .await;
    assert_eq!(s_ok, StatusCode::CREATED);

    let (sw, w) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(sw, StatusCode::OK);
    assert_eq!(w["message"]["body"].as_str(), Some("hi B"));
    assert_eq!(w["message"]["to"].as_str(), Some(b_handle.as_str()));
}

#[tokio::test]
async fn send_missing_room_is_404() {
    let app = test_router().await;
    let (s, _) = send(&app, "nope-20260529-0000", "r", "m", "/c", None, "x").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wait_missing_room_is_404() {
    let app = test_router().await;
    let (s, _) = wait(&app, "nope-20260529-0000", "r", "m", "/c").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn late_joiner_does_not_replay_backlog_via_wait() {
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "late join").await;

    // A joins and posts BEFORE B exists.
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "before B",
    )
    .await;

    // B joins later — it sees the backlog via recent_messages (the log view) ...
    let (_, b) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    let recent = b["recent_messages"]
        .as_array()
        .expect("recent_messages array");
    assert!(
        recent
            .iter()
            .any(|m| m["body"].as_str() == Some("before B")),
        "B's join should surface the backlog; got {b}"
    );

    // ... but B's wait must NOT replay that pre-join message — it should time out.
    let (s1, w1) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(
        w1["status"].as_str(),
        Some("paused_by_timeout"),
        "pre-join backlog must not replay through wait; got {w1}"
    );

    // A message sent AFTER B joined is delivered to B's wait.
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "after B",
    )
    .await;
    let (s2, w2) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(
        w2["message"]["body"].as_str(),
        Some("after B"),
        "post-join message should reach B; got {w2}"
    );
}

#[tokio::test]
async fn join_returns_recent_messages() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "history").await;

    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "first",
    )
    .await;
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "second",
    )
    .await;

    // A newcomer's join carries the room's recent messages — the log view, which
    // includes every sender (unlike `wait`, which excludes the caller's own).
    let (_, b) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    let recent = b["recent_messages"]
        .as_array()
        .expect("recent_messages array");
    assert_eq!(
        recent.len(),
        2,
        "join should surface prior messages; got {b}"
    );
    assert_eq!(recent[0]["body"].as_str(), Some("first"));
    assert_eq!(recent[1]["body"].as_str(), Some("second"));
}

#[tokio::test]
async fn wait_times_out_with_paused_by_timeout() {
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "timeout").await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;

    // Nothing has been sent: wait parks until the (short) cap, then reports it.
    let (status, body) = wait(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"].as_str(), Some("paused_by_timeout"));
    assert!(
        body.get("message").is_none(),
        "a timeout response carries no message, got {body}"
    );
}

#[tokio::test]
async fn join_is_idempotent_per_tuple_and_distinct_otherwise() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "join flow").await;

    // First join mints a fresh handle of the form <repo>-<model>-<sess4hex>.
    let (s1, b1) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    assert_eq!(s1, StatusCode::CREATED);
    let h1 = b1["handle"].as_str().expect("handle").to_string();
    assert_eq!(b1["resumed"].as_bool(), Some(false));
    assert_eq!(b1["room_state"].as_str(), Some("active"));
    assert!(
        b1["recent_messages"].as_array().expect("array").is_empty(),
        "no messages exist in slice 2"
    );
    assert!(
        h1.starts_with("mvp-engine-opus47-"),
        "handle should be <repo>-<model>-<sess>, got {h1}"
    );

    // Same tuple → same handle, resumed=true.
    let (s2, b2) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    assert_eq!(s2, StatusCode::CREATED);
    assert_eq!(b2["handle"].as_str(), Some(h1.as_str()));
    assert_eq!(b2["resumed"].as_bool(), Some(true));

    // Different cwd → different handle.
    let (_, b3) = join(&app, &room_id, "mvp-engine", "opus47", "/work/b").await;
    assert_ne!(b3["handle"].as_str(), Some(h1.as_str()));
    assert_eq!(b3["resumed"].as_bool(), Some(false));

    // Different model → different handle.
    let (_, b4) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/a").await;
    assert_ne!(b4["handle"].as_str(), Some(h1.as_str()));
    assert_eq!(b4["resumed"].as_bool(), Some(false));
}

#[tokio::test]
async fn status_lists_participants_after_join() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "roster").await;

    let (_, b1) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    let (_, b2) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/a").await;
    let h1 = b1["handle"].as_str().unwrap();
    let h2 = b2["handle"].as_str().unwrap();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;

    let roster = body["participants"].as_array().expect("participants array");
    assert_eq!(roster.len(), 2, "both joins should be listed");
    let handles: Vec<&str> = roster
        .iter()
        .map(|p| p["handle"].as_str().unwrap())
        .collect();
    assert!(handles.contains(&h1) && handles.contains(&h2));
    // Participant view carries the self-reported tuple fields.
    assert_eq!(roster[0]["repo"].as_str(), Some("mvp-engine"));
    assert!(roster[0]["model"].as_str().is_some());
    assert!(roster[0]["cwd"].as_str().is_some());
    assert!(roster[0]["joined_at"].as_str().is_some());
}

#[tokio::test]
async fn join_missing_room_is_404() {
    let app = test_router().await;
    let (status, _) = join(&app, "nope-20260528-1500", "r", "m", "/c").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn repeated_open_same_subject_gets_distinct_ids() {
    let app = test_router().await;

    // Two opens for the same subject collide on the minute-granular base id.
    // The second must NOT 500 — it must disambiguate to a fresh, retrievable id.
    let (s1, b1) = open_subject(&app, "same subject").await;
    let (s2, b2) = open_subject(&app, "same subject").await;

    assert_eq!(s1, StatusCode::CREATED);
    assert_eq!(
        s2,
        StatusCode::CREATED,
        "second open of the same subject must not error on id collision"
    );

    let id1 = b1["room_id"].as_str().unwrap();
    let id2 = b2["room_id"].as_str().unwrap();
    assert_ne!(id1, id2, "colliding opens must get distinct room ids");

    // Both rooms exist independently.
    for id in [id1, id2] {
        let req = Request::builder()
            .method("GET")
            .uri(format!("/rooms/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "room {id} should be retrievable"
        );
    }
}

#[tokio::test]
async fn wait_surfaces_to_user_at_the_soft_cap_threshold() {
    // soft_cap = 3 → the conversation surfaces to the user on the (3 - 1) = 2nd
    // consecutive autonomous msg, the loop-insurance that pulls a human in before
    // agents talk in circles. A sends; B waits and reads the surface signal.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "soft cap", None, Some(3)).await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;

    // 1st autonomous turn: below the threshold, no surface.
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "m1",
    )
    .await;
    let (s1, b1) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(b1["message"]["body"], "m1");
    assert_eq!(
        b1["surface_to_user"],
        json!(false),
        "the 1st of 3 is below the soft cap; got {b1}"
    );

    // 2nd consecutive autonomous turn hits soft_cap - 1 → surface.
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "m2",
    )
    .await;
    let (s2, b2) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b2["message"]["body"], "m2");
    assert_eq!(
        b2["surface_to_user"],
        json!(true),
        "the 2nd consecutive msg hits soft_cap - 1 and must surface; got {b2}"
    );
}

#[tokio::test]
async fn human_send_resets_the_soft_cap_counter() {
    // soft_cap = 2 → surface on each 1st consecutive autonomous turn. A `--human`
    // send is the reset boundary, so the next autonomous turn restarts the run
    // and surfaces again — without the reset the run would climb past the strict
    // threshold and go quiet. That re-surface is the discriminating signal.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "human reset", None, Some(2)).await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;

    // 1st autonomous turn → run length 1 == soft_cap - 1 → surface.
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "m1",
    )
    .await;
    let (_, b1) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(b1["surface_to_user"], json!(true), "got {b1}");

    // A folds the user in with a --human send → resets the run. The human turn is
    // the reset boundary (count 0 at its own delivery), not itself a surface.
    let (sh, _) = send_human(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        "user weighs in",
    )
    .await;
    assert_eq!(sh, StatusCode::CREATED);
    let (_, bh) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(bh["message"]["body"], "user weighs in");
    assert_eq!(
        bh["surface_to_user"],
        json!(false),
        "the human turn is the reset boundary, not a surface; got {bh}"
    );

    // The next autonomous turn restarts the run at 1 → surfaces again.
    send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "m2",
    )
    .await;
    let (_, b2) = wait(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    assert_eq!(
        b2["surface_to_user"],
        json!(true),
        "after the human reset the run restarts and surfaces again; got {b2}"
    );
}

#[tokio::test]
async fn hard_cap_refuses_sends_once_the_room_is_full() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "caps").await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;

    // The default hard cap is 10 (RoomConfig::default). A room-wide count, so a
    // single sender filling the budget exercises the gate.
    const HARD_CAP: usize = 10;
    for i in 0..HARD_CAP {
        let (s, _) = send(
            &app,
            &room_id,
            "mvp-engine",
            "opus47",
            "/work/a",
            None,
            &format!("msg {i}"),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "send {i} should be accepted");
    }

    // The cap+1th send is refused with 409 Conflict — retrying won't clear it,
    // the user must raise the cap or close the room.
    let (s_over, body_over) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "over the cap",
    )
    .await;
    assert_eq!(s_over, StatusCode::CONFLICT);

    // The rejection is recognizable and actionable: a human-readable message
    // that names the cap.
    let err = body_over["error"]
        .as_str()
        .expect("409 carries an error message");
    assert!(
        err.contains("hard cap"),
        "rejection should name the hard cap; got {body_over}"
    );

    // The refused message must NOT be persisted — a fresh joiner sees exactly the
    // capped 10 in the room log, not 11.
    let (_, joiner) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    let recent = joiner["recent_messages"]
        .as_array()
        .expect("recent_messages array");
    assert_eq!(
        recent.len(),
        HARD_CAP,
        "the rejected send must not have been written; got {} messages",
        recent.len()
    );
}

#[tokio::test]
async fn open_rejects_pathological_cap_configs() {
    // The open-time cap opts are a new input surface: a hard_cap of 0 would accept
    // no messages at all, and a soft_cap below 2 has no valid surface threshold
    // (surface fires on the soft_cap-1 th consecutive autonomous turn). Reject both
    // with 400 rather than silently minting a degenerate room. (soft_cap > hard_cap
    // is intentionally NOT rejected — a low hard_cap with the default soft_cap is a
    // legitimate "soft cap effectively off" config.)
    let app = test_router().await;

    let bad: [(Option<u32>, Option<u32>, &str); 3] = [
        (Some(0), None, "hard_cap 0 accepts no sends"),
        (None, Some(0), "soft_cap 0 never surfaces"),
        (None, Some(1), "soft_cap 1 has no valid threshold"),
    ];
    for (hard, soft, why) in bad {
        let mut payload = json!({ "subject": "bad caps" });
        if let Some(h) = hard {
            payload["hard_cap"] = json!(h);
        }
        if let Some(s) = soft {
            payload["soft_cap"] = json!(s);
        }
        let req = Request::builder()
            .method("POST")
            .uri("/rooms")
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{why}");
    }

    // A valid low-cap edge (hard_cap 1, soft_cap 2) is still accepted.
    let ok = open_with_caps(&app, "ok caps", Some(1), Some(2)).await;
    assert!(!ok.is_empty());
}

#[tokio::test]
async fn open_time_hard_cap_is_honored_end_to_end() {
    // Open with hard_cap = 2 via the open API (no storage seeding); the 3rd send
    // is refused with 409 — proving open-time cap opts reach the enforcement gate.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "open hard cap", Some(2), None).await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;

    for i in 0..2 {
        let (s, _) = send(
            &app,
            &room_id,
            "mvp-engine",
            "opus47",
            "/work/a",
            None,
            &format!("m{i}"),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::CREATED,
            "send {i} under the open-time cap of 2"
        );
    }

    let (s_over, _) = send(
        &app,
        &room_id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "over",
    )
    .await;
    assert_eq!(
        s_over,
        StatusCode::CONFLICT,
        "the 3rd send exceeds the open-time cap of 2"
    );
}

#[tokio::test]
async fn hard_cap_honors_a_non_default_persisted_room_config() {
    // Seed a room whose persisted config carries a non-default cap of 2, then
    // drive sends through the real HTTP path. Proves the gate reads the stored
    // `RoomConfig.hard_cap`, not a hard-coded constant — the persisted-config
    // path, complementary to `open_time_hard_cap_is_honored_end_to_end` which
    // covers authoring the cap at open time.
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    let now = OffsetDateTime::now_utc();
    let room = Room {
        id: "caps-custom-20260529-0000".into(),
        subject: "custom cap".into(),
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        state_changed_at: now,
        config: RoomConfig {
            hard_cap: 2,
            soft_cap: 4,
        },
        prev_room_id: None,
    };
    storage.create_room(&room).await.expect("seed room");
    let app = router(AppState::new(storage));

    join(&app, &room.id, "mvp-engine", "opus47", "/work/a").await;

    for i in 0..2 {
        let (s, _) = send(
            &app,
            &room.id,
            "mvp-engine",
            "opus47",
            "/work/a",
            None,
            &format!("msg {i}"),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "send {i} under the cap of 2");
    }

    let (s_over, _) = send(
        &app,
        &room.id,
        "mvp-engine",
        "opus47",
        "/work/a",
        None,
        "third",
    )
    .await;
    assert_eq!(
        s_over,
        StatusCode::CONFLICT,
        "the 3rd send exceeds the persisted cap of 2"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_cap_holds_under_concurrent_sends() {
    // Fire many sends at a default-capped (10) room without waiting between them.
    // Because the gate is enforced inside a single atomic INSERT statement, the
    // count-then-write race cannot let an 11th slip through: exactly 10 succeed,
    // the rest are refused — no matter how the requests interleave.
    let app = test_router().await;
    let room_id = open_room_id(&app, "concurrent caps").await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;

    const ATTEMPTS: usize = 30;
    const HARD_CAP: usize = 10;

    let mut set = tokio::task::JoinSet::new();
    for i in 0..ATTEMPTS {
        let app = app.clone();
        let room_id = room_id.clone();
        set.spawn(async move {
            let payload = json!({
                "repo": "mvp-engine", "model": "opus47", "cwd": "/work/a",
                "body": format!("concurrent {i}")
            });
            let req = Request::builder()
                .method("POST")
                .uri(format!("/rooms/{room_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        });
    }

    let mut created = 0usize;
    let mut conflict = 0usize;
    while let Some(res) = set.join_next().await {
        match res.expect("task ok") {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => conflict += 1,
            other => panic!("unexpected status under concurrency: {other}"),
        }
    }

    assert_eq!(created, HARD_CAP, "exactly the cap may be admitted");
    assert_eq!(
        conflict,
        ATTEMPTS - HARD_CAP,
        "every send past the cap must be refused"
    );

    // And the room genuinely holds exactly the cap — no over-commit.
    let (_, joiner) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/b").await;
    let recent = joiner["recent_messages"]
        .as_array()
        .expect("recent_messages array");
    assert_eq!(recent.len(), HARD_CAP, "the room must not exceed the cap");
}

/// POST /rooms/{id}/{op} where `op` is `close` | `pause` | `wake`. Identity is
/// the `(repo, model, cwd)` tuple in the body; `reason` is the optional pause
/// note. Mirrors the `send`/`signal` helpers.
#[allow(clippy::too_many_arguments)]
async fn lifecycle_op(
    app: &axum::Router,
    room_id: &str,
    op: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    reason: Option<&str>,
) -> (StatusCode, Value) {
    let mut payload = json!({ "repo": repo, "model": model, "cwd": cwd });
    if let Some(r) = reason {
        payload["reason"] = json!(r);
    }
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/{op}"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn close_transitions_room_to_closed() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "wrap it up").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    let (status, body) =
        lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::OK, "close should succeed: {body}");
    assert_eq!(
        body["state"].as_str(),
        Some("closed"),
        "close must report the new state"
    );

    // And it sticks: a status read reflects `closed`.
    let status_req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(status_req).await.unwrap();
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["state"].as_str(), Some("closed"));
}

#[tokio::test]
async fn send_is_rejected_on_a_closed_room() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "wrap it up").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    // A send while active is fine.
    let (status, _) = send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        None,
        "still here",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Close it, then a further send is refused — the conversation is over.
    let (status, _) =
        lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &app, &room_id, "repo-a", "opus47", "/work/a", None, "too late",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a send into a closed room must be refused, got: {body}"
    );
}

#[tokio::test]
async fn pause_then_wake_round_trips_through_paused() {
    // A storage-returning router: pause/wake go through HTTP, while the
    // events.detail assertion reads the audit log directly off the handle.
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "blocked on real work").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    let (status, body) = lifecycle_op(
        &app,
        &room_id,
        "pause",
        "repo-a",
        "opus47",
        "/work/a",
        Some("running the migration by hand"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pause should succeed: {body}");
    assert_eq!(body["state"].as_str(), Some("paused"));

    // The pause reason is recorded in the room's audit log.
    let events = storage.list_events(&room_id).await.expect("list events");
    assert!(
        events.iter().any(|e| {
            e.to_state == Some(RoomState::Paused)
                && e.detail.as_deref() == Some("running the migration by hand")
        }),
        "pause reason must land in events.detail, got {events:?}"
    );

    let (status, body) =
        lifecycle_op(&app, &room_id, "wake", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::OK, "wake should succeed: {body}");
    assert_eq!(body["state"].as_str(), Some("active"));
}

#[tokio::test]
async fn illegal_lifecycle_transitions_are_409() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "edge cases").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    // wake on an active room: there is nothing to wake.
    let (status, body) =
        lifecycle_op(&app, &room_id, "wake", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "wake-on-active: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot"),
        "409 body should explain the illegal transition, got {body}"
    );

    // pause, then pause again: the second is illegal from `paused`.
    lifecycle_op(&app, &room_id, "pause", "repo-a", "opus47", "/work/a", None).await;
    let (status, _) =
        lifecycle_op(&app, &room_id, "pause", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "pause-on-paused");

    // wake back to active, close, then close again: the second is illegal.
    lifecycle_op(&app, &room_id, "wake", "repo-a", "opus47", "/work/a", None).await;
    lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/work/a", None).await;
    let (status, _) =
        lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/work/a", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "close-on-closed");
}

#[tokio::test]
async fn lifecycle_op_by_a_non_participant_is_400() {
    let app = test_router().await;
    let room_id = open_room_id(&app, "membership").await;
    // No join: the caller is not a participant of this room.
    let (status, _) = lifecycle_op(
        &app,
        &room_id,
        "close",
        "stranger",
        "opus47",
        "/elsewhere",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// GET the room's current lifecycle state string.
async fn room_state(app: &axum::Router, room_id: &str) -> String {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let body = body_json(resp.into_body()).await;
    body["state"].as_str().expect("state").to_string()
}

/// POST a `blocker_real_work` signal with an optional `reason`.
async fn signal_blocker(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    reason: Option<&str>,
) -> (StatusCode, Value) {
    let mut payload =
        json!({ "repo": repo, "model": model, "cwd": cwd, "type": "blocker_real_work" });
    if let Some(r) = reason {
        payload["reason"] = json!(r);
    }
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/signals"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn blocker_real_work_signal_pauses_the_room() {
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "blocked on real work").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    let (status, body) = signal_blocker(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        Some("rebasing the branch by hand"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "blocker_real_work must be accepted: {body}"
    );

    // The signal pauses the room.
    assert_eq!(room_state(&app, &room_id).await, "paused");

    // The reason is recorded in the transition's audit row.
    let events = storage.list_events(&room_id).await.expect("events");
    assert!(
        events.iter().any(|e| {
            e.to_state == Some(RoomState::Paused)
                && e.detail.as_deref() == Some("rebasing the branch by hand")
        }),
        "blocker reason must land in events.detail, got {events:?}"
    );
}

#[tokio::test]
async fn a_msg_revives_an_idle_or_stale_room_to_active() {
    let (app, storage) = test_router_returning_storage().await;
    let now = OffsetDateTime::now_utc();

    for arranged in [RoomState::Idle, RoomState::Stale] {
        let room_id = open_room_id(&app, &format!("quiet {arranged:?}")).await;
        join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

        // No 6b HTTP path reaches idle/stale (sweeper-driven, 6c) — arrange it
        // directly via the storage handle.
        let changed = storage
            .update_room_state(&room_id, RoomState::Active, arranged, now, None)
            .await
            .expect("arrange state");
        assert!(changed, "precondition CAS should apply");
        assert_eq!(room_state(&app, &room_id).await, arranged.as_str());

        // A conversation `msg` is accepted and revives the room to active.
        let (status, _) = send(
            &app,
            &room_id,
            "repo-a",
            "opus47",
            "/work/a",
            None,
            "back online",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "msg accepted on {arranged:?}");
        assert_eq!(
            room_state(&app, &room_id).await,
            "active",
            "a msg on {arranged:?} must revive the room to active"
        );
    }
}

#[tokio::test]
async fn writes_are_rejected_on_paused_and_archived_rooms() {
    let (app, storage) = test_router_returning_storage().await;
    let now = OffsetDateTime::now_utc();

    // `paused` is reachable via the pause endpoint.
    let paused = open_room_id(&app, "paused room").await;
    join(&app, &paused, "repo-a", "opus47", "/work/a").await;
    lifecycle_op(&app, &paused, "pause", "repo-a", "opus47", "/work/a", None).await;
    let (s, _) = send(&app, &paused, "repo-a", "opus47", "/work/a", None, "hi").await;
    assert_eq!(s, StatusCode::CONFLICT, "send on paused must be refused");
    let (s, _) = signal(
        &app, &paused, "repo-a", "opus47", "/work/a", "fold", None, None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "signal on paused must be refused");

    // `archived` has no 6b HTTP path — arrange it directly.
    let arch = open_room_id(&app, "archived room").await;
    join(&app, &arch, "repo-a", "opus47", "/work/a").await;
    storage
        .update_room_state(&arch, RoomState::Active, RoomState::Archived, now, None)
        .await
        .expect("arrange archived");
    let (s, _) = send(&app, &arch, "repo-a", "opus47", "/work/a", None, "hi").await;
    assert_eq!(s, StatusCode::CONFLICT, "send on archived must be refused");
    let (s, _) = signal(
        &app, &arch, "repo-a", "opus47", "/work/a", "fold", None, None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "signal on archived must be refused"
    );
}

#[tokio::test]
async fn a_wait_on_a_non_active_room_returns_its_state_immediately() {
    // Short cap: if the entry gate regressed and the wait parked instead, it would
    // return `paused_by_timeout` after the cap (fast failure), not hang.
    let cap = std::time::Duration::from_millis(100);
    let (app, storage) = test_router_with_cap_returning_storage(cap).await;
    let now = OffsetDateTime::now_utc();

    let paused = open_room_id(&app, "paused").await;
    join(&app, &paused, "repo-a", "opus47", "/work/a").await;
    lifecycle_op(&app, &paused, "pause", "repo-a", "opus47", "/work/a", None).await;
    let (st, body) = wait(&app, &paused, "repo-a", "opus47", "/work/a").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused"),
        "a wait on a paused room reports paused at once, got {body}"
    );
    assert!(
        body.get("retry_after").is_none(),
        "a state-gated wait carries no retry_after, got {body}"
    );

    let closed = open_room_id(&app, "closed").await;
    join(&app, &closed, "repo-a", "opus47", "/work/a").await;
    lifecycle_op(&app, &closed, "close", "repo-a", "opus47", "/work/a", None).await;
    let (_, body) = wait(&app, &closed, "repo-a", "opus47", "/work/a").await;
    assert_eq!(body["status"].as_str(), Some("closed"), "got {body}");

    let arch = open_room_id(&app, "archived").await;
    join(&app, &arch, "repo-a", "opus47", "/work/a").await;
    storage
        .update_room_state(&arch, RoomState::Active, RoomState::Archived, now, None)
        .await
        .expect("arrange archived");
    let (_, body) = wait(&app, &arch, "repo-a", "opus47", "/work/a").await;
    assert_eq!(body["status"].as_str(), Some("archived"), "got {body}");
}

#[tokio::test]
async fn a_parked_waiter_receives_the_blocker_reason_before_the_pause_gates_it() {
    // The blocker reason is delivered to a counterpart that is *already parked*:
    // it wakes on the broadcast and claims the sentinel before any re-entry hits
    // the paused entry-gate. (A fresh poll after the pause gets `status:"paused"`
    // and reads the reason from the message log / events.detail instead.) This
    // pins the delivery channel that 6c's wait-path reordering must preserve.
    let app = test_router().await;
    let room_id = open_room_id(&app, "blocker delivery").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await; // signaller
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await; // parked waiter

    // B parks on the still-active room (passes the entry gate, then blocks).
    let waiter = {
        let app = app.clone();
        let room_id = room_id.clone();
        tokio::spawn(async move { wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // A signals blocker_real_work: persists the broadcast sentinel, pauses the
    // room, and rings it.
    let (status, _) = signal_blocker(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        Some("merging the branch by hand"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // B wakes on the broadcast and receives the blocker sentinel with its reason —
    // not a `paused`/`paused_by_timeout` status.
    let (st, body) = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("waiter resolved before the deadline")
        .expect("waiter task did not panic");
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        body["message"]["type"].as_str(),
        Some("blocker_real_work"),
        "the parked waiter must receive the blocker sentinel, got {body}"
    );
    assert_eq!(
        body["message"]["body"].as_str(),
        Some("merging the branch by hand"),
        "the blocker reason must reach the counterpart in the message body, got {body}"
    );
}

#[tokio::test]
async fn blocker_real_work_on_a_stale_room_is_409_and_persists_nothing() {
    // `Pause` is illegal from `stale` (locked table), and the legality is checked
    // before any write — so the signal 409s and leaves neither a pause nor an
    // orphaned blocker message behind. This pins the data-integrity invariant the
    // validate-before-write ordering exists to protect.
    let (app, storage) = test_router_returning_storage().await;
    let now = OffsetDateTime::now_utc();
    let room_id = open_room_id(&app, "stale blocker").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;

    // Arrange `stale` directly (no 6b HTTP path reaches it — sweeper-driven, 6c).
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Stale, now, None)
        .await
        .expect("arrange stale");

    let (status, body) = signal_blocker(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        Some("too late to block"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "blocker_real_work from stale is illegal, got {body}"
    );

    // No pause applied: the room is still stale.
    assert_eq!(room_state(&app, &room_id).await, "stale");
    // No orphaned sentinel: the room holds no messages (seq high-water still 0).
    assert_eq!(
        storage.current_seq(&room_id).await.expect("current_seq"),
        0,
        "an illegal blocker must not persist a message"
    );
    // And no pause transition was logged.
    let events = storage.list_events(&room_id).await.expect("events");
    assert!(
        events.iter().all(|e| e.to_state != Some(RoomState::Paused)),
        "no pause should be recorded for an illegal blocker, got {events:?}"
    );
}

#[tokio::test]
async fn opening_with_prev_room_id_records_the_link() {
    let (app, storage) = test_router_returning_storage().await;

    // A continuation room carries the id of the room it succeeds (AC #7). The
    // link is persisted but not surfaced on RoomStatus, so assert it through the
    // storage handle.
    let req = Request::builder()
        .method("POST")
        .uri("/rooms")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "subject": "follow-up room",
                "prev_room_id": "prior-room-20260501-1000"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let room_id = body_json(resp.into_body()).await["room_id"]
        .as_str()
        .expect("room_id")
        .to_string();

    let room = storage
        .get_room(&room_id)
        .await
        .expect("get ok")
        .expect("room exists");
    assert_eq!(
        room.prev_room_id.as_deref(),
        Some("prior-room-20260501-1000"),
        "the prev_room_id from the open request must be persisted"
    );
}

#[tokio::test]
async fn opening_without_prev_room_id_leaves_the_link_empty() {
    // The field is optional (`#[serde(default)]`), so a 6b-era client that omits
    // it stays wire-compatible: the room opens with no predecessor link.
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "standalone room").await;

    let room = storage
        .get_room(&room_id)
        .await
        .expect("get ok")
        .expect("room exists");
    assert!(
        room.prev_room_id.is_none(),
        "omitting prev_room_id must leave it null, got {:?}",
        room.prev_room_id
    );
}

#[tokio::test]
async fn a_wait_returns_counterpart_stale_when_the_other_agent_has_gone_dark() {
    // Short cap so a regressed ghost path (one that parks instead of returning
    // immediately) fails fast as `paused_by_timeout` rather than hanging the
    // suite — and the status assertion still catches it.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "ghost room").await;

    // A (the caller) and B (the counterpart) both join.
    let (_, _a) = join(&app, &room_id, "repo-a", "opus47", "/a").await;
    let (_, b) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let b_handle = b["handle"].as_str().expect("b handle").to_string();

    // B last polled 20 min ago — past GHOST_AFTER (15 min): it has gone dark with
    // no away-signal.
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(&b_handle, stale)
        .await
        .expect("backdate B's last poll");

    // A waits: nothing to read and the counterpart is dark → stop waiting on a
    // ghost, immediately (AC #5).
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("counterpart_stale"),
        "a dark counterpart must yield counterpart_stale, got {body}"
    );
}

#[tokio::test]
async fn an_active_waiting_user_suppresses_counterpart_stale() {
    // Pins the locked decision AND discriminates the branch ordering. B posts a
    // waiting_user, A consumes it, THEN B goes dark. On A's *next* wait there is
    // no unread message left to claim, so the response reveals which branch ran:
    //   - backoff-first (correct): the waiting_user is still the latest message
    //     from B, so `active_sentinel_backoff` stays `Some` → A parks and times
    //     out as `paused_by_timeout` WITH a `retry_after` hint. The away-signal
    //     suppressed ghost detection.
    //   - ghost-first (buggy): A would short-circuit to `counterpart_stale`.
    // A single-wait test can't tell these apart — the unread sentinel is claimed
    // under either ordering — so the consume-then-rewait is the real proof.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(150)).await;
    let room_id = open_room_id(&app, "suppress room").await;

    let (_, _a) = join(&app, &room_id, "repo-a", "opus47", "/a").await;
    let (_, b) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let b_handle = b["handle"].as_str().expect("b handle").to_string();

    // B signals it is consulting its user.
    let (s, _) = signal(
        &app,
        &room_id,
        "repo-b",
        "opus47",
        "/b",
        "waiting_user",
        Some("high"),
        Some("which option do you want?"),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // A consumes the sentinel, advancing its read cursor past it.
    let (_, first) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        first["message"]["type"].as_str(),
        Some("waiting_user"),
        "A should first receive the sentinel, got {first}"
    );

    // Now B goes dark past GHOST_AFTER, with nothing left for A to read.
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(&b_handle, stale)
        .await
        .expect("backdate B's last poll");

    // A waits again. The active away-signal must suppress ghost detection: A
    // parks and reports `paused_by_timeout` WITH a backoff hint — NOT
    // `counterpart_stale` (which would carry no hint).
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "an active away-signal must suppress ghost detection, got {body}"
    );
    assert!(
        body["retry_after"].as_u64().is_some(),
        "the suppressed slice-5b path carries a backoff hint, got {body}"
    );
}

#[tokio::test]
async fn an_archived_room_still_serves_reads() {
    // AC #6: archived is read-*only*, not read-*blocked*. Writes are refused
    // (covered by `writes_are_rejected_on_paused_and_archived_rooms`) and a wait
    // returns `archived` at once (covered by
    // `a_wait_on_a_non_active_room_returns_its_state_immediately`), but a plain
    // status GET must still succeed so the conversation stays inspectable after
    // the room has ended. `get_room` carries no state gate, so this pins that
    // reads remain ungated.
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "archived readable room").await;
    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Archived, now, None)
        .await
        .expect("arrange archived");

    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "reads on an archived room must still succeed"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["state"].as_str(), Some("archived"));
    assert_eq!(body["subject"].as_str(), Some("archived readable room"));
}
