use chatbotchat_server::{app_state, serve};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// Spawn the daemon on an ephemeral loopback port backed by a temp-file DB.
/// Returns the base URL and keeps the tempdir alive for the caller.
async fn spawn_daemon() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("state.db");
    let db_url = format!("sqlite://{}", db_path.display());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
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
    assert!(room_id.starts_with("cbc-real-tcp-test-"));

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

// ----- shared helpers for the real-binary tests (cycles 6 & 7) -----

/// Grab a currently-free loopback port by binding to :0 and immediately
/// releasing it. There is a small reuse window, acceptable for tests.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Discover this host's primary non-loopback IPv4 without sending packets.
/// Returns None when the machine has no default route (e.g. offline CI), so
/// callers can skip the LAN-reachability assertion rather than flake.
fn local_non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip)
    }
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

#[test]
fn port_conflict_exits_with_helpful_error() {
    // Hold a port so the daemon's bind must fail.
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("hold port");
    let port = blocker.local_addr().unwrap().port();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("state.db");

    let output = Command::new(env!("CARGO_BIN_EXE_chatbotchat-server"))
        .args(["--port", &port.to_string(), "--db", db.to_str().unwrap()])
        .output()
        .expect("run daemon");

    assert!(
        !output.status.success(),
        "daemon should exit non-zero when the port is taken"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        stderr.contains(&port.to_string())
            && stderr.contains("another instance")
            && stderr.contains("--port"),
        "stderr should name the port, hint at a running instance, and point at --port; got: {stderr}"
    );
}

#[test]
fn daemon_binds_loopback_only() {
    let port = free_port();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("state.db");

    let mut child = Command::new(env!("CARGO_BIN_EXE_chatbotchat-server"))
        .args(["--port", &port.to_string(), "--db", db.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");

    let loopback = SocketAddr::from(([127, 0, 0, 1], port));
    wait_until_connectable(loopback, Duration::from_secs(5));

    // Reachable on loopback.
    assert!(TcpStream::connect_timeout(&loopback, Duration::from_millis(500)).is_ok());

    // NOT reachable via the host's LAN address — proves it did not bind 0.0.0.0.
    if let Some(lan) = local_non_loopback_ipv4() {
        let lan_addr = SocketAddr::new(lan, port);
        let res = TcpStream::connect_timeout(&lan_addr, Duration::from_millis(500));
        assert!(
            res.is_err(),
            "daemon must not be reachable on the LAN address {lan_addr}"
        );
    }

    child.kill().ok();
    child.wait().ok();
}
