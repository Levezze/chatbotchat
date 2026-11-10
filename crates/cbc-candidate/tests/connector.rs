use cbc_candidate::{
    bindings::Role,
    connector::{LocalClient, LocalConnector},
    store::Store,
    Sandbox,
};

#[tokio::test]
async fn targeted_feed_routes_only_changed_members_without_consuming_their_inboxes() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-targets-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    for member in ["alice", "bob", "carol", "outside"] {
        store
            .register_member("test", member, Role::Participant)
            .await
            .unwrap();
    }
    for (room, member) in [("ab", "bob"), ("ac", "carol"), ("ax", "outside")] {
        store
            .create_room("test", room, &["alice", member])
            .await
            .unwrap();
    }
    let alice = store
        .bind_member("test", "alice", "device", "a")
        .await
        .unwrap();
    let bob = store
        .bind_member("test", "bob", "device", "b")
        .await
        .unwrap();
    let carol = store
        .bind_member("test", "carol", "device", "c")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let a = LocalClient::new(&path, &alice);
    let b = LocalClient::new(&path, &bob);
    let c = LocalClient::new(&path, &carol);
    let first = a
        .send("ab", "one", "private payload for bob")
        .await
        .unwrap();
    let second = a
        .send("ac", "two", "private payload for carol")
        .await
        .unwrap();
    a.send("ax", "unrelated", "must not leak").await.unwrap();
    let changes = b.watch_members(&[&carol], 0).await.unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(
        (&changes[0].context, &changes[0].member, changes[0].sequence),
        (&"test".into(), &"bob".into(), first)
    );
    assert_eq!(
        (&changes[1].context, &changes[1].member, changes[1].sequence),
        (&"test".into(), &"carol".into(), second)
    );
    let wire = serde_json::to_string(&changes).unwrap();
    assert!(
        !wire.contains("payload")
            && !wire.contains("outside")
            && !wire.contains(bob.expose_token_for_adapter())
    );
    assert_eq!(b.check().await.unwrap().unwrap().items[0].sequence, first);
    assert_eq!(c.check().await.unwrap().unwrap().items[0].sequence, second);
    // Feed reads neither create receipts nor move immutable pending batches.
    assert_eq!(b.watch_members(&[&carol], 0).await.unwrap(), changes);
    let subscribed = [&carol];
    let mut waiting = Box::pin(b.watch_members(&subscribed, second));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    a.send("ax", "still-unrelated", "no wake").await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    let third = a.send("ab", "three", "only bob changed").await.unwrap();
    let targeted = tokio::time::timeout(std::time::Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(targeted.len(), 1);
    assert_eq!(targeted[0].member, "bob");
    assert_eq!(targeted[0].sequence, third);
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    // A connector restart recovers all unread members at zero without an in-memory event.
    let reopened = Store::open(&sandbox).await.unwrap();
    let connector = LocalConnector::bind(&sandbox, reopened).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let recovered = b.watch_members(&[&carol], 0).await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].sequence, third);
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn shared_router_dispatches_two_host_adapters_against_the_real_core() {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, process::Command};
    let temp = tempfile::Builder::new()
        .prefix("cbc-shared-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("fixture").await.unwrap();
    let mut bindings = Vec::new();
    for member in ["writer", "codex", "claude"] {
        store
            .register_member("fixture", member, Role::Participant)
            .await
            .unwrap();
        let binding = store
            .bind_member("fixture", member, "device", member)
            .await
            .unwrap();
        bindings.push(serde_json::json!({"context":"fixture", "member":member,
            "device":"device", "session":member, "epoch":binding.epoch(),
            "token":binding.expose_token_for_adapter()}));
    }
    store
        .create_room("fixture", "room", &["writer", "codex", "claude"])
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let profile = temp.path().join("fixture.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&profile)
        .unwrap();
    file.write_all(
        &serde_json::to_vec(
            &serde_json::json!({"core_socket":connector.path(), "bindings":bindings}),
        )
        .unwrap(),
    )
    .unwrap();
    drop(file);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let output = tokio::task::spawn_blocking(move || {
        Command::new("/usr/bin/python3")
            .arg("-I")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/adapters/shared/run_fixture.py"
            ))
            .arg(profile)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(
        output.status.success(),
        "shared adapter fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn one_local_connector_routes_two_bound_clients_through_the_real_durable_store() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-sock-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    for member in ["alice", "bob"] {
        store
            .register_member("test", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("test", "room", &["alice", "bob"])
        .await
        .unwrap();
    let alice = store
        .bind_member("test", "alice", "device", "native-a")
        .await
        .unwrap();
    let bob = store
        .bind_member("test", "bob", "device", "native-b")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let a = LocalClient::new(&path, &alice);
    let b = LocalClient::new(&path, &bob);
    let seq = a
        .send("room", "message-1", "real core, no poll process")
        .await
        .unwrap();
    assert!(a.check().await.unwrap().is_none());
    let batch = b.check().await.unwrap().unwrap();
    assert_eq!(batch.items[0].sequence, seq);
    assert_eq!(batch.items[0].body, "real core, no poll process");
    b.receipt(&batch).await.unwrap();
    assert!(b.check().await.unwrap().is_none());
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(!path.exists());
    let reopened = Store::open(&sandbox).await.unwrap();
    assert!(reopened.check(&bob, 32).await.unwrap().is_none());
    reopened.close().await;
}

#[tokio::test]
async fn the_socket_rejects_unbound_stale_wrong_context_and_admin_requests() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    let temp = tempfile::Builder::new()
        .prefix("cbc-auth-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    store
        .register_member("test", "bob", Role::Participant)
        .await
        .unwrap();
    let stale = store
        .bind_member("test", "bob", "device", "old-chat")
        .await
        .unwrap();
    let current = store
        .bind_member("test", "bob", "device", "new-chat")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    assert!(LocalClient::new(&path, &stale).check().await.is_err());
    assert!(LocalClient::new(&path, &stale).watch(&[], 0).await.is_err());
    assert!(LocalClient::new(&path, &stale)
        .watch_members(&[], 0)
        .await
        .is_err());
    assert!(LocalClient::new(&path, &current)
        .watch_members(&[&current], 0)
        .await
        .is_err());
    assert!(LocalClient::new(&path, &current)
        .watch_members(&[], -1)
        .await
        .is_err());
    assert!(LocalClient::new(&path, &current)
        .watch(&[&current], 0)
        .await
        .is_err());
    assert!(LocalClient::new(&path, &current)
        .watch(&[], -1)
        .await
        .is_err());
    let auth = serde_json::json!({"context":"other","member":"bob","device":"device","session":"new-chat","epoch":current.epoch(),"token":current.expose_token_for_adapter()});
    let requests = [
        serde_json::json!({"v":1,"request":{"op":"check"}}),
        serde_json::json!({"v":1,"auth":auth,"request":{"op":"check"}}),
        serde_json::json!({"v":1,"auth":auth,"request":{"op":"watch_members", "others":[], "after":0}}),
        serde_json::json!({"v":1,"auth":auth,"request":{"op":"register_member","member":"evil"}}),
        serde_json::json!({"v":2,"auth":auth,"request":{"op":"check"}}),
    ];
    for request in requests {
        let mut stream = UnixStream::connect(&path).await.unwrap();
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["type"], "error");
        assert!(!response
            .to_string()
            .contains(current.expose_token_for_adapter()));
    }
    assert!(LocalClient::new(&path, &current)
        .check()
        .await
        .unwrap()
        .is_none());
    // Shutdown does not wait for an unfinished client frame or release ownership early.
    let mut slow = UnixStream::connect(&path).await.unwrap();
    slow.write_all(b"{").await.unwrap();
    stop_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    Store::open(&sandbox).await.unwrap().close().await;
}

#[tokio::test]
async fn binding_the_connector_never_overwrites_an_existing_socket_path() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-path-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let path = sandbox.root().join("connector.sock");
    std::fs::write(&path, b"keep this file").unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(LocalConnector::bind(&sandbox, store).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"keep this file");
}

#[tokio::test]
async fn a_core_recovers_from_its_own_file_descriptor_exhaustion() {
    use std::{
        io::{BufRead, BufReader},
        os::unix::{net::UnixStream, process::CommandExt},
        process::{Child, Command, Stdio},
    };
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::Builder::new()
        .prefix("cbc-fds-")
        .tempdir_in("/tmp")
        .unwrap();
    let protected = temp.path().join("daily/state.db");
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        std::slice::from_ref(&protected),
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    store
        .register_member("test", "member", Role::Participant)
        .await
        .unwrap();
    let binding = store
        .bind_member("test", "member", "device", "chat")
        .await
        .unwrap();
    store.close().await;
    let mut command = Command::new(env!("CARGO_BIN_EXE_cbc-candidate"));
    command
        .args(["serve", "--root"])
        .arg(sandbox.root())
        .arg("--protected-db")
        .arg(protected)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Only this new disposable core receives the low limit. The test runner and live apps do not.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 32,
                rlim_max: 32,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let mut ready = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("listening"), "candidate did not start");
    let socket = sandbox.root().join("connector.sock");
    let mut held = Vec::new();
    for _ in 0..64 {
        match UnixStream::connect(&socket) {
            Ok(s) => held.push(s),
            Err(_) => break,
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "temporary descriptor pressure killed the core"
    );
    drop(held);
    let recovered = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        LocalClient::new(&socket, &binding).check(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(recovered.is_none());
    // SAFETY: this is the still-running, unreaped child owned by this test, never a native chat.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    assert!(child.0.wait().unwrap().success());
    Store::open(&sandbox).await.unwrap().close().await;
}

#[tokio::test]
async fn one_change_feed_waits_for_authorized_work_and_recovers_from_the_durable_sequence() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-feed-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    for member in ["alice", "bob", "carol", "dave"] {
        store
            .register_member("test", member, Role::Participant)
            .await
            .unwrap();
    }
    store
        .create_room("test", "ab", &["alice", "bob"])
        .await
        .unwrap();
    store
        .create_room("test", "cd", &["carol", "dave"])
        .await
        .unwrap();
    let alice = store
        .bind_member("test", "alice", "device", "a")
        .await
        .unwrap();
    let bob = store
        .bind_member("test", "bob", "device", "b")
        .await
        .unwrap();
    let carol = store
        .bind_member("test", "carol", "device", "c")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let a = LocalClient::new(&path, &alice);
    let b = LocalClient::new(&path, &bob);
    let c = LocalClient::new(&path, &carol);
    let subscribed = [&carol];
    let mut waiting = Box::pin(b.watch(&subscribed, 0));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    c.send("cd", "unrelated", "not for bob").await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    let sequence = a.send("ab", "eligible", "for bob").await.unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(3), waiting)
            .await
            .unwrap()
            .unwrap(),
        sequence
    );
    // Feed notification is not a check or receipt: the actual inbox still contains the message.
    let batch = b.check().await.unwrap().unwrap();
    assert_eq!(batch.items[0].sequence, sequence);
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    assert_eq!(
        b.watch(&subscribed, 0).await.unwrap(),
        sequence,
        "restart must recover a missed wake from durable rows"
    );
    b.receipt(&batch).await.unwrap();
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn disconnecting_parked_feeds_releases_their_connection_slots() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-unwatch-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("test").await.unwrap();
    store
        .register_member("test", "member", Role::Participant)
        .await
        .unwrap();
    let member = store
        .bind_member("test", "member", "device", "chat")
        .await
        .unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let path = connector.path().to_owned();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stop_rx.await;
    }));
    let client = LocalClient::new(&path, &member);
    for _ in 0..36 {
        let mut waiting = Box::pin(client.watch(&[], 0));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        drop(waiting);
    }
    assert!(client.check().await.unwrap().is_none());
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}
