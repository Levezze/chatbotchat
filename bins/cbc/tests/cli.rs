use assert_cmd::Command;
use chatbotchat_server::{app_state, serve};
use std::sync::mpsc;
use std::thread;
use tokio::net::TcpListener;

/// Spawn the daemon on its own thread + runtime so the synchronous test body can
/// then drive the `cbc` binary via assert_cmd. Returns the base URL.
fn spawn_daemon() -> String {
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let dir = tempfile::tempdir().expect("tempdir");
            let db_url = format!("sqlite://{}", dir.path().join("state.db").display());
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tx.send(format!("http://{addr}")).expect("send url");
            let state = app_state(&db_url).await.expect("state");
            serve(listener, state).await.expect("serve");
            // keep tempdir alive for the lifetime of the server
            drop(dir);
        });
    });
    rx.recv().expect("daemon url")
}

#[test]
fn open_prints_room_id_and_share_line() {
    let base = spawn_daemon();

    let assert = Command::cargo_bin("cbc")
        .unwrap()
        .args(["open", "CLI smoke"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("cli-smoke-"),
        "open stdout should contain the room id; got:\n{stdout}"
    );
    assert!(
        stdout.contains("/cbc-join cli-smoke-"),
        "open stdout should contain the share line; got:\n{stdout}"
    );
}

/// Open a room over the CLI and return its id.
fn open_room(base: &str, subject: &str) -> String {
    let open = Command::cargo_bin("cbc")
        .unwrap()
        .args(["open", subject])
        .env("CBC_SERVER", base)
        .assert()
        .success();
    let out = String::from_utf8(open.get_output().stdout.clone()).unwrap();
    out.split_whitespace()
        .find(|tok| tok.contains("-202"))
        .expect("room id in open output")
        .to_string()
}

fn join(base: &str, room_id: &str, model: &str) -> String {
    let assert = Command::cargo_bin("cbc")
        .unwrap()
        .args(["join", room_id, "--model", model])
        .env("CBC_SERVER", base)
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn join_prints_handle_and_is_idempotent() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "cli join");

    // First join mints a fresh handle of the form <repo>-<model>-<sess4hex>.
    let first = join(&base, &room_id, "opus47");
    assert!(
        first.contains("-opus47-"),
        "join stdout should carry a <repo>-opus47-<sess> handle; got:\n{first}"
    );
    assert!(
        first.contains("Resumed: false"),
        "first join should report Resumed: false; got:\n{first}"
    );
    let handle = first
        .lines()
        .find_map(|l| l.strip_prefix("Handle:"))
        .map(str::trim)
        .expect("Handle line")
        .to_string();

    // Re-joining from the same repo/cwd/model resumes the same handle.
    let second = join(&base, &room_id, "opus47");
    assert!(
        second.contains(&handle),
        "second join should resume the same handle {handle}; got:\n{second}"
    );
    assert!(
        second.contains("Resumed: true"),
        "second join should report Resumed: true; got:\n{second}"
    );

    // status now lists the participant.
    let status = Command::cargo_bin("cbc")
        .unwrap()
        .args(["status", &room_id])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let status_out = String::from_utf8(status.get_output().stdout.clone()).unwrap();
    assert!(
        status_out.contains("Participants:") && status_out.contains(&handle),
        "status should list the joined participant {handle}; got:\n{status_out}"
    );
}

#[test]
fn send_then_wait_round_trips_over_cli() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "cli send wait");

    // Two participants distinguished by model (same cwd).
    join(&base, &room_id, "opus47");
    join(&base, &room_id, "sonnet46");

    // opus47 posts a broadcast BEFORE the wait so the wait returns immediately
    // (the real daemon's cap is 10 minutes — a test must never park on it).
    Command::cargo_bin("cbc")
        .unwrap()
        .args(["send", &room_id, "--model", "opus47", "hello over cli"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    // sonnet46 waits and receives the message.
    let waited = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wait", &room_id, "--model", "sonnet46"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(waited.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("hello over cli"),
        "wait stdout should carry the delivered message body; got:\n{out}"
    );
}

#[test]
fn status_reports_open_room() {
    let base = spawn_daemon();

    // open first, capture the room id
    let open = Command::cargo_bin("cbc")
        .unwrap()
        .args(["open", "status check"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let open_out = String::from_utf8(open.get_output().stdout.clone()).unwrap();
    let room_id = open_out
        .split_whitespace()
        .find(|tok| tok.starts_with("status-check-"))
        .expect("room id in open output")
        .to_string();

    let status = Command::cargo_bin("cbc")
        .unwrap()
        .args(["status", &room_id])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let status_out = String::from_utf8(status.get_output().stdout.clone()).unwrap();
    assert!(status_out.contains(&room_id));
    assert!(status_out.contains("active"));
    assert!(status_out.contains("status check"));
}
