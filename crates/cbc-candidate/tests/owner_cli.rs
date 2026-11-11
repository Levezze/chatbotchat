use assert_cmd::Command;
use cbc_candidate::{settings::RuntimeSettings, Sandbox};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command as ProcessCommand, Stdio},
    thread,
    time::Duration,
};
use tempfile::{tempdir, TempDir};

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    protected: PathBuf,
    server: Child,
}

impl Fixture {
    fn start() -> Self {
        Self::start_with(8984, &[]).0
    }

    /// Start the binary and return its readiness line too.
    fn start_with(port: u16, extra: &[&str]) -> (Self, Value) {
        let temp = tempdir().unwrap();
        let protected = temp.path().join("daily/state.db");
        let sandbox = Sandbox::initialize(
            &temp.path().join("candidate"),
            port,
            std::slice::from_ref(&protected),
        )
        .unwrap();
        let root = sandbox.root().to_path_buf();
        let mut server = ProcessCommand::new(env!("CARGO_BIN_EXE_cbc-candidate"))
            .args(["serve", "--root"])
            .arg(&root)
            .arg("--protected-db")
            .arg(&protected)
            .args(extra)
            .env_remove("CBC_DB")
            .env_remove("CBC_PORT")
            .env_remove("CBC_SERVER")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The inode exists during bind, before owner/settings setup completes. Await the
        // executable's advertised readiness instead of racing its initialization.
        let output = server.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(output).read_line(&mut line);
            let _ = sender.send((read, line));
        });
        let (read, line) = match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => {
                let _ = server.kill();
                let _ = server.wait();
                panic!("candidate startup timed out");
            }
        };
        reader.join().unwrap();
        assert!(read.unwrap() > 0, "owned candidate exited before readiness");
        let ready: Value = serde_json::from_str(&line).expect("candidate readiness must be JSON");
        assert_eq!(ready["status"], "listening");
        assert!(root.join("connector.sock").exists());
        (
            Self {
                _temp: temp,
                root,
                protected,
                server,
            },
            ready,
        )
    }

    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cbc-candidate"));
        command
            .arg(action)
            .arg("--root")
            .arg(&self.root)
            .arg("--protected-db")
            .arg(&self.protected)
            .env_remove("CBC_DB")
            .env_remove("CBC_PORT")
            .env_remove("CBC_SERVER")
            .timeout(Duration::from_secs(5));
        command
    }

    fn document(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn spec(&self) -> PathBuf {
        self.document("provision-input.json", &json!({"context":"demo","device":"laptop",
            "members":[{"member":"owner","role":"repo_lead","host":"codex","session":"native-session",
                "evidence_ref":"actual-host-reference","repository":"api","label":"api owner","summary":"Disposable enrollment", "target":target()}],
            "grants":[]}))
    }
}

fn target() -> Value {
    json!({"v":1,"profile":"unknown","repository":"api","worktree":null,
        "cwd":"/fixtures/api","saved_project":{"status":"not_exposed"},
        "saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"synthetic-ui-observation"}})
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

#[test]
fn provisions_against_live_store_and_prints_only_private_profile_location() {
    let mut fixture = Fixture::start();
    let pid = fixture.server.id();
    let spec = fixture.spec();
    let profile = fixture.root.join("native-bindings.json");
    let output = fixture
        .command("provision")
        .arg("--spec")
        .arg(&spec)
        .arg("--profile-out")
        .arg(&profile)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("cbcc1_"));
    assert!(!stdout.contains("cbco1_"));
    let status: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(status["status"], "provisioned");
    assert_eq!(status["members"], 1);
    assert_eq!(status["profile"], profile.to_string_lossy().as_ref());
    let bytes = fs::read(&profile).unwrap();
    let private: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        private["targets"],
        json!([{"session":"native-session","host":"codex","target":target()}])
    );
    assert!(private["bindings"][0]["token"]
        .as_str()
        .unwrap()
        .starts_with("cbcc1_"));
    assert_eq!(
        fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fixture
        .command("provision")
        .arg("--spec")
        .arg(&spec)
        .arg("--profile-out")
        .arg(&profile)
        .assert()
        .failure();
    assert_eq!(fs::read(profile).unwrap(), bytes);
    assert_eq!(fixture.server.id(), pid);
    assert!(fixture.server.try_wait().unwrap().is_none());
}

#[test]
fn settings_cli_applies_live_cas_and_rejects_stale_revision_without_restarting_core() {
    let mut fixture = Fixture::start();
    let pid = fixture.server.id();
    let output = fixture
        .command("settings")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let initial: Value = serde_json::from_slice(&output).unwrap();
    let revision = initial["revision"].as_str().unwrap();
    let mut policy = RuntimeSettings::default();
    policy.defaults.retry_initial_ms = 50;
    policy.defaults.retry_max_ms = 500;
    let input = fixture.document(
        "updated-policy.json",
        &serde_json::to_value(&policy).unwrap(),
    );
    let output = fixture
        .command("settings")
        .arg("--file")
        .arg(&input)
        .arg("--expected-revision")
        .arg(revision)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let updated: Value = serde_json::from_slice(&output).unwrap();
    assert_ne!(updated["revision"], initial["revision"]);
    assert_eq!(updated["settings"]["defaults"]["retry_initial_ms"], 50);
    fixture
        .command("settings")
        .arg("--file")
        .arg(&input)
        .arg("--expected-revision")
        .arg(revision)
        .assert()
        .failure();
    fixture
        .command("settings")
        .arg("--file")
        .arg(&input)
        .assert()
        .failure();
    let output = fixture
        .command("settings")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let current: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(current["revision"], updated["revision"]);
    assert_eq!(fixture.server.id(), pid);
    assert!(fixture.server.try_wait().unwrap().is_none());
}

#[test]
fn invalid_profile_destinations_are_rejected_before_provisioning() {
    let fixture = Fixture::start();
    let spec = fixture.spec();
    let original_manifest = fs::read(fixture.root.join("candidate.json")).unwrap();
    let outside = fixture.root.parent().unwrap().join("outside-profile.json");
    for destination in [
        outside.clone(),
        fixture.root.join("candidate.json"),
        fixture.root.join("owner.key"),
    ] {
        fixture
            .command("provision")
            .arg("--spec")
            .arg(&spec)
            .arg("--profile-out")
            .arg(destination)
            .assert()
            .failure();
    }
    assert!(!outside.exists());
    assert_eq!(
        fs::read(fixture.root.join("candidate.json")).unwrap(),
        original_manifest
    );
    // A later correct request succeeds: invalid outputs caused no earlier binding rotation.
    fixture
        .command("provision")
        .arg("--spec")
        .arg(&spec)
        .arg("--profile-out")
        .arg(fixture.root.join("profile.json"))
        .assert()
        .success();
}

#[test]
fn symlink_and_fifo_documents_are_refused_without_waiting_or_touching_targets() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let fixture = Fixture::start();
    let source = fixture.spec();
    let link = fixture.root.join("linked-spec.json");
    symlink(&source, &link).unwrap();
    fixture
        .command("provision")
        .arg("--spec")
        .arg(&link)
        .arg("--profile-out")
        .arg(fixture.root.join("linked-profile.json"))
        .assert()
        .failure();
    let fifo = fixture.root.join("fifo-spec.json");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    fixture
        .command("provision")
        .arg("--spec")
        .arg(&fifo)
        .arg("--profile-out")
        .arg(fixture.root.join("fifo-profile.json"))
        .assert()
        .failure();
    assert!(!fixture.root.join("linked-profile.json").exists());
    assert!(!fixture.root.join("fifo-profile.json").exists());
    fixture
        .command("settings")
        .arg("--file")
        .arg(fixture.root.join("owner.key"))
        .arg("--expected-revision")
        .arg("unused")
        .assert()
        .failure();
}

#[test]
fn revoke_cli_fences_expected_binding_and_refuses_a_stale_revoke() {
    use cbc_candidate::{bindings::SessionBinding, connector::LocalClient};
    let mut fixture = Fixture::start();
    let spec = fixture.spec();
    let profile = fixture.root.join("native-bindings.json");
    fixture
        .command("provision")
        .arg("--spec")
        .arg(spec)
        .arg("--profile-out")
        .arg(&profile)
        .assert()
        .success();
    let profile: Value = serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
    let saved = &profile["bindings"][0];
    let epoch = saved["epoch"].as_i64().unwrap();
    let binding = SessionBinding::from_token(
        saved["context"].as_str().unwrap(),
        saved["member"].as_str().unwrap(),
        saved["device"].as_str().unwrap(),
        saved["session"].as_str().unwrap(),
        epoch,
        saved["token"].as_str().unwrap(),
    )
    .unwrap();
    let socket = fixture.root.join("connector.sock");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert!(runtime
        .block_on(LocalClient::new(&socket, &binding).settings())
        .is_ok());
    let output = fixture
        .command("revoke")
        .args(["--context", "demo", "--member", "owner", "--expected-epoch"])
        .arg(epoch.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["status"], "binding-revoked");
    assert_eq!(result["epoch"], epoch + 1);
    assert!(!String::from_utf8(output).unwrap().contains("cbcc1_"));
    assert!(runtime
        .block_on(LocalClient::new(&socket, &binding).settings())
        .is_err());
    fixture
        .command("revoke")
        .args(["--context", "demo", "--member", "owner", "--expected-epoch"])
        .arg(epoch.to_string())
        .assert()
        .failure();
    assert!(fixture.server.try_wait().unwrap().is_none());
}

#[test]
fn retire_cli_ends_the_identity_for_good() {
    use cbc_candidate::{bindings::SessionBinding, connector::LocalClient};
    let mut fixture = Fixture::start();
    let spec = fixture.spec();
    let profile = fixture.root.join("native-bindings.json");
    fixture
        .command("provision")
        .arg("--spec")
        .arg(spec)
        .arg("--profile-out")
        .arg(&profile)
        .assert()
        .success();
    let profile: Value = serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
    let saved = &profile["bindings"][0];
    let binding = SessionBinding::from_token(
        saved["context"].as_str().unwrap(),
        saved["member"].as_str().unwrap(),
        saved["device"].as_str().unwrap(),
        saved["session"].as_str().unwrap(),
        saved["epoch"].as_i64().unwrap(),
        saved["token"].as_str().unwrap(),
    )
    .unwrap();
    let socket = fixture.root.join("connector.sock");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let retire = |fixture: &mut Fixture| {
        fixture
            .command("retire")
            .args([
                "--context",
                "demo",
                "--member",
                "owner",
                "--reason",
                "trial ended",
            ])
            .assert()
    };
    let output = retire(&mut fixture).success().get_output().stdout.clone();
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["status"], "retired");
    assert_eq!(result["member"], "owner");
    assert!(runtime
        .block_on(LocalClient::new(&socket, &binding).settings())
        .is_err());
    retire(&mut fixture).failure();
    assert!(fixture.server.try_wait().unwrap().is_none());
}

/// A core started without `--dashboard` mints no sign-in code, so the command fails before it
/// would open a browser, and it prints nothing. The success path opens the user's real browser,
/// so it is exercised only by the user, from the launch sheet.
#[test]
fn dashboard_cli_refuses_without_a_dashboard_and_prints_nothing() {
    let fixture = Fixture::start();
    let output = fixture
        .command("dashboard")
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("login?code="));
}

/// `serve --dashboard` answers on 127.0.0.1 at the candidate port, and only to a session.
#[test]
fn serve_with_dashboard_listens_on_loopback_at_the_candidate_port() {
    let (fixture, ready) = Fixture::start_with(8997, &["--dashboard"]);
    assert_eq!(ready["dashboard"], "http://127.0.0.1:8997/");
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", 8997)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:8997\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 401"), "{reply}");
    drop(fixture);
    let (_plain, ready) = Fixture::start_with(8998, &[]);
    assert!(ready["dashboard"].is_null());
    assert!(std::net::TcpStream::connect(("127.0.0.1", 8998)).is_err());
}
