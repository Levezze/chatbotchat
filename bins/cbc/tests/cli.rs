use assert_cmd::Command;
use chatbotchat_server::{app_state, serve};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
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

// Identity within a room is the `instance` token. These tests run as
// subprocesses that all inherit one `CLAUDE_CODE_SESSION_ID`, so to simulate N
// *distinct* agents (and to stay deterministic when no session id is set) each
// call passes an explicit `--as <model>`: same model ⇒ same agent (resumes),
// different model ⇒ a separate participant.
fn join(base: &str, room_id: &str, model: &str) -> String {
    // Same agent ⇒ `--as <model>`; this is just `join_as` with the label fixed
    // to the model (defined below, used by the multi-identity tests).
    join_as(base, room_id, model, model)
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
        .args([
            "send",
            &room_id,
            "--model",
            "opus47",
            "--as",
            "opus47",
            "hello over cli",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    // sonnet46 waits and receives the message.
    let waited = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wait", &room_id, "--model", "sonnet46", "--as", "sonnet46"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(waited.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("hello over cli"),
        "wait stdout should carry the delivered message body; got:\n{out}"
    );
}

/// THE REPORTED BUG, end-to-end: two agents in the *same* repo+model+cwd that
/// previously collapsed onto one handle (and so were invisible to each other).
/// Distinguished only by `--as`, they must be two participants, both visible in
/// status, each able to receive the other's message — and re-joining with the
/// same `--as` resumes the identity (the handoff/continuity contract).
fn join_as(base: &str, room_id: &str, model: &str, as_label: &str) -> String {
    let assert = Command::cargo_bin("cbc")
        .unwrap()
        .args(["join", room_id, "--model", model, "--as", as_label])
        .env("CBC_SERVER", base)
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn handle_of(join_out: &str) -> String {
    join_out
        .lines()
        .find_map(|l| l.strip_prefix("Handle:"))
        .map(str::trim)
        .expect("Handle line")
        .to_string()
}

#[test]
fn same_model_distinct_as_are_separate_participants_that_can_talk() {
    let base = spawn_daemon();
    let room_id = open_room(&base, "same model distinct as");

    // Same model, same (auto-detected) repo+cwd — only `--as` differs.
    let alpha_out = join_as(&base, &room_id, "opus48", "alpha");
    let beta_out = join_as(&base, &room_id, "opus48", "beta");
    let alpha = handle_of(&alpha_out);
    let beta = handle_of(&beta_out);
    assert_ne!(
        alpha, beta,
        "same tuple, distinct --as must mint distinct handles; got {alpha} / {beta}"
    );
    assert!(
        beta_out.contains("Resumed: false"),
        "the second distinct identity is a fresh participant, not a resume; got:\n{beta_out}"
    );

    // status lists both.
    let status = Command::cargo_bin("cbc")
        .unwrap()
        .args(["status", &room_id])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let status_out = String::from_utf8(status.get_output().stdout.clone()).unwrap();
    assert!(
        status_out.contains(&alpha) && status_out.contains(&beta),
        "status must list both agents; got:\n{status_out}"
    );

    // alpha broadcasts; beta receives it (it is NOT filtered as beta's own).
    Command::cargo_bin("cbc")
        .unwrap()
        .args([
            "send",
            &room_id,
            "--model",
            "opus48",
            "--as",
            "alpha",
            "hi from alpha",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let waited = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wait", &room_id, "--model", "opus48", "--as", "beta"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(waited.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("hi from alpha"),
        "beta must receive alpha's message; got:\n{out}"
    );

    // Handoff/continuity: re-joining with the same --as resumes alpha's handle,
    // even though a fresh process (new pid) issues the call.
    let resumed = join_as(&base, &room_id, "opus48", "alpha");
    assert!(
        resumed.contains("Resumed: true") && resumed.contains(&alpha),
        "re-joining with the same --as must resume the same handle; got:\n{resumed}"
    );
}

#[test]
fn distinct_sessions_auto_derive_distinct_identities_without_as() {
    // The incident exactly: two agents, same model, same repo+cwd, and NO
    // explicit `--as`. They must still be separate participants — auto-derived
    // from a distinct CLAUDE_CODE_SESSION_ID per process (CBC_INSTANCE removed so
    // the session-id rung is the one exercised). This guards the code path that
    // actually failed, not the explicit-label path.
    let base = spawn_daemon();
    let room_id = open_room(&base, "auto identity");

    let join_sess = |sess: &str| {
        let assert = Command::cargo_bin("cbc")
            .unwrap()
            .args(["join", &room_id, "--model", "opus48"])
            .env("CBC_SERVER", &base)
            .env_remove("CBC_INSTANCE")
            .env("CLAUDE_CODE_SESSION_ID", sess)
            .assert()
            .success();
        handle_of(&String::from_utf8(assert.get_output().stdout.clone()).unwrap())
    };

    let a = join_sess("sess-a");
    let b = join_sess("sess-b");
    assert_ne!(
        a, b,
        "two sessions sharing repo+model+cwd must auto-derive distinct handles; got {a} / {b}"
    );

    let status = Command::cargo_bin("cbc")
        .unwrap()
        .args(["status", &room_id])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let status_out = String::from_utf8(status.get_output().stdout.clone()).unwrap();
    assert!(
        status_out.contains(&a) && status_out.contains(&b),
        "both auto-derived agents must appear in status; got:\n{status_out}"
    );

    // Same session id re-joining resumes (idempotent on the auto path too).
    let a_again = join_sess("sess-a");
    assert_eq!(a, a_again, "same session id must resume the same handle");
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
            "--as",
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
        .args(["wait", &room_id, "--model", "sonnet46", "--as", "sonnet46"])
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
            "--as",
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
        .args(["wake", &room_id, "--model", "opus47", "--as", "opus47"])
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
        .args(["close", &room_id, "--model", "opus47", "--as", "opus47"])
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
        .args(["send", room_id, "--model", model, "--as", model, body])
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
        .args(["close", &closed_room, "--model", "opus47", "--as", "opus47"])
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
            "--as",
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

// ----- #10: --port honored end-to-end across both binaries -----

/// Grab a currently-free loopback port (small reuse window, fine for tests).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_until_connectable(addr: SocketAddr, timeout: Duration) {
    let start = Instant::now();
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("daemon never became connectable on {addr}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Spawn the real `chatbotchat-server` *binary* on an explicit `--port`. Unlike
/// `spawn_daemon` (which runs the server in-process on an ephemeral port), this
/// exercises the daemon's `--port` flag and returns a child the caller must reap.
/// The tempdir is returned so its DB outlives the daemon.
fn spawn_daemon_binary_on_port() -> (String, u16, Child, tempfile::TempDir) {
    let port = free_port();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("state.db");
    let bin = assert_cmd::cargo::cargo_bin("chatbotchat-server");
    let child = StdCommand::new(bin)
        .args(["--port", &port.to_string(), "--db", db.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn chatbotchat-server binary");
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    wait_until_connectable(addr, Duration::from_secs(10));
    (format!("http://127.0.0.1:{port}"), port, child, dir)
}

/// AC #5: `--port <N>` works end-to-end with both binaries — the daemon binds the
/// override and a `CBC_SERVER`-pointed `cbc` does a full open→send→wait round-trip
/// against it, on a port that is never the default 8484.
#[test]
fn port_override_round_trips_across_both_binaries() {
    let (base, port, mut child, _dir) = spawn_daemon_binary_on_port();
    assert_ne!(port, 8484, "test must prove a non-default port");

    let room_id = open_room(&base, "port override");
    join(&base, &room_id, "opus47");
    join(&base, &room_id, "sonnet46");

    // Post before the wait so the long-poll returns at once (never park on the cap).
    Command::cargo_bin("cbc")
        .unwrap()
        .args([
            "send",
            &room_id,
            "--model",
            "opus47",
            "--as",
            "opus47",
            "hello on a custom port",
        ])
        .env("CBC_SERVER", &base)
        .assert()
        .success();

    let waited = Command::cargo_bin("cbc")
        .unwrap()
        .args(["wait", &room_id, "--model", "sonnet46", "--as", "sonnet46"])
        .env("CBC_SERVER", &base)
        .assert()
        .success();
    let out = String::from_utf8(waited.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("hello on a custom port"),
        "wait should deliver the body sent via the custom-port daemon; got:\n{out}"
    );

    child.kill().ok();
    child.wait().ok();
}
