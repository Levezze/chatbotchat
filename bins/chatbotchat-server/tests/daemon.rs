use chatbotchat_server::{app_state, serve};
use serde_json::{json, Value};
use tokio::net::TcpListener;

/// Spawn the daemon on an ephemeral loopback port backed by a temp-file DB.
/// Returns the base URL and keeps the tempdir alive for the caller.
async fn spawn_daemon() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("state.db");
    let db_url = format!("sqlite://{}", db_path.display());

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    assert!(addr.ip().is_loopback(), "daemon must bind loopback");

    let state = app_state(&db_url).await.expect("build state");
    tokio::spawn(async move {
        serve(listener, state).await.expect("serve");
    });

    (format!("http://{addr}"), dir)
}

#[tokio::test]
async fn open_then_status_over_real_tcp() {
    let (base, _dir) = spawn_daemon().await;
    let client = reqwest::Client::new();

    let open: Value = client
        .post(format!("{base}/rooms"))
        .json(&json!({ "subject": "real tcp test" }))
        .send()
        .await
        .expect("open request")
        .json()
        .await
        .expect("open json");

    let room_id = open["room_id"].as_str().expect("room_id");
    assert!(room_id.starts_with("real-tcp-test-"));

    let status: Value = client
        .get(format!("{base}/rooms/{room_id}"))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("status json");

    assert_eq!(status["id"].as_str().unwrap(), room_id);
    assert_eq!(status["subject"].as_str().unwrap(), "real tcp test");
    assert_eq!(status["state"].as_str().unwrap(), "active");
}
