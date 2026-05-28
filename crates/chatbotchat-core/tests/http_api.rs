use axum::body::Body;
use axum::http::{Request, StatusCode};
use chatbotchat_core::http::{router, AppState};
use chatbotchat_core::storage::Storage;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt; // for `oneshot`

async fn test_router() -> axum::Router {
    let storage = Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    router(AppState::new(storage))
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
