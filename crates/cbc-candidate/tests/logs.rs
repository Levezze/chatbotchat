//! B2: the core's structured log, through a real `serve` process.
use cbc_candidate::{bindings::Role, connector::LocalClient, sampler::Mode, store::Store, Sandbox};
use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
};

struct Core(Child);
impl Drop for Core {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn set_level(root: &Path, level: Option<&str>) {
    let mut settings = serde_json::json!({
        "schema": 1,
        "defaults": {"retry_initial_ms": 1000, "retry_max_ms": 30000},
    });
    if let Some(level) = level {
        settings["log_level"] = level.into();
    }
    let temporary = root.join("settings.tmp");
    fs::write(&temporary, serde_json::to_vec(&settings).unwrap()).unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(temporary, root.join("runtime-settings.json")).unwrap();
}

fn lines(root: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join("logs/core.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn requests(root: &Path) -> Vec<serde_json::Value> {
    lines(root)
        .into_iter()
        .filter(|l| l["event"] == "request")
        .collect()
}

#[tokio::test]
async fn a_served_core_logs_requests_only_while_debug_and_never_a_token_or_a_body() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-log-")
        .tempdir_in("/tmp")
        .unwrap();
    let protected = temp.path().join("daily/state.db");
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        std::slice::from_ref(&protected),
    )
    .unwrap();
    let root = sandbox.root().to_path_buf();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    for member in ["alice", "bob"] {
        store
            .register_member("test", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("test", "ab", &["alice", "bob"])
        .await
        .unwrap();
    let alice = store
        .bind_member("test", "alice", "device", "chat-a")
        .await
        .unwrap();
    let bob = store
        .bind_member("test", "bob", "device", "chat-b")
        .await
        .unwrap();
    store.close().await;

    let mut core = Core(
        Command::new(env!("CARGO_BIN_EXE_cbc-candidate"))
            .args(["serve", "--run", "trial-1", "--root"])
            .arg(&root)
            .arg("--protected-db")
            .arg(&protected)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(core.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("listening"), "candidate did not start");
    let socket = root.join("connector.sock");
    let a = LocalClient::new(&socket, &alice);
    let b = LocalClient::new(&socket, &bob);

    // Normal: no per-request lines.
    a.send("ab", "k1", "private body one").await.unwrap();
    let batch = b.check().await.unwrap().unwrap();
    b.receipt(&batch).await.unwrap();
    assert!(requests(&root).is_empty(), "{:?}", lines(&root));
    assert_eq!(b.binding_status().await.unwrap().log_level, Mode::Normal);

    // Debug, without a restart. A request is logged when debug holds at its start and its end,
    // so the send that reads the change is not logged and the next one is.
    set_level(&root, Some("debug"));
    a.send("ab", "k2", "private body two").await.unwrap();
    let sent = a.send("ab", "k2b", "private body two b").await.unwrap();
    let batch = b.check().await.unwrap().unwrap();
    b.receipt(&batch).await.unwrap();
    assert_eq!(b.binding_status().await.unwrap().log_level, Mode::Debug);
    let logged = requests(&root);
    let kinds: Vec<&str> = logged.iter().map(|l| l["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["sent", "batch", "receipted", "binding_status"]);
    assert_eq!(logged[0]["seq"], sent);
    assert_eq!(logged[0]["member"], "alice");
    assert_eq!(logged[1]["member"], "bob");
    assert_eq!(logged[1]["count"], 2);
    // A receipt reference is a capability: the log carries a digest that correlates core and
    // driver lines, never the reference itself.
    assert_eq!(
        logged[1]["reference"],
        cbc_candidate::facts::batch_tag(batch.reference_for_adapter())
    );
    for line in &logged {
        assert_eq!(line["v"], 1);
        assert_eq!(line["level"], "debug");
        assert_eq!(line["component"], "core");
        assert_eq!(line["run"], "trial-1");
        assert_eq!(line["context"], "test");
        assert_eq!(line["instance"], logged[0]["instance"]);
    }

    // Back to normal: the send that reads the change is not logged either, nor anything after.
    set_level(&root, None);
    a.send("ab", "k3", "private body three").await.unwrap();
    a.send("ab", "k4", "private body four").await.unwrap();
    assert_eq!(requests(&root).len(), logged.len());

    let text = fs::read_to_string(root.join("logs/core.jsonl")).unwrap();
    for secret in [
        alice.expose_token_for_adapter(),
        bob.expose_token_for_adapter(),
        batch.reference_for_adapter(),
        "private body",
    ] {
        assert!(!text.contains(secret), "the log holds a secret or a body");
    }
    // SAFETY: this is the still-running, unreaped child owned by this test, never a native chat.
    assert_eq!(unsafe { libc::kill(core.0.id() as i32, libc::SIGTERM) }, 0);
    assert!(core.0.wait().unwrap().success());
}
