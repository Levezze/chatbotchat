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
fn signal_then_wait_surfaces_the_question_over_cli() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "cli signal wait");

    join(&base, &room_id, "opus47");
    join(&base, &room_id, "sonnet46");

    // opus47 signals that it is consulting its user, carrying the question.
    Command::cargo_bin("cbc")
        .unwrap()
        .args([
            "signal",
            &room_id,
            "--model",
            "opus47",
            "--type",
            "waiting_user",
            "--severity",
            "high",
            "--question",
            "should I merge to production?",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    // sonnet46 waits and sees the sentinel + its question.
    let waited = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wait", &room_id, "--model", "sonnet46"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(waited.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("should I merge to production?"),
        "wait stdout should surface the sentinel's question; got:\n{out}"
    );
    assert!(
        out.contains("waiting_user"),
        "wait stdout should name the sentinel type; got:\n{out}"
    );
}

#[test]
fn pause_wake_close_round_trip_over_cli() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "cli lifecycle");
    join(&base, &room_id, "opus47");

    // pause (with a reason) parks the room.
    let paused = Command::cargo_bin("cbc")
        .unwrap()
        .args([
            "pause",
            &room_id,
            "--model",
            "opus47",
            "--reason",
            "stepping away",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(paused.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("State: paused"),
        "pause should report the paused state; got:\n{out}"
    );

    // wake brings it back to active.
    let woken = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wake", &room_id, "--model", "opus47"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(woken.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("State: active"),
        "wake should report the active state; got:\n{out}"
    );

    // close ends it.
    let closed = Command::cargo_bin("cbc")
        .unwrap()
        .args(["close", &room_id, "--model", "opus47"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(closed.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("State: closed"),
        "close should report the closed state; got:\n{out}"
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

// --- list + show (browse surface, issue #27) ---

/// Send a `msg` over the CLI (helper for transcript tests).
fn send(base: &str, room_id: &str, model: &str, body: &str) {
    Command::cargo_bin("cbc")
        .unwrap()
        .args(["send", room_id, "--model", model, body])
        .env("CBC_SERVER", base)
        .assert()
        .success();
}

#[test]
fn list_shows_open_rooms_with_state_and_subject() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "list me");
    join(&base, &room_id, "opus47");

    let listed = Command::cargo_bin("cbc")
        .unwrap()
        .args(["list"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(listed.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains(&room_id),
        "list should show the room id; got:\n{out}"
    );
    assert!(
        out.contains("active"),
        "list should show the state; got:\n{out}"
    );
    assert!(
        out.contains("list me"),
        "list should show the subject; got:\n{out}"
    );
}

#[test]
fn list_with_no_rooms_prints_placeholder() {
    let base = spawn_daemon();
    let listed = Command::cargo_bin("cbc")
        .unwrap()
        .args(["list"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(listed.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("(no rooms)"),
        "an empty list should print a placeholder, not nothing; got:\n{out}"
    );
}

#[test]
fn list_state_filter_narrows_results() {
    let base = spawn_daemon();
    let active_room = open_room(&base, "stays active");
    let closed_room = open_room(&base, "gets closed");
    join(&base, &closed_room, "opus47");
    Command::cargo_bin("cbc")
        .unwrap()
        .args(["close", &closed_room, "--model", "opus47"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    let listed = Command::cargo_bin("cbc")
        .unwrap()
        .args(["list", "--state", "closed"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(listed.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains(&closed_room),
        "closed filter should show the closed room; got:\n{out}"
    );
    assert!(
        !out.contains(&active_room),
        "closed filter must exclude the active room; got:\n{out}"
    );
}

#[test]
fn list_unknown_state_exits_nonzero() {
    let base = spawn_daemon();
    Command::cargo_bin("cbc")
        .unwrap()
        .args(["list", "--state", "bogus"])
        .env("CBC_SERVER", &base)
        .assert()
        .failure();
}

#[test]
fn show_renders_markdown_transcript_with_sentinel() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "show me");
    join(&base, &room_id, "opus47");
    join(&base, &room_id, "sonnet46");
    send(&base, &room_id, "opus47", "the message body");
    Command::cargo_bin("cbc")
        .unwrap()
        .args([
            "signal",
            &room_id,
            "--model",
            "sonnet46",
            "--type",
            "waiting_user",
            "--severity",
            "high",
            "--question",
            "should I merge?",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    let shown = Command::cargo_bin("cbc")
        .unwrap()
        .args(["show", &room_id])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(shown.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("show me"),
        "markdown should carry the subject; got:\n{out}"
    );
    assert!(
        out.contains("the message body"),
        "markdown should carry the message; got:\n{out}"
    );
    assert!(
        out.contains("waiting_user"),
        "markdown should render the sentinel type; got:\n{out}"
    );
    assert!(
        out.contains("high"),
        "markdown should render the sentinel severity; got:\n{out}"
    );
    assert!(
        out.contains("should I merge?"),
        "markdown should render the question; got:\n{out}"
    );
}

#[test]
fn show_json_format_outputs_parseable_json() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "show json");
    join(&base, &room_id, "opus47");
    send(&base, &room_id, "opus47", "jsonable");

    let shown = Command::cargo_bin("cbc")
        .unwrap()
        .args(["show", &room_id, "--format", "json"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(shown.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&out).expect("show --format json must emit valid JSON");
    assert_eq!(v["id"].as_str(), Some(room_id.as_str()));
    assert!(
        v["messages"].is_array(),
        "transcript json carries a messages array"
    );
    assert_eq!(v["messages"][0]["body"].as_str(), Some("jsonable"));
}

#[test]
fn show_unknown_room_exits_nonzero() {
    let base = spawn_daemon();
    Command::cargo_bin("cbc")
        .unwrap()
        .args(["show", "no-such-room-20260101-0000"])
        .env("CBC_SERVER", &base)
        .assert()
        .failure();
}
