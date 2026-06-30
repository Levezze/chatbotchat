use axum::body::Body;
use axum::http::{Request, StatusCode};
use chatbotchat_core::http::{router, AppState};
use chatbotchat_core::participant::Participant;
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
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

/// A router with both an explicit long-poll cap and a short presence grace, so a
/// test can observe the `poll_live` truth flip from a dropped connection without
/// waiting the full default grace window.
async fn test_router_with_cap_and_grace(
    cap: std::time::Duration,
    grace: std::time::Duration,
) -> axum::Router {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    router(AppState::with_wait_cap_and_grace(storage, cap, grace))
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
        room_id.starts_with("cbc-slider-labels-"),
        "room id should be kebab subject + timestamp, got {room_id}"
    );
    assert_eq!(
        open_body["share_line"]
            .as_str()
            .expect("share_line present"),
        format!("Join CBC room {room_id}")
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

/// Join supplying an explicit `instance` identity (what the client resolves from
/// an `--as` label / harness session id).
async fn join_as(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    instance: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/join"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "repo": repo, "model": model, "cwd": cwd, "instance": instance }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

/// Count the participant rows a room currently has (via its public roster).
async fn participant_count(app: &axum::Router, room_id: &str) -> usize {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let body = body_json(resp.into_body()).await;
    body["participants"]
        .as_array()
        .expect("participants array")
        .len()
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
async fn a_busy_counterpart_who_read_my_message_yields_retry_after() {
    // The dogfood gap: A asked a dense question, B consumed it and is composing a
    // long autonomous reply (no `waiting_user` sentinel — B isn't consulting a
    // human, just thinking). With nothing unread, A's wait parks and times out —
    // but because B has *read* A's latest and not yet replied (the ball is in B's
    // court, tracked by `last_read_seq`), the timeout now carries a `retry_after`
    // so A stays autonomous instead of giving up and pinging its human.
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "busy room").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A asks; B consumes it (B's cursor advances past A's message).
    send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        None,
        "9-part question",
    )
    .await;
    let (_, got) = wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    assert_eq!(got["message"]["body"].as_str(), Some("9-part question"));

    // A waits again. B has read A's latest and not replied → busy. The timeout
    // carries the Med-base busy backoff (20s) even though no sentinel was posted.
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(20),
        "a busy counterpart (read-but-not-replied) yields the Med-base busy backoff, got {body}"
    );
}

#[tokio::test]
async fn a_busy_wait_still_parks_the_full_cap_and_does_not_short_circuit() {
    // Guards the load-bearing scope constraint that busy does NOT collapse the
    // long-poll: busy is consulted only for the post-park `retry_after` hint, never
    // in the cap-decision tree, so a busy waiter must still park ~the full cap and
    // time out — not return early the way the ghost/zero-cap branch does. (The
    // *shortening* case — busy mistakenly `min`-ed into the cap like waiting_user —
    // is unobservable in a fast test because the Med backoff floor is 20s, far
    // above any test cap; this catches the cheaper, real regression: busy wired to
    // zero/short-circuit the park.) A 200ms cap with no reply must take ~200ms; an
    // early return (≈0ms) would mean busy was miswired into the cap path.
    let app = test_router_with_cap(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "busy parks").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A asks; B reads it → A is busy with nothing left to deliver.
    send(&app, &room_id, "repo-a", "opus47", "/work/a", None, "q").await;
    wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let start = std::time::Instant::now();
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    let elapsed = start.elapsed();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(20),
        "the busy hint still rides the full-cap timeout, got {body}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(120),
        "a busy wait must park ~the full 200ms cap, not short-circuit; took {elapsed:?}"
    );
}

#[tokio::test]
async fn the_busy_hint_clears_once_the_counterpart_replies() {
    // User-facing behaviour: once B replies, A's delivery of that reply must carry
    // no `retry_after` — the conversation moved on, don't tell A to back off.
    // (Mechanism note: in 2-agent v1 it is the read-cursor guard that clears it —
    // B cannot claim its own reply, so its `last_read_seq` stays below the reply's
    // seq. The `latest.sender != handle` guard in `counterpart_busy_backoff` is a
    // redundant-but-explicit safeguard in v1, not what this test isolates. This
    // test pins the behaviour; tests 4 and 5 give the individual guards teeth.)
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "busy clears").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A asks; B consumes it; B replies.
    send(
        &app, &room_id, "repo-a", "opus47", "/work/a", None, "question",
    )
    .await;
    wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    send(
        &app, &room_id, "repo-b", "sonnet46", "/work/b", None, "answer",
    )
    .await;

    // A waits and receives B's reply — no busy backoff, the conversation moved on.
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["message"]["body"].as_str(),
        Some("answer"),
        "got {body}"
    );
    assert!(
        body.get("retry_after").is_none(),
        "once the counterpart replies the busy hint must clear, got {body}"
    );
}

#[tokio::test]
async fn no_busy_hint_until_the_counterpart_has_read_my_message() {
    // Busy is gated on the counterpart's *read cursor*, not merely on my having
    // spoken. If B has not yet claimed A's message (last_read_seq < its seq), B is
    // not yet "sitting on" a reply obligation, so A's wait carries no hint.
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "unread room").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A speaks but B never waits, so B's cursor stays behind A's message.
    send(
        &app, &room_id, "repo-a", "opus47", "/work/a", None, "unread",
    )
    .await;

    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert!(
        body.get("retry_after").is_none(),
        "an unread message is not yet a busy obligation, got {body}"
    );
}

#[tokio::test]
async fn an_active_waiting_user_takes_precedence_over_the_busy_hint() {
    // Both states can hold at once: B signalled `waiting_user` (consulting its
    // human), A replied anyway, and B read that reply — so A is "busy-eligible"
    // (it spoke last, B read it) AND B still has an active away-signal. The
    // explicit, more-specific `waiting_user` must win: A's timeout carries the
    // severity-scaled sentinel backoff (high = 45), not the Med busy base (20).
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "precedence room").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // B says it is consulting its user.
    signal(
        &app,
        &room_id,
        "repo-b",
        "sonnet46",
        "/work/b",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;
    // A consumes the sentinel, then replies anyway.
    wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    send(
        &app, &room_id, "repo-a", "opus47", "/work/a", None, "my reply",
    )
    .await;
    // B reads A's reply (cursor advances past it) — A is now busy-eligible too.
    wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // A waits again: the active away-signal wins over the inferred busy hint.
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "waiting_user (45) must win over the Med busy base (20), got {body}"
    );
}

#[tokio::test]
async fn the_busy_backoff_grows_the_longer_the_counterpart_stays_silent() {
    // The busy hint reuses the `waiting_user` decay curve at a fixed Med severity,
    // measured from the unanswered message's `created_at`. A fresh obligation sits
    // at the Med base (20, asserted elsewhere); one ~7 minutes old has decayed two
    // ×1.5 steps past the 5-minute flat zone → 20·1.5² = 45. We backdate the
    // message via storage because the HTTP send path always stamps "now".
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "decay room").await;
    let (_, a) = join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;
    let a_handle = a["handle"].as_str().expect("a handle").to_string();

    // A's question, backdated 7 minutes (in the n=2 decay window [420s, 480s)).
    let seven_min_ago = OffsetDateTime::now_utc() - time::Duration::minutes(7);
    storage
        .create_message(&room_id, &a_handle, None, "old question", seven_min_ago)
        .await
        .expect("inject backdated message");
    // B reads it (cursor advances past A's message) so A is busy.
    wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "a 7-min-old busy obligation has decayed past the Med base (20) to 45, got {body}"
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
async fn wait_with_no_counterpart_returns_awaiting_counterpart_immediately() {
    // A long cap proves the early return: if the handler parked, this lone-waiter
    // wait would hang the full second and report `paused_by_timeout` instead. The
    // opener is the only participant — nobody has been told the room id yet — so
    // the server must short-circuit, not silently long-poll.
    let app = test_router_with_cap(std::time::Duration::from_secs(1)).await;
    let room_id = open_room_id(&app, "alone").await;
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;

    let (status, body) = wait(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("awaiting_counterpart"),
        "a sole participant must get awaiting_counterpart, not a parked timeout; got {body}"
    );
    assert!(
        body.get("retry_after").is_none(),
        "awaiting_counterpart carries no backoff hint; got {body}"
    );
}

#[tokio::test]
async fn wait_refreshes_presence_even_on_awaiting_counterpart() {
    // The sole-participant `awaiting_counterpart` path returns *before* the parking
    // logic that normally refreshes liveness (the `wait_for_message_until` touch).
    // Yet a background poll holding the line for a late joiner must keep proving it
    // is alive — otherwise it ages past GHOST_AFTER and the joiner that finally
    // arrives sees the holder as a stale ghost. So every wait, including this early
    // return, must refresh `last_poll_at`.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_secs(1)).await;
    let room_id = open_room_id(&app, "presence").await;
    let (_, a) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    let a_handle = a["handle"].as_str().expect("a handle").to_string();

    // Backdate the poller's liveness to simulate a long hold while still alone.
    let stale = OffsetDateTime::now_utc() - Duration::minutes(20);
    storage
        .touch_last_poll(&a_handle, stale)
        .await
        .expect("backdate last_poll_at");

    // A sole-participant wait short-circuits with awaiting_counterpart...
    let (status, body) = wait(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"].as_str(), Some("awaiting_counterpart"));

    // ...but it must still have refreshed the caller's presence on the way out.
    let p = storage
        .get_participant_by_handle(&room_id, &a_handle)
        .await
        .expect("query participant")
        .expect("participant exists");
    assert!(
        p.last_poll_at > stale,
        "awaiting_counterpart wait must refresh last_poll_at; it stayed at {}",
        p.last_poll_at
    );
}

#[tokio::test]
async fn wait_times_out_with_paused_by_timeout() {
    let app = test_router_with_cap(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "timeout").await;
    // Two participants joined: the counterpart exists (so we are past the
    // awaiting_counterpart gate), but neither has sent — wait parks until the
    // (short) cap, then reports paused_by_timeout.
    join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    join(&app, &room_id, "mvp-api", "sonnet46", "/work/b").await;

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
async fn resuming_with_the_handle_round_trips_to_the_same_participant() {
    // The core identity-churn fix. The only identity an agent is ever shown is its
    // handle (the canonical `instance` is never returned). When it re-attaches by
    // passing that handle back as its identity, it must resume the SAME participant
    // — not mint a duplicate. A duplicate is what inflates quorum and stalls
    // close/extend consensus.
    let app = test_router().await;
    let room_id = open_room_id(&app, "resume by handle").await;

    // First join under a session-derived instance mints the handle the agent sees.
    let (s1, b1) = join_as(
        &app,
        &room_id,
        "mvp-engine",
        "opus48",
        "/work/a",
        "session-1",
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED);
    let handle = b1["handle"].as_str().expect("handle").to_string();
    assert_eq!(b1["resumed"].as_bool(), Some(false));

    // The agent lost its session id (reinstall / churn) and re-attaches with the
    // only label it has: the handle it was shown.
    let (s2, b2) = join_as(&app, &room_id, "mvp-engine", "opus48", "/work/a", &handle).await;
    assert_eq!(s2, StatusCode::CREATED);
    assert_eq!(
        b2["resumed"].as_bool(),
        Some(true),
        "passing the handle back must resume, not mint"
    );
    assert_eq!(
        b2["handle"].as_str(),
        Some(handle.as_str()),
        "same participant, same handle"
    );

    assert_eq!(
        participant_count(&app, &room_id).await,
        1,
        "no duplicate participant row was created"
    );
}

#[tokio::test]
async fn calling_paths_accept_the_handle_as_identity() {
    // Resuming by handle has to work on the *calling* paths too, not just join —
    // otherwise `cbc poll --as <handle>` (the natural resume) is rejected as "not a
    // participant", which is exactly the churn failure. Exercise it through `send`.
    let app = test_router().await;
    let room_id = open_room_id(&app, "handle on calling paths").await;

    let (_, b1) = join_as(
        &app,
        &room_id,
        "mvp-engine",
        "opus48",
        "/work/a",
        "session-1",
    )
    .await;
    let handle = b1["handle"].as_str().expect("handle").to_string();

    // Send identifying ourselves by the handle we were shown, not the instance.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "repo": "mvp-engine", "model": "opus48", "cwd": "/work/a",
                "instance": handle, "body": "resumed by handle"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "the handle must resolve the sender, not 400"
    );

    // The message is attributed to the original participant, and no row was minted.
    assert_eq!(participant_count(&app, &room_id).await, 1);
}

#[tokio::test]
async fn prune_endpoint_drops_ghost_rows_and_keeps_live() {
    // Operational cleanup for already-accumulated churn: a stale duplicate row is
    // pruned, the live participant is kept, and the response reports both counts.
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "prune ghosts").await;

    // One live participant (joins now), one ghost (last polled 20 min ago).
    let (_, b1) = join_as(
        &app,
        &room_id,
        "mvp-engine",
        "opus48",
        "/work/a",
        "session-live",
    )
    .await;
    assert_eq!(b1["resumed"].as_bool(), Some(false));

    let now = OffsetDateTime::now_utc();
    let ghost = Participant {
        handle: "mvp-engine-opus48-ghst".into(),
        room_id: room_id.clone(),
        repo: "mvp-engine".into(),
        model: "opus48".into(),
        cwd: "/work/a".into(),
        instance: "session-ghost".into(),
        joined_at: now - Duration::hours(1),
        last_poll_at: now - Duration::minutes(20),
        last_read_seq: 0,
        nickname: None,
        wants_close_at: None,
        wants_extend_at: None,
    };
    storage
        .create_participant(&ghost)
        .await
        .expect("seed ghost");
    assert_eq!(
        participant_count(&app, &room_id).await,
        2,
        "ghost is present pre-prune"
    );

    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/prune"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["pruned"].as_u64(), Some(1), "the one ghost was pruned");
    assert_eq!(
        body["remaining"].as_u64(),
        Some(1),
        "the live participant remains"
    );

    assert_eq!(participant_count(&app, &room_id).await, 1);
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

/// `GET /rooms/:id` must include `seconds_since_poll` (non-negative integer) and
/// `stale: bool` for every participant.  A participant who just joined has a fresh
/// poll timestamp, so `stale` must be `false` and `seconds_since_poll` must be
/// small (< 60).  A participant whose `last_poll_at` was seeded 20 min in the past
/// must have `stale: true` and a large `seconds_since_poll`.
#[tokio::test]
async fn status_includes_per_participant_poll_freshness() {
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "freshness check").await;

    // Join one participant — server sets last_poll_at = now.
    let (_, body) = join(&app, &room_id, "mvp-engine", "opus47", "/work/a").await;
    let fresh_handle = body["handle"].as_str().unwrap().to_string();

    // Join a second participant then seed its last_poll_at 20 min in the past.
    let (_, body2) = join(&app, &room_id, "mvp-engine", "sonnet46", "/work/a").await;
    let stale_handle = body2["handle"].as_str().unwrap().to_string();
    let past = OffsetDateTime::now_utc() - Duration::minutes(20);
    storage
        .touch_last_poll(&stale_handle, past)
        .await
        .expect("seed stale timestamp");

    // Fetch status.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let status = body_json(resp.into_body()).await;

    let roster = status["participants"]
        .as_array()
        .expect("participants array");
    assert_eq!(roster.len(), 2);

    let fresh_p = roster
        .iter()
        .find(|p| p["handle"].as_str() == Some(&fresh_handle))
        .expect("fresh participant in roster");
    let stale_p = roster
        .iter()
        .find(|p| p["handle"].as_str() == Some(&stale_handle))
        .expect("stale participant in roster");

    // Fresh participant: seconds_since_poll < 60, stale = false.
    let fresh_secs = fresh_p["seconds_since_poll"]
        .as_i64()
        .expect("seconds_since_poll must be an integer for fresh participant");
    assert!(
        (0..60).contains(&fresh_secs),
        "fresh participant should have seconds_since_poll in [0, 60), got {fresh_secs}"
    );
    assert_eq!(
        fresh_p["stale"].as_bool(),
        Some(false),
        "fresh participant must not be stale"
    );

    // Stale participant: seconds_since_poll >= 1200 (20 min), stale = true.
    let stale_secs = stale_p["seconds_since_poll"]
        .as_i64()
        .expect("seconds_since_poll must be an integer for stale participant");
    assert!(
        stale_secs >= 1200,
        "stale participant should have seconds_since_poll >= 1200, got {stale_secs}"
    );
    assert_eq!(
        stale_p["stale"].as_bool(),
        Some(true),
        "participant with last_poll_at 20 min ago must be stale"
    );
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

    // The default hard cap is 20 (RoomConfig::default). A room-wide count, so a
    // single sender filling the budget exercises the gate.
    const HARD_CAP: usize = 20;
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
    // capped 20 in the room log, not 21.
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
            close_quorum: Default::default(),
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
    const HARD_CAP: usize = 20;

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
    assert!(
        body.get("retry_after").is_none(),
        "counterpart_stale carries no backoff hint — the peer is gone, not paused, got {body}"
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

// --- GET /rooms (list) and GET /rooms/:id/transcript (show) — issue #27 ---

async fn get_rooms(app: &axum::Router, query: &str) -> (StatusCode, Value) {
    let uri = if query.is_empty() {
        "/rooms".to_string()
    } else {
        format!("/rooms?{query}")
    };
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn get_transcript(app: &axum::Router, room_id: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}/transcript"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn list_rooms_returns_summaries_newest_first() {
    let app = test_router().await;
    let older = open_room_id(&app, "older room").await;
    let newer = open_room_id(&app, "newer room").await;
    // one participant on the older room — count must be per-room.
    join(&app, &older, "repo", "opus47", "/a").await;

    let (status, body) = get_rooms(&app, "").await;
    assert_eq!(status, StatusCode::OK);
    let rooms = body.as_array().expect("list is a json array");
    assert_eq!(rooms.len(), 2);
    // newest-first by last_activity_at.
    assert_eq!(rooms[0]["room_id"].as_str(), Some(newer.as_str()));
    assert_eq!(rooms[1]["room_id"].as_str(), Some(older.as_str()));
    // summary fields present.
    assert_eq!(rooms[0]["state"].as_str(), Some("active"));
    assert_eq!(rooms[0]["subject"].as_str(), Some("newer room"));
    assert!(rooms[0]["last_activity_at"].is_string());
    assert_eq!(rooms[0]["participant_count"].as_i64(), Some(0));
    assert_eq!(rooms[1]["participant_count"].as_i64(), Some(1));
}

#[tokio::test]
async fn list_rooms_is_empty_array_when_no_rooms() {
    let app = test_router().await;
    let (status, body) = get_rooms(&app, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0), "no rooms -> []");
}

#[tokio::test]
async fn list_rooms_hides_archived_by_default_but_state_filter_shows_it() {
    let (app, storage) = test_router_returning_storage().await;
    let room_id = open_room_id(&app, "to be archived").await;
    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Archived, now, None)
        .await
        .expect("arrange archived");

    // default hides archived.
    let (status, body) = get_rooms(&app, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0), "archived hidden");

    // explicit ?state=archived surfaces it.
    let (status, body) = get_rooms(&app, "state=archived").await;
    assert_eq!(status, StatusCode::OK);
    let rooms = body.as_array().expect("array");
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0]["room_id"].as_str(), Some(room_id.as_str()));

    // ?all=true also includes it.
    let (_, body) = get_rooms(&app, "all=true").await;
    assert_eq!(
        body.as_array().map(|a| a.len()),
        Some(1),
        "--all includes archived"
    );
}

#[tokio::test]
async fn list_rooms_rejects_unknown_state_with_400() {
    let app = test_router().await;
    // A query-extractor rejection has a plain-text body, so assert on status
    // directly rather than going through the JSON-parsing `get_rooms` helper.
    let req = Request::builder()
        .method("GET")
        .uri("/rooms?state=bogus")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an unknown ?state value is a 400, not a silent empty list"
    );
}

#[tokio::test]
async fn transcript_returns_full_room_with_caps_counters_and_messages() {
    let app = test_router().await;
    let room_id = open_with_caps(&app, "transcript room", Some(5), Some(3)).await;
    join(&app, &room_id, "transcript", "opus47", "/a").await;
    let (_, p2) = join(&app, &room_id, "transcript", "sonnet46", "/b").await;
    let p2_handle = p2["handle"].as_str().expect("p2 handle").to_string();

    // a msg, then a waiting_user sentinel, then two more msgs (so the soft-cap
    // consecutive run at the latest message is 2, and the sentinel is mid-stream).
    send(&app, &room_id, "transcript", "opus47", "/a", None, "first").await;
    signal(
        &app,
        &room_id,
        "transcript",
        "sonnet46",
        "/b",
        "waiting_user",
        Some("high"),
        Some("which label fits 0-100?"),
    )
    .await;
    send(
        &app,
        &room_id,
        "transcript",
        "sonnet46",
        "/b",
        None,
        "second",
    )
    .await;
    send(&app, &room_id, "transcript", "opus47", "/a", None, "third").await;

    let (status, body) = get_transcript(&app, &room_id).await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(body["id"].as_str(), Some(room_id.as_str()));
    assert_eq!(body["state"].as_str(), Some("active"));
    assert_eq!(body["hard_cap"].as_i64(), Some(5));
    assert_eq!(body["soft_cap"].as_i64(), Some(3));
    // three `msg` rows count toward the hard cap; the sentinel does not.
    assert_eq!(body["hard_cap_count"].as_i64(), Some(3));
    // two autonomous msgs since the waiting_user boundary.
    assert_eq!(body["soft_cap_consecutive"].as_i64(), Some(2));
    assert_eq!(body["participants"].as_array().map(|a| a.len()), Some(2));

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(
        messages.len(),
        4,
        "all messages incl. the sentinel, chronological"
    );
    assert_eq!(messages[0]["body"].as_str(), Some("first"));
    // the sentinel carries its severity and question_text.
    let sentinel = messages
        .iter()
        .find(|m| m["type"].as_str() == Some("waiting_user"))
        .expect("a waiting_user sentinel in the transcript");
    assert_eq!(sentinel["from"].as_str(), Some(p2_handle.as_str()));
    assert_eq!(sentinel["severity"].as_str(), Some("high"));
    assert_eq!(
        sentinel["question_text"].as_str(),
        Some("which label fits 0-100?")
    );
}

#[tokio::test]
async fn transcript_for_missing_room_is_404() {
    let app = test_router().await;
    let (status, _) = get_transcript(&app, "no-such-room-20260101-0000").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ----- #10: per-call max_wait_secs (MCP path returns before the client's tool timeout) -----

/// `wait` with an explicit `max_wait_secs` query param. The MCP `cbc_wait` tool
/// passes this to return before a client's tool-call timeout, well under the
/// server's 10-minute cap.
async fn wait_with_max(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    max_wait_secs: u32,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/rooms/{room_id}/wait?repo={repo}&model={model}&cwd={cwd}&max_wait_secs={max_wait_secs}"
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn max_wait_secs_caps_below_server_wait_cap() {
    // Server cap is the full 10 minutes (DEFAULT_WAIT_CAP); a per-call
    // max_wait_secs must shorten it so the MCP path returns before a client's
    // tool-call timeout. With no message queued, the wait returns
    // paused_by_timeout promptly — not after parking for 600s.
    let app = test_router().await;
    let room_id = open_room_id(&app, "mcp wait cap").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let (status, body) = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        wait_with_max(&app, &room_id, "repo-b", "sonnet46", "/work/b", 1),
    )
    .await
    .expect("wait must honor max_wait_secs and return well before the 600s server cap");

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"].as_str(), Some("paused_by_timeout"));
}

#[tokio::test]
async fn max_wait_secs_still_delivers_a_queued_message() {
    // A short cap must not drop a message already waiting: the claim happens
    // before the deadline check.
    let app = test_router().await;
    let room_id = open_room_id(&app, "mcp wait cap delivery").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        None,
        "queued before wait",
    )
    .await;

    let (status, body) = wait_with_max(&app, &room_id, "repo-b", "sonnet46", "/work/b", 1).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"]["body"].as_str(), Some("queued before wait"));
}

#[tokio::test]
async fn max_wait_secs_caps_below_the_counterpart_backoff() {
    // When the counterpart is parked behind an active waiting_user sentinel, the
    // server normally shortens the poll to the severity backoff (>= 10s). A
    // smaller per-call max_wait_secs must still win — otherwise an MCP cbc_wait
    // would overshoot its cap (and the client's tool-call timeout) in exactly the
    // scenario the backoff machinery exists for.
    let app = test_router().await; // 600s server cap
    let room_id = open_room_id(&app, "mcp cap vs backoff").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // opus47 signals it is consulting its user → sonnet46's wait would back off.
    signal(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/work/a",
        "waiting_user",
        Some("low"),
        Some("which label?"),
    )
    .await;

    // First wait consumes the sentinel message itself (delivered immediately).
    let (_, first) = wait_with_max(&app, &room_id, "repo-b", "sonnet46", "/work/b", 1).await;
    assert_eq!(first["message"]["type"].as_str(), Some("waiting_user"));

    // Second wait has no new message but the sentinel is still active, so the
    // server would park for the severity backoff (>= 10s). max_wait_secs=1 must
    // still cap it, returning paused_by_timeout within the per-call bound.
    let (status, body) = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        wait_with_max(&app, &room_id, "repo-b", "sonnet46", "/work/b", 1),
    )
    .await
    .expect("wait must honor max_wait_secs even while backing off behind a sentinel");

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"].as_str(), Some("paused_by_timeout"));
}

// --- Ghost-robust coordination (2-live-max + harmless dead "ghost" rows) ---
//
// A room may accrue extra participant rows when an agent churns identity (a new
// session id on /clear, fork, or crash) or simply dies, leaving its old row
// behind. We support 2 LIVE agents at a time; any extra row must be an inert
// ghost (last_poll_at older than GHOST_AFTER). These tests pin that a single
// ghost cannot poison the 2-agent coordination of the live pair.

#[tokio::test]
async fn a_live_counterpart_with_a_ghost_row_is_not_stale() {
    // A (caller), B (live), and a third row C that has gone dark. The OLD
    // `counterpart_is_stale` used `.any(stale)`, so C alone flipped A's wait to
    // `counterpart_stale` — telling A to STOP polling a conversation in which B is
    // very much alive. The fix is `.all(stale)` over the others: A must park
    // normally (`paused_by_timeout`), because a live counterpart still exists.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "ghost not stale room").await;

    join(&app, &room_id, "repo-a", "opus47", "/a").await; // caller
    join(&app, &room_id, "repo-b", "opus47", "/b").await; // live counterpart
    let (_, c) = join(&app, &room_id, "repo-c", "opus47", "/c").await; // ghost-to-be
    let c_handle = c["handle"].as_str().expect("c handle").to_string();

    // C went dark 20 min ago (past GHOST_AFTER); B just joined and is fresh.
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(&c_handle, stale)
        .await
        .expect("backdate C's last poll");

    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "one ghost row must not abandon a live counterpart, got {body}"
    );
}

#[tokio::test]
async fn an_all_ghost_room_still_reports_counterpart_stale() {
    // Regression guard for the `.all` fix: if EVERY other participant is dark,
    // the room is genuinely stale and the wait must still short-circuit to
    // `counterpart_stale` (the `.all` must not over-suppress).
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "all ghost room").await;

    join(&app, &room_id, "repo-a", "opus47", "/a").await; // caller
    let (_, b) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let (_, c) = join(&app, &room_id, "repo-c", "opus47", "/c").await;
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    for h in [&b["handle"], &c["handle"]] {
        storage
            .touch_last_poll(h.as_str().expect("handle"), stale)
            .await
            .expect("backdate poll");
    }

    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("counterpart_stale"),
        "all others dark must still be counterpart_stale, got {body}"
    );
}

#[tokio::test]
async fn busy_hint_survives_a_ghost_that_never_read_my_message() {
    // The busy hint must follow the LIVE counterpart, not an arbitrary row. The
    // OLD `counterpart_busy_backoff` did `.find(first non-caller)`, and
    // `list_participants` is `ORDER BY joined_at`, so it deterministically picked
    // the oldest row — the ghost C (it joined first), whose frozen low cursor
    // never reached A's latest. That wrongly cleared the busy hint. The fix asks
    // "did ANY other read my latest?" so the live B's advanced cursor counts.
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(80)).await;
    let room_id = open_room_id(&app, "busy with ghost room").await;

    // C joins FIRST (so the old `.find` picks it), never reads anything.
    join(&app, &room_id, "repo-c", "opus47", "/c").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await; // live counterpart
    join(&app, &room_id, "repo-a", "opus47", "/a").await; // caller

    // A asks a question; B reads it (cursor advances); C's cursor stays behind.
    send(&app, &room_id, "repo-a", "opus47", "/a", None, "ping?").await;
    wait(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(20),
        "B read A's latest, so A is busy-eligible (Med base 20) despite the ghost C, got {body}"
    );
}

#[tokio::test]
async fn a_paused_counterpart_is_still_detected_with_a_ghost_present() {
    // A live-but-paused counterpart (not polling because it is consulting its
    // human) must still be read as paused, not confused with the dead ghost. B
    // posts a `waiting_user`; A consumes it; on A's next wait the sentinel backoff
    // (high = 45) must win and the response must NOT be `counterpart_stale`, even
    // though a stale ghost row C is present.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(150)).await;
    let room_id = open_room_id(&app, "paused with ghost room").await;

    join(&app, &room_id, "repo-a", "opus47", "/a").await; // caller
    join(&app, &room_id, "repo-b", "opus47", "/b").await; // live, will pause
    let (_, c) = join(&app, &room_id, "repo-c", "opus47", "/c").await; // ghost
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(c["handle"].as_str().expect("c handle"), stale)
        .await
        .expect("backdate C");

    signal(
        &app,
        &room_id,
        "repo-b",
        "opus47",
        "/b",
        "waiting_user",
        Some("high"),
        Some("merge?"),
    )
    .await;
    // A consumes the sentinel.
    wait(&app, &room_id, "repo-a", "opus47", "/a").await;

    // A waits again: the sentinel is still B's latest → backoff wins; not stale.
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"].as_str(),
        Some("paused_by_timeout"),
        "a paused counterpart with a ghost present must not read as counterpart_stale, got {body}"
    );
    assert_eq!(
        body["retry_after"].as_u64(),
        Some(45),
        "the high-severity sentinel backoff must survive a ghost row, got {body}"
    );
}

// --- Re-join must not resurface the unread inbox (Bug B) ---

#[tokio::test]
async fn resume_does_not_resurface_unread_inbox_messages() {
    // The MCP loop re-joins before sending, so a stable agent re-joins often. A
    // re-join's `recent_messages` must be bounded to the participant's read cursor:
    // a message it has NOT yet consumed via `wait` (seq > cursor) must arrive ONLY
    // through `wait`, never also as "recent context" — otherwise the agent acts on
    // it from the join response and then `wait` re-delivers the same seq.
    let app = test_router().await;
    let room_id = open_room_id(&app, "resume replay room").await;

    // A joins (cursor = 0, room empty); B joins and sends one message.
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;
    send(&app, &room_id, "repo-b", "opus47", "/b", None, "from-b").await;

    // A re-joins with the same identity (resume fast-path). Its unread inbox
    // message must NOT appear in recent_messages.
    let (_, rejoin) = join(&app, &room_id, "repo-a", "opus47", "/a").await;
    let recent = rejoin["recent_messages"].as_array().expect("recent array");
    assert!(
        recent.iter().all(|m| m["body"].as_str() != Some("from-b")),
        "resume must not resurface the unread inbox message, got {rejoin}"
    );

    // ...and `wait` still delivers it exactly once.
    let (status, w) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        w["message"]["body"].as_str(),
        Some("from-b"),
        "wait must deliver the message the resume omitted, got {w}"
    );
}

#[tokio::test]
async fn fresh_join_still_returns_full_recent_backlog() {
    // Regression guard for the cursor-bounding: a FRESH joiner (cursor = current
    // high-water) must still receive the full pre-join backlog as recent context.
    let app = test_router().await;
    let room_id = open_room_id(&app, "fresh backlog room").await;

    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    send(&app, &room_id, "repo-a", "opus47", "/a", None, "msg1").await;
    send(&app, &room_id, "repo-a", "opus47", "/a", None, "msg2").await;

    // B joins fresh (distinct identity). It sees both prior messages.
    let (_, j) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let recent = j["recent_messages"].as_array().expect("recent array");
    let bodies: Vec<&str> = recent.iter().filter_map(|m| m["body"].as_str()).collect();
    assert!(
        bodies.contains(&"msg1") && bodies.contains(&"msg2"),
        "a fresh joiner must still get the full recent backlog, got {bodies:?}"
    );
}

// --- Participant nicknames (friendly display label, distinct from identity) ---

async fn join_with_nick(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    nickname: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/join"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "repo": repo, "model": model, "cwd": cwd, "nickname": nickname }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn get_status(app: &axum::Router, room_id: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

#[tokio::test]
async fn nickname_round_trips_through_join_and_status() {
    // A nickname is a human-friendly display label, NOT identity: it never changes
    // the handle, and a re-join may update it.
    let app = test_router().await;
    let room_id = open_room_id(&app, "nickname room").await;

    let (status, body) =
        join_with_nick(&app, &room_id, "repo-a", "opus47", "/a", "concierge-agent").await;
    assert_eq!(status, StatusCode::CREATED);
    let handle = body["handle"].as_str().expect("handle").to_string();
    assert!(
        handle.starts_with("repo-a-opus47-"),
        "the nickname must not affect handle derivation, got {handle}"
    );

    let (_, st) = get_status(&app, &room_id).await;
    let p = &st["participants"][0];
    assert_eq!(
        p["nickname"].as_str(),
        Some("concierge-agent"),
        "status must surface the nickname, got {st}"
    );
    assert_eq!(
        p["handle"].as_str(),
        Some(handle.as_str()),
        "identity is unchanged, got {st}"
    );

    // A re-join with the same identity updates the nickname.
    join_with_nick(&app, &room_id, "repo-a", "opus47", "/a", "results-agent").await;
    let (_, st2) = get_status(&app, &room_id).await;
    assert_eq!(
        st2["participants"][0]["nickname"].as_str(),
        Some("results-agent"),
        "a re-join must update the nickname, got {st2}"
    );
    assert_eq!(
        st2["participants"][0]["handle"].as_str(),
        Some(handle.as_str()),
        "the handle is still stable across the nickname update, got {st2}"
    );
}

/// GET a room's status (`GET /rooms/:id`) as JSON.
async fn room_status(app: &axum::Router, room_id: &str) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    body_json(resp.into_body()).await
}

/// POST a close vote with the human-only `force` flag set.
async fn close_force(
    app: &axum::Router,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/rooms/{room_id}/close"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "repo": repo, "model": model, "cwd": cwd, "force": true }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

// ---- Hole 1: wait drains unread before reporting a terminal state ----

#[tokio::test]
async fn wait_on_a_closed_room_drains_unread_then_reports_closed() {
    // A message sent before a close must still reach the counterpart: closing a
    // room must not strand unread messages behind the wait state-gate (the
    // "agents never see the latest messages / servers appear stale" bug).
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "drain room").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // B broadcasts a message A has not read.
    let (s, _) = send(&app, &room_id, "repo-b", "opus47", "/b", None, "last words").await;
    assert_eq!(s, StatusCode::CREATED);

    // Drive the room terminal directly (isolates Hole 1 from the consensus path).
    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Closed, now, None)
        .await
        .expect("force room closed");

    // A waits on the now-closed room: it must still receive the unread message,
    // carrying a `room_state` hint that the room is closed.
    let (status, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["message"]["body"].as_str(),
        Some("last words"),
        "a closed room must still drain its unread backlog, got {body}"
    );
    assert_eq!(
        body["room_state"].as_str(),
        Some("closed"),
        "a message drained from a closed room must carry the terminal hint, got {body}"
    );

    // Inbox now empty → the next wait reports the terminal status.
    let (_, body2) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        body2["status"].as_str(),
        Some("closed"),
        "once drained, a closed room reports its state, got {body2}"
    );
}

#[tokio::test]
async fn wait_drains_multiple_unread_from_a_closed_room_in_order() {
    // Mirrors the real incident exactly: TWO messages stranded behind a close
    // (seq 119 + 123 in the report). Each wait drains one, oldest-first, and only
    // the third wait — inbox empty — reports `closed`.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "double drain").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    send(&app, &room_id, "repo-b", "opus47", "/b", None, "first").await;
    send(&app, &room_id, "repo-b", "opus47", "/b", None, "second").await;
    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Closed, now, None)
        .await
        .expect("force room closed");

    let (_, m1) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(m1["message"]["body"].as_str(), Some("first"), "got {m1}");
    assert_eq!(m1["room_state"].as_str(), Some("closed"), "got {m1}");
    let (_, m2) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(m2["message"]["body"].as_str(), Some("second"), "got {m2}");
    assert_eq!(m2["room_state"].as_str(), Some("closed"), "got {m2}");
    let (_, done) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        done["status"].as_str(),
        Some("closed"),
        "after draining both, the closed status is reported, got {done}"
    );
}

#[tokio::test]
async fn wait_drains_unread_from_a_paused_room_too() {
    // Same guarantee for paused/archived — the gate covers all three terminal
    // states, so the drain must apply uniformly.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "paused drain").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (s, _) = send(
        &app,
        &room_id,
        "repo-b",
        "opus47",
        "/b",
        None,
        "before pause",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room_id, RoomState::Active, RoomState::Paused, now, None)
        .await
        .expect("force room paused");

    let (_, body) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        body["message"]["body"].as_str(),
        Some("before pause"),
        "a paused room must still drain unread, got {body}"
    );
    assert_eq!(body["room_state"].as_str(), Some("paused"), "got {body}");
}

// ---- Hole 2: consensus close ----

#[tokio::test]
async fn close_is_a_proposal_until_the_counterpart_agrees() {
    // With two live agents, one close is a PROPOSAL, not a close. The room stays
    // open and the counterpart learns of it via a `close_proposed` wait status;
    // only when the second agent also votes does the room actually close.
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "consensus room").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes close.
    let (status, body) =
        lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(status, StatusCode::OK, "vote accepted: {body}");
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "one vote of two live agents is a proposal, not a close, got {body}"
    );
    assert_eq!(body["votes"].as_u64(), Some(1), "got {body}");
    assert_eq!(body["needed"].as_u64(), Some(2), "got {body}");

    // Room is still open.
    let st = room_status(&app, &room_id).await;
    assert_eq!(st["state"].as_str(), Some("active"), "got {st}");

    // B waits and is told a close was proposed.
    let (_, wbody) = wait(&app, &room_id, "repo-b", "opus47", "/b").await;
    assert_eq!(
        wbody["status"].as_str(),
        Some("close_proposed"),
        "the counterpart must learn of the pending close, got {wbody}"
    );

    // B agrees → quorum met → closed.
    let (_, cbody) = lifecycle_op(&app, &room_id, "close", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        cbody["status"].as_str(),
        Some("closed"),
        "the second vote meets quorum and closes, got {cbody}"
    );
    let st2 = room_status(&app, &room_id).await;
    assert_eq!(st2["state"].as_str(), Some("closed"), "got {st2}");
}

#[tokio::test]
async fn a_counterparts_send_does_not_cancel_my_pending_close() {
    // The consensus-deadlock fix. A landed message clears only the SENDER's own
    // pending close vote, never the counterpart's. So A's close vote stands while B
    // keeps talking, and B's own "substance then vote" close then reaches 2/2 — the
    // room actually closes, instead of the two agents wiping each other's votes
    // forever (the room-wide-clear deadlock observed live).
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "close survives").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes close.
    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "got {body}"
    );

    // B sends a wrap-up message (substance before its own vote). This must NOT
    // clear A's pending close vote.
    let (s, _) = send(
        &app,
        &room_id,
        "repo-b",
        "opus47",
        "/b",
        None,
        "agreed, wrapping up",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // A drains B's message normally.
    let (_, wbody) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        wbody["message"]["body"].as_str(),
        Some("agreed, wrapping up"),
        "got {wbody}"
    );

    // B now votes close. A's vote survived B's send, so quorum is 2/2 → closed.
    let (_, cbody) = lifecycle_op(&app, &room_id, "close", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        cbody["status"].as_str(),
        Some("closed"),
        "A's close vote must survive the counterpart's message so B's vote reaches 2/2, got {cbody}"
    );
}

#[tokio::test]
async fn my_own_send_cancels_my_own_pending_close() {
    // The retained, scoped half: a sender's OWN later message retracts its OWN
    // pending close — "I changed my mind, let's keep talking." A proposes close,
    // then A itself sends a message; A's vote is gone, so B voting close is only a
    // fresh 1/2 proposal, not a close.
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "close self-cancel").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "got {body}"
    );

    // A retracts by talking again.
    let (s, _) = send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/a",
        None,
        "actually, one more thing",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // B votes close → only 1/2, because A's own message cleared A's own vote.
    let (_, cbody) = lifecycle_op(&app, &room_id, "close", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        cbody["status"].as_str(),
        Some("close_proposed"),
        "A's own send must clear A's own vote, so B's vote is a fresh 1/2 proposal, got {cbody}"
    );
}

#[tokio::test]
async fn a_lone_live_agent_closes_immediately_when_counterpart_is_a_ghost() {
    // Consensus counts only LIVE participants. If the counterpart has gone dark
    // (ghost), the remaining live agent is the whole quorum → its vote closes the
    // room at once. No force needed for the dead-counterpart case.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "ghost close").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    let (_, b) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let b_handle = b["handle"].as_str().expect("b handle").to_string();

    // B is a ghost: last poll 20 min ago (> GHOST_AFTER).
    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(&b_handle, stale)
        .await
        .expect("backdate B");

    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("closed"),
        "a lone live agent closes immediately past a ghost, got {body}"
    );
    let st = room_status(&app, &room_id).await;
    assert_eq!(st["state"].as_str(), Some("closed"), "got {st}");
}

#[tokio::test]
async fn force_close_overrides_consensus() {
    // The human escape hatch: `--force` closes a room with two live agents in one
    // shot, bypassing the vote.
    let app = test_router().await;
    let room_id = open_room_id(&app, "force room").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (status, body) = close_force(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(status, StatusCode::OK, "force close: {body}");
    assert_eq!(
        body["status"].as_str(),
        Some("closed"),
        "force must close despite an un-consented counterpart, got {body}"
    );
    let st = room_status(&app, &room_id).await;
    assert_eq!(st["state"].as_str(), Some("closed"), "got {st}");
}

// ---- Consensus cap-extend (mirrors consensus close) ----

#[tokio::test]
async fn extend_is_a_proposal_until_the_counterpart_agrees() {
    // With two live agents, one extend vote is a PROPOSAL, not a bump. The cap is
    // unchanged and the counterpart learns of it via an `extend_proposed` wait
    // status; only the second vote bumps the hard cap by +20.
    let app = test_router_with_cap(std::time::Duration::from_millis(200)).await;
    let room_id = open_with_caps(&app, "extend room", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes extend.
    let (status, body) =
        lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(status, StatusCode::OK, "vote accepted: {body}");
    assert_eq!(
        body["status"].as_str(),
        Some("extend_proposed"),
        "one vote of two live agents is a proposal, got {body}"
    );
    assert_eq!(body["votes"].as_u64(), Some(1), "got {body}");
    assert_eq!(body["needed"].as_u64(), Some(2), "got {body}");
    assert!(
        body["hard_cap"].is_null(),
        "cap unchanged while proposed, got {body}"
    );

    // B waits and is told an extend was proposed.
    let (_, wbody) = wait(&app, &room_id, "repo-b", "opus47", "/b").await;
    assert_eq!(
        wbody["status"].as_str(),
        Some("extend_proposed"),
        "the counterpart must learn of the pending extend, got {wbody}"
    );

    // B agrees → quorum met → cap bumps by +20.
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        ebody["status"].as_str(),
        Some("extended"),
        "the second vote meets quorum and extends, got {ebody}"
    );
    assert_eq!(
        ebody["hard_cap"].as_u64(),
        Some(30),
        "a 10 cap extends to 30 (+20 step), got {ebody}"
    );
}

#[tokio::test]
async fn default_room_cap_is_twenty_and_extend_adds_twenty() {
    // The cap defaults: a room opened with no override caps at 20 messages, and one
    // consensus extend raises it to 40 (+20 step). Pins both the default hard cap
    // and the extend increment in the canonical, no-override path.
    let app = test_router().await;
    let room_id = open_room_id(&app, "default cap").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // Default hard cap is 20 (read from the transcript, which exposes the caps).
    let (_, tx) = get_transcript(&app, &room_id).await;
    assert_eq!(
        tx["hard_cap"].as_u64(),
        Some(20),
        "a default room caps at 20 messages, got {tx}"
    );

    // One consensus extend → 40.
    lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(ebody["status"].as_str(), Some("extended"), "got {ebody}");
    assert_eq!(
        ebody["hard_cap"].as_u64(),
        Some(40),
        "the default 20 cap extends to 40, got {ebody}"
    );
}

#[tokio::test]
async fn repeated_extends_stack_by_twenty_each() {
    // Extending is repeatable: each consensus round adds another +20 (10 → 30 → 50).
    let app = test_router().await;
    let room_id = open_with_caps(&app, "stack room", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // Round 1 → 30.
    lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    let (_, r1) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(r1["hard_cap"].as_u64(), Some(30), "round 1 → 30, got {r1}");

    // Round 2 → 50 (votes cleared after the first bump, so each side votes afresh).
    let (_, p2) = lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        p2["status"].as_str(),
        Some("extend_proposed"),
        "the first round's votes were cleared, so this is a fresh proposal, got {p2}"
    );
    let (_, r2) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(r2["hard_cap"].as_u64(), Some(50), "round 2 → 50, got {r2}");
}

#[tokio::test]
async fn a_lone_live_agent_extends_immediately_when_counterpart_is_a_ghost() {
    // Consensus counts only LIVE participants — symmetric with consensus close. If
    // the counterpart has gone dark, the remaining live agent is the whole quorum,
    // so its single vote bumps the cap at once.
    let (app, storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_with_caps(&app, "ghost extend", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    let (_, b) = join(&app, &room_id, "repo-b", "opus47", "/b").await;
    let b_handle = b["handle"].as_str().expect("b handle").to_string();

    let stale = OffsetDateTime::now_utc() - time::Duration::minutes(20);
    storage
        .touch_last_poll(&b_handle, stale)
        .await
        .expect("backdate B");

    let (_, body) = lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("extended"),
        "a lone live agent extends immediately past a ghost, got {body}"
    );
    assert_eq!(
        body["hard_cap"].as_u64(),
        Some(30),
        "10 + 20 step, got {body}"
    );
}

#[tokio::test]
async fn a_counterparts_send_does_not_cancel_my_pending_extend() {
    // Symmetric with close. A landed message clears only the SENDER's own pending
    // extend vote, never the counterpart's. So A's extend vote stands while B keeps
    // talking, and B's own vote then reaches 2/2 → the cap bumps, instead of the
    // agents wiping each other's extend votes into a deadlock at the cap wall.
    let app = test_router_with_cap(std::time::Duration::from_millis(200)).await;
    let room_id = open_with_caps(&app, "extend survives", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (_, p) = lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(p["status"].as_str(), Some("extend_proposed"), "got {p}");

    // B sends a message under the cap — this must NOT clear A's extend vote.
    let (s, _) = send(
        &app,
        &room_id,
        "repo-b",
        "opus47",
        "/b",
        None,
        "one more point",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // B votes extend → A's vote survived → 2/2 → the cap bumps.
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        ebody["status"].as_str(),
        Some("extended"),
        "A's extend vote must survive the counterpart's message so B's vote reaches 2/2, got {ebody}"
    );
    assert_eq!(
        ebody["hard_cap"].as_u64(),
        Some(30),
        "10 + 20 step, got {ebody}"
    );
}

#[tokio::test]
async fn my_own_send_cancels_my_own_pending_extend() {
    // Retained scoped half: a sender's OWN message retracts its OWN extend vote — a
    // landed message means the room had cap room, so the sender did not need the
    // extend (implicit self-decline). A proposes extend, A sends, A's vote is gone,
    // so B voting extend is a fresh 1/2 proposal, not a bump.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "extend self-cancel", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (_, p) = lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(p["status"].as_str(), Some("extend_proposed"), "got {p}");

    // A talks again under the cap — clears A's own extend vote.
    let (s, _) = send(
        &app,
        &room_id,
        "repo-a",
        "opus47",
        "/a",
        None,
        "still going",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // B votes extend → A's vote was cleared by A's own send, so this is a fresh 1/2.
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        ebody["status"].as_str(),
        Some("extend_proposed"),
        "A's own send must clear A's own extend vote, so B's vote is a fresh proposal, got {ebody}"
    );
    assert_eq!(ebody["votes"].as_u64(), Some(1), "got {ebody}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deciding_votes_bump_the_cap_exactly_once() {
    // Both agents fire the extend vote at the same time. `try_extend` records the
    // vote, counts, and bumps+clears inside ONE transaction, so the two requests
    // serialize: exactly one sees quorum and bumps (+20 to 30), the other lands as
    // a proposal or reads the already-cleared votes — never a double-bump to 50.
    // (The pre-tx per-statement code could interleave at await points and bump
    // twice; this test guards that regression.)
    let app = test_router().await;
    let room_id = open_with_caps(&app, "concurrent extend", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let (ra, rb) = tokio::join!(
        lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None),
        lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None),
    );

    let extended = [&ra.1, &rb.1]
        .iter()
        .filter(|b| b["status"].as_str() == Some("extended"))
        .count();
    assert_eq!(
        extended, 1,
        "exactly one vote completes the extend, got {} / {}",
        ra.1, rb.1
    );
    let caps: Vec<u64> = [&ra.1, &rb.1]
        .iter()
        .filter_map(|b| b["hard_cap"].as_u64())
        .collect();
    assert_eq!(
        caps,
        vec![30],
        "the cap rises by exactly +20, never double-bumped, got {caps:?}"
    );
}

#[tokio::test]
async fn extend_works_after_hitting_the_hard_cap_wall() {
    // The extend vote is uncapped, so an agent that has hit the cap wall (a 409 on
    // send) can still propose and complete an extend, after which sends resume.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "wall room", Some(2), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    for i in 0..2 {
        let (s, _) = send(
            &app,
            &room_id,
            "repo-a",
            "opus47",
            "/a",
            None,
            &format!("m{i}"),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "send {i} under the cap of 2");
    }
    // The 3rd send hits the wall.
    let (over, _) = send(&app, &room_id, "repo-a", "opus47", "/a", None, "over").await;
    assert_eq!(
        over,
        StatusCode::CONFLICT,
        "the 3rd send exceeds the cap of 2"
    );

    // Both agents extend (the vote is uncapped even at the wall).
    lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(ebody["status"].as_str(), Some("extended"), "got {ebody}");
    assert_eq!(ebody["hard_cap"].as_u64(), Some(22), "2 → 22, got {ebody}");

    // The previously-refused send now succeeds.
    let (after, _) = send(&app, &room_id, "repo-a", "opus47", "/a", None, "now ok").await;
    assert_eq!(
        after,
        StatusCode::CREATED,
        "sends resume once the cap is extended"
    );
}

#[tokio::test]
async fn a_send_refused_at_the_cap_wall_does_not_clear_the_senders_extend_vote() {
    // The sender-scoped extend clear runs ONLY after a message actually lands — a
    // send refused at the cap wall (409) returns before it, so it must not retract
    // a vote the agents are mid-negotiating. Proven behaviorally: A votes extend
    // (1/2), A's next send hits the wall (409); if that 409 had wrongly cleared A's
    // vote, B's later vote would read as a fresh 1/2 proposal — so asserting B's
    // vote COMPLETES the extend (2/2) proves A's vote survived the refused send.
    let app = test_router().await;
    let room_id = open_with_caps(&app, "wall preserves vote", Some(2), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // Fill the cap of 2.
    for i in 0..2 {
        let (s, _) = send(
            &app,
            &room_id,
            "repo-a",
            "opus47",
            "/a",
            None,
            &format!("m{i}"),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "send {i} under the cap of 2");
    }

    // A proposes extend → 1/2.
    let (_, p) = lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    assert_eq!(p["status"].as_str(), Some("extend_proposed"), "got {p}");

    // A's next send hits the wall and is refused — this must NOT clear A's extend vote.
    let (over, _) = send(&app, &room_id, "repo-a", "opus47", "/a", None, "over").await;
    assert_eq!(
        over,
        StatusCode::CONFLICT,
        "the 3rd send exceeds the cap of 2"
    );

    // B votes extend → completes to 2/2 BECAUSE A's vote survived the wall-refused send.
    let (_, ebody) = lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        ebody["status"].as_str(),
        Some("extended"),
        "A's vote must survive a cap-wall 409, so B's vote completes the extend; got {ebody}"
    );
    assert_eq!(ebody["hard_cap"].as_u64(), Some(22), "2 → 22, got {ebody}");
}

#[tokio::test]
async fn a_proposer_is_not_told_extend_proposed_on_its_own_wait() {
    // The proposer already knows it voted; the `extend_proposed` status is for the
    // OTHER agent to act on. A's own wait must not echo its proposal back at it.
    // Short wait cap: A's wait parks (it has nothing) and must return promptly.
    let app = test_router_with_cap(std::time::Duration::from_millis(200)).await;
    let room_id = open_with_caps(&app, "self wait", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;

    let (_, wbody) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_ne!(
        wbody["status"].as_str(),
        Some("extend_proposed"),
        "the proposer must not be told of its own proposal, got {wbody}"
    );
}

#[tokio::test]
async fn an_extend_broadcasts_a_notice_the_counterpart_receives() {
    // When the cap bumps, a broadcast `extend` sentinel is posted so a polling
    // proposer learns the extend landed (and can take its turn) — its own poll
    // would otherwise just see paused_by_timeout and not know to continue.
    let app = test_router_with_cap(std::time::Duration::from_millis(200)).await;
    let room_id = open_with_caps(&app, "notice room", Some(10), None).await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes, B agrees → B's vote bumps the cap and posts the notice.
    lifecycle_op(&app, &room_id, "extend", "repo-a", "opus47", "/a", None).await;
    lifecycle_op(&app, &room_id, "extend", "repo-b", "opus47", "/b", None).await;

    // A (the proposer) waits and receives the broadcast extend notice.
    let (_, wbody) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        wbody["message"]["type"].as_str(),
        Some("extend"),
        "the proposer receives the extend notice, got {wbody}"
    );
    assert!(
        wbody["message"]["body"]
            .as_str()
            .is_some_and(|b| b.contains("20")),
        "the notice names the new cap, got {wbody}"
    );
}

// ---- Hole 2 latency: a parked wait must learn of a close/proposal promptly ----
//
// The consensus close is only useful if a peer that is *already parked* in a
// long-poll discovers the outcome promptly — not a full cap later. `close_now`
// and the proposal path both `hub.notify`, but a contentless wake that finds no
// claimable message used to re-park to the full cap with a status frozen at
// entry. These two tests pin both directions: the proposer learning the close,
// and the counterpart learning the proposal. Both arrange the peer to be
// genuinely parked (a sleep past the park entry) before the triggering call, so
// the fast entry-gate path cannot mask a regression — the assertion is on the
// elapsed time, which only means something if the park was actually entered.

#[tokio::test]
async fn a_parked_proposer_learns_of_the_close_promptly() {
    // A proposes, then parks waiting. When B's agreeing vote meets quorum and
    // closes the room, A's in-flight wait must return `closed` promptly (woken by
    // the close), not re-park and report `paused_by_timeout` a full cap later.
    let cap = std::time::Duration::from_secs(4);
    let app = test_router_with_cap(cap).await;
    let room_id = open_room_id(&app, "proposer parks").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes the close (1/2), then parks waiting for B to decide.
    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "got {body}"
    );

    let app_a = app.clone();
    let rid = room_id.clone();
    let parked = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let (_, b) = wait(&app_a, &rid, "repo-a", "opus47", "/a").await;
        (start.elapsed(), b)
    });

    // Let A reach the park, then B agrees → quorum → close_now → notify.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (_, cbody) = lifecycle_op(&app, &room_id, "close", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        cbody["status"].as_str(),
        Some("closed"),
        "B's vote closes: {cbody}"
    );

    let (elapsed, wbody) = parked.await.expect("parked wait task");
    assert_eq!(
        wbody["status"].as_str(),
        Some("closed"),
        "the parked proposer must learn of the close, got {wbody}"
    );
    assert!(
        elapsed < cap / 2,
        "the proposer must learn promptly (woken by the close), not a full cap \
         later; took {elapsed:?} of a {cap:?} cap"
    );
}

#[tokio::test]
async fn a_parked_counterpart_learns_of_the_proposal_promptly() {
    // B parks waiting on an active room. When A proposes a close, A's `hub.notify`
    // must wake B's in-flight wait and have it return `close_proposed` promptly —
    // not re-park and report `paused_by_timeout` a full cap later (the symmetric
    // half of the proposer case). This is what makes the close_room:798 notify
    // actually do what its comment claims.
    let cap = std::time::Duration::from_secs(4);
    let app = test_router_with_cap(cap).await;
    let room_id = open_room_id(&app, "counterpart parks").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    let app_b = app.clone();
    let rid = room_id.clone();
    let parked = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let (_, b) = wait(&app_b, &rid, "repo-b", "opus47", "/b").await;
        (start.elapsed(), b)
    });

    // Let B reach the park, then A proposes a close.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "A proposes: {body}"
    );

    let (elapsed, wbody) = parked.await.expect("parked wait task");
    assert_eq!(
        wbody["status"].as_str(),
        Some("close_proposed"),
        "the parked counterpart must learn of the proposal, got {wbody}"
    );
    assert!(
        elapsed < cap / 2,
        "the counterpart must learn promptly (woken by the proposal), not a full \
         cap later; took {elapsed:?} of a {cap:?} cap"
    );
}

#[tokio::test]
async fn a_proposer_is_not_told_close_proposed_on_its_own_wait() {
    // The `close_proposed` status is for the *counterpart* — the agent who has NOT
    // voted and must decide. A proposer waiting after its own vote must get the
    // ordinary `paused_by_timeout`, never `close_proposed`. Guards the
    // `caller_voted` short-circuit in `close_proposed`: a regression that reported
    // a proposal on the voter's own wait would have both agents ping-ponging
    // `close_proposed` at each other and never converging.
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "self view").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // A proposes (1/2). B is live but silent; the room stays open.
    let (_, body) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        body["status"].as_str(),
        Some("close_proposed"),
        "got {body}"
    );

    // A then waits. Its own vote must NOT come back to it as a proposal.
    let (_, wbody) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        wbody["status"].as_str(),
        Some("paused_by_timeout"),
        "the proposer must not be told `close_proposed` on its own wait, got {wbody}"
    );
}

#[tokio::test]
async fn wait_drains_unread_through_a_real_consensus_close() {
    // The drain-before-gate must hold when the room reaches `closed` through the
    // real two-vote consensus path (`close_now`: CAS + clear votes + notify), not
    // only when a test forces the state via `update_room_state`. An unread message
    // sent before the close must still drain, carrying the terminal hint, before
    // the close is reported.
    let (app, _storage) =
        test_router_with_cap_returning_storage(std::time::Duration::from_millis(200)).await;
    let room_id = open_room_id(&app, "consensus drain").await;
    join(&app, &room_id, "repo-a", "opus47", "/a").await;
    join(&app, &room_id, "repo-b", "opus47", "/b").await;

    // B leaves an unread message, then both agents vote to close (real quorum).
    let (s, _) = send(&app, &room_id, "repo-b", "opus47", "/b", None, "final word").await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, a) = lifecycle_op(&app, &room_id, "close", "repo-a", "opus47", "/a", None).await;
    assert_eq!(
        a["status"].as_str(),
        Some("close_proposed"),
        "A proposes: {a}"
    );
    let (_, b) = lifecycle_op(&app, &room_id, "close", "repo-b", "opus47", "/b", None).await;
    assert_eq!(
        b["status"].as_str(),
        Some("closed"),
        "B agrees → closed: {b}"
    );

    // A's wait on the now-consensus-closed room still drains the unread first...
    let (_, m) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(
        m["message"]["body"].as_str(),
        Some("final word"),
        "a consensus close must not strand unread, got {m}"
    );
    assert_eq!(m["room_state"].as_str(), Some("closed"), "got {m}");

    // ...and only then reports the terminal status.
    let (_, done) = wait(&app, &room_id, "repo-a", "opus47", "/a").await;
    assert_eq!(done["status"].as_str(), Some("closed"), "got {done}");
}

/// Read a participant's `poll_live` flag from the room status by repo.
async fn poll_live_of(app: &axum::Router, room_id: &str, repo: &str) -> bool {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/rooms/{room_id}"))
        .body(Body::empty())
        .unwrap();
    let body = body_json(app.clone().oneshot(req).await.unwrap().into_body()).await;
    let p = body["participants"]
        .as_array()
        .expect("participants array")
        .iter()
        .find(|p| p["repo"].as_str() == Some(repo))
        .unwrap_or_else(|| panic!("participant {repo} not in status: {body}"));
    p["poll_live"]
        .as_bool()
        .unwrap_or_else(|| panic!("poll_live missing for {repo}: {p}"))
}

/// The keystone regression: a participant currently parked on a long-poll reads
/// `poll_live: true`; once that connection is dropped (process death / TCP reset,
/// modeled here by aborting the handler future), `poll_live` flips to `false`
/// within the grace window — instead of the old `last_poll_at` timestamp lying
/// "fresh" for the full 15-minute ghost window.
#[tokio::test]
async fn poll_live_is_true_while_parked_and_false_after_the_connection_drops() {
    // Long cap so the wait genuinely parks; short grace so the drop→false flip is
    // observable without a long sleep.
    let app = test_router_with_cap_and_grace(
        std::time::Duration::from_secs(10),
        std::time::Duration::from_millis(150),
    )
    .await;
    let room_id = open_room_id(&app, "poll live truth").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    // B parks on an empty room; A never waits.
    let waiter = {
        let app = app.clone();
        let room_id = room_id.clone();
        tokio::spawn(async move { wait(&app, &room_id, "repo-b", "sonnet46", "/work/b").await })
    };

    // Let B reach the park, then observe the truth: B is live, A (never parked)
    // is not.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(
        poll_live_of(&app, &room_id, "repo-b").await,
        "a parked long-poll must read poll_live: true"
    );
    assert!(
        !poll_live_of(&app, &room_id, "repo-a").await,
        "a participant with no parked poll must read poll_live: false"
    );

    // The connection dies: aborting the task drops the handler future, which
    // drops the ParkGuard — exactly what Axum does when a client disconnects.
    waiter.abort();
    let _ = waiter.await;

    // Within grace it may still read live (covers a healthy re-poll gap); past
    // grace it must read dead — the corpse no longer lies "fresh".
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !poll_live_of(&app, &room_id, "repo-b").await,
        "a dropped long-poll must read poll_live: false past the grace window"
    );
}

/// The load-bearing assumption of the whole fix, proven over a *real* TCP socket:
/// when a client disconnects mid-park, hyper cancels the in-flight handler future,
/// which drops the `ParkGuard`. The `oneshot`/`abort()` test above drops the future
/// directly and so can't prove hyper actually cancels on socket EOF — this one
/// closes a real connection and asserts the guard releases. If RAII-on-cancel were
/// insufficient, this is where it would surface (the wait would keep parking to its
/// cap and `poll_live` would stay true), and explicit disconnect detection would be
/// required.
#[tokio::test]
async fn poll_live_reflects_a_real_tcp_client_disconnect() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};

    // One router instance, shared by the served socket and the in-process status
    // reads — both see the same `Arc<Presence>`.
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    let app = router(AppState::with_wait_cap_and_grace(
        storage,
        std::time::Duration::from_secs(10), // long cap → the wait genuinely parks
        std::time::Duration::from_millis(150), // short grace → fast false flip
    ));

    let room_id = open_room_id(&app, "tcp disconnect").await;
    join(&app, &room_id, "repo-a", "opus47", "/work/a").await;
    join(&app, &room_id, "repo-b", "sonnet46", "/work/b").await;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let served = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, served).await.expect("serve");
    });

    // A real client opens a connection and parks a wait for repo-b.
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!(
        "GET /rooms/{room_id}/wait?repo=repo-b&model=sonnet46&cwd=/work/b HTTP/1.1\r\n\
         Host: localhost\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write req");
    stream.flush().await.expect("flush");

    // Let the handler reach the park, then confirm the real connection is live.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        poll_live_of(&app, &room_id, "repo-b").await,
        "a wait parked over a real TCP connection must read poll_live: true"
    );

    // The client dies: closing the socket is exactly a reaped `cbc poll`.
    drop(stream);

    // hyper must cancel the parked handler future on EOF → ParkGuard drops.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !poll_live_of(&app, &room_id, "repo-b").await,
        "a real client disconnect must drop the guard → poll_live: false past grace"
    );

    server.abort();
}
