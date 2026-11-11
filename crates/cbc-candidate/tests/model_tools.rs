//! S1: the trial's model tools (room close, work items, requests) through a real host adapter
//! and a real `serve` process. A fake core cannot catch a wire shape the real one refuses.
use assert_cmd::Command;
use cbc_candidate::Sandbox;
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
use tempfile::{tempdir, TempDir};

const NEW_TOOLS: [&str; 12] = [
    "cbc_close",
    "cbc_work_register",
    "cbc_work_update",
    "cbc_work_hitl_on",
    "cbc_work_get",
    "cbc_request_open",
    "cbc_request_answer",
    "cbc_request_withdraw",
    "cbc_request_applied",
    "cbc_request_get",
    "cbc_request_answers",
    "cbc_request_pending",
];

struct Core {
    _temp: TempDir,
    root: PathBuf,
    protected: PathBuf,
    cwd: PathBuf,
    server: Child,
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

fn private(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

impl Core {
    /// A served core with a Primarch and two Chapter Masters, all Codex threads in one
    /// working directory, provisioned through the owner CLI as the trial will be.
    fn provisioned() -> (Self, PathBuf) {
        let temp = tempdir().unwrap();
        let protected = temp.path().join("daily/state.db");
        let sandbox = Sandbox::initialize(
            &temp.path().join("c"),
            8984,
            std::slice::from_ref(&protected),
        )
        .unwrap();
        let root = sandbox.root().to_path_buf();
        let cwd = temp.path().join("project");
        fs::create_dir(&cwd).unwrap();
        let cwd = fs::canonicalize(cwd).unwrap();
        let mut server = ProcessCommand::new(env!("CARGO_BIN_EXE_cbc-candidate"))
            .args(["serve", "--root"])
            .arg(&root)
            .arg("--protected-db")
            .arg(&protected)
            .env_remove("CBC_DB")
            .env_remove("CBC_PORT")
            .env_remove("CBC_SERVER")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = server.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = sender.send(line);
        });
        let line = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("candidate startup timed out");
        let ready: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ready["status"], "listening");
        let core = Self {
            _temp: temp,
            root,
            protected,
            cwd,
            server,
        };
        let target = json!({"v":1,"profile":"unknown","repository":"trial","worktree":null,
            "cwd":core.cwd,"saved_project":{"status":"not_exposed"},
            "saved_environment":{"status":"not_exposed"},"placement":{"evidence_ref":"synthetic"}});
        let member = |member: &str, role: &str| {
            json!({"member":member,"role":role,"host":"codex","session":format!("thread-{member}"),
                "evidence_ref":"synthetic","repository":"trial","label":member,
                "summary":"Synthetic model-tool test","target":target})
        };
        let spec = core.root.join("provision.json");
        private(
            &spec,
            &json!({"context":"trial","device":"laptop",
                "members":[member("pri","repo_lead"),member("cm-a","feature_lead"),member("cm-b","feature_lead")],
                "grants":[
                    {"requester":"cm-a","target":"pri","kind":"report","scope_ref":"observer"},
                    {"requester":"cm-b","target":"pri","kind":"report","scope_ref":"view"},
                    {"requester":"cm-a","target":"cm-b","kind":"peer","scope_ref":"contract"}]}),
        );
        let profile = core.root.join("device-profile.json");
        Command::new(env!("CARGO_BIN_EXE_cbc-candidate"))
            .arg("provision")
            .arg("--root")
            .arg(&core.root)
            .arg("--protected-db")
            .arg(&core.protected)
            .arg("--spec")
            .arg(&spec)
            .arg("--profile-out")
            .arg(&profile)
            .env_remove("CBC_DB")
            .env_remove("CBC_PORT")
            .env_remove("CBC_SERVER")
            .timeout(Duration::from_secs(5))
            .assert()
            .success();
        (core, profile)
    }
}

/// The real Codex adapter over stdio. Each call names the thread the host would supply.
struct Adapter {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Adapter {
    fn start(core: &Core, profile: &Path) -> Self {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters/codex/adapter.py");
        let mut child = ProcessCommand::new("python3")
            .args(["-B", "-I"])
            .arg(script)
            .current_dir(&core.cwd)
            .env("CBC_CANDIDATE_PROFILE", profile)
            .env_remove("CBC_CANDIDATE_CONNECTOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            next: 0,
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let frame = json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params});
        writeln!(self.input, "{frame}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.next, "{reply}");
        reply
    }

    fn tool_names(&mut self) -> Vec<String> {
        self.rpc("tools/list", json!({}))["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The tool's payload and whether the adapter marked it an error.
    fn call(&mut self, member: &str, name: &str, arguments: Value) -> (Value, bool) {
        let reply = self.rpc(
            "tools/call",
            json!({"name":name,"arguments":arguments,"_meta":{"threadId":format!("thread-{member}")}}),
        );
        let result = &reply["result"];
        assert!(result.is_object(), "{name}: {reply}");
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            serde_json::from_str(text).unwrap(),
            result["isError"] == true,
        )
    }

    fn ok(&mut self, member: &str, name: &str, arguments: Value) -> Value {
        let (payload, error) = self.call(member, name, arguments);
        assert!(!error, "{member} {name}: {payload}");
        payload
    }

    fn refused(&mut self, member: &str, name: &str, arguments: Value) -> String {
        let (payload, error) = self.call(member, name, arguments);
        assert!(error, "{member} {name} should fail: {payload}");
        payload["error"].as_str().unwrap().to_owned()
    }
}

#[test]
fn the_adapter_lists_every_trial_tool() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);
    let names = adapter.tool_names();
    for name in NEW_TOOLS {
        assert!(names.iter().any(|n| n == name), "missing {name}: {names:?}");
    }
}

#[test]
fn a_chapter_master_registers_and_revises_its_work_item_and_the_primarch_can_only_turn_hitl_on() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);

    let registered = adapter.ok(
        "cm-a",
        "cbc_work_register",
        json!({"item":"observer","summary":"PR and CI observer","key":"register-1"}),
    );
    assert_eq!(registered["work"]["type"], "registered");
    let item = &registered["work"]["item"];
    assert_eq!(item["owner"], "cm-a");
    assert_eq!(item["hitl"], false);
    let revision = item["revision"].as_i64().unwrap();

    let updated = adapter.ok(
        "cm-a",
        "cbc_work_update",
        json!({"item":"observer","expected_revision":revision,"key":"update-1",
            "stage":"implementing","next_action":"write the poller tests",
            "blocked_operation":"exclusive test port","branch":"feat/pr-observer"}),
    );
    assert_eq!(updated["work"]["type"], "outcome", "{updated}");
    let read = adapter.ok("cm-a", "cbc_work_get", json!({"item":"observer"}));
    let item = &read["work"]["item"];
    assert_eq!(item["stage"], "implementing");
    assert_eq!(item["next_action"], "write the poller tests");
    assert_eq!(item["blocked_operation"], "exclusive test port");
    assert_eq!(item["branch"], "feat/pr-observer");

    // An explicit null clears a clearable field; an absent field is left alone.
    let revision = item["revision"].as_i64().unwrap();
    adapter.ok(
        "cm-a",
        "cbc_work_update",
        json!({"item":"observer","expected_revision":revision,"key":"update-2",
            "blocked_operation":null}),
    );
    let item =
        adapter.ok("cm-a", "cbc_work_get", json!({"item":"observer"}))["work"]["item"].clone();
    assert_eq!(item["blocked_operation"], Value::Null);
    assert_eq!(item["branch"], "feat/pr-observer");

    // T-07: the owner's Primarch may turn HITL on; a peer may not; nobody's tool turns it off.
    // The core refuses a member with one generic message, so the item itself is the evidence.
    adapter.refused(
        "cm-b",
        "cbc_work_hitl_on",
        json!({"item":"observer","key":"hitl-b"}),
    );
    let item =
        adapter.ok("cm-a", "cbc_work_get", json!({"item":"observer"}))["work"]["item"].clone();
    assert_eq!(item["hitl"], false);
    adapter.ok(
        "pri",
        "cbc_work_hitl_on",
        json!({"item":"observer","key":"hitl-1"}),
    );
    let item =
        adapter.ok("pri", "cbc_work_get", json!({"item":"observer"}))["work"]["item"].clone();
    assert_eq!(item["hitl"], true);
    adapter.refused(
        "pri",
        "cbc_work_hitl_on",
        json!({"item":"observer","key":"hitl-2","on":false}),
    );
    let item =
        adapter.ok("cm-a", "cbc_work_get", json!({"item":"observer"}))["work"]["item"].clone();
    assert_eq!(item["hitl"], true);
}

#[test]
fn a_question_for_the_user_relayed_by_a_member_stays_relayed_never_answered() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);
    adapter.ok(
        "cm-a",
        "cbc_work_register",
        json!({"item":"observer","summary":"PR and CI observer","key":"register-1"}),
    );
    let opened = adapter.ok(
        "cm-a",
        "cbc_request_open",
        json!({"to":"user","item":"observer","question":"Poll GitHub every 30 s or every 60 s?","key":"ask-1"}),
    );
    assert_eq!(opened["request"]["type"], "opened", "{opened}");
    let request = opened["request"]["request"].clone();
    assert_eq!(request["owner"], "user");
    assert_eq!(request["item"], "observer");
    assert_eq!(request["state"], "open");
    let id = request["request"].as_str().unwrap().to_owned();

    let relayed = adapter.ok(
        "cm-a",
        "cbc_request_answer",
        json!({"request":id,"body":"The user said 60 s in the chat.","key":"relay-1"}),
    );
    assert_eq!(
        relayed["request"]["answer"]["channel"], "relayed",
        "{relayed}"
    );
    let read = adapter.ok("cm-a", "cbc_request_get", json!({"request":id}));
    assert_eq!(read["request"]["request"]["state"], "relayed");
}

#[test]
fn a_request_to_the_primarch_is_answered_read_applied_and_another_withdrawn() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);
    // A question for a member travels on the room between them, so the route comes first.
    adapter.ok(
        "cm-b",
        "cbc_connect",
        json!({"target":"pri","kind":"report","scope_ref":"view","key":"report-1"}),
    );
    let opened = adapter.ok(
        "cm-b",
        "cbc_request_open",
        json!({"to":"pri","question":"May I take port 3000 for an hour?","key":"ask-1"}),
    );
    let request = &opened["request"]["request"];
    assert_eq!(request["owner"], json!({"member":"pri"}));
    let id = request["request"].as_str().unwrap().to_owned();

    let pending = adapter.ok("pri", "cbc_request_pending", json!({}));
    let ids: Vec<&str> = pending["request"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["request"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [id.as_str()]);

    adapter.ok(
        "pri",
        "cbc_request_answer",
        json!({"request":id,"body":"Yes, until 15:00.","key":"answer-1"}),
    );
    let answers = adapter.ok("cm-b", "cbc_request_answers", json!({"request":id}));
    let answers = answers["request"]["answers"].as_array().unwrap().clone();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["body"], "Yes, until 15:00.");
    let answer = answers[0]["id"].as_i64().unwrap();
    let applied = adapter.ok(
        "cm-b",
        "cbc_request_applied",
        json!({"request":id,"answer":answer}),
    );
    assert_eq!(applied["request"]["type"], "applied", "{applied}");
    let read = adapter.ok("cm-b", "cbc_request_get", json!({"request":id}));
    assert_eq!(read["request"]["request"]["state"], "answered");

    let second = adapter.ok(
        "cm-b",
        "cbc_request_open",
        json!({"to":"pri","question":"And port 3001?","key":"ask-2"}),
    );
    let second = second["request"]["request"]["request"]
        .as_str()
        .unwrap()
        .to_owned();
    let withdrawn = adapter.ok(
        "cm-b",
        "cbc_request_withdraw",
        json!({"request":second,"reason":"3000 is enough"}),
    );
    assert_eq!(withdrawn["request"]["request"]["state"], "withdrawn");
}

#[test]
fn either_peer_closes_their_room_alone_and_the_next_open_replaces_it() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);
    let connect = json!({"target":"cm-b","kind":"peer","scope_ref":"contract","key":"connect-1"});
    let first = adapter.ok("cm-a", "cbc_connect", connect.clone());
    let room = first["connection"]["room"].as_str().unwrap().to_owned();

    let closed = adapter.ok(
        "cm-a",
        "cbc_close",
        json!({"room":room,"reason":"contract agreed"}),
    );
    assert!(closed["sequence"].as_i64().is_some(), "{closed}");
    let refusal = adapter.refused("cm-a", "cbc_close", json!({"room":room}));
    assert!(refusal.contains("reason"), "{refusal}");

    let second = adapter.ok(
        "cm-b",
        "cbc_connect",
        json!({"target":"cm-a","kind":"peer","scope_ref":"contract","key":"connect-2"}),
    );
    let replacement = second["connection"]["room"].as_str().unwrap();
    assert_ne!(replacement, room);
}

#[test]
fn identity_in_arguments_never_reaches_the_core() {
    let (core, profile) = Core::provisioned();
    let mut adapter = Adapter::start(&core, &profile);
    for (name, arguments) in [
        (
            "cbc_work_register",
            json!({"item":"x","summary":"y","key":"k","member":"pri"}),
        ),
        (
            "cbc_request_open",
            json!({"to":"user","question":"q","key":"k","owner":"cm-b"}),
        ),
        (
            "cbc_close",
            json!({"room":"r","reason":"x","context":"trial"}),
        ),
    ] {
        adapter.refused("cm-a", name, arguments);
    }
}
