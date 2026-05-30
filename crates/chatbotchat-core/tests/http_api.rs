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

    // The conversation `msg` and the lifecycle types are not signals.
    for ty in ["msg", "blocker_real_work", "close", "bogus"] {
        let (s, _) = sig(ty, Some("high"), Some("q?")).await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{ty} is not a valid signal type"
        );
    }

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
